//! What `team.yaml` holds: the agent templates an agent may start agents
//! from, the connectors and their directives, the queue's id, and the
//! credential store. It does not hold how many agents run (the first agent's
//! instructions say that), nor any provider or model name in code.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use fantastic_kernel::bootstrap::DEFAULT_ROOT_ID;
use serde::Deserialize;
use serde_json::{json, Map, Value};

/// The file name the team looks for in its directory.
pub const FILE_NAME: &str = "team.yaml";

/// Keys a template's `extra` may not set.
pub const RESERVED: [&str; 12] = [
    "type",
    "id",
    "parent_id",
    "handler_module",
    "model",
    "endpoint",
    "credentials",
    "price_input_per_mtok",
    "price_output_per_mtok",
    "memory_store_id",
    "can_call",
    "callable_by",
];

/// Keys a connector's `record` may not set.
pub const CONNECTOR_RESERVED: [&str; 4] = ["type", "id", "parent_id", "handler_module"];

/// The whole file.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamConfig {
    /// The agent templates, by name.
    pub templates: BTreeMap<String, Template>,
    /// The agent the binary creates and instructs.
    pub first_agent: FirstAgent,
    /// The connector agents, by id.
    #[serde(default)]
    pub connectors: BTreeMap<String, Connector>,
    /// Where the queue lives.
    pub queue: QueueRef,
    /// The credential store the team creates when it has none.
    #[serde(default)]
    pub credentials: Option<CredentialStore>,
}

/// An agent template: the record fields of an AI agent, as data.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Template {
    /// The backend's handler module.
    pub handler_module: String,
    /// The model the backend calls.
    pub model: String,
    /// The endpoint the backend calls.
    pub endpoint: String,
    /// The credential store ref the backend reads its key from.
    #[serde(default)]
    pub credentials: Option<CredRef>,
    /// The price of one million input tokens.
    #[serde(default)]
    pub price_input_per_mtok: Option<f64>,
    /// The price of one million output tokens.
    #[serde(default)]
    pub price_output_per_mtok: Option<f64>,
    /// Agent ids an agent of this template may call.
    #[serde(default)]
    pub can_call: Vec<String>,
    /// Any other record fields, passed through as is.
    #[serde(default)]
    pub extra: Map<String, Value>,
}

/// A credential store ref on a record.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredRef {
    /// The credential store's agent id.
    pub store: String,
    /// The ref inside that store.
    #[serde(rename = "ref")]
    pub ref_: String,
}

/// The agent the binary creates and instructs.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstAgent {
    /// The first agent's id.
    #[serde(default = "default_first_id")]
    pub id: String,
    /// The name of the template it is made from.
    pub template: String,
    /// The file with its instructions, relative to the config file's folder.
    #[serde(default)]
    pub directive_file: Option<String>,
}

/// A connector agent and what agents may do with it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connector {
    /// Its handler module.
    pub kind: String,
    /// Its record fields.
    #[serde(default)]
    pub record: Map<String, Value>,
    /// What agents may do with it, inline.
    #[serde(default)]
    pub directive: Option<String>,
    /// What agents may do with it, read from a file relative to the config file's folder.
    #[serde(default)]
    pub directive_file: Option<String>,
}

/// Where the queue lives.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueRef {
    /// The queue agent's id.
    pub id: String,
}

/// The credential store the team creates when it has none.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialStore {
    /// The store's agent id.
    #[serde(default = "default_store_id")]
    pub id: String,
    /// The backend handler, e.g. `env`.
    pub backend: String,
    /// The refs the store knows.
    #[serde(default)]
    pub refs: Map<String, Value>,
}

fn default_first_id() -> String {
    "lead".into()
}

fn default_store_id() -> String {
    "credentials".into()
}

/// Parse and validate a `team.yaml` text.
pub fn parse(text: &str) -> Result<TeamConfig, String> {
    let v = fantastic_kernel::yaml_io::load(text).map_err(|e| format!("not YAML — {e}"))?;
    let cfg: TeamConfig = serde_json::from_value(v).map_err(|e| e.to_string())?;
    validate(&cfg)?;
    Ok(cfg)
}

/// Read, parse and validate the file at `path`.
pub fn load(path: &Path) -> Result<TeamConfig, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Check the cross-field rules; the first failure is the error.
pub fn validate(cfg: &TeamConfig) -> Result<(), String> {
    if cfg.templates.is_empty() {
        return Err("templates is empty — declare at least one agent template".into());
    }
    for (n, t) in &cfg.templates {
        for (field, val) in [
            ("handler_module", &t.handler_module),
            ("model", &t.model),
            ("endpoint", &t.endpoint),
        ] {
            if val.trim().is_empty() {
                return Err(format!("template `{n}`: {field} is empty"));
            }
        }
        for (field, price) in [
            ("price_input_per_mtok", t.price_input_per_mtok),
            ("price_output_per_mtok", t.price_output_per_mtok),
        ] {
            if let Some(p) = price {
                if !p.is_finite() || p < 0.0 {
                    return Err(format!("template `{n}`: {field} must be a number >= 0"));
                }
            }
        }
        if let Some(r) = &t.credentials {
            if r.store.trim().is_empty() || r.ref_.trim().is_empty() {
                return Err(format!("template `{n}`: credentials needs store and ref"));
            }
        }
        for key in t.extra.keys() {
            if RESERVED.contains(&key.as_str()) {
                return Err(format!(
                    "template `{n}`: extra.{key} is reserved — use the template's own field, or let the team set it"
                ));
            }
        }
        if t.can_call.iter().any(|c| c.trim().is_empty()) {
            return Err(format!("template `{n}`: can_call has an empty id"));
        }
    }
    if cfg.first_agent.id.trim().is_empty() {
        return Err("first_agent.id is empty".into());
    }
    if !cfg.templates.contains_key(&cfg.first_agent.template) {
        let names: Vec<&str> = cfg.templates.keys().map(String::as_str).collect();
        return Err(format!(
            "first_agent.template `{}` is not a template (have: {})",
            cfg.first_agent.template,
            names.join(", ")
        ));
    }
    if cfg.queue.id.trim().is_empty() {
        return Err("queue.id is empty".into());
    }
    for (c, conn) in &cfg.connectors {
        if conn.kind.trim().is_empty() {
            return Err(format!("connector `{c}`: kind is empty"));
        }
        if conn.directive.is_some() && conn.directive_file.is_some() {
            return Err(format!(
                "connector `{c}`: give directive or directive_file, not both"
            ));
        }
        for key in conn.record.keys() {
            if CONNECTOR_RESERVED.contains(&key.as_str()) {
                return Err(format!(
                    "connector `{c}`: record.{key} is reserved — the team sets it"
                ));
            }
        }
    }
    if let Some(s) = &cfg.credentials {
        if s.id.trim().is_empty() || s.backend.trim().is_empty() {
            return Err("credentials: id and backend are required".into());
        }
        for (n, t) in &cfg.templates {
            if let Some(r) = &t.credentials {
                if r.store != s.id {
                    return Err(format!(
                        "template `{n}`: credentials.store `{}` is not the configured store `{}`",
                        r.store, s.id
                    ));
                }
            }
        }
    }
    let mut ids: Vec<&str> = vec![cfg.first_agent.id.as_str(), cfg.queue.id.as_str()];
    ids.extend(cfg.connectors.keys().map(String::as_str));
    if let Some(s) = &cfg.credentials {
        ids.push(s.id.as_str());
    }
    let mut seen: HashSet<&str> = HashSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(format!("agent id `{id}` is used twice"));
        }
    }
    Ok(())
}

/// The memory store id for an agent.
pub fn memory_id(agent_id: &str) -> String {
    format!("{agent_id}_memory")
}

/// A fantastic `create_agent` payload for an agent made from `tpl`.
pub fn agent_record(tpl: &Template, id: &str) -> Value {
    let mem = memory_id(id);
    let mut m = Map::new();
    m.insert("type".into(), json!("create_agent"));
    m.insert("parent_id".into(), json!(DEFAULT_ROOT_ID));
    m.insert("id".into(), json!(id));
    m.insert("handler_module".into(), json!(tpl.handler_module));
    m.insert("model".into(), json!(tpl.model));
    m.insert("endpoint".into(), json!(tpl.endpoint));
    if let Some(r) = &tpl.credentials {
        m.insert(
            "credentials".into(),
            json!({"store": r.store, "ref": r.ref_}),
        );
    }
    if let Some(p) = tpl.price_input_per_mtok {
        m.insert("price_input_per_mtok".into(), json!(p));
    }
    if let Some(p) = tpl.price_output_per_mtok {
        m.insert("price_output_per_mtok".into(), json!(p));
    }
    m.insert("memory_store_id".into(), json!(&mem));
    let mut can_call = Map::new();
    can_call.insert("$self".into(), json!("*"));
    can_call.insert("$subtree".into(), json!("*"));
    can_call.insert(mem.clone(), json!("*"));
    for c in &tpl.can_call {
        can_call.insert(c.clone(), json!("*"));
    }
    m.insert("can_call".into(), Value::Object(can_call));
    m.insert("callable_by".into(), json!({"$self": "*", "$parent": "*"}));
    for (k, v) in &tpl.extra {
        m.insert(k.clone(), v.clone());
    }
    Value::Object(m)
}

/// A `create_agent` payload for the memory store of `agent_id`.
pub fn memory_record(agent_id: &str) -> Value {
    let mut callable_by = Map::new();
    callable_by.insert("$self".into(), json!("*"));
    callable_by.insert(agent_id.into(), json!("*"));
    json!({
        "type": "create_agent",
        "parent_id": DEFAULT_ROOT_ID,
        "id": memory_id(agent_id),
        "handler_module": "ram_store.tools",
        "callable_by": Value::Object(callable_by),
    })
}

/// A `create_agent` payload for the credential store.
pub fn store_record(s: &CredentialStore) -> Value {
    json!({
        "type": "create_agent",
        "parent_id": DEFAULT_ROOT_ID,
        "id": s.id,
        "handler_module": "credential_store.tools",
        "backend": s.backend,
        "refs": s.refs,
    })
}

/// A `create_agent` payload for a connector called `id`.
pub fn connector_record(id: &str, c: &Connector) -> Value {
    let mut m = Map::new();
    m.insert("type".into(), json!("create_agent"));
    m.insert("parent_id".into(), json!(DEFAULT_ROOT_ID));
    m.insert("id".into(), json!(id));
    m.insert("handler_module".into(), json!(c.kind));
    for (k, v) in &c.record {
        m.insert(k.clone(), v.clone());
    }
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: &str = "templates:\n  t:\n    handler_module: h.tools\n    model: m\n    endpoint: http://e\nfirst_agent:\n  template: t\nqueue:\n  id: q\n";

    fn tpl(extra: Value) -> Template {
        Template {
            handler_module: "x_backend.tools".into(),
            model: "m-small".into(),
            endpoint: "http://127.0.0.1:9/v1".into(),
            credentials: Some(CredRef {
                store: "credentials".into(),
                ref_: "x/127.0.0.1".into(),
            }),
            price_input_per_mtok: Some(0.1),
            price_output_per_mtok: Some(0.5),
            can_call: vec!["exec".into(), "queue".into()],
            extra: extra.as_object().cloned().unwrap_or_default(),
        }
    }

    #[test]
    fn the_example_parses() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/team.yaml");
        let cfg = load(&path).unwrap();
        let keys: Vec<&str> = cfg.templates.keys().map(String::as_str).collect();
        assert_eq!(keys, ["local", "small", "strong"]);
        assert_eq!(cfg.first_agent.id, "lead");
        assert_eq!(cfg.first_agent.template, "strong");
        assert_eq!(
            cfg.first_agent.directive_file.as_deref(),
            Some("directives/lead.md")
        );
        assert_eq!(cfg.templates["small"].price_input_per_mtok, Some(0.1));
        assert_eq!(
            cfg.templates["small"].credentials.as_ref().unwrap().ref_,
            "anthropic/api.anthropic.com"
        );
        assert_eq!(cfg.templates["strong"].extra["max_tokens"], json!(32000));
        assert_eq!(cfg.connectors["git"].kind, "team_git.tools");
        assert_eq!(cfg.connectors["git"].record["remote"], json!("origin"));
        assert_eq!(cfg.queue.id, "queue");
        assert_eq!(cfg.credentials.as_ref().unwrap().backend, "env");
    }

    #[test]
    fn no_example_template_grants_exec_or_files() {
        let cfg = parse(include_str!("../../../examples/team.yaml")).unwrap();
        for (n, t) in &cfg.templates {
            for c in &t.can_call {
                assert!(
                    c != "exec" && c != "files",
                    "template `{n}` grants the unrestricted agent `{c}`"
                );
            }
        }
    }

    #[test]
    fn a_minimal_file_parses_with_defaults() {
        let cfg = parse(MIN).unwrap();
        assert_eq!(cfg.first_agent.id, "lead");
        assert_eq!(cfg.first_agent.directive_file, None);
        assert!(cfg.connectors.is_empty());
        assert_eq!(cfg.credentials, None);
        let t = &cfg.templates["t"];
        assert!(t.can_call.is_empty());
        assert!(t.extra.is_empty());
        assert_eq!(t.price_input_per_mtok, None);
        assert_eq!(t.price_output_per_mtok, None);
    }

    #[test]
    fn unknown_keys_are_refused() {
        let e = parse(&format!("{MIN}colour: red\n")).unwrap_err();
        assert!(e.contains("unknown field `colour`"));
        let e = parse(&format!("{MIN}workers: 4\n")).unwrap_err();
        assert!(e.contains("unknown field `workers`"));
        let e = parse(&MIN.replace(
            "    endpoint: http://e\n",
            "    endpoint: http://e\n    count: 3\n",
        ))
        .unwrap_err();
        assert!(e.contains("unknown field `count`"));
        let e = parse(&MIN.replace("  id: q\n", "  id: q\n  extra_thing: 1\n")).unwrap_err();
        assert!(e.contains("unknown field `extra_thing`"));
    }

    #[test]
    fn a_missing_template_is_refused() {
        let e = parse(&MIN.replace("  template: t\n", "  template: big\n")).unwrap_err();
        assert_eq!(e, "first_agent.template `big` is not a template (have: t)");
    }

    #[test]
    fn missing_required_fields_are_refused() {
        let e = parse(&MIN.replace("queue:\n  id: q\n", "")).unwrap_err();
        assert!(e.contains("missing field `queue`"));
        let e = parse(&MIN.replace("    model: m\n", "")).unwrap_err();
        assert!(e.contains("missing field `model`"));
    }

    #[test]
    fn validate_texts_are_exact() {
        let base = parse(MIN).unwrap();

        let mut cfg = base.clone();
        cfg.templates.clear();
        assert_eq!(
            validate(&cfg).unwrap_err(),
            "templates is empty — declare at least one agent template"
        );

        let mut cfg = base.clone();
        cfg.templates.get_mut("t").unwrap().model = " ".into();
        assert_eq!(validate(&cfg).unwrap_err(), "template `t`: model is empty");

        let mut cfg = base.clone();
        cfg.templates.get_mut("t").unwrap().price_output_per_mtok = Some(-1.0);
        assert_eq!(
            validate(&cfg).unwrap_err(),
            "template `t`: price_output_per_mtok must be a number >= 0"
        );

        let mut cfg = base.clone();
        cfg.templates
            .get_mut("t")
            .unwrap()
            .extra
            .insert("memory_store_id".into(), json!("x"));
        assert_eq!(
            validate(&cfg).unwrap_err(),
            "template `t`: extra.memory_store_id is reserved — use the template's own field, or let the team set it"
        );

        let mut cfg = base.clone();
        cfg.connectors.insert(
            "c".into(),
            Connector {
                kind: "k.tools".into(),
                record: Map::new(),
                directive: Some("d".into()),
                directive_file: Some("d.md".into()),
            },
        );
        assert_eq!(
            validate(&cfg).unwrap_err(),
            "connector `c`: give directive or directive_file, not both"
        );

        let mut cfg = base.clone();
        cfg.connectors.insert(
            "c".into(),
            Connector {
                kind: "k.tools".into(),
                record: json!({"id": "x"}).as_object().unwrap().clone(),
                directive: None,
                directive_file: None,
            },
        );
        assert_eq!(
            validate(&cfg).unwrap_err(),
            "connector `c`: record.id is reserved — the team sets it"
        );

        let mut cfg = base.clone();
        cfg.templates.get_mut("t").unwrap().credentials = Some(CredRef {
            store: "credentials".into(),
            ref_: "a/b".into(),
        });
        cfg.credentials = Some(CredentialStore {
            id: "creds".into(),
            backend: "env".into(),
            refs: Map::new(),
        });
        assert_eq!(
            validate(&cfg).unwrap_err(),
            "template `t`: credentials.store `credentials` is not the configured store `creds`"
        );

        let mut cfg = base.clone();
        cfg.connectors.insert(
            "q".into(),
            Connector {
                kind: "k.tools".into(),
                record: Map::new(),
                directive: None,
                directive_file: None,
            },
        );
        assert_eq!(validate(&cfg).unwrap_err(), "agent id `q` is used twice");
    }

    #[test]
    fn agent_record_is_exact() {
        let r = agent_record(&tpl(json!({"max_tokens": 8000})), "w1");
        assert_eq!(
            r,
            json!({
                "type": "create_agent",
                "parent_id": "kernel_state",
                "id": "w1",
                "handler_module": "x_backend.tools",
                "model": "m-small",
                "endpoint": "http://127.0.0.1:9/v1",
                "credentials": {"store": "credentials", "ref": "x/127.0.0.1"},
                "price_input_per_mtok": 0.1,
                "price_output_per_mtok": 0.5,
                "memory_store_id": "w1_memory",
                "can_call": {"$self": "*", "$subtree": "*", "w1_memory": "*", "exec": "*", "queue": "*"},
                "callable_by": {"$self": "*", "$parent": "*"},
                "max_tokens": 8000
            })
        );
    }

    #[test]
    fn agent_record_leaves_out_what_is_unset() {
        let mut t = tpl(json!({}));
        t.credentials = None;
        t.price_input_per_mtok = None;
        t.price_output_per_mtok = None;
        t.can_call.clear();
        let r = agent_record(&t, "w2");
        assert!(r.get("credentials").is_none());
        assert!(r.get("price_input_per_mtok").is_none());
        assert!(r.get("price_output_per_mtok").is_none());
        assert_eq!(
            r["can_call"],
            json!({"$self": "*", "$subtree": "*", "w2_memory": "*"})
        );
    }

    #[test]
    fn memory_store_and_connector_records_are_exact() {
        assert_eq!(
            memory_record("w1"),
            json!({
                "type": "create_agent",
                "parent_id": "kernel_state",
                "id": "w1_memory",
                "handler_module": "ram_store.tools",
                "callable_by": {"$self": "*", "w1": "*"}
            })
        );
        let s = CredentialStore {
            id: "credentials".into(),
            backend: "env".into(),
            refs: json!({"a/h": {"host": "h"}}).as_object().unwrap().clone(),
        };
        assert_eq!(
            store_record(&s),
            json!({
                "type": "create_agent",
                "parent_id": "kernel_state",
                "id": "credentials",
                "handler_module": "credential_store.tools",
                "backend": "env",
                "refs": {"a/h": {"host": "h"}}
            })
        );
        let c = Connector {
            kind: "team_git.tools".into(),
            record: json!({"remote": "origin"}).as_object().unwrap().clone(),
            directive: None,
            directive_file: None,
        };
        assert_eq!(
            connector_record("git", &c),
            json!({
                "type": "create_agent",
                "parent_id": "kernel_state",
                "id": "git",
                "handler_module": "team_git.tools",
                "remote": "origin"
            })
        );
    }

    #[test]
    fn a_missing_file_is_named() {
        let e = load(Path::new("/nonexistent/team.yaml")).unwrap_err();
        assert!(e.starts_with("cannot read /nonexistent/team.yaml: "));
    }
}
