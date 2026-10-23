//! The pack manifest (#40): what a worker pack says about itself. Name,
//! version, author, license, price (free only, for now), the content hash of
//! its files, an opaque signature, and its PORTS: what the pack needs from the
//! team (a model with requirements, a connector, a sandboxed exec). Never a
//! model name, a key or a token. Signature verification and wiring into
//! `pack`/`install` come later.

use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The file name a pack's manifest lives in, inside the pack's folder.
pub const MANIFEST_FILE: &str = "pack.yaml";

/// The only price this version accepts.
pub const FREE: &str = "free";

/// Prefixes that mark a value as a key or token rather than a label.
const KEY_PREFIXES: [&str; 8] = [
    "sk-",
    "ghp_",
    "gho_",
    "ghs_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "AKIA",
];

/// A pack manifest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The pack's name.
    pub name: String,
    /// Its version.
    pub version: String,
    /// Its author.
    pub author: String,
    /// Its license.
    pub license: String,
    /// Its price; `free` in this version.
    #[serde(default = "default_price")]
    pub price: String,
    /// The sha256 of its files (see [`content_hash`]), hex.
    #[serde(default)]
    pub content_hash: String,
    /// An opaque signature; verified later.
    #[serde(default)]
    pub signature: String,
    /// What the pack needs from the team, as port strings (see [`Port`]).
    #[serde(default)]
    pub ports: Vec<String>,
    /// The files the pack ships, relative to its folder.
    #[serde(default)]
    pub files: Vec<String>,
}

fn default_price() -> String {
    FREE.to_string()
}

/// What a pack needs from the team.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Port {
    /// A model with requirements.
    Model(Vec<Requirement>),
    /// A connector, by name.
    Connector(String),
    /// A sandboxed exec.
    ExecSandboxed,
}

/// A requirement on a model port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Requirement {
    /// A context window of at least this many tokens.
    Window(u64),
    /// Tool use.
    Tools,
}

impl Port {
    /// Parse one port string: `@model window=200k tools`, `@connector:<name>`
    /// or `@exec:sandboxed`. Any other kind is refused.
    pub fn parse(s: &str) -> Result<Port, String> {
        let mut parts = s.split_whitespace();
        let head = parts.next().ok_or_else(|| "empty port".to_string())?;
        if head == "@model" {
            let mut reqs = Vec::new();
            for r in parts {
                if r == "tools" {
                    reqs.push(Requirement::Tools);
                } else if let Some(v) = r.strip_prefix("window=") {
                    let n = parse_tokens(v).ok_or_else(|| format!("bad window `{v}` in `{s}`"))?;
                    reqs.push(Requirement::Window(n));
                } else {
                    return Err(format!("unknown model requirement `{r}` in `{s}`"));
                }
            }
            return Ok(Port::Model(reqs));
        }
        if parts.next().is_some() {
            return Err(format!("a port takes one word, got `{s}`"));
        }
        if let Some(name) = head.strip_prefix("@connector:") {
            if name.is_empty() {
                return Err(format!("connector port needs a name: `{s}`"));
            }
            return Ok(Port::Connector(name.to_string()));
        }
        if head == "@exec:sandboxed" {
            return Ok(Port::ExecSandboxed);
        }
        Err(format!("unknown port kind `{head}`"))
    }
}

/// A token count such as `8192`, `200k` or `1m`.
fn parse_tokens(v: &str) -> Option<u64> {
    let v = v.to_ascii_lowercase();
    let (digits, mult) = if let Some(d) = v.strip_suffix('k') {
        (d, 1_000)
    } else if let Some(d) = v.strip_suffix('m') {
        (d, 1_000_000)
    } else {
        (v.as_str(), 1)
    };
    digits.parse::<u64>().ok().and_then(|n| n.checked_mul(mult))
}

impl Manifest {
    /// Parse a manifest from YAML text. It does not validate; call
    /// [`Manifest::validate`] for that.
    pub fn parse(text: &str) -> Result<Manifest, String> {
        let v = fantastic_kernel::yaml_io::load(text).map_err(|e| format!("not YAML — {e}"))?;
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    /// The parsed ports, in order.
    pub fn port_list(&self) -> Result<Vec<Port>, String> {
        self.ports.iter().map(|p| Port::parse(p.as_str())).collect()
    }

    /// Check the rules; the first failure is the error.
    pub fn validate(&self) -> Result<(), String> {
        for (field, val) in [
            ("name", &self.name),
            ("version", &self.version),
            ("author", &self.author),
            ("license", &self.license),
        ] {
            if val.trim().is_empty() {
                return Err(format!("{field} is empty"));
            }
            if looks_like_key(val) {
                return Err(format!(
                    "{field} looks like a key or token; a manifest holds no secrets"
                ));
            }
        }
        if self.price != FREE {
            return Err(format!(
                "price must be `{FREE}` in this version, got `{}`",
                self.price
            ));
        }
        self.port_list()?;
        for f in &self.files {
            check_relative(f)?;
        }
        Ok(())
    }
}

/// Whether a value looks like a key or token: a known key prefix, a private
/// key block, or one long unbroken alphanumeric run mixing letters and digits.
fn looks_like_key(s: &str) -> bool {
    let t = s.trim();
    if KEY_PREFIXES.iter().any(|p| t.starts_with(p))
        || (t.contains("BEGIN") && t.contains("PRIVATE KEY"))
    {
        return true;
    }
    t.len() >= 32
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && t.chars().any(|c| c.is_ascii_digit())
        && t.chars().any(|c| c.is_ascii_alphabetic())
}

/// A listed file must be relative and stay inside the pack.
fn check_relative(f: &str) -> Result<(), String> {
    if f.trim().is_empty() {
        return Err("a listed file is empty".to_string());
    }
    let p = Path::new(f);
    if p.is_absolute()
        || p.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "listed file `{f}` must be relative and stay inside the pack"
        ));
    }
    Ok(())
}

/// The content hash of a pack: sha256 over its files sorted by relative path,
/// each as its path, a NUL, its length (u64 little-endian) and its bytes; hex.
/// The manifest file itself is left out.
pub fn content_hash(files: &[(String, Vec<u8>)]) -> String {
    let mut sorted: Vec<&(String, Vec<u8>)> = files
        .iter()
        .filter(|(p, _)| p.as_str() != MANIFEST_FILE)
        .collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = Sha256::new();
    for (path, bytes) in sorted {
        h.update(path.as_bytes());
        h.update([0u8]);
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = "name: review-pack\nversion: 0.1.0\nauthor: aisixteen\nlicense: MIT\nprice: free\ncontent_hash: abc123\nsignature: opaque-sig\nports:\n  - \"@model window=200k tools\"\n  - \"@connector:github\"\n  - \"@exec:sandboxed\"\nfiles:\n  - src/lib.rs\n  - README.md\n";

    fn files() -> Vec<(String, Vec<u8>)> {
        vec![
            ("src/lib.rs".to_string(), b"fn a() {}".to_vec()),
            ("README.md".to_string(), b"hi".to_vec()),
        ]
    }

    #[test]
    fn a_full_manifest_round_trips() {
        let m = Manifest::parse(FULL).unwrap();
        assert_eq!(m.name, "review-pack");
        assert!(m.validate().is_ok());
        let out = serde_yaml::to_string(&m).unwrap();
        let back = Manifest::parse(&out).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn ports_parse_model_connector_and_exec() {
        let m = Manifest::parse(FULL).unwrap();
        assert_eq!(
            m.port_list().unwrap(),
            vec![
                Port::Model(vec![Requirement::Window(200_000), Requirement::Tools]),
                Port::Connector("github".to_string()),
                Port::ExecSandboxed,
            ]
        );
    }

    #[test]
    fn an_unknown_port_kind_is_refused() {
        assert!(Port::parse("@keys:anthropic").is_err());
        assert!(Port::parse("@exec:full").is_err());
        assert!(Port::parse("@model gpu=yes").is_err());
    }

    #[test]
    fn a_paid_price_is_refused_in_this_version() {
        let m = Manifest::parse(&FULL.replace("price: free", "price: \"5\"")).unwrap();
        assert!(m.validate().is_err());
    }

    #[test]
    fn content_hash_is_stable_under_file_order() {
        let mut rev = files();
        rev.reverse();
        assert_eq!(content_hash(&files()), content_hash(&rev));
    }

    #[test]
    fn content_hash_changes_with_one_byte() {
        let mut changed = files();
        changed[1].1 = b"hj".to_vec();
        assert_ne!(content_hash(&files()), content_hash(&changed));
    }

    #[test]
    fn content_hash_ignores_the_manifest_file() {
        let mut with = files();
        with.push((MANIFEST_FILE.to_string(), b"name: x".to_vec()));
        assert_eq!(content_hash(&files()), content_hash(&with));
    }

    #[test]
    fn a_manifest_with_a_key_shaped_value_is_refused() {
        let m = Manifest::parse(&FULL.replace(
            "author: aisixteen",
            &format!("author: {}{}", "sk-ant-", "api03-abcdefghijklmnop"),
        ))
        .unwrap();
        assert!(m.validate().is_err());
    }

    #[test]
    fn a_path_with_dotdot_is_refused() {
        let m = Manifest::parse(&FULL.replace("README.md", "../secrets.txt")).unwrap();
        assert!(m.validate().is_err());
    }
}
