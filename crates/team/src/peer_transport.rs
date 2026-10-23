//! Peer transport seam (#47 part c): the trait a relay or loopback connection
//! plugs into later, an in-memory double for tests, and leases that return to
//! the queue when a peer drops. Frames are opaque bytes; this module never
//! parses them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex as AsyncMutex};

/// A transport failure: an unknown or dropped peer, or a dropped sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError(pub String);

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TransportError {}

/// The result type of transport operations.
pub type Result<T> = std::result::Result<T, TransportError>;

/// A connection to peers. `recv` returns `None` once the endpoint is closed.
#[async_trait]
pub trait PeerTransport: Send + Sync {
    /// Send one opaque frame to the peer named `to`.
    async fn send(&self, to: &str, frame: Vec<u8>) -> Result<()>;
    /// Receive the next frame as `(sender, frame)`, or `None` once closed.
    async fn recv(&self) -> Option<(String, Vec<u8>)>;
    /// The names of the peers currently connected, sorted.
    fn connected(&self) -> Vec<String>;
}

type Inbox = mpsc::UnboundedSender<(String, Vec<u8>)>;

#[derive(Default)]
struct HubInner {
    peers: HashMap<String, Inbox>,
}

/// An in-process network of endpoints. Clones share the same network.
#[derive(Clone, Default)]
pub struct InMemoryHub {
    inner: Arc<Mutex<HubInner>>,
}

impl InMemoryHub {
    /// An empty hub with no peers.
    pub fn new() -> Self {
        Self::default()
    }

    /// Join the hub as `name`. A second endpoint with the same name replaces the first's inbox.
    pub fn endpoint(&self, name: &str) -> InMemoryEndpoint {
        let (tx, rx) = mpsc::unbounded_channel();
        self.inner
            .lock()
            .expect("hub lock")
            .peers
            .insert(name.to_string(), tx);
        InMemoryEndpoint {
            name: name.to_string(),
            hub: self.clone(),
            rx: AsyncMutex::new(rx),
        }
    }

    /// Simulate a disconnect: the peer leaves, and its endpoint's `recv` ends
    /// once what was already queued is drained.
    pub fn drop_peer(&self, name: &str) {
        self.inner.lock().expect("hub lock").peers.remove(name);
    }
}

/// One peer's view of the hub; implements [`PeerTransport`].
pub struct InMemoryEndpoint {
    name: String,
    hub: InMemoryHub,
    rx: AsyncMutex<mpsc::UnboundedReceiver<(String, Vec<u8>)>>,
}

#[async_trait]
impl PeerTransport for InMemoryEndpoint {
    // NOTE: this holds the hub lock while pushing into the peer's inbox. That is
    // fine for an unbounded in-process channel. A bounded or async transport must
    // not copy this: release the lock before any await or blocking send.
    async fn send(&self, to: &str, frame: Vec<u8>) -> Result<()> {
        let inner = self.hub.inner.lock().expect("hub lock");
        if !inner.peers.contains_key(&self.name) {
            return Err(TransportError(format!("{}: sender is dropped", self.name)));
        }
        let inbox = inner
            .peers
            .get(to)
            .ok_or_else(|| TransportError(format!("{to}: unknown or dropped peer")))?;
        inbox
            .send((self.name.clone(), frame))
            .map_err(|_| TransportError(format!("{to}: unknown or dropped peer")))
    }

    async fn recv(&self) -> Option<(String, Vec<u8>)> {
        self.rx.lock().await.recv().await
    }

    fn connected(&self) -> Vec<String> {
        let mut live: Vec<String> = self
            .hub
            .inner
            .lock()
            .expect("hub lock")
            .peers
            .keys()
            .filter(|k| **k != self.name)
            .cloned()
            .collect();
        live.sort();
        live
    }
}

#[derive(Debug, Clone)]
struct Lease {
    peer: String,
    ttl: Duration,
    expires: Instant,
}

/// Task leases held by peers. Time is injected as `now`; nothing sleeps.
#[derive(Debug, Default)]
pub struct LeaseTable {
    leases: HashMap<String, Lease>,
}

impl LeaseTable {
    /// An empty lease table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Lease `task_id` to `peer` for `ttl` from `now`, replacing any earlier lease.
    pub fn lease(&mut self, task_id: &str, peer: &str, ttl: Duration, now: Instant) {
        self.leases.insert(
            task_id.to_string(),
            Lease {
                peer: peer.to_string(),
                ttl,
                expires: now + ttl,
            },
        );
    }

    /// Extend a live lease by its TTL from `now`. Returns false if there is no such lease.
    pub fn renew(&mut self, task_id: &str, now: Instant) -> bool {
        match self.leases.get_mut(task_id) {
            Some(l) => {
                l.expires = now + l.ttl;
                true
            }
            None => false,
        }
    }

    /// Remove and return (sorted) the tasks whose leases have run out by `now`.
    pub fn expire(&mut self, now: Instant) -> Vec<String> {
        let mut gone: Vec<String> = self
            .leases
            .iter()
            .filter(|(_, l)| now >= l.expires)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &gone {
            self.leases.remove(id);
        }
        gone.sort();
        gone
    }

    /// Return every task leased to `peer` to the queue (sorted), and only those.
    pub fn peer_dropped(&mut self, peer: &str) -> Vec<String> {
        let mut gone: Vec<String> = self
            .leases
            .iter()
            .filter(|(_, l)| l.peer == peer)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &gone {
            self.leases.remove(id);
        }
        gone.sort();
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn two_endpoints_exchange_frames_in_order() {
        let hub = InMemoryHub::new();
        let a = hub.endpoint("a");
        let b = hub.endpoint("b");
        a.send("b", b"one".to_vec()).await.unwrap();
        a.send("b", b"two".to_vec()).await.unwrap();
        assert_eq!(b.recv().await, Some(("a".to_string(), b"one".to_vec())));
        assert_eq!(b.recv().await, Some(("a".to_string(), b"two".to_vec())));
    }

    #[tokio::test]
    async fn a_task_and_its_result_round_trip() {
        let hub = InMemoryHub::new();
        let a = hub.endpoint("a");
        let b = hub.endpoint("b");
        a.send("b", b"task".to_vec()).await.unwrap();
        let (from, frame) = b.recv().await.unwrap();
        assert_eq!((from.as_str(), frame.as_slice()), ("a", &b"task"[..]));
        b.send(&from, b"result".to_vec()).await.unwrap();
        assert_eq!(a.recv().await, Some(("b".to_string(), b"result".to_vec())));
    }

    #[tokio::test]
    async fn send_to_a_dropped_peer_is_an_error() {
        let hub = InMemoryHub::new();
        let a = hub.endpoint("a");
        let _b = hub.endpoint("b");
        hub.drop_peer("b");
        assert!(a.send("b", b"x".to_vec()).await.is_err());
        assert!(a.send("nobody", b"x".to_vec()).await.is_err());
    }

    #[tokio::test]
    async fn recv_on_a_dropped_endpoint_ends() {
        let hub = InMemoryHub::new();
        let _a = hub.endpoint("a");
        let b = hub.endpoint("b");
        hub.drop_peer("b");
        assert_eq!(b.recv().await, None);
    }

    #[tokio::test]
    async fn connected_lists_live_peers() {
        let hub = InMemoryHub::new();
        let a = hub.endpoint("a");
        let _b = hub.endpoint("b");
        let _c = hub.endpoint("c");
        assert_eq!(a.connected(), vec!["b".to_string(), "c".to_string()]);
        hub.drop_peer("b");
        assert_eq!(a.connected(), vec!["c".to_string()]);
    }

    #[test]
    fn a_lease_expires_after_its_ttl() {
        let t0 = Instant::now();
        let mut leases = LeaseTable::new();
        leases.lease("t1", "a", Duration::from_secs(10), t0);
        assert!(leases.expire(t0 + Duration::from_secs(9)).is_empty());
        assert_eq!(
            leases.expire(t0 + Duration::from_secs(10)),
            vec!["t1".to_string()]
        );
        assert!(!leases.renew("t1", t0 + Duration::from_secs(11)));
    }

    #[test]
    fn renew_keeps_a_lease() {
        let t0 = Instant::now();
        let mut leases = LeaseTable::new();
        leases.lease("t1", "a", Duration::from_secs(10), t0);
        assert!(leases.renew("t1", t0 + Duration::from_secs(8)));
        assert!(leases.expire(t0 + Duration::from_secs(15)).is_empty());
        assert_eq!(
            leases.expire(t0 + Duration::from_secs(18)),
            vec!["t1".to_string()]
        );
    }

    #[test]
    fn a_dropped_peer_returns_all_its_tasks() {
        let t0 = Instant::now();
        let ttl = Duration::from_secs(60);
        let mut leases = LeaseTable::new();
        leases.lease("t1", "a", ttl, t0);
        leases.lease("t2", "b", ttl, t0);
        leases.lease("t3", "a", ttl, t0);
        assert_eq!(
            leases.peer_dropped("a"),
            vec!["t1".to_string(), "t3".to_string()]
        );
        assert_eq!(leases.peer_dropped("a"), Vec::<String>::new());
        assert_eq!(leases.expire(t0 + ttl), vec!["t2".to_string()]);
    }
}
