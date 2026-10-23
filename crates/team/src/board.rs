//! Board: the shift's shared picture of the work. It caches every open issue
//! of the configured repos (through `team_github.tools`) with its queue state
//! (`queue/*.state`), knows the agents, keeps its own state file in the store,
//! and audits the picture. It NEVER assigns: no verb sets an assignee or writes
//! a claim for someone else. Audit findings only become asks.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::puller::{task_file_names, KernelPeers, Peers};

/// The bundle's handler module name.
pub const HANDLER_MODULE: &str = "team_board.tools";
/// Failures by one agent before the audit flags it.
pub const FAIL_LIMIT: u32 = 3;
/// Where the board keeps its state inside the store.
const STATE_PATH: &str = "board/state.json";

/// Where a task stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Accepted, nobody working on it.
    #[default]
    Pending,
    /// Claimed and being worked.
    InWork,
    /// Finished, with a PR.
    Done,
    /// Given up.
    Failed,
}

impl Status {
    /// The wire name of the status.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::InWork => "in_work",
            Status::Done => "done",
            Status::Failed => "failed",
        }
    }

    /// The status with this wire name.
    pub fn parse(s: &str) -> Option<Status> {
        match s {
            "pending" => Some(Status::Pending),
            "in_work" => Some(Status::InWork),
            "done" => Some(Status::Done),
            "failed" => Some(Status::Failed),
            _ => None,
        }
    }
}

/// One issue as the board sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// `repo#number`.
    pub id: String,
    /// The issue title.
    pub title: String,
    /// Where the task stands.
    pub status: Status,
    /// Agents named by queue state files as claimers.
    pub claimers: Vec<String>,
    /// The PR link, once done.
    pub pr: Option<String>,
    /// The last note an agent reported.
    pub note: Option<String>,
    /// Unix seconds of the last change.
    pub updated: u64,
}

/// One entry in the board's history: a report, or an ask and its answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Unix seconds.
    pub at: u64,
    /// Who wrote it.
    pub author: String,
    /// `report` or `ask:<to>`.
    pub kind: String,
    /// The text.
    pub text: String,
    /// The answer, once it came back.
    pub answer: Option<String>,
}

/// What the board knows about one agent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentInfo {
    /// The agent's own description of its state.
    pub state: String,
    /// Unix seconds of its last report.
    pub last_seen: u64,
    /// Its cost so far, as it reports it.
    pub cost: f64,
}

/// A configured group: its AI driver (empty when it has none) and its members.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    /// The agent that receives the group's digest. Empty for no driver.
    #[serde(default)]
    pub driver: String,
    /// Agent ids in the group.
    #[serde(default)]
    pub members: Vec<String>,
}

/// The whole picture. Serialized into the store on every change.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Board {
    /// Tasks by `repo#number`.
    pub tasks: BTreeMap<String, Task>,
    /// Agents by id.
    pub agents: BTreeMap<String, AgentInfo>,
    /// Reports, oldest first.
    pub reports: Vec<Record>,
    /// Asks and their answers, oldest first.
    pub asks: Vec<Record>,
    /// Failed reports per agent.
    pub failures: BTreeMap<String, u32>,
    /// PR link to the task it belongs to.
    pub prs: BTreeMap<String, String>,
}

/// One thing the audit found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// A claim has been silent past the limit.
    StaleClaim {
        /// The task.
        task: String,
    },
    /// Work is recorded on a task nobody claimed.
    UnclaimedWork {
        /// The task.
        task: String,
    },
    /// Two agents claim one task.
    DoubleClaim {
        /// The task.
        task: String,
    },
    /// Done, but no PR is linked.
    DoneWithoutPr {
        /// The task.
        task: String,
    },
    /// A PR points at a task the board does not have.
    PrWithoutTask {
        /// The PR link.
        pr: String,
    },
    /// An agent has failed repeatedly.
    RepeatedFailure {
        /// The agent.
        agent: String,
    },
}

impl Finding {
    /// The sentence an ask carries.
    pub fn text(&self) -> String {
        match self {
            Finding::StaleClaim { task } => format!("{task}: claim has been silent past the limit"),
            Finding::UnclaimedWork { task } => {
                format!("{task}: work recorded but nobody claimed it")
            }
            Finding::DoubleClaim { task } => format!("{task}: more than one agent claims it"),
            Finding::DoneWithoutPr { task } => format!("{task}: done with no PR linked"),
            Finding::PrWithoutTask { pr } => format!("{pr}: PR points at no known task"),
            Finding::RepeatedFailure { agent } => format!("{agent}: failing repeatedly"),
        }
    }
}

/// Parse a queue state file: `pending`, `in_work <claimer>`, `done <pr>`,
/// `failed`. Returns the status, the claimer and the PR link, if written.
pub fn parse_state(content: &str) -> Option<(Status, Option<String>, Option<String>)> {
    let line = content.lines().next()?.trim();
    let mut parts = line.splitn(2, ' ');
    let status = Status::parse(parts.next()?)?;
    let rest = parts
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Some(match status {
        Status::InWork => (status, rest, None),
        Status::Done => (status, None, rest),
        _ => (status, None, None),
    })
}

impl Board {
    /// Cache an issue as pending. An issue already cached keeps its status.
    pub fn add_issue(&mut self, repo: &str, number: u64, title: &str, now: u64) {
        let id = format!("{repo}#{number}");
        self.tasks.entry(id.clone()).or_insert_with(|| Task {
            id,
            title: title.into(),
            status: Status::Pending,
            updated: now,
            ..Default::default()
        });
    }

    /// Apply a queue state file to a cached task. A claim is recorded as
    /// written by the file; nothing here writes a claim.
    pub fn apply_state(&mut self, id: &str, content: &str, now: u64) {
        let Some(task) = self.tasks.get_mut(id) else {
            return;
        };
        let Some((status, claimer, pr)) = parse_state(content) else {
            return;
        };
        task.status = status;
        if let Some(c) = claimer {
            if !task.claimers.contains(&c) {
                task.claimers.push(c);
            }
        }
        if let Some(p) = pr {
            self.prs.insert(p.clone(), id.to_string());
            task.pr = Some(p);
        }
        task.updated = now;
    }

    /// Record what an agent did or decided on a task.
    pub fn report(
        &mut self,
        id: &str,
        status: Status,
        note: &str,
        pr: Option<&str>,
        author: &str,
        now: u64,
    ) -> Result<(), String> {
        let task = self
            .tasks
            .get_mut(id)
            .ok_or_else(|| format!("no task `{id}`"))?;
        task.status = status;
        task.note = (!note.is_empty()).then(|| note.to_string());
        task.updated = now;
        if let Some(p) = pr {
            self.prs.insert(p.to_string(), id.to_string());
            task.pr = Some(p.to_string());
        }
        if status == Status::Failed {
            *self.failures.entry(author.to_string()).or_insert(0) += 1;
        }
        self.reports.push(Record {
            at: now,
            author: author.into(),
            kind: "report".into(),
            text: format!("{id} {}: {note}", status.as_str()),
            answer: None,
        });
        Ok(())
    }

    /// Record an agent's own state, cost and time.
    pub fn touch_agent(&mut self, name: &str, state: &str, cost: f64, now: u64) {
        self.agents.insert(
            name.into(),
            AgentInfo {
                state: state.into(),
                last_seen: now,
                cost,
            },
        );
    }

    /// The whole picture: tasks grouped by state, and the agents.
    pub fn snapshot(&self) -> Value {
        let mut by: BTreeMap<&str, Vec<&Task>> = BTreeMap::new();
        for s in [
            Status::Pending,
            Status::InWork,
            Status::Done,
            Status::Failed,
        ] {
            by.insert(s.as_str(), Vec::new());
        }
        for task in self.tasks.values() {
            by.entry(task.status.as_str()).or_default().push(task);
        }
        json!({"tasks": by, "agents": self.agents})
    }

    /// Run the rules over the cached state. Pure: reads only.
    pub fn audit(&self, now: u64, stale_secs: u64) -> Vec<Finding> {
        let mut out = Vec::new();
        for task in self.tasks.values() {
            if task.status == Status::InWork && now.saturating_sub(task.updated) > stale_secs {
                out.push(Finding::StaleClaim {
                    task: task.id.clone(),
                });
            }
            if matches!(task.status, Status::InWork | Status::Done) && task.claimers.is_empty() {
                out.push(Finding::UnclaimedWork {
                    task: task.id.clone(),
                });
            }
            if task.claimers.len() > 1 {
                out.push(Finding::DoubleClaim {
                    task: task.id.clone(),
                });
            }
            if task.status == Status::Done && task.pr.is_none() {
                out.push(Finding::DoneWithoutPr {
                    task: task.id.clone(),
                });
            }
        }
        for (pr, task) in &self.prs {
            if !self.tasks.contains_key(task) {
                out.push(Finding::PrWithoutTask { pr: pr.clone() });
            }
        }
        for (agent, n) in &self.failures {
            if *n >= FAIL_LIMIT {
                out.push(Finding::RepeatedFailure {
                    agent: agent.clone(),
                });
            }
        }
        out
    }
}

/// What the kernel says about one agent right now.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Live {
    /// Whether it is generating a reply.
    pub generating: bool,
    /// Its cache hit rate, 0..1.
    pub cache_hit: f64,
}

/// One row per agent the board knows, in the shape `cadence_feed::tree_from_board` reads.
/// Rows for the board: one row per configured group (`parent` null, `driver` set),
/// then one row per agent. A member of a group has `parent` = the group, and gets a
/// row with live state even if it never registered.
pub fn rows_from(
    board: &Board,
    groups: &BTreeMap<String, Group>,
    live: &BTreeMap<String, Live>,
    now: u64,
) -> Vec<Value> {
    let mut b = board.clone();
    for g in groups.values() {
        for m in &g.members {
            b.agents.entry(m.clone()).or_insert_with(|| AgentInfo {
                state: String::new(),
                last_seen: now,
                cost: 0.0,
            });
        }
    }
    let mut rows = agent_rows(&b, live, now);
    for row in rows.iter_mut() {
        let id = row["id"].as_str().unwrap_or("").to_string();
        if let Some((gname, _)) = groups.iter().find(|(_, g)| g.members.contains(&id)) {
            row["parent"] = json!(gname);
        }
    }
    let mut out = Vec::new();
    for (gname, g) in groups {
        out.push(json!({
            "id": gname,
            "parent": null,
            "has_ai": !g.driver.is_empty(),
            "driver": g.driver,
            "state": "group",
            "task": "",
            "idle_s": 0,
            "time_on_task_s": 0,
            "cost": 0.0,
            "cache_hit": 0.0,
            "new_results": 0,
        }));
    }
    out.extend(rows);
    out
}

fn agent_rows(board: &Board, live: &BTreeMap<String, Live>, now: u64) -> Vec<Value> {
    board
        .agents
        .iter()
        .map(|(name, info)| {
            let claim = board
                .tasks
                .values()
                .find(|t| t.status == Status::InWork && t.claimers.contains(name));
            let lv = live.get(name).cloned().unwrap_or_default();
            let busy = lv.generating || claim.is_some();
            let done = board
                .tasks
                .values()
                .any(|t| t.status == Status::Done && t.claimers.contains(name));
            let failed = board
                .tasks
                .values()
                .any(|t| t.status == Status::Failed && t.claimers.contains(name));
            let state = if busy {
                "busy"
            } else if done {
                "done"
            } else if failed {
                "failed"
            } else {
                "idle"
            };
            json!({
                "id": name,
                "parent": null,
                "has_ai": true,
                "state": state,
                "task": claim.map(|t| t.id.clone()).unwrap_or_default(),
                "idle_s": now.saturating_sub(info.last_seen),
                "time_on_task_s": claim.map(|t| now.saturating_sub(t.updated)).unwrap_or(0),
                "cost": info.cost,
                "cache_hit": lv.cache_hit,
                "new_results": 0,
            })
        })
        .collect()
}

/// Ask one agent for its status and usage through the kernel.
async fn live_of(peers: &dyn Peers, name: &str) -> Live {
    let status = peers.ask(name, json!({"type": "status"})).await;
    let reflect = peers.ask(name, json!({"type": "reflect"})).await;
    Live {
        generating: status["status"].as_str() == Some("generating"),
        cache_hit: reflect["cache_hit"].as_f64().unwrap_or(0.0),
    }
}

/// The board's config, read from its own record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardCfg {
    /// Repos to cache, `owner/name`.
    pub repos: Vec<String>,
    /// The github connector agent (`team_github.tools`).
    pub github: String,
    /// The store that holds the queue and the board's state.
    pub store: String,
    /// Who audit findings are asked of.
    pub ask_to: String,
    /// Seconds without activity before a claim is stale.
    pub stale_secs: u64,
    /// Groups from the board record: name → driver and members.
    pub groups: BTreeMap<String, Group>,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cfg_of(
    kernel: &Arc<fantastic_kernel::Kernel>,
    id: &fantastic_kernel::AgentId,
) -> Result<BoardCfg, String> {
    let meta = kernel
        .agents
        .get(id)
        .map(|e| e.meta.read().expect("meta").clone())
        .unwrap_or_default();
    let text = |k: &str| {
        meta.get(k)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|v| !v.trim().is_empty())
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
    Ok(BoardCfg {
        repos,
        github: text("github").ok_or_else(|| "team_board: record needs `github`".to_string())?,
        store: text("store").ok_or_else(|| "team_board: record needs `store`".to_string())?,
        ask_to: text("ask_to").unwrap_or_else(|| "user".into()),
        groups: meta
            .get("team_groups")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default(),
        stale_secs: meta
            .get("stale_minutes")
            .and_then(|v| v.as_u64())
            .unwrap_or(30)
            * 60,
    })
}

/// Read the board from the store. A missing or unreadable file is an empty board.
pub async fn load(peers: &dyn Peers, store: &str) -> Board {
    let r = peers
        .ask(store, json!({"type": "read", "path": STATE_PATH}))
        .await;
    r["content"]
        .as_str()
        .and_then(|c| serde_json::from_str(c).ok())
        .unwrap_or_default()
}

/// Write the board to the store.
pub async fn save(peers: &dyn Peers, store: &str, board: &Board) -> Value {
    let bytes = serde_json::to_string_pretty(board).unwrap_or_default();
    peers
        .ask(
            store,
            json!({"type": "write", "path": STATE_PATH, "bytes": bytes}),
        )
        .await
}

/// Cache every open issue of every repo, and read each one's queue state.
/// Returns the errors met on the way.
pub async fn pull(peers: &dyn Peers, cfg: &BoardCfg, board: &mut Board, now: u64) -> Vec<String> {
    let mut errors = Vec::new();
    for repo in &cfg.repos {
        let listed = peers
            .ask(
                &cfg.github,
                json!({"type": "issues", "repo": repo, "state": "open"}),
            )
            .await;
        let Some(issues) = listed["issues"].as_array() else {
            errors.push(format!("{repo}: {listed}"));
            continue;
        };
        for issue in issues {
            let Some(number) = issue["number"].as_u64() else {
                continue;
            };
            let title = issue["title"].as_str().unwrap_or("");
            board.add_issue(repo, number, title, now);
            let (_, state) = task_file_names(repo, number);
            let read = peers
                .ask(&cfg.store, json!({"type": "read", "path": state}))
                .await;
            if let Some(content) = read["content"].as_str() {
                board.apply_state(&format!("{repo}#{number}"), content, now);
            }
        }
    }
    errors
}

/// Record a question before it is sent; returns its index in the history.
fn record_ask(board: &mut Board, to: &str, text: &str, now: u64) -> usize {
    board.asks.push(Record {
        at: now,
        author: "board".into(),
        kind: format!("ask:{to}"),
        text: text.into(),
        answer: None,
    });
    board.asks.len() - 1
}

/// Ask an agent, record the question, and record its answer when it comes back.
async fn ask_recorded(
    peers: &dyn Peers,
    board: &mut Board,
    to: &str,
    text: &str,
    now: u64,
) -> Value {
    let idx = record_ask(board, to, text, now);
    let reply = peers
        .ask(to, json!({"type": "message", "text": text}))
        .await;
    board.asks[idx].answer = Some(reply.to_string());
    reply
}

async fn dispatch(peers: &dyn Peers, cfg: &BoardCfg, t: &str, p: &Value) -> Value {
    let now = now_secs();
    let mut board = load(peers, &cfg.store).await;
    let out = match t {
        "pull" => {
            let errors = pull(peers, cfg, &mut board, now).await;
            json!({"tasks": board.tasks.len(), "errors": errors})
        }
        "board" => board.snapshot(),
        "rows" => {
            let mut live = BTreeMap::new();
            let mut names: BTreeSet<String> = board.agents.keys().cloned().collect();
            for g in cfg.groups.values() {
                names.extend(g.members.iter().cloned());
            }
            for name in &names {
                live.insert(name.clone(), live_of(peers, name).await);
            }
            json!({"rows": rows_from(&board, &cfg.groups, &live, now)})
        }
        "task" => {
            let id = p["id"].as_str().unwrap_or("");
            match board.tasks.get(id) {
                Some(task) => json!(task),
                None => json!({"error": format!("no task `{id}`")}),
            }
        }
        "report" => {
            let id = p["task"].as_str().unwrap_or("");
            match p["status"].as_str().and_then(Status::parse) {
                None => json!({"error": "report needs status pending|in_work|done|failed"}),
                Some(status) => match board.report(
                    id,
                    status,
                    p["note"].as_str().unwrap_or(""),
                    p["pr"].as_str(),
                    p["author"].as_str().unwrap_or("unknown"),
                    now,
                ) {
                    Ok(()) => json!({"ok": true}),
                    Err(e) => json!({"error": e}),
                },
            }
        }
        "agent" => {
            let name = p["name"].as_str().unwrap_or("");
            board.touch_agent(
                name,
                p["state"].as_str().unwrap_or(""),
                p["cost"].as_f64().unwrap_or(0.0),
                now,
            );
            json!({"ok": true})
        }
        "ask" => {
            let to = p["to"].as_str().unwrap_or(cfg.ask_to.as_str());
            let question = p["question"].as_str().unwrap_or("");
            ask_recorded(peers, &mut board, to, question, now).await
        }
        "audit" => {
            let findings = board.audit(now, cfg.stale_secs);
            let mut texts = Vec::new();
            for finding in &findings {
                let text = finding.text();
                ask_recorded(peers, &mut board, &cfg.ask_to, &text, now).await;
                texts.push(text);
            }
            json!({"findings": texts})
        }
        other => json!({"error": format!("team_board: unknown type '{other}'")}),
    };
    if matches!(t, "pull" | "report" | "agent" | "ask" | "audit") {
        save(peers, &cfg.store, &board).await;
    }
    out
}

/// The board bundle: `pull`, `board`, `task`, `report`, `agent`, `ask`, `audit`.
#[derive(Default)]
pub struct BoardBundle;

#[async_trait::async_trait]
impl fantastic_kernel::bundle::Bundle for BoardBundle {
    fn name(&self) -> &str {
        HANDLER_MODULE
    }

    fn verb_docs(&self) -> Value {
        json!({
            "pull": "pull → {tasks: n, errors: [..]}",
            "board": "board → {tasks: {pending, in_work, done, failed}, agents: {..}}",
            "task": "task {id} → the task",
            "report": "report {task, status, note?, pr?, author?} → {ok}",
            "agent": "agent {name, state, cost} → {ok}",
            "ask": "ask {to, question} → the answer, recorded",
            "audit": "audit → {findings: [..]}, each asked of ask_to",
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
        let out = match cfg_of(kernel, agent_id) {
            Err(e) => json!({"error": e}),
            Ok(cfg) => dispatch(&KernelPeers(kernel.clone()), &cfg, t, payload).await,
        };
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn task(id: &str, status: Status, claimers: &[&str], pr: Option<&str>, updated: u64) -> Task {
        Task {
            id: id.into(),
            title: String::new(),
            status,
            claimers: claimers.iter().map(|c| c.to_string()).collect(),
            pr: pr.map(str::to_string),
            note: None,
            updated,
        }
    }

    /// Mock github connector and store: issues 25 (labelled) and 26 (not),
    /// and a files map for the store.
    struct Fake {
        files: Mutex<BTreeMap<String, String>>,
    }

    #[async_trait::async_trait]
    impl Peers for Fake {
        async fn ask(&self, to: &str, p: Value) -> Value {
            match (to, p["type"].as_str().unwrap_or("")) {
                ("gh", "issues") => json!({"issues": [
                    {"number": 25, "title": "labelled", "labels": ["team-ready"]},
                    {"number": 26, "title": "unlabelled", "labels": []}
                ]}),
                ("store", "read") => match self
                    .files
                    .lock()
                    .unwrap()
                    .get(p["path"].as_str().unwrap_or(""))
                {
                    Some(c) => json!({"content": c}),
                    None => json!({"error": "not found"}),
                },
                ("store", "write") => {
                    let path = p["path"].as_str().unwrap_or("").to_string();
                    let bytes = p["bytes"].as_str().unwrap_or("").to_string();
                    self.files.lock().unwrap().insert(path, bytes);
                    json!({"ok": true})
                }
                _ => json!({"reply": "noted"}),
            }
        }
    }

    fn cfg() -> BoardCfg {
        BoardCfg {
            repos: vec!["aisixteen/fantastic_team".into()],
            github: "gh".into(),
            store: "store".into(),
            ask_to: "user".into(),
            stale_secs: 1800,
            groups: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn pull_caches_all_open_issues_from_mock_github() {
        let fake = Fake {
            files: Mutex::new(BTreeMap::new()),
        };
        let mut board = Board::default();
        let errors = pull(&fake, &cfg(), &mut board, 100).await;
        assert!(errors.is_empty());
        assert_eq!(board.tasks.len(), 2);
        assert_eq!(
            board.tasks["aisixteen/fantastic_team#25"].status,
            Status::Pending
        );
        assert_eq!(
            board.tasks["aisixteen/fantastic_team#26"].status,
            Status::Pending
        );
    }

    #[test]
    fn claim_file_moves_task_to_in_work() {
        let mut board = Board::default();
        board.add_issue("o/r", 1, "t", 0);
        board.apply_state("o/r#1", "in_work alice\n", 5);
        let t = &board.tasks["o/r#1"];
        assert_eq!(t.status, Status::InWork);
        assert_eq!(t.claimers, vec!["alice".to_string()]);
    }

    #[test]
    fn report_done_with_pr_moves_task_to_done() {
        let mut board = Board::default();
        board.add_issue("o/r", 2, "t", 0);
        board
            .report(
                "o/r#2",
                Status::Done,
                "shipped",
                Some("https://pr/9"),
                "alice",
                7,
            )
            .unwrap();
        let t = &board.tasks["o/r#2"];
        assert_eq!(t.status, Status::Done);
        assert_eq!(t.pr.as_deref(), Some("https://pr/9"));
        assert_eq!(board.prs["https://pr/9"], "o/r#2");
    }

    #[test]
    fn audit_flags_stale_claim() {
        let mut board = Board::default();
        board.tasks.insert(
            "o/r#1".into(),
            task("o/r#1", Status::InWork, &["a"], None, 0),
        );
        assert_eq!(
            board.audit(10_000, 60),
            vec![Finding::StaleClaim {
                task: "o/r#1".into()
            }]
        );
    }

    #[test]
    fn audit_flags_unclaimed_work() {
        let mut board = Board::default();
        board
            .tasks
            .insert("o/r#1".into(), task("o/r#1", Status::InWork, &[], None, 0));
        assert_eq!(
            board.audit(0, 60),
            vec![Finding::UnclaimedWork {
                task: "o/r#1".into()
            }]
        );
    }

    #[test]
    fn audit_flags_double_claim() {
        let mut board = Board::default();
        board.tasks.insert(
            "o/r#1".into(),
            task("o/r#1", Status::InWork, &["a", "b"], None, 0),
        );
        assert_eq!(
            board.audit(0, 60),
            vec![Finding::DoubleClaim {
                task: "o/r#1".into()
            }]
        );
    }

    #[test]
    fn audit_flags_done_without_pr() {
        let mut board = Board::default();
        board
            .tasks
            .insert("o/r#1".into(), task("o/r#1", Status::Done, &["a"], None, 0));
        assert_eq!(
            board.audit(0, 60),
            vec![Finding::DoneWithoutPr {
                task: "o/r#1".into()
            }]
        );
    }

    #[test]
    fn audit_flags_pr_without_task() {
        let mut board = Board::default();
        board.prs.insert("https://pr/3".into(), "ghost#3".into());
        assert_eq!(
            board.audit(0, 60),
            vec![Finding::PrWithoutTask {
                pr: "https://pr/3".into()
            }]
        );
    }

    #[test]
    fn audit_flags_repeated_failure() {
        let mut board = Board::default();
        board.failures.insert("alice".into(), FAIL_LIMIT);
        assert_eq!(
            board.audit(0, 60),
            vec![Finding::RepeatedFailure {
                agent: "alice".into()
            }]
        );
    }

    #[test]
    fn audit_never_assigns() {
        let mut board = Board::default();
        board
            .tasks
            .insert("o/r#1".into(), task("o/r#1", Status::InWork, &[], None, 0));
        let before = board.tasks.clone();
        let findings = board.audit(0, 60);
        assert_eq!(findings.len(), 1);
        assert_eq!(board.tasks, before);
    }

    #[tokio::test]
    async fn ask_is_recorded_with_answer() {
        let fake = Fake {
            files: Mutex::new(BTreeMap::new()),
        };
        let mut board = Board::default();
        ask_recorded(&fake, &mut board, "user", "ship it?", 3).await;
        assert_eq!(board.asks.len(), 1);
        assert_eq!(board.asks[0].text, "ship it?");
        assert!(board.asks[0].answer.as_deref().unwrap().contains("noted"));
    }

    #[test]
    fn board_rows_have_the_feed_shape() {
        let mut board = Board::default();
        board.add_issue("o/r", 1, "t", 0);
        board.apply_state("o/r#1", "in_work busy-w\n", 90);
        board.touch_agent("busy-w", "working", 1.5, 95);
        board.touch_agent("idle-w", "waiting", 0.0, 60);
        let mut live = BTreeMap::new();
        live.insert(
            "busy-w".to_string(),
            Live {
                generating: true,
                cache_hit: 0.8,
            },
        );
        let rows = rows_from(&board, &BTreeMap::new(), &live, 100);
        assert_eq!(rows.len(), 2);
        let busy = rows.iter().find(|r| r["id"] == "busy-w").unwrap();
        assert_eq!(busy["state"], "busy");
        assert_eq!(busy["task"], "o/r#1");
        assert_eq!(busy["idle_s"], 5);
        assert_eq!(busy["time_on_task_s"], 10);
        let idle = rows.iter().find(|r| r["id"] == "idle-w").unwrap();
        assert_eq!(idle["state"], "idle");
        assert_eq!(idle["task"], "");
    }

    #[test]
    fn board_reply_shape_is_stable() {
        let v = Board::default().snapshot();
        for k in ["pending", "in_work", "done", "failed"] {
            assert!(v["tasks"][k].is_array(), "missing tasks.{k}");
        }
        assert!(v["agents"].is_object());
    }

    #[test]
    fn board_survives_restart() {
        let mut board = Board::default();
        board.add_issue("o/r", 1, "t", 0);
        board.apply_state("o/r#1", "in_work bob", 4);
        board.touch_agent("bob", "coding", 1.5, 4);
        let saved = serde_json::to_string(&board).unwrap();
        let reloaded: Board = serde_json::from_str(&saved).unwrap();
        assert_eq!(reloaded, board);
    }

    #[test]
    fn register_team_registers_board() {
        let reg = crate::registry();
        assert!(reg.get(HANDLER_MODULE).is_some());
    }

    #[test]
    fn rows_list_configured_groups_and_members() {
        let mut board = Board::default();
        board.touch_agent("m1", "working", 0.0, 90);
        let mut groups = BTreeMap::new();
        groups.insert(
            "g1".to_string(),
            Group {
                driver: "d1".into(),
                members: vec!["m1".into(), "m2".into()],
            },
        );
        let rows = rows_from(&board, &groups, &BTreeMap::new(), 100);
        let g = rows.iter().find(|r| r["id"] == "g1").unwrap();
        assert_eq!(g["parent"], Value::Null);
        assert_eq!(g["has_ai"], true);
        assert_eq!(g["driver"], "d1");
        let m1 = rows.iter().find(|r| r["id"] == "m1").unwrap();
        assert_eq!(m1["parent"], "g1");
    }

    #[test]
    fn a_member_never_registered_still_has_a_row() {
        let board = Board::default();
        let mut groups = BTreeMap::new();
        groups.insert(
            "g1".to_string(),
            Group {
                driver: String::new(),
                members: vec!["ghost".into()],
            },
        );
        let mut live = BTreeMap::new();
        live.insert(
            "ghost".to_string(),
            Live {
                generating: true,
                cache_hit: 0.0,
            },
        );
        let rows = rows_from(&board, &groups, &live, 100);
        let m = rows.iter().find(|r| r["id"] == "ghost").unwrap();
        assert_eq!(m["parent"], "g1");
        assert_eq!(m["state"], "busy");
        let g = rows.iter().find(|r| r["id"] == "g1").unwrap();
        assert_eq!(g["has_ai"], false);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn team_groups_survive_a_real_kernel_create() {
        let dir = tempfile::TempDir::new().unwrap();
        let kernel = crate::start(dir.path()).await.unwrap();
        let created = kernel
            .send(vec![fantastic_kernel::Item::new(
                "kernel",
                json!({
                    "type": "create_agent",
                    "id": "board_t",
                    "parent_id": "kernel_state",
                    "handler_module": HANDLER_MODULE,
                    "github": "gh",
                    "store": "store",
                    "team_groups": {
                        "g1": {"driver": "d1", "members": ["m1", "m2"]}
                    }
                }),
            )])
            .await;
        assert!(
            matches!(created[0].status, fantastic_kernel::Status::Completed),
            "create_agent failed: {:?}",
            created[0].result
        );
        let outcomes = kernel
            .send(vec![fantastic_kernel::Item::new(
                "board_t",
                json!({"type": "rows"}),
            )])
            .await;
        let rows = outcomes[0].result["rows"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let g = rows.iter().find(|r| r["id"] == "g1");
        assert!(g.is_some(), "group g1 missing from rows: {rows:?}");
        let m1 = rows.iter().find(|r| r["id"] == "m1").unwrap();
        assert_eq!(m1["parent"], "g1");
        kernel.shutdown().await;
        crate::bootstrap::shutdown(dir.path()).unwrap();
    }
}
