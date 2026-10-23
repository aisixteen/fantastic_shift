//! Team to team: a stable team id and the JSON envelope that carries a peer
//! verb. The team id is not a secret and carries no host or user name. `check`
//! runs on both send and receive: key-like fields and secret-shaped values are
//! refused, and the refusal names the field or shape, never the value.

use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The only envelope version this code speaks.
pub const VERSION: u32 = 1;
/// The largest encoded envelope accepted, in bytes.
pub const MAX_BYTES: usize = 1024 * 1024;

const KEY_FIELDS: &[&str] = &["token", "key", "secret", "password", "authorization"];
const SECRET_PREFIXES: &[&str] = &["sk-ant-", "ghp_", "github_pat_"];
const PEM_MARK: &str = "PRIVATE KEY-----";
const RUN_MIN: usize = 40;

/// A team's stable id: a random 128-bit value, hex encoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TeamId(String);

impl TeamId {
    /// The id as hex text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Read `<dir>/team_id`, or create it once with a random 128-bit hex id.
    pub fn load_or_create(dir: &Path) -> std::io::Result<Self> {
        let path = dir.join("team_id");
        if let Ok(s) = std::fs::read_to_string(&path) {
            let s = s.trim().to_string();
            if is_hex32(&s) {
                return Ok(Self(s));
            }
        }
        std::fs::create_dir_all(dir)?;
        let mut b = [0u8; 16];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
        let id: String = b.iter().map(|x| format!("{x:02x}")).collect();
        std::fs::write(&path, format!("{id}\n"))?;
        Ok(Self(id))
    }
}

fn is_hex32(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The kind of a peer message: the verbs local control already has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A task for the peer to take.
    Task,
    /// A result for a task.
    Result,
    /// A board read.
    Board,
    /// A start, pause or stop.
    Control(ControlOp),
}

/// A control operation on the peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlOp {
    /// Start.
    Start,
    /// Pause.
    Pause,
    /// Stop.
    Stop,
}

/// One message between two teams.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// Envelope version; must be [`VERSION`].
    pub v: u32,
    /// The sending team.
    pub from: TeamId,
    /// The receiving team.
    pub to: TeamId,
    /// Unique id of this message.
    pub msg_id: String,
    /// The message this one answers, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// What the message asks for.
    pub kind: Kind,
    /// The payload.
    pub body: Value,
}

/// Serialize an envelope to bytes. Callers send through [`encode_checked`].
pub fn encode(env: &Envelope) -> Vec<u8> {
    serde_json::to_vec(env).expect("an envelope always serializes")
}

/// Run [`check`], then encode; refuses an envelope over [`MAX_BYTES`].
pub fn encode_checked(env: &Envelope) -> Result<Vec<u8>, String> {
    check(env)?;
    let bytes = encode(env);
    if bytes.len() > MAX_BYTES {
        return Err(format!(
            "envelope is {} bytes; the limit is {MAX_BYTES}",
            bytes.len()
        ));
    }
    Ok(bytes)
}

/// Parse and check received bytes. Refuses size, unknown version and bad content.
pub fn decode(bytes: &[u8]) -> Result<Envelope, String> {
    if bytes.len() > MAX_BYTES {
        return Err(format!(
            "envelope is {} bytes; the limit is {MAX_BYTES}",
            bytes.len()
        ));
    }
    let raw: Value = serde_json::from_slice(bytes).map_err(|e| format!("not JSON: {e}"))?;
    let v = raw.get("v").and_then(|x| x.as_u64()).unwrap_or(0);
    if v != u64::from(VERSION) {
        return Err(format!("unknown envelope version {v}"));
    }
    let env: Envelope = serde_json::from_value(raw).map_err(|e| format!("not an envelope: {e}"))?;
    check(&env)?;
    Ok(env)
}

/// Check an envelope on send and on receive.
pub fn check(env: &Envelope) -> Result<(), String> {
    if env.v != VERSION {
        return Err(format!("unknown envelope version {}", env.v));
    }
    for id in [&env.from, &env.to] {
        if !is_hex32(id.as_str()) {
            return Err("refused: a team id is not a 128-bit hex id".into());
        }
    }
    if let Some(field) = key_field(&env.body) {
        return Err(format!(
            "refused: body field `{field}` is a credential field"
        ));
    }
    let text = serde_json::to_string(&env.body).unwrap_or_default();
    if let Some(shape) = secret_shape(&text) {
        return Err(format!("refused: body holds a {shape}; no value is echoed"));
    }
    Ok(())
}

/// The first key-like field at any depth, by name.
fn key_field(v: &Value) -> Option<String> {
    match v {
        Value::Object(m) => m.iter().find_map(|(k, x)| {
            if KEY_FIELDS.contains(&k.to_ascii_lowercase().as_str()) {
                Some(k.clone())
            } else {
                key_field(x)
            }
        }),
        Value::Array(a) => a.iter().find_map(key_field),
        _ => None,
    }
}

/// A secret-shaped value in the encoded body, by its shape only.
fn secret_shape(text: &str) -> Option<&'static str> {
    if SECRET_PREFIXES.iter().any(|p| text.contains(p)) {
        return Some("token-prefixed secret");
    }
    if text.contains(PEM_MARK) {
        return Some("private key");
    }
    if has_long_mixed_run(text) {
        return Some("high-entropy string");
    }
    None
}

fn has_long_mixed_run(text: &str) -> bool {
    text.split(|c: char| {
        !(c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '_' || c == '-')
    })
    .any(|run| {
        run.len() >= RUN_MIN
            && run.bytes().any(|b| b.is_ascii_digit())
            && run.bytes().any(|b| b.is_ascii_alphabetic())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "0123456789abcdef0123456789abcdef";
    const B: &str = "fedcba9876543210fedcba9876543210";

    fn env(kind: Kind, body: Value) -> Envelope {
        Envelope {
            v: VERSION,
            from: TeamId(A.into()),
            to: TeamId(B.into()),
            msg_id: "m1".into(),
            reply_to: None,
            kind,
            body,
        }
    }

    #[test]
    fn team_id_is_created_once_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let first = TeamId::load_or_create(dir.path()).unwrap();
        let second = TeamId::load_or_create(dir.path()).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn team_id_is_hex_and_has_no_hostname() {
        let dir = tempfile::tempdir().unwrap();
        let id = TeamId::load_or_create(dir.path()).unwrap();
        assert!(is_hex32(id.as_str()));
        if let Ok(host) = std::env::var("HOSTNAME") {
            assert!(!id.as_str().contains(&host));
        }
    }

    #[test]
    fn envelope_round_trips_each_kind() {
        let kinds = [
            Kind::Task,
            Kind::Result,
            Kind::Board,
            Kind::Control(ControlOp::Start),
            Kind::Control(ControlOp::Pause),
            Kind::Control(ControlOp::Stop),
        ];
        for k in kinds {
            let e = env(k, serde_json::json!({"n": 1}));
            let back = decode(&encode_checked(&e).unwrap()).unwrap();
            assert_eq!(back, e);
        }
    }

    #[test]
    fn an_unknown_version_is_refused() {
        let bytes = br#"{"v":2,"from":"0123456789abcdef0123456789abcdef","to":"fedcba9876543210fedcba9876543210","msg_id":"m","kind":"task","body":{}}"#;
        let err = decode(bytes).unwrap_err();
        assert!(err.contains("version"), "{err}");
    }

    #[test]
    fn an_oversized_message_is_refused() {
        let big = "x".repeat(MAX_BYTES + 1);
        let err = decode(big.as_bytes()).unwrap_err();
        assert!(err.contains("limit"), "{err}");
    }

    #[test]
    fn a_body_with_a_token_field_is_refused_and_not_echoed() {
        let value = "plain-value-hunter2";
        let e = env(Kind::Task, serde_json::json!({"nested": {"token": value}}));
        let err = check(&e).unwrap_err();
        assert!(err.contains("token"), "{err}");
        assert!(!err.contains(value), "{err}");
    }

    #[test]
    fn a_body_with_a_secret_shaped_value_is_refused() {
        let secret = format!("{}{}", "sk-ant-", "api03abcdefghijklmnop");
        let e = env(Kind::Task, serde_json::json!({"note": secret}));
        let err = check(&e).unwrap_err();
        assert!(err.contains("token-prefixed"), "{err}");
        assert!(!err.contains(&secret), "{err}");
    }

    #[test]
    fn a_task_body_without_secrets_passes() {
        let e = env(
            Kind::Task,
            serde_json::json!({"task": "review the board", "id": 7}),
        );
        assert!(check(&e).is_ok());
        assert!(decode(&encode_checked(&e).unwrap()).is_ok());
    }
}
