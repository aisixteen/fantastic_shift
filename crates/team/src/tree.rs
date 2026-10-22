//! The agent tree from `team.yaml`: the plan (ids, parents, records), its
//! validation, and its apply to a running kernel. Only an ancestor can write a
//! callee's `callable_by`, so the lead is created under the root and every tool
//! under the lead; the lead then spawns workers itself.
//!
//! Built-in tools (`worker_exec`, `worker_files`, `queue`, `kernel`,
//! `kernel_state`) need no definition in `team.yaml`; anything else a template
//! grants must be a connector defined there.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use fantastic_kernel::bootstrap::{self, DEFAULT_ROOT_ID};
use fantastic_kernel::{Item, Kernel, Status};
use serde_json::{json, Value};

use crate::config::{self, TeamConfig};
use crate::worker_exec;

/// Tools the binary always knows; a template may grant them without a definition.
pub const BUILTIN_TOOLS: [&str; 5] = [
    "worker_exec",
    "worker_files",
    "queue",
    "kernel",
    "kernel_state",
];

/// What a planned agent is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The first agent, created under the root.
    Lead,
    /// The lead's memory store, under the root.
    Memory,
    /// The credential store, under the root.
    Store,
    /// A tool, under the lead.
    Tool,
}

/// One agent of the plan: its `create_agent` record and where it goes.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    /// The agent's id.
    pub id: String,
    /// The parent agent's id.
    pub parent: String,
    /// What the agent is for.
    pub role: Role,
    /// The `create_agent` payload.
    pub record: Value,
}

/// What `apply` did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Applied {
    /// Ids created by this apply.
    pub created: Vec<String>,
    /// Ids that already existed and were left as they are.
    pub reused: Vec<String>,
}

/// Refuse a template that grants a tool neither built in nor defined in config.
pub fn check_grants(cfg: &TeamConfig) -> Result<(), String> {
    let mut known: BTreeSet<&str> = BUILTIN_TOOLS.iter().copied().collect();
    known.insert(cfg.queue.id.as_str());
    known.extend(cfg.connectors.keys().map(String::as_str));
    for (name, t) in &cfg.templates {
        let unknown: Vec<&str> = t
            .can_call
            .iter()
            .map(String::as_str)
            .filter(|c| !known.contains(c))
            .collect();
        if !unknown.is_empty() {
            return Err(format!(
                "template `{name}` grants tools not defined in team.yaml: {}",
                unknown.join(", ")
            ));
        }
    }
    Ok(())
}

/// The plan, lead first. Validates the config and the grants.
pub fn plan(cfg: &TeamConfig, team_dir: &Path) -> Result<Vec<Planned>, String> {
    config::validate(cfg)?;
    check_grants(cfg)?;
    let lead = cfg.first_agent.id.clone();
    let tpl = &cfg.templates[&cfg.first_agent.template];
    let mut out = vec![
        Planned {
            id: lead.clone(),
            parent: DEFAULT_ROOT_ID.into(),
            role: Role::Lead,
            record: config::agent_record(tpl, &lead),
        },
        Planned {
            id: config::memory_id(&lead),
            parent: DEFAULT_ROOT_ID.into(),
            role: Role::Memory,
            record: config::memory_record(&lead),
        },
    ];
    if let Some(s) = &cfg.credentials {
        out.push(Planned {
            id: s.id.clone(),
            parent: DEFAULT_ROOT_ID.into(),
            role: Role::Store,
            record: config::store_record(s),
        });
    }
    let work = worker_exec::work_dir(team_dir)
        .to_string_lossy()
        .into_owned();
    out.push(tool_plan(
        "worker_exec",
        &lead,
        worker_exec::worker_exec_record(team_dir),
    ));
    out.push(tool_plan(
        "worker_files",
        &lead,
        json!({"handler_module": "file_bridge.tools", "root": work, "allow": ["."]}),
    ));
    out.push(tool_plan(
        &cfg.queue.id,
        &lead,
        json!({"handler_module": "ram_store.tools"}),
    ));
    for (id, c) in &cfg.connectors {
        out.push(tool_plan(id, &lead, config::connector_record(id, c)));
    }
    Ok(out)
}

/// A tool under `parent`: its record with the header a tool needs.
fn tool_plan(id: &str, parent: &str, mut rec: Value) -> Planned {
    if let Some(m) = rec.as_object_mut() {
        m.insert("type".into(), json!("create_agent"));
        m.insert("id".into(), json!(id));
        m.insert("parent_id".into(), json!(parent));
        m.insert("callable_by".into(), json!({"$self": "*", "$parent": "*"}));
    }
    Planned {
        id: id.into(),
        parent: parent.into(),
        role: Role::Tool,
        record: rec,
    }
}

/// True when the agent `id` already exists in the kernel.
async fn exists(kernel: &Arc<Kernel>, id: &str) -> bool {
    kernel
        .send(vec![Item::new(id, json!({"type": "reflect"}))])
        .await
        .first()
        .is_some_and(|o| matches!(o.status, Status::Completed) && o.result.get("id").is_some())
}

/// Create the planned agents that are missing; leave the existing ones as they are.
pub async fn apply(kernel: &Arc<Kernel>, plan: &[Planned]) -> Result<Applied, String> {
    let mut out = Applied::default();
    for p in plan {
        if exists(kernel, &p.id).await {
            out.reused.push(p.id.clone());
            continue;
        }
        let mut outs = kernel
            .send(vec![Item::new(DEFAULT_ROOT_ID, p.record.clone())])
            .await;
        match outs.pop() {
            Some(o) if matches!(o.status, Status::Completed) => out.created.push(p.id.clone()),
            Some(o) => return Err(format!("creating `{}` failed: {}", p.id, o.result)),
            None => return Err(format!("creating `{}`: no reply", p.id)),
        }
    }
    kernel.flush_records().await;
    Ok(out)
}

/// Whether the first message is due: when the lead was just created, or when asked.
pub fn instructions_due(applied: &Applied, lead: &str, resend: bool) -> bool {
    resend || applied.created.iter().any(|c| c == lead)
}

/// Send the rendered instructions to the first agent as its first message.
pub async fn deliver_instructions(
    kernel: &Arc<Kernel>,
    lead: &str,
    text: &str,
) -> Result<(), String> {
    let mut outs = kernel
        .send(vec![Item::new(lead, json!({"type": "send", "text": text}))])
        .await;
    match outs.pop() {
        Some(o) if matches!(o.status, Status::Completed) => Ok(()),
        Some(o) => Err(format!(
            "delivering instructions to `{lead}` failed: {}",
            o.result
        )),
        None => Err(format!("delivering instructions to `{lead}`: no reply")),
    }
}

/// Start-up: plan the tree from `team.yaml` in `dir` (if there is one), apply it
/// to a headless kernel, deliver the instructions when due, then stop the kernel.
/// Returns `None` when there is no `team.yaml`.
pub fn run_at_start(
    dir: &Path,
    instructions: Option<&str>,
    resend: bool,
) -> Result<Option<Applied>, String> {
    let path = dir.join(config::FILE_NAME);
    if !path.exists() {
        return Ok(None);
    }
    let cfg = config::load(&path)?;
    let tree = plan(&cfg, dir)?;
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    rt.block_on(async {
        let kernel = crate::start(dir).await.map_err(|e| e.to_string())?;
        let res: Result<Applied, String> = async {
            let applied = apply(&kernel, &tree).await?;
            if let Some(text) = instructions {
                if instructions_due(&applied, &cfg.first_agent.id, resend) {
                    deliver_instructions(&kernel, &cfg.first_agent.id, text).await?;
                }
            }
            Ok(applied)
        }
        .await;
        kernel.shutdown().await;
        bootstrap::shutdown(dir).map_err(|e| e.to_string())?;
        res.map(Some)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse;

    const SAMPLE: &str = "templates:\n  lead_t:\n    handler_module: ram_store.tools\n    model: m\n    endpoint: http://e\n    can_call: [worker_exec, worker_files, queue, git]\n  worker_t:\n    handler_module: ram_store.tools\n    model: m\n    endpoint: http://e\n    can_call: [worker_exec, worker_files]\nfirst_agent:\n  template: lead_t\nconnectors:\n  git:\n    kind: team_git.tools\n    record:\n      remote: origin\n      repo_dir: /tmp/clone\n      workspace_root: /tmp/wt\n      default_branch: main\nqueue:\n  id: queue\n";

    fn sample() -> TeamConfig {
        parse(SAMPLE).unwrap()
    }

    fn ids(t: &[Planned]) -> Vec<String> {
        t.iter().map(|p| p.id.clone()).collect()
    }

    #[test]
    fn sample_config_plan_ids_parents_grants() {
        let tree = plan(&sample(), Path::new("/team")).unwrap();
        assert_eq!(
            ids(&tree),
            [
                "lead",
                "lead_memory",
                "worker_exec",
                "worker_files",
                "queue",
                "git"
            ]
        );
        assert_eq!(tree[0].parent, DEFAULT_ROOT_ID);
        assert_eq!(tree[1].parent, DEFAULT_ROOT_ID);
        for p in &tree[2..] {
            assert_eq!(p.parent, "lead", "{}", p.id);
            assert_eq!(p.role, Role::Tool);
        }
        let grants = tree[0].record["can_call"].as_object().unwrap();
        for g in ["worker_exec", "worker_files", "queue", "git"] {
            assert!(grants.contains_key(g), "{g}");
        }
    }

    #[test]
    fn lead_is_created_first() {
        let tree = plan(&sample(), Path::new("/team")).unwrap();
        assert_eq!(tree[0].id, "lead");
        assert_eq!(tree[0].role, Role::Lead);
    }

    #[test]
    fn instructions_sent_only_on_first_create_or_when_asked() {
        let created = Applied {
            created: vec!["lead".into()],
            reused: vec![],
        };
        let reused = Applied {
            created: vec![],
            reused: vec!["lead".into()],
        };
        assert!(instructions_due(&created, "lead", false));
        assert!(!instructions_due(&reused, "lead", false));
        assert!(instructions_due(&reused, "lead", true));
    }

    #[test]
    fn template_with_undefined_tool_refused_at_start() {
        let mut cfg = sample();
        cfg.templates
            .get_mut("worker_t")
            .unwrap()
            .can_call
            .push("mystery".into());
        let e = plan(&cfg, Path::new("/team")).unwrap_err();
        assert!(e.contains("worker_t"), "{e}");
        assert!(e.contains("mystery"), "{e}");
    }

    #[test]
    fn unknown_tool_name_is_still_refused() {
        let text = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/team.yaml"),
        )
        .unwrap()
        .replace(
            "can_call: [worker_exec, worker_files, queue]",
            "can_call: [worker_exec, worker_files, queue, ghost]",
        );
        let e = plan(&parse(&text).unwrap(), Path::new("/team")).unwrap_err();
        assert!(e.contains("ghost"), "{e}");
    }

    #[test]
    fn example_team_yaml_validates() {
        let cfg =
            config::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/team.yaml"))
                .unwrap();
        let tree = plan(&cfg, Path::new("/team")).unwrap();
        assert_eq!(tree[0].id, "lead");
    }

    #[test]
    fn plan_contains_no_secret_values() {
        let cfg =
            config::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/team.yaml"))
                .unwrap();
        let tree = plan(&cfg, Path::new("/team")).unwrap();
        let text =
            serde_json::to_string(&tree.iter().map(|p| &p.record).collect::<Vec<_>>()).unwrap();
        assert!(!text.contains("sk-"));
        assert!(!text.contains("Bearer "));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn apply_twice_is_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let tree = plan(&sample(), dir.path()).unwrap();
        let kernel = crate::start(dir.path()).await.unwrap();
        let first = apply(&kernel, &tree).await.unwrap();
        assert_eq!(first.created, ids(&tree));
        let second = apply(&kernel, &tree).await.unwrap();
        assert!(second.created.is_empty());
        assert_eq!(second.reused, ids(&tree));
        kernel.shutdown().await;
        bootstrap::shutdown(dir.path()).unwrap();
    }
}
