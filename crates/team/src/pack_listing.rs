//! Pack listing: the packs a team can install, as messages for the web UI, the
//! socket and MCP alike. `list` and `show` read a LOCAL index json and the
//! installed templates directory through the file bridge; nothing here reaches
//! the network. Index text is untrusted: it is parsed, never executed.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::puller::{KernelPeers, Peers};

/// The bundle's handler module name.
pub const HANDLER_MODULE: &str = "team_packs.tools";

/// A pack's measured record, as the index holds it. Absent fields stay `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    /// Tasks the record measured.
    pub tasks: Option<u64>,
    /// Share of tasks that succeeded, 0.0 to 1.0.
    pub success_rate: Option<f64>,
    /// Mean cost per task.
    pub cost_per_task: Option<f64>,
}

/// One pack from the index: only the fields the listing shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Pack name, unique in the index.
    pub name: String,
    /// Pack version.
    pub version: String,
    /// One-line summary.
    pub summary: String,
    /// Port names the pack exposes.
    pub ports: Vec<String>,
    /// Permissions the pack asks for.
    pub grants: Vec<String>,
    /// Number of evals, when the index has them.
    pub evals_count: Option<u64>,
    /// Measured record, when the index has one.
    pub measured: Option<Measured>,
    /// Price as the index states it; `null` when absent.
    pub price: Value,
    /// Whether the pack has been revoked.
    pub revoked: bool,
}

/// The listing's config, read from its own record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingCfg {
    /// Local index json, read through the file bridge.
    pub index_path: String,
    /// Installed templates directory, listed through the file bridge.
    pub templates_dir: String,
    /// The file bridge agent. Defaults to `files`.
    pub store: String,
}

/// Parse the index json: its `packs` array. Entries without a name are skipped.
pub fn parse_index(index: &Value) -> Result<Vec<Entry>, String> {
    let packs = index["packs"]
        .as_array()
        .ok_or("index has no `packs` array")?;
    Ok(packs.iter().filter_map(parse_entry).collect())
}

fn parse_entry(v: &Value) -> Option<Entry> {
    let name = v["name"].as_str()?.to_string();
    let strings = |k: &str| -> Vec<String> {
        v[k].as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let measured = v
        .get("measured")
        .filter(|m| m.is_object())
        .map(|m| Measured {
            tasks: m["tasks"].as_u64(),
            success_rate: m["success_rate"].as_f64(),
            cost_per_task: m["cost_per_task"].as_f64(),
        });
    Some(Entry {
        name,
        version: v["version"].as_str().unwrap_or("").to_string(),
        summary: v["summary"].as_str().unwrap_or("").to_string(),
        ports: strings("ports"),
        grants: strings("grants"),
        evals_count: v["evals_count"].as_u64(),
        measured,
        price: v.get("price").cloned().unwrap_or(Value::Null),
        revoked: v["revoked"].as_bool().unwrap_or(false),
    })
}

/// The names installed in the templates directory, from its listing
/// (`entries`: strings, or objects with a `name`).
pub fn installed_names(templates: &Value) -> BTreeSet<String> {
    templates["entries"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().or_else(|| x["name"].as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The short card a list shows for one pack.
pub fn to_card(e: &Entry, installed: bool) -> Value {
    json!({
        "name": e.name,
        "version": e.version,
        "summary": e.summary,
        "price": e.price,
        "installed": installed,
        "revoked": e.revoked,
    })
}

/// Cards for the packs matching `query` (substring of name or summary,
/// case-insensitive; none or empty matches all), sorted by name.
pub fn list_view(
    entries: &[Entry],
    installed: &BTreeSet<String>,
    query: Option<&str>,
) -> Vec<Value> {
    let q = query.map(str::to_lowercase).filter(|q| !q.is_empty());
    let mut hits: Vec<&Entry> = entries
        .iter()
        .filter(|e| match &q {
            None => true,
            Some(q) => e.name.to_lowercase().contains(q) || e.summary.to_lowercase().contains(q),
        })
        .collect();
    hits.sort_by(|a, b| a.name.cmp(&b.name));
    hits.into_iter()
        .map(|e| to_card(e, installed.contains(&e.name)))
        .collect()
}

fn measured_json(m: &Measured) -> Value {
    let mut o = serde_json::Map::new();
    if let Some(t) = m.tasks {
        o.insert("tasks".into(), json!(t));
    }
    if let Some(s) = m.success_rate {
        o.insert("success_rate".into(), json!(s));
    }
    if let Some(c) = m.cost_per_task {
        o.insert("cost_per_task".into(), json!(c));
    }
    Value::Object(o)
}

/// The full view of one pack. Unknown name is an error. Fields the index
/// does not have (evals, measured record) are left out, not invented.
pub fn show_view(
    entries: &[Entry],
    installed: &BTreeSet<String>,
    name: &str,
) -> Result<Value, String> {
    let e = entries
        .iter()
        .find(|e| e.name == name)
        .ok_or_else(|| format!("no pack named `{name}`"))?;
    let mut out = json!({
        "name": e.name,
        "version": e.version,
        "summary": e.summary,
        "ports": e.ports,
        "grants": e.grants,
        "price": e.price,
        "installed": installed.contains(name),
        "revoked": e.revoked,
    });
    if let Some(obj) = out.as_object_mut() {
        if let Some(n) = e.evals_count {
            obj.insert("evals_count".into(), json!(n));
        }
        if let Some(m) = &e.measured {
            obj.insert("measured".into(), measured_json(m));
        }
    }
    Ok(out)
}

/// Read the index and the installed names through the file bridge. A missing
/// or unreadable templates listing means nothing is installed.
pub async fn load(
    peers: &dyn Peers,
    cfg: &ListingCfg,
) -> Result<(Vec<Entry>, BTreeSet<String>), String> {
    let index_r = peers
        .ask(&cfg.store, json!({"type": "read", "path": cfg.index_path}))
        .await;
    let text = index_r["content"]
        .as_str()
        .ok_or_else(|| format!("index: {index_r}"))?;
    let index: Value = serde_json::from_str(text).map_err(|e| format!("index is not json: {e}"))?;
    let entries = parse_index(&index)?;
    let templates = peers
        .ask(
            &cfg.store,
            json!({"type": "list", "path": cfg.templates_dir}),
        )
        .await;
    Ok((entries, installed_names(&templates)))
}

/// Read the listing's config from the agent's own record.
fn cfg_of(
    kernel: &Arc<fantastic_kernel::Kernel>,
    id: &fantastic_kernel::AgentId,
) -> Result<ListingCfg, String> {
    let meta = kernel
        .agents
        .get(id)
        .map(|e| e.meta.read().expect("meta").clone())
        .unwrap_or_default();
    let get = |k: &str| {
        meta.get(k)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|v| !v.trim().is_empty())
    };
    Ok(ListingCfg {
        index_path: get("index_path").ok_or("team_packs: record needs `index_path`")?,
        templates_dir: get("templates_dir").ok_or("team_packs: record needs `templates_dir`")?,
        store: get("store").unwrap_or_else(|| "files".to_string()),
    })
}

/// The pack listing bundle: `list` and `show`.
pub struct ListingBundle;

#[async_trait::async_trait]
impl fantastic_kernel::bundle::Bundle for ListingBundle {
    fn name(&self) -> &str {
        HANDLER_MODULE
    }

    fn verb_docs(&self) -> Value {
        json!({
            "list": "list {query?} → [{name, version, summary, price, installed, revoked}]",
            "show": "show {name} → {name, version, summary, ports, grants, evals_count?, measured?, price, installed, revoked}",
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
        let peers = KernelPeers(kernel.clone());
        let out = match cfg_of(kernel, agent_id) {
            Err(e) => json!({"error": e}),
            Ok(cfg) => match t {
                "list" | "show" => match load(&peers, &cfg).await {
                    Err(e) => json!({"error": e}),
                    Ok((entries, installed)) => {
                        if t == "list" {
                            let query = payload.get("query").and_then(|v| v.as_str());
                            json!(list_view(&entries, &installed, query))
                        } else {
                            let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            show_view(&entries, &installed, name)
                                .unwrap_or_else(|e| json!({"error": e}))
                        }
                    }
                },
                other => json!({"error": format!("team_packs: unknown type '{other}'")}),
            },
        };
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        index: Option<String>,
        installed: Vec<String>,
    }

    #[async_trait::async_trait]
    impl Peers for Fake {
        async fn ask(&self, _to: &str, p: Value) -> Value {
            match p["type"].as_str() {
                Some("read") => match &self.index {
                    Some(t) => json!({"content": t}),
                    None => json!({"error": "not found"}),
                },
                Some("list") => json!({"entries": self.installed}),
                _ => json!({"error": "unknown"}),
            }
        }
    }

    const INDEX: &str = r#"{"packs":[
        {"name":"zeta","version":"1.0","summary":"Zeta pack","ports":["in"],"grants":["read"],"evals_count":3,"measured":{"tasks":10,"success_rate":0.9,"cost_per_task":0.02},"price":0},
        {"name":"alpha","version":"0.1","summary":"Alpha search tool","ports":[],"grants":[],"price":5},
        {"name":"mid","version":"2.0","summary":"Mid","ports":["out"],"grants":[],"revoked":true,"price":1}
    ]}"#;

    fn cfg() -> ListingCfg {
        ListingCfg {
            index_path: "packs/index.json".into(),
            templates_dir: "templates".into(),
            store: "files".into(),
        }
    }

    async fn loaded(installed: &[&str]) -> (Vec<Entry>, BTreeSet<String>) {
        let fake = Fake {
            index: Some(INDEX.into()),
            installed: installed.iter().map(|s| s.to_string()).collect(),
        };
        load(&fake, &cfg()).await.unwrap()
    }

    #[tokio::test]
    async fn list_marks_installed_packs() {
        let (entries, installed) = loaded(&["alpha"]).await;
        let cards = list_view(&entries, &installed, None);
        let alpha = cards.iter().find(|c| c["name"] == "alpha").unwrap();
        let zeta = cards.iter().find(|c| c["name"] == "zeta").unwrap();
        assert_eq!(alpha["installed"], json!(true));
        assert_eq!(zeta["installed"], json!(false));
    }

    #[tokio::test]
    async fn list_filters_by_query() {
        let (entries, installed) = loaded(&[]).await;
        let by_name = list_view(&entries, &installed, Some("ALPHA"));
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0]["name"], json!("alpha"));
        let by_summary = list_view(&entries, &installed, Some("search"));
        assert_eq!(by_summary.len(), 1);
        assert_eq!(list_view(&entries, &installed, Some("")).len(), 3);
    }

    #[tokio::test]
    async fn list_is_sorted_by_name() {
        let (entries, installed) = loaded(&[]).await;
        let names: Vec<Value> = list_view(&entries, &installed, None)
            .iter()
            .map(|c| c["name"].clone())
            .collect();
        assert_eq!(names, vec![json!("alpha"), json!("mid"), json!("zeta")]);
    }

    #[tokio::test]
    async fn show_returns_ports_grants_and_evals_count() {
        let (entries, installed) = loaded(&[]).await;
        let v = show_view(&entries, &installed, "zeta").unwrap();
        assert_eq!(v["ports"], json!(["in"]));
        assert_eq!(v["grants"], json!(["read"]));
        assert_eq!(v["evals_count"], json!(3));
        assert_eq!(v["measured"]["tasks"], json!(10));
    }

    #[tokio::test]
    async fn show_omits_a_missing_measured_record() {
        let (entries, installed) = loaded(&[]).await;
        let v = show_view(&entries, &installed, "alpha").unwrap();
        assert!(v.get("measured").is_none());
        assert!(v.get("evals_count").is_none());
    }

    #[tokio::test]
    async fn show_unknown_name_is_an_error() {
        let (entries, installed) = loaded(&[]).await;
        assert!(show_view(&entries, &installed, "nope").is_err());
    }

    #[tokio::test]
    async fn a_revoked_pack_is_marked() {
        let (entries, installed) = loaded(&[]).await;
        let cards = list_view(&entries, &installed, None);
        let mid = cards.iter().find(|c| c["name"] == "mid").unwrap();
        assert_eq!(mid["revoked"], json!(true));
    }

    #[test]
    fn register_team_registers_listing() {
        let mut reg = fantastic_kernel::BundleRegistry::new();
        crate::register_team(&mut reg);
        assert!(reg.get(HANDLER_MODULE).is_some());
    }
}
