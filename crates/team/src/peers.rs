//! How a team names its peers (#47): `--peer <addr>` flags and a `peers:` list
//! in `shift.yaml`, merged by name with the flags winning. Who leads is
//! configuration: a `group` peer is a remote group the team leads, an `equal`
//! peer is a team it talks to as a peer. Addresses are `relay:<opaque id>`
//! (through the relay, as it is) or `direct:<host:port>`, which must be
//! loopback (same machine or an SSH tunnel). Keys never cross: a peer entry
//! with a key-like field is refused.

use std::net::SocketAddr;

use serde_yaml::Value;

/// Field names that would carry a key or secret; a peer entry with one is refused.
const KEY_FIELDS: &[&str] = &["token", "key", "secret", "password"];

/// Where a peer can be reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerAddr {
    /// Through the relay, by its opaque id.
    Relay(String),
    /// Directly, on a loopback address.
    Direct(SocketAddr),
}

impl PeerAddr {
    /// Parse `relay:<id>` or `direct:<host:port>` (loopback only).
    pub fn parse(s: &str) -> Result<Self, String> {
        if let Some(id) = s.strip_prefix("relay:") {
            if id.is_empty() {
                return Err("relay address needs an id after `relay:`".to_string());
            }
            return Ok(PeerAddr::Relay(id.to_string()));
        }
        if let Some(a) = s.strip_prefix("direct:") {
            let sa: SocketAddr = a
                .parse()
                .map_err(|e| format!("bad direct address `{a}`: {e}"))?;
            if !sa.ip().is_loopback() {
                return Err(format!("direct address `{a}` must be loopback"));
            }
            return Ok(PeerAddr::Direct(sa));
        }
        Err(format!("address `{s}` must start with relay: or direct:"))
    }
}

/// Who leads: a remote group the team leads, or an equal peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Role {
    /// A remote group this team leads (the default).
    #[default]
    Group,
    /// A team this team talks to as an equal.
    Equal,
}

/// One configured peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerSpec {
    /// The name the peer is known by; unique within a source.
    pub name: String,
    /// Where the peer is reached.
    pub addr: PeerAddr,
    /// Its role.
    pub role: Role,
}

fn parse_role(s: &str) -> Result<Role, String> {
    match s {
        "group" => Ok(Role::Group),
        "equal" => Ok(Role::Equal),
        other => Err(format!("role must be `group` or `equal`, got `{other}`")),
    }
}

fn scalar(v: &Value, key: &str) -> Result<String, String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        _ => Err(format!("peer field `{key}` must be text")),
    }
}

fn entry_from_yaml(v: &Value) -> Result<PeerSpec, String> {
    let map = v.as_mapping().ok_or("each peers entry must be a mapping")?;
    let mut name: Option<String> = None;
    let mut addr: Option<String> = None;
    let mut role = Role::default();
    for (k, val) in map {
        let key = k.as_str().ok_or("peer keys must be strings")?;
        if KEY_FIELDS.contains(&key) {
            return Err(format!("peer field `{key}` refused: keys never cross"));
        }
        match key {
            "name" => name = Some(scalar(val, key)?),
            "addr" => addr = Some(scalar(val, key)?),
            "role" => role = parse_role(&scalar(val, key)?)?,
            _ => {}
        }
    }
    let addr_s = addr.ok_or("a peers entry needs an addr")?;
    let parsed = PeerAddr::parse(&addr_s)?;
    let name = name.unwrap_or_else(|| addr_s.clone());
    Ok(PeerSpec {
        name,
        addr: parsed,
        role,
    })
}

/// Read the `peers:` key of a `shift.yaml` text; other keys are ignored. A
/// missing key, or an empty file, gives no peers.
pub fn peers_from_yaml(yaml: &str) -> Result<Vec<PeerSpec>, String> {
    if yaml.trim().is_empty() {
        return Ok(Vec::new());
    }
    let doc: Value = serde_yaml::from_str(yaml).map_err(|e| format!("shift.yaml: {e}"))?;
    let list = match doc.get("peers") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(v) => v,
    };
    let seq = list.as_sequence().ok_or("peers must be a list")?;
    let mut out: Vec<PeerSpec> = Vec::new();
    for item in seq {
        let spec = entry_from_yaml(item)?;
        if out.iter().any(|p| p.name == spec.name) {
            return Err(format!("duplicate peer name `{}`", spec.name));
        }
        out.push(spec);
    }
    Ok(out)
}

fn spec_from_arg(v: &str) -> Result<PeerSpec, String> {
    let (name, addr_s) = match v.find('=') {
        Some(i) if v[i + 1..].starts_with("relay:") || v[i + 1..].starts_with("direct:") => {
            (Some(v[..i].to_string()), &v[i + 1..])
        }
        _ => (None, v),
    };
    let addr = PeerAddr::parse(addr_s)?;
    let name = name.unwrap_or_else(|| addr_s.to_string());
    if name.is_empty() {
        return Err(format!("peer `{v}` has an empty name"));
    }
    Ok(PeerSpec {
        name,
        addr,
        role: Role::Group,
    })
}

/// Read every `--peer` value in `args`, as `name=addr` or a bare addr (named
/// by its address). `--peer=addr` works too. Other arguments are skipped.
pub fn peers_from_args(args: &[String]) -> Result<Vec<PeerSpec>, String> {
    let mut out: Vec<PeerSpec> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let value = if a == "--peer" {
            i += 1;
            args.get(i).ok_or("--peer needs a value")?.clone()
        } else if let Some(v) = a.strip_prefix("--peer=") {
            v.to_string()
        } else {
            i += 1;
            continue;
        };
        i += 1;
        let spec = spec_from_arg(&value)?;
        if out.iter().any(|p| p.name == spec.name) {
            return Err(format!("duplicate peer name `{}`", spec.name));
        }
        out.push(spec);
    }
    Ok(out)
}

/// Merge the `shift.yaml` peers with the `--peer` flags. A flag wins over a
/// yaml peer of the same name; the rest keep the yaml order, new flags follow.
pub fn merge(yaml: &str, args: &[String]) -> Result<Vec<PeerSpec>, String> {
    let mut out = peers_from_yaml(yaml)?;
    for a in peers_from_args(args)? {
        match out.iter().position(|p| p.name == a.name) {
            Some(i) => out[i] = a,
            None => out.push(a),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn relay_and_loopback_direct_parse() {
        assert_eq!(
            PeerAddr::parse("relay:abc123").unwrap(),
            PeerAddr::Relay("abc123".to_string())
        );
        assert!(matches!(
            PeerAddr::parse("direct:127.0.0.1:7000").unwrap(),
            PeerAddr::Direct(_)
        ));
        assert!(matches!(
            PeerAddr::parse("direct:[::1]:7000").unwrap(),
            PeerAddr::Direct(_)
        ));
    }

    #[test]
    fn a_non_loopback_direct_addr_is_refused() {
        assert!(PeerAddr::parse("direct:10.0.0.5:7000").is_err());
        assert!(PeerAddr::parse("direct:example.com:7000").is_err());
        assert!(PeerAddr::parse("relay:").is_err());
        assert!(PeerAddr::parse("tcp:127.0.0.1:7000").is_err());
    }

    #[test]
    fn yaml_peers_parse_with_roles() {
        let yaml = r#"
peers:
  - name: alpha
    addr: relay:alpha-id
    role: equal
  - addr: direct:127.0.0.1:7100
other: 1
"#;
        let peers = peers_from_yaml(yaml).unwrap();
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].name, "alpha");
        assert_eq!(peers[0].role, Role::Equal);
        assert_eq!(peers[1].name, "direct:127.0.0.1:7100");
        assert_eq!(peers[1].role, Role::Group);
    }

    #[test]
    fn missing_peers_key_is_empty() {
        assert_eq!(peers_from_yaml("other: 1\n").unwrap(), Vec::new());
        assert_eq!(peers_from_yaml("").unwrap(), Vec::new());
    }

    #[test]
    fn repeated_peer_flags_all_count() {
        let peers = peers_from_args(&argv(&[
            "--dir",
            "x",
            "--peer",
            "relay:a",
            "--peer",
            "alpha=direct:127.0.0.1:7000",
        ]))
        .unwrap();
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].name, "relay:a");
        assert_eq!(peers[1].name, "alpha");
    }

    #[test]
    fn args_override_yaml_by_name() {
        let yaml = "peers:\n  - name: alpha\n    addr: relay:x\n";
        let merged = merge(yaml, &argv(&["--peer", "alpha=relay:y"])).unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].addr, PeerAddr::Relay("y".to_string()));
    }

    #[test]
    fn duplicate_names_in_yaml_are_an_error() {
        let yaml =
            "peers:\n  - name: alpha\n    addr: relay:x\n  - name: alpha\n    addr: relay:y\n";
        assert!(peers_from_yaml(yaml).is_err());
    }

    #[test]
    fn a_peer_with_a_token_field_is_refused() {
        let yaml = "peers:\n  - name: alpha\n    addr: relay:x\n    token: abc\n";
        assert!(peers_from_yaml(yaml).is_err());
    }
}
