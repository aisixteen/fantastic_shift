//! Webhook receiver: GitHub events, HMAC-verified, become board updates
//! (`pull`) without polling. Off unless configured. The secret is never
//! logged, stored in a row, or echoed in a reply.

use std::sync::Arc;

use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

use crate::puller::{KernelPeers, Peers};

/// The bundle's handler module name.
pub const HANDLER_MODULE: &str = "team_webhook.tools";

/// The header GitHub signs the raw body with.
const SIG_HEADER: &str = "X-Hub-Signature-256";
/// The header that names the event.
const EVENT_HEADER: &str = "X-GitHub-Event";

/// Receiver configuration.
#[derive(Debug, Clone, Default)]
pub struct WebhookCfg {
    /// `webhook.enabled`.
    pub enabled: bool,
    /// The shared secret, resolved from `webhook.secret_ref`.
    pub secret: Option<String>,
    /// The board agent that gets `pull`.
    pub board: String,
}

/// Case-insensitive header lookup.
fn header<'a>(headers: &'a Value, name: &str) -> Option<&'a str> {
    headers
        .as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .and_then(|(_, v)| v.as_str())
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Verify `X-Hub-Signature-256` (`sha256=<hex>`) over the raw body, in
/// constant time. A missing or malformed header is a refusal.
pub fn verify(secret: &[u8], body: &[u8], header: Option<&str>) -> bool {
    let Some(hex) = header.and_then(|h| h.strip_prefix("sha256=")) else {
        return false;
    };
    let Some(sig) = decode_hex(hex) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&sig).is_ok()
}

/// Whether an event should make the board pull. Unknown events are ignored.
pub fn wants_pull(event: &str, payload: &Value) -> bool {
    let action = payload["action"].as_str().unwrap_or("");
    match event {
        "issues" => matches!(action, "opened" | "edited" | "labeled" | "closed"),
        "pull_request" => matches!(action, "opened" | "closed"),
        "issue_comment" => true,
        _ => false,
    }
}

/// Verify, then map. Replies never contain the secret.
pub async fn receive(
    peers: &dyn Peers,
    cfg: &WebhookCfg,
    headers: &Value,
    body: &str,
) -> Value {
    if !cfg.enabled {
        return json!({"refused": "webhook disabled"});
    }
    let Some(secret) = cfg.secret.as_deref() else {
        return json!({"refused": "no webhook secret configured"});
    };
    if !verify(secret.as_bytes(), body.as_bytes(), header(headers, SIG_HEADER)) {
        return json!({"refused": "bad or missing signature"});
    }
    let event = header(headers, EVENT_HEADER).unwrap_or("");
    let payload: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    if !wants_pull(event, &payload) {
        return json!({"accepted": true, "board_call": null});
    }
    peers.ask(&cfg.board, json!({"type": "pull"})).await;
    json!({"accepted": true, "board_call": "pull"})
}

/// Read the receiver's config from its own record. The secret is taken from
/// the environment variable named by `secret_ref`, so it lives outside the
/// record and outside every reply.
fn cfg_of(kernel: &Arc<fantastic_kernel::Kernel>, id: &fantastic_kernel::AgentId) -> WebhookCfg {
    let meta = kernel
        .agents
        .get(id)
        .map(|e| e.meta.read().expect("meta").clone())
        .unwrap_or_default();
    let secret = meta
        .get("secret_ref")
        .and_then(|v| v.as_str())
        .and_then(|name| std::env::var(name).ok())
        .filter(|s| !s.is_empty());
    WebhookCfg {
        enabled: meta.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false),
        secret,
        board: meta
            .get("board")
            .and_then(|v| v.as_str())
            .unwrap_or("team_board")
            .to_string(),
    }
}

/// The webhook bundle: `receive {headers, body}`.
#[derive(Default)]
pub struct WebhookBundle;

#[async_trait::async_trait]
impl fantastic_kernel::bundle::Bundle for WebhookBundle {
    fn name(&self) -> &str {
        HANDLER_MODULE
    }

    fn verb_docs(&self) -> Value {
        json!({
            "receive": "receive {headers, body} → accepted {board_call} | refused {reason}",
        })
    }

    async fn handle(
        &self,
        agent_id: &fantastic_kernel::AgentId,
        payload: &Value,
        kernel: &Arc<fantastic_kernel::Kernel>,
    ) -> Result<fantastic_kernel::bundle::Reply, fantastic_kernel::bundle::BundleError> {
        let t = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if t == "boot" {
            return Ok(None);
        }
        let out = match t {
            "receive" => {
                let cfg = cfg_of(kernel, agent_id);
                let headers = payload.get("headers").cloned().unwrap_or(json!({}));
                let body = payload["body"].as_str().unwrap_or("");
                receive(&KernelPeers(kernel.clone()), &cfg, &headers, body).await
            }
            other => json!({"error": format!("team_webhook: unknown type '{other}'")}),
        };
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records every ask as `to:type`.
    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl Peers for Fake {
        async fn ask(&self, to: &str, p: Value) -> Value {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{to}:{}", p["type"].as_str().unwrap_or("")));
            json!({"ok": true})
        }
    }

    fn sign(secret: &str, body: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body.as_bytes());
        let hex: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        format!("sha256={hex}")
    }

    fn cfg(secret: Option<&str>, enabled: bool) -> WebhookCfg {
        WebhookCfg {
            enabled,
            secret: secret.map(str::to_string),
            board: "board".into(),
        }
    }

    fn headers(sig: Option<&str>, event: &str) -> Value {
        let mut h = json!({ EVENT_HEADER: event });
        if let Some(s) = sig {
            h[SIG_HEADER] = json!(s);
        }
        h
    }

    #[test]
    fn valid_signature_is_accepted() {
        // RFC 4231 test case 2: key "Jefe", data "what do ya want for nothing?".
        let sig = "sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843";
        assert!(verify(b"Jefe", b"what do ya want for nothing?", Some(sig)));
    }

    #[test]
    fn bad_signature_is_refused() {
        let body = r#"{"action":"opened"}"#;
        let sig = sign("s3cret", body);
        let tampered = r#"{"action":"opened "}"#;
        assert!(!verify(b"s3cret", tampered.as_bytes(), Some(&sig)));
    }

    #[test]
    fn missing_signature_is_refused() {
        assert!(!verify(b"s3cret", b"{}", None));
        assert!(!verify(b"s3cret", b"{}", Some("sha256=zz")));
    }

    #[tokio::test]
    async fn issue_event_triggers_board_pull() {
        let fake = Fake::default();
        let body = r#"{"action":"opened"}"#;
        let h = headers(Some(&sign("s", body)), "issues");
        let reply = receive(&fake, &cfg(Some("s"), true), &h, body).await;
        assert_eq!(reply["board_call"], "pull");
        assert_eq!(*fake.calls.lock().unwrap(), vec!["board:pull".to_string()]);
    }

    #[tokio::test]
    async fn pr_event_triggers_board_pull() {
        let fake = Fake::default();
        let body = r#"{"action":"closed"}"#;
        let h = headers(Some(&sign("s", body)), "pull_request");
        receive(&fake, &cfg(Some("s"), true), &h, body).await;
        assert_eq!(*fake.calls.lock().unwrap(), vec!["board:pull".to_string()]);
    }

    #[tokio::test]
    async fn unknown_event_is_ignored() {
        let fake = Fake::default();
        let body = r#"{"zen":"x"}"#;
        let h = headers(Some(&sign("s", body)), "star");
        let reply = receive(&fake, &cfg(Some("s"), true), &h, body).await;
        assert_eq!(reply["accepted"], true);
        assert!(reply["board_call"].is_null());
        assert!(fake.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn disabled_receiver_refuses() {
        let fake = Fake::default();
        let body = "{}";
        let h = headers(Some(&sign("s", body)), "issues");
        let reply = receive(&fake, &cfg(Some("s"), false), &h, body).await;
        assert_eq!(reply["refused"], "webhook disabled");
        assert!(fake.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn secret_never_in_reply() {
        let fake = Fake::default();
        let secret = "top-secret-value";
        let body = r#"{"action":"opened"}"#;
        let bad = headers(Some("sha256=00"), "issues");
        let good = headers(Some(&sign(secret, body)), "issues");
        for h in [bad, good] {
            let reply = receive(&fake, &cfg(Some(secret), true), &h, body)
                .await
                .to_string();
            assert!(!reply.contains(secret));
        }
    }

    #[test]
    fn register_team_registers_webhook() {
        let mut reg = fantastic_kernel::BundleRegistry::new();
        crate::register_team(&mut reg);
        assert!(reg.get(HANDLER_MODULE).is_some());
    }
}
