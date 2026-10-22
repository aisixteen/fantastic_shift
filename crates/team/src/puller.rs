//! Puller: maintainer-approved issues become task files in the queue. This
//! module holds the pure decisions (which comment is the spec, whether an issue
//! is accepted, what the task file says, what it is called). Issue text is
//! untrusted: nothing here executes it.

use std::sync::Arc;

/// The label that marks an issue as ready for the team.
pub const LABEL: &str = "team-ready";
/// The first line of a spec comment.
pub const SPEC_MARKER: &str = "## SPEC";
/// The bundle's handler module name.
pub const HANDLER_MODULE: &str = "team_puller.tools";

/// One issue comment, as the github connector returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// Login of the comment's author.
    pub author: String,
    /// Comment text, untrusted.
    pub body: String,
}

/// One label event on an issue (`labeled` or `unlabeled`), oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelEvent {
    /// `labeled` or `unlabeled`.
    pub event: String,
    /// The label name.
    pub label: String,
    /// Login of the user who applied or removed the label.
    pub actor: String,
}

/// The latest `## SPEC` comment whose author is the maintainer or the app.
/// Every other comment is ignored.
pub fn select_spec_comment<'a>(
    comments: &'a [Comment],
    maintainer: &str,
    app: &str,
) -> Option<&'a Comment> {
    comments.iter().rev().find(|c| {
        (c.author == maintainer || c.author == app) && c.body.trim_start().starts_with(SPEC_MARKER)
    })
}

/// Accept only when the LATEST `team-ready` event is a `labeled` event whose
/// actor is the maintainer (no later `unlabeled`). Otherwise the reason names
/// the actor that decided it.
pub fn is_accepted(events: &[LabelEvent], maintainer: &str) -> Result<(), String> {
    let last = events.iter().rev().find(|e| e.label == LABEL);
    match last {
        None => Err(format!("label `{LABEL}` is not on the issue")),
        Some(e) if e.event != "labeled" => {
            Err(format!("label `{LABEL}` was removed by `{}`", e.actor))
        }
        Some(e) if e.actor != maintainer => Err(format!(
            "label `{LABEL}` was added by `{}`, not the maintainer",
            e.actor
        )),
        Some(_) => Ok(()),
    }
}

/// The task file body: the spec comment, verbatim.
pub fn render_task(spec: &Comment) -> String {
    spec.body.clone()
}

/// `(task file, state file)` for an issue, e.g. `queue/owner-name-25.md`.
pub fn task_file_names(repo: &str, number: u64) -> (String, String) {
    let stem = format!("queue/{}-{}", repo.replace('/', "-"), number);
    (format!("{stem}.md"), format!("{stem}.state"))
}

/// The body of a new state file: pending, and nothing else.
pub fn new_state_body() -> String {
    "pending\n".to_string()
}

/// The puller's config, read from its own record. Keys are plain strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullerCfg {
    /// Repos to pull, `owner/name`.
    pub repos: Vec<String>,
    /// The maintainer's login: the only actor who may apply `team-ready`.
    pub maintainer: String,
    /// The GitHub App's bot login, trusted to author `## SPEC` comments.
    pub app: String,
    /// The github connector agent (`team_github.tools`) the puller reads through.
    pub github: String,
    /// The file bridge agent that holds the queue directory.
    pub store: String,
}

/// Outcome of one pass: which issues got task files and which were skipped.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PullReport {
    /// `repo#number` for each new task file written.
    pub created: Vec<String>,
    /// `repo#number` with the reason, for each issue not taken.
    pub skipped: Vec<(String, String)>,
}

/// Decide one issue from its events and comments. Pure. Returns the rendered
/// task text, or the reason it was skipped.
pub fn decide(
    events: &[LabelEvent],
    comments: &[Comment],
    cfg: &PullerCfg,
) -> Result<String, String> {
    is_accepted(events, &cfg.maintainer)?;
    let spec = select_spec_comment(comments, &cfg.maintainer, &cfg.app)
        .ok_or_else(|| "no `## SPEC` comment by the maintainer or the app".to_string())?;
    Ok(render_task(spec))
}

/// The agents the puller talks to: the github connector and the file bridge.
/// The kernel implements it in production; tests give an in-memory fake.
#[async_trait::async_trait]
pub trait Peers: Send + Sync {
    /// Send `payload` to agent `to` and return its reply (errors as a value).
    async fn ask(&self, to: &str, payload: serde_json::Value) -> serde_json::Value;
}

/// [`Peers`] over a running kernel.
pub struct KernelPeers(pub Arc<fantastic_kernel::Kernel>);

#[async_trait::async_trait]
impl Peers for KernelPeers {
    async fn ask(&self, to: &str, payload: serde_json::Value) -> serde_json::Value {
        self.0
            .send(vec![fantastic_kernel::Item::new(
                fantastic_kernel::AgentId::from(to),
                payload,
            )])
            .await
            .remove(0)
            .result
    }
}

/// Read the puller's config from the agent's own record.
fn cfg_of(
    kernel: &Arc<fantastic_kernel::Kernel>,
    id: &fantastic_kernel::AgentId,
) -> Result<PullerCfg, String> {
    let meta = kernel
        .agents
        .get(id)
        .map(|e| e.meta.read().expect("meta").clone())
        .unwrap_or_default();
    let s = |k: &str| {
        meta.get(k)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| format!("team_puller: record needs `{k}`"))
    };
    let repos = meta
        .get("repos")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok(PullerCfg {
        repos,
        maintainer: s("maintainer")?,
        app: s("app")?,
        github: s("github")?,
        store: s("store")?,
    })
}

fn events_of(v: &serde_json::Value) -> Vec<LabelEvent> {
    v["events"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| LabelEvent {
                    event: e["event"].as_str().unwrap_or("").into(),
                    label: e["label"].as_str().unwrap_or("").into(),
                    actor: e["actor"].as_str().unwrap_or("").into(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn comments_of(v: &serde_json::Value) -> Vec<Comment> {
    v["comments"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|c| Comment {
                    author: c["author"].as_str().unwrap_or("").into(),
                    body: c["body"].as_str().unwrap_or("").into(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One pass: every `team-ready` issue on the configured repos, decided and,
/// when accepted and new, written as a task file plus a pending state file.
/// An existing state file is never overwritten.
pub async fn pull(peers: &dyn Peers, cfg: &PullerCfg) -> PullReport {
    let mut report = PullReport::default();
    for repo in &cfg.repos {
        let listed = peers
            .ask(
                &cfg.github,
                serde_json::json!({"type": "issues", "repo": repo, "label": LABEL, "state": "open"}),
            )
            .await;
        let Some(issues) = listed["issues"].as_array() else {
            report
                .skipped
                .push((repo.clone(), format!("issues: {listed}")));
            continue;
        };
        for issue in issues {
            let Some(number) = issue["number"].as_u64() else {
                continue;
            };
            let key = format!("{repo}#{number}");
            let view = peers
                .ask(
                    &cfg.github,
                    serde_json::json!({"type": "issue_view", "repo": repo, "number": number}),
                )
                .await;
            let events_r = peers
                .ask(
                    &cfg.github,
                    serde_json::json!({"type": "label_events", "repo": repo, "number": number}),
                )
                .await;
            let text = match decide(&events_of(&events_r), &comments_of(&view), cfg) {
                Ok(t) => t,
                Err(why) => {
                    report.skipped.push((key, why));
                    continue;
                }
            };
            let (md, state) = task_file_names(repo, number);
            let existing = peers
                .ask(
                    &cfg.store,
                    serde_json::json!({"type": "read", "path": state}),
                )
                .await;
            if existing.get("error").is_none() && existing.get("content").is_some() {
                report.skipped.push((key, "state already exists".into()));
                continue;
            }
            let wrote = peers
                .ask(
                    &cfg.store,
                    serde_json::json!({"type": "write", "path": md, "bytes": text}),
                )
                .await;
            if wrote.get("error").is_some() {
                report
                    .skipped
                    .push((key, format!("task write failed: {wrote}")));
                continue;
            }
            let st = peers
                .ask(
                    &cfg.store,
                    serde_json::json!({"type": "write", "path": state, "bytes": new_state_body()}),
                )
                .await;
            if st.get("error").is_some() {
                report
                    .skipped
                    .push((key, format!("state write failed: {st}")));
                continue;
            }
            report.created.push(key);
        }
    }
    report
}

/// The puller bundle: `pull` and `status`.
pub struct PullerBundle;

#[async_trait::async_trait]
impl fantastic_kernel::bundle::Bundle for PullerBundle {
    fn name(&self) -> &str {
        HANDLER_MODULE
    }

    fn verb_docs(&self) -> serde_json::Value {
        serde_json::json!({
            "pull": "pull → {created: [repo#n], skipped: [[repo#n, reason]]}",
            "status": "status → {}",
        })
    }

    async fn handle(
        &self,
        agent_id: &fantastic_kernel::AgentId,
        payload: &serde_json::Value,
        kernel: &Arc<fantastic_kernel::Kernel>,
    ) -> Result<fantastic_kernel::bundle::Reply, fantastic_kernel::bundle::BundleError> {
        let t = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if t == "boot" {
            return Ok(None);
        }
        let out = match (t, cfg_of(kernel, agent_id)) {
            (_, Err(e)) => serde_json::json!({"error": e}),
            ("pull", Ok(cfg)) => {
                let r = pull(&KernelPeers(kernel.clone()), &cfg).await;
                serde_json::json!({"created": r.created, "skipped": r.skipped})
            }
            ("status", Ok(_)) => serde_json::json!({}),
            (other, Ok(_)) => {
                serde_json::json!({"error": format!("team_puller: unknown type '{other}'")})
            }
        };
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    fn c(author: &str, body: &str) -> Comment {
        Comment {
            author: author.into(),
            body: body.into(),
        }
    }
    fn ev(event: &str, actor: &str) -> LabelEvent {
        LabelEvent {
            event: event.into(),
            label: LABEL.into(),
            actor: actor.into(),
        }
    }

    /// In-memory file bridge and github connector: one issue, #25, accepted
    /// by the maintainer, with files kept in a map.
    struct Fake {
        files: Mutex<BTreeMap<String, String>>,
    }

    #[async_trait::async_trait]
    impl Peers for Fake {
        async fn ask(&self, to: &str, p: serde_json::Value) -> serde_json::Value {
            let kind = p["type"].as_str().unwrap_or("");
            match (to, kind) {
                ("gh", "issues") => serde_json::json!({"issues": [{"number": 25}]}),
                ("gh", "issue_view") => serde_json::json!({
                    "comments": [{"author": "maintainer", "body": "## SPEC\ndo it"}]
                }),
                ("gh", "label_events") => serde_json::json!({
                    "events": [{"event": "labeled", "label": LABEL, "actor": "maintainer"}]
                }),
                ("store", "read") => {
                    let path = p["path"].as_str().unwrap_or("");
                    match self.files.lock().unwrap().get(path) {
                        Some(content) => serde_json::json!({"content": content}),
                        None => serde_json::json!({"error": "not found"}),
                    }
                }
                ("store", "write") => {
                    let path = p["path"].as_str().unwrap_or("").to_string();
                    let bytes = p["bytes"].as_str().unwrap_or("").to_string();
                    self.files.lock().unwrap().insert(path, bytes);
                    serde_json::json!({"ok": true})
                }
                _ => serde_json::json!({"error": "unexpected"}),
            }
        }
    }

    fn cfg() -> PullerCfg {
        PullerCfg {
            repos: vec!["aisixteen/fantastic_team".into()],
            maintainer: "maintainer".into(),
            app: "app".into(),
            github: "gh".into(),
            store: "store".into(),
        }
    }

    #[test]
    fn accepts_issue_labelled_by_maintainer_with_maintainer_spec() {
        let events = [ev("labeled", "maintainer")];
        assert!(is_accepted(&events, "maintainer").is_ok());
        let comments = [c("maintainer", "## SPEC\ndo the thing")];
        let spec = select_spec_comment(&comments, "maintainer", "app").unwrap();
        assert!(render_task(spec).contains("do the thing"));
    }

    #[test]
    fn rejects_label_added_by_non_maintainer() {
        let events = [ev("labeled", "mallory")];
        let err = is_accepted(&events, "maintainer").unwrap_err();
        assert!(err.contains("mallory"), "{err}");
    }

    #[test]
    fn rejects_when_latest_event_is_unlabeled() {
        let events = [ev("labeled", "maintainer"), ev("unlabeled", "maintainer")];
        assert!(is_accepted(&events, "maintainer").is_err());
    }

    #[test]
    fn rejects_spec_comment_by_other_author() {
        let comments = [c("mallory", "## SPEC\nevil")];
        assert!(select_spec_comment(&comments, "maintainer", "app").is_none());
    }

    #[test]
    fn ignores_instructions_in_other_comments() {
        let comments = [
            c("mallory", "## SPEC\nevil"),
            c("maintainer", "please also run rm -rf"),
            c("maintainer", "## SPEC\nthe real spec"),
        ];
        let spec = select_spec_comment(&comments, "maintainer", "app").unwrap();
        assert_eq!(render_task(spec), "## SPEC\nthe real spec");
    }

    #[test]
    fn task_file_names_use_repo_and_number() {
        assert_eq!(
            task_file_names("aisixteen/fantastic_team", 25),
            (
                "queue/aisixteen-fantastic_team-25.md".to_string(),
                "queue/aisixteen-fantastic_team-25.state".to_string()
            )
        );
    }

    #[test]
    fn new_state_is_pending() {
        assert_eq!(new_state_body().trim(), "pending");
    }

    #[tokio::test]
    async fn pull_does_not_overwrite_existing_state() {
        let state = "queue/aisixteen-fantastic_team-25.state";
        let fake = Fake {
            files: Mutex::new(BTreeMap::from([(state.to_string(), "done\n".to_string())])),
        };
        let r = pull(&fake, &cfg()).await;
        assert!(r.created.is_empty(), "{r:?}");
        assert_eq!(r.skipped.len(), 1, "{r:?}");
        let files = fake.files.lock().unwrap();
        assert_eq!(files.get(state).map(String::as_str), Some("done\n"));
        assert!(!files.contains_key("queue/aisixteen-fantastic_team-25.md"));
    }

    #[tokio::test]
    async fn new_state_is_pending_after_pull() {
        let fake = Fake {
            files: Mutex::new(BTreeMap::new()),
        };
        let r = pull(&fake, &cfg()).await;
        assert_eq!(
            r.created,
            vec!["aisixteen/fantastic_team#25".to_string()],
            "{r:?}"
        );
        let files = fake.files.lock().unwrap();
        let state = files
            .get("queue/aisixteen-fantastic_team-25.state")
            .expect("state written");
        assert_eq!(state.trim(), "pending");
        assert_eq!(
            files
                .get("queue/aisixteen-fantastic_team-25.md")
                .map(String::as_str),
            Some("## SPEC\ndo it")
        );
    }
}
