//! Pack registry index: a local index of signed manifests. Packs are pinned
//! by hash, updates are opt-in and show a permission diff, and a revoked
//! signature blocks new installs and warns existing ones. Pure decisions only:
//! no fetching and no signature verification here.

use serde_json::Value;
use std::collections::BTreeSet;

/// One signed manifest in the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Pack name.
    pub name: String,
    /// Version string, dotted numeric (e.g. `1.2.0`).
    pub version: String,
    /// Content hash the pack is pinned by.
    pub hash: String,
    /// Key that signed this manifest.
    pub signer_key: String,
    /// Download location; must be https.
    pub url: String,
    /// Ports the pack exposes.
    pub ports: Vec<String>,
    /// Grants the pack requests.
    pub grants: Vec<String>,
    /// True when the signature has been revoked.
    pub revoked: bool,
}

/// A parsed registry index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    /// All manifests in the index.
    pub entries: Vec<Entry>,
}

/// Permission changes between two versions of a pack.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Diff {
    /// Ports present in the new version only.
    pub added_ports: Vec<String>,
    /// Ports present in the old version only.
    pub removed_ports: Vec<String>,
    /// Grants present in the new version only.
    pub added_grants: Vec<String>,
    /// Grants present in the old version only.
    pub removed_grants: Vec<String>,
}

/// Outcome of an install check.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// The install may proceed.
    Allow,
    /// The install is refused, with the reason.
    Refuse(String),
}

impl Index {
    /// Parse an index document. Any entry with a non-https url fails the parse.
    pub fn parse(json: &str) -> Result<Self, String> {
        let root: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let items = root
            .get("entries")
            .and_then(Value::as_array)
            .ok_or("index must have an `entries` list")?;
        let mut entries = Vec::with_capacity(items.len());
        for item in items {
            let entry = Entry {
                name: str_field(item, "name")?,
                version: str_field(item, "version")?,
                hash: str_field(item, "hash")?,
                signer_key: str_field(item, "signer_key")?,
                url: str_field(item, "url")?,
                ports: str_list(item, "ports")?,
                grants: str_list(item, "grants")?,
                revoked: item
                    .get("revoked")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            };
            check_https(&entry.url)?;
            entries.push(entry);
        }
        Ok(Index { entries })
    }
}

fn str_field(v: &Value, key: &str) -> Result<String, String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("entry field `{key}` must be a string"))
}

fn str_list(v: &Value, key: &str) -> Result<Vec<String>, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| {
                i.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("`{key}` items must be strings"))
            })
            .collect(),
        Some(_) => Err(format!("`{key}` must be a list")),
    }
}

fn check_https(url: &str) -> Result<(), String> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err(format!("url must be https: {url}"))
    }
}

/// Version parts compared numerically where possible.
fn version_key(v: &str) -> Vec<u64> {
    v.split('.')
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect()
}

/// The entry for exactly this name and pinned hash, or an error.
pub fn resolve<'a>(index: &'a Index, name: &str, pinned_hash: &str) -> Result<&'a Entry, String> {
    index
        .entries
        .iter()
        .find(|e| e.name == name && e.hash == pinned_hash)
        .ok_or_else(|| format!("no entry for {name} at pinned hash {pinned_hash}"))
}

/// The newest non-revoked version of the installed pack. None without opt-in,
/// or when the newest is already the installed hash.
pub fn update_candidate<'a>(
    index: &'a Index,
    installed: (&str, &str),
    opt_in: bool,
) -> Option<&'a Entry> {
    if !opt_in {
        return None;
    }
    let (name, hash) = installed;
    let best = index
        .entries
        .iter()
        .filter(|e| e.name == name && !e.revoked)
        .max_by_key(|e| version_key(&e.version))?;
    if best.hash == hash {
        None
    } else {
        Some(best)
    }
}

fn difference(from: &[String], minus: &[String]) -> Vec<String> {
    let minus: BTreeSet<&str> = minus.iter().map(String::as_str).collect();
    from.iter()
        .filter(|x| !minus.contains(x.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// What moving from `old` to `new` adds and removes in ports and grants.
pub fn permission_diff(old: &Entry, new: &Entry) -> Diff {
    Diff {
        added_ports: difference(&new.ports, &old.ports),
        removed_ports: difference(&old.ports, &new.ports),
        added_grants: difference(&new.grants, &old.grants),
        removed_grants: difference(&old.grants, &new.grants),
    }
}

/// Whether a new install of this entry is allowed.
pub fn install_decision(entry: &Entry) -> Decision {
    if entry.revoked {
        return Decision::Refuse("revoked".to_string());
    }
    if check_https(&entry.url).is_err() {
        return Decision::Refuse("non-https url".to_string());
    }
    Decision::Allow
}

/// One warning per installed (name, hash) whose index entry is revoked.
pub fn installed_warnings(index: &Index, installed: &[(&str, &str)]) -> Vec<String> {
    installed
        .iter()
        .filter_map(|(name, hash)| {
            index
                .entries
                .iter()
                .find(|e| e.name == *name && e.hash == *hash && e.revoked)
                .map(|e| {
                    format!(
                        "{} {} is revoked (signer {}): stop using it",
                        e.name, e.version, e.signer_key
                    )
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = r#"{"entries":[
 {"name":"notes","version":"1.0.0","hash":"h1","signer_key":"k1","url":"https://example.org/notes-1.0.0.tar","ports":["read"],"grants":["files.read"]},
 {"name":"notes","version":"1.2.0","hash":"h2","signer_key":"k1","url":"https://example.org/notes-1.2.0.tar","ports":["read","write"],"grants":["files.read"]},
 {"name":"notes","version":"1.1.0","hash":"h3","signer_key":"k1","url":"https://example.org/notes-1.1.0.tar","ports":["read"],"grants":["files.read"]},
 {"name":"notes","version":"1.3.0","hash":"h4","signer_key":"k2","url":"https://example.org/notes-1.3.0.tar","revoked":true}
]}"#;

    #[test]
    fn index_parses() {
        let idx = Index::parse(INDEX).unwrap();
        assert_eq!(idx.entries.len(), 4);
        assert_eq!(idx.entries[0].ports, vec!["read".to_string()]);
        assert!(idx.entries[3].revoked);
    }

    #[test]
    fn a_non_https_url_is_refused() {
        let json = r#"{"entries":[{"name":"x","version":"1.0.0","hash":"h","signer_key":"k","url":"http://example.org/x.tar"}]}"#;
        assert!(Index::parse(json).is_err());
    }

    #[test]
    fn resolve_matches_only_the_pinned_hash() {
        let idx = Index::parse(INDEX).unwrap();
        assert_eq!(resolve(&idx, "notes", "h2").unwrap().version, "1.2.0");
        assert!(resolve(&idx, "notes", "h9").is_err());
        assert!(resolve(&idx, "other", "h2").is_err());
    }

    #[test]
    fn no_update_without_opt_in() {
        let idx = Index::parse(INDEX).unwrap();
        assert!(update_candidate(&idx, ("notes", "h1"), false).is_none());
    }

    #[test]
    fn update_picks_the_newest_unrevoked() {
        let idx = Index::parse(INDEX).unwrap();
        let candidate = update_candidate(&idx, ("notes", "h1"), true).unwrap();
        assert_eq!(candidate.version, "1.2.0");
        assert!(update_candidate(&idx, ("notes", "h2"), true).is_none());
    }

    #[test]
    fn permission_diff_lists_added_and_removed() {
        let idx = Index::parse(INDEX).unwrap();
        let old = resolve(&idx, "notes", "h1").unwrap();
        let new = resolve(&idx, "notes", "h2").unwrap();
        let up = permission_diff(old, new);
        assert_eq!(up.added_ports, vec!["write".to_string()]);
        assert!(up.removed_ports.is_empty());
        assert!(up.added_grants.is_empty());
        let down = permission_diff(new, old);
        assert_eq!(down.removed_ports, vec!["write".to_string()]);
        assert!(down.added_ports.is_empty());
    }

    #[test]
    fn a_revoked_entry_refuses_new_installs() {
        let idx = Index::parse(INDEX).unwrap();
        let revoked = resolve(&idx, "notes", "h4").unwrap();
        assert_eq!(
            install_decision(revoked),
            Decision::Refuse("revoked".to_string())
        );
        let fine = resolve(&idx, "notes", "h2").unwrap();
        assert_eq!(install_decision(fine), Decision::Allow);
    }

    #[test]
    fn an_installed_revoked_pack_gets_a_warning() {
        let idx = Index::parse(INDEX).unwrap();
        let warnings = installed_warnings(&idx, &[("notes", "h4"), ("notes", "h1")]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("revoked"));
    }
}
