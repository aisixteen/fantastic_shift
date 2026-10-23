//! The cadencer (`team_cadencer.tools`): on a cadence each lead gets a short
//! digest of the level directly below it. A sub-lead's line rolls up its whole
//! subtree. Only changes since the last digest plus every stall are sent, and an
//! empty digest is not sent. A lead with no AI gets its digest on the board.
//!
//! The builder is pure: it takes a snapshot of the tree and the previous
//! snapshot. The bundle returns the digests; delivering them into each lead's
//! conversation is the caller's job. The bundle runs its own timer (when enabled)
//! and beats with the last tree given by a `tick`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use fantastic_kernel::bundle::{Bundle, BundleError, Reply};
use fantastic_kernel::{AgentId, Kernel};
use serde_json::{json, Value};
use tokio::task::JoinHandle;

/// The handler module name this bundle registers under.
pub const HANDLER_MODULE: &str = "team_cadencer.tools";
const NAME: &str = "team_cadencer";
const BOARD: &str = "board";

/// One agent in a snapshot of the tree.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Node {
    pub(crate) id: String,
    pub(crate) parent: Option<String>,
    pub(crate) has_ai: bool,
    pub(crate) state: String,
    pub(crate) task: String,
    pub(crate) open_inquiry: bool,
    pub(crate) idle_s: u64,
    pub(crate) time_on_task_s: u64,
    pub(crate) cost: f64,
    pub(crate) cache_hit: f64,
    pub(crate) new_results: u64,
}

/// The live settings of the cadencer.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    pub(crate) enabled: bool,
    pub(crate) cadence_s: u64,
    pub(crate) stall_after_s: u64,
    pub(crate) quiet: bool,
    pub(crate) off: BTreeSet<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: true,
            cadence_s: 60,
            stall_after_s: 300,
            quiet: true,
            off: BTreeSet::new(),
        }
    }
}

/// One digest: who it goes to and its lines.
#[derive(Debug, PartialEq)]
pub(crate) struct Digest {
    pub(crate) to: String,
    pub(crate) lines: Vec<String>,
}

fn children_of<'a>(tree: &'a [Node], id: &str) -> Vec<&'a Node> {
    tree.iter()
        .filter(|n| n.parent.as_deref() == Some(id))
        .collect()
}

fn subtree<'a>(tree: &'a [Node], id: &str) -> Vec<&'a Node> {
    let mut out = Vec::new();
    let mut stack = children_of(tree, id);
    while let Some(n) = stack.pop() {
        out.push(n);
        stack.extend(children_of(tree, &n.id));
    }
    out
}

/// A node holds a task when it has one, or an inquiry is open on it.
fn holds_task(n: &Node) -> bool {
    !n.task.is_empty() || n.open_inquiry
}

/// Stalled only while holding a task and idle past the threshold.
fn stalled(n: &Node, s: &Settings) -> bool {
    holds_task(n) && n.idle_s >= s.stall_after_s
}

fn changed(now: &Node, prev: Option<&Node>) -> bool {
    match prev {
        None => true,
        Some(p) => {
            p.state != now.state
                || p.task != now.task
                || p.cost != now.cost
                || p.new_results != now.new_results
        }
    }
}

fn node_line(n: &Node, s: &Settings) -> String {
    let new = if n.new_results > 0 {
        format!(" · +{} new", n.new_results)
    } else {
        String::new()
    };
    let stall = if stalled(n, s) {
        format!(" · STALLED {}s", n.idle_s)
    } else {
        String::new()
    };
    format!(
        "{}: {} · {} · {}s on task · cost {:.2} · cache {:.0}%{}{}",
        n.id,
        n.state,
        n.task,
        n.time_on_task_s,
        n.cost,
        n.cache_hit * 100.0,
        new,
        stall
    )
}

fn rollup_line(id: &str, group: &[&Node], s: &Settings) -> String {
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for n in group {
        *by.entry(n.state.as_str()).or_insert(0) += 1;
    }
    let states = by
        .iter()
        .map(|(k, v)| format!("{v} {k}"))
        .collect::<Vec<_>>()
        .join(", ");
    let new: u64 = group.iter().map(|n| n.new_results).sum();
    let stalls = group.iter().filter(|n| stalled(n, s)).count();
    let stall = if stalls > 0 {
        format!(" · {stalls} STALLED")
    } else {
        String::new()
    };
    format!(
        "{id}: {} agents ({states}) · +{new} new{stall}",
        group.len()
    )
}

/// Build the digests for one tick. A line goes out when its branch changed
/// since `prev`, or when a worker in it is stalled; in quiet mode nothing else
/// does. A lead with no lines gets no digest.
pub(crate) fn build(tree: &[Node], prev: &[Node], s: &Settings) -> Vec<Digest> {
    let mut out = Vec::new();
    for lead in tree.iter().filter(|n| !children_of(tree, &n.id).is_empty()) {
        if s.off.contains(&lead.id) {
            continue;
        }
        let mut lines = Vec::new();
        for c in children_of(tree, &lead.id) {
            let mut group: Vec<&Node> = vec![c];
            group.extend(subtree(tree, &c.id));
            let any_change = group
                .iter()
                .any(|n| changed(n, prev.iter().find(|p| p.id == n.id)));
            let any_stall = group.iter().any(|n| stalled(n, s));
            if s.quiet && !any_change && !any_stall {
                continue;
            }
            lines.push(if group.len() == 1 {
                node_line(c, s)
            } else {
                rollup_line(&c.id, &group, s)
            });
        }
        if lines.is_empty() {
            continue;
        }
        let to = if lead.has_ai {
            lead.id.clone()
        } else {
            BOARD.to_string()
        };
        out.push(Digest { to, lines });
    }
    out
}

fn str_of(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn u64_of(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(|x| x.as_u64()).unwrap_or(0)
}

fn node_from(v: &Value) -> Option<Node> {
    let id = v.get("id")?.as_str()?.to_string();
    Some(Node {
        id,
        parent: v.get("parent").and_then(|x| x.as_str()).map(String::from),
        has_ai: v.get("has_ai").and_then(|x| x.as_bool()).unwrap_or(true),
        state: str_of(v, "state"),
        task: str_of(v, "task"),
        open_inquiry: v.get("inquiry").and_then(|x| x.as_bool()).unwrap_or(false),
        idle_s: u64_of(v, "idle_s"),
        time_on_task_s: u64_of(v, "time_on_task_s"),
        cost: v.get("cost").and_then(|x| x.as_f64()).unwrap_or(0.0),
        cache_hit: v.get("cache_hit").and_then(|x| x.as_f64()).unwrap_or(0.0),
        new_results: u64_of(v, "new_results"),
    })
}

fn apply_set(s: &mut Settings, p: &Value) {
    if let Some(e) = p.get("enabled").and_then(|x| x.as_bool()) {
        s.enabled = e;
    }
    if let Some(c) = p.get("cadence").and_then(|x| x.as_u64()) {
        s.cadence_s = c;
    }
    if let Some(c) = p.get("stall_after").and_then(|x| x.as_u64()) {
        s.stall_after_s = c;
    }
    if let Some(q) = p.get("quiet").and_then(|x| x.as_bool()) {
        s.quiet = q;
    }
    if let Some(lead) = p.get("lead").and_then(|x| x.as_str()) {
        let on = p.get("on").and_then(|x| x.as_bool()).unwrap_or(true);
        if on {
            s.off.remove(lead);
        } else {
            s.off.insert(lead.to_string());
        }
    }
}

fn settings_json(s: &Settings) -> Value {
    json!({
        "enabled": s.enabled,
        "cadence": s.cadence_s,
        "stall_after": s.stall_after_s,
        "quiet": s.quiet,
        "off": s.off.iter().collect::<Vec<_>>(),
    })
}

fn digest_json(d: &Digest) -> Value {
    json!({"to": d.to, "lines": d.lines, "text": d.lines.join("\n")})
}

/// Seconds since the first call in this process; honors tokio paused time.
fn now_s() -> u64 {
    static ORIGIN: OnceLock<tokio::time::Instant> = OnceLock::new();
    let origin = *ORIGIN.get_or_init(tokio::time::Instant::now);
    tokio::time::Instant::now().duration_since(origin).as_secs()
}

#[derive(Default)]
struct Inner {
    settings: Settings,
    parent: Option<String>,
    tree: Vec<Node>,
    prev: Vec<Node>,
    ticks: u64,
    digests_sent: u64,
    last_beat_s: Option<u64>,
    next_beat_s: Option<u64>,
    timer: Option<JoinHandle<()>>,
}

fn timer_alive(inner: &Inner) -> bool {
    inner.timer.as_ref().is_some_and(|h| !h.is_finished())
}

/// One beat: build digests from the stored tree, then remember it.
fn beat(inner: &mut Inner) -> Vec<Digest> {
    let digests = build(&inner.tree, &inner.prev, &inner.settings);
    inner.prev = inner.tree.clone();
    inner.ticks += 1;
    inner.digests_sent += digests.len() as u64;
    let now = now_s();
    inner.last_beat_s = Some(now);
    inner.next_beat_s = if timer_alive(inner) {
        Some(now + inner.settings.cadence_s)
    } else {
        None
    };
    digests
}

/// Stop the old timer and, when enabled with a cadence, start a new one.
fn restart_timer(shared: &Arc<Mutex<Inner>>, inner: &mut Inner) {
    if let Some(h) = inner.timer.take() {
        h.abort();
    }
    inner.next_beat_s = None;
    if !inner.settings.enabled || inner.settings.cadence_s == 0 {
        return;
    }
    let period = Duration::from_secs(inner.settings.cadence_s);
    let shared = Arc::clone(shared);
    inner.timer = Some(tokio::spawn(async move {
        let mut every = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        loop {
            every.tick().await;
            let mut g = shared.lock().expect("cadencer state");
            beat(&mut g);
        }
    }));
    inner.next_beat_s = Some(now_s() + inner.settings.cadence_s);
}

/// Only the parent, `maintainer` or `user` may steer the cadencer.
fn allowed(inner: &Inner, sender: &str) -> bool {
    sender == "maintainer"
        || sender == "user"
        || inner.parent.as_deref().is_some_and(|p| p == sender)
}

fn dispatch(shared: &Arc<Mutex<Inner>>, payload: &Value) -> Value {
    let t = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let mut inner = shared.lock().expect("cadencer state");
    match t {
        "configure" => {
            if let Some(p) = payload.get("parent").and_then(|v| v.as_str()) {
                inner.parent = Some(p.to_string());
            }
            json!({"parent": inner.parent})
        }
        "tick" => {
            inner.tree = payload
                .get("tree")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(node_from).collect())
                .unwrap_or_default();
            let digests = beat(&mut inner);
            json!({
                "digests": digests.iter().map(digest_json).collect::<Vec<_>>(),
                "sent": digests.len(),
            })
        }
        "set" => {
            if !allowed(&inner, &str_of(payload, "sender")) {
                return json!({"error": "team_cadencer: set refused"});
            }
            apply_set(&mut inner.settings, payload);
            restart_timer(shared, &mut inner);
            settings_json(&inner.settings)
        }
        "status" => json!({
            "settings": settings_json(&inner.settings),
            "ticks": inner.ticks,
            "last_beat_s": inner.last_beat_s,
            "next_beat_s": inner.next_beat_s,
            "digests_sent": inner.digests_sent,
        }),
        other => json!({"error": format!("team_cadencer: unknown type '{other}'")}),
    }
}

/// The cadencer bundle.
#[derive(Default)]
pub struct CadencerBundle {
    inner: Arc<Mutex<Inner>>,
}

#[async_trait::async_trait]
impl Bundle for CadencerBundle {
    fn name(&self) -> &str {
        NAME
    }

    fn verb_docs(&self) -> Value {
        json!({
            "tick": "tick {tree: [{id, parent?, has_ai?, state, task, inquiry?, idle_s, time_on_task_s, cost, cache_hit, new_results}]} → {digests: [{to, lines, text}], sent}",
            "set": "set {sender, enabled?, cadence?, stall_after?, quiet?, lead?, on?} → settings (sender must be the parent, maintainer or user)",
            "configure": "configure {parent} → {parent}",
            "status": "status {} → {settings, ticks, last_beat_s, next_beat_s, digests_sent}",
        })
    }

    async fn handle(
        &self,
        _agent_id: &AgentId,
        payload: &Value,
        _kernel: &Arc<Kernel>,
    ) -> Result<Reply, BundleError> {
        Ok(Some(dispatch(&self.inner, payload)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(id: &str, parent: Option<&str>, has_ai: bool, state: &str, idle: u64) -> Node {
        Node {
            id: id.into(),
            parent: parent.map(String::from),
            has_ai,
            state: state.into(),
            task: "t".into(),
            open_inquiry: false,
            idle_s: idle,
            time_on_task_s: 10,
            cost: 0.5,
            cache_hit: 0.8,
            new_results: 0,
        }
    }

    /// root → A, B (sub-leads) → three workers each.
    fn two_by_three() -> Vec<Node> {
        let mut t = vec![
            n("root", None, true, "busy", 0),
            n("A", Some("root"), true, "busy", 0),
            n("B", Some("root"), true, "busy", 0),
        ];
        for lead in ["A", "B"] {
            for w in 1..=3 {
                t.push(n(&format!("{lead}{w}"), Some(lead), true, "busy", 0));
            }
        }
        t
    }

    fn digest_for<'a>(ds: &'a [Digest], to: &str) -> Option<&'a Digest> {
        ds.iter().find(|d| d.to == to)
    }

    #[test]
    fn root_gets_one_rollup_line_per_sublead() {
        let t = two_by_three();
        let ds = build(&t, &[], &Settings::default());
        let root = digest_for(&ds, "root").expect("root digest");
        assert_eq!(root.lines.len(), 2);
        assert!(root.lines[0].starts_with("A:") && root.lines[0].contains("4 agents"));
        assert!(root.lines[1].starts_with("B:") && root.lines[1].contains("4 agents"));
    }

    #[test]
    fn sublead_gets_one_line_per_worker() {
        let t = two_by_three();
        let ds = build(&t, &[], &Settings::default());
        assert_eq!(digest_for(&ds, "A").unwrap().lines.len(), 3);
        assert_eq!(digest_for(&ds, "B").unwrap().lines.len(), 3);
    }

    #[test]
    fn stalled_worker_reported_after_stall_after_once() {
        let mut t = two_by_three();
        let prev = t.clone();
        // Below the threshold: nothing reported in quiet mode.
        t[3].idle_s = 299;
        let ds = build(&t, &prev, &Settings::default());
        assert!(digest_for(&ds, "A").is_none());
        // Past the threshold: reported, one line for that worker.
        t[3].idle_s = 300;
        let ds = build(&t, &prev, &Settings::default());
        let a = digest_for(&ds, "A").expect("A digest");
        assert_eq!(a.lines.len(), 1);
        assert!(a.lines[0].contains("A1") && a.lines[0].contains("STALLED"));
    }

    #[test]
    fn still_stalled_worker_reported_again_next_digest() {
        let mut t = two_by_three();
        t[3].idle_s = 400;
        let prev = t.clone();
        let first = build(&t, &prev, &Settings::default());
        let second = build(&t, &prev, &Settings::default());
        assert!(digest_for(&first, "A").unwrap().lines[0].contains("STALLED"));
        assert!(digest_for(&second, "A").unwrap().lines[0].contains("STALLED"));
    }

    #[test]
    fn idle_member_without_a_task_is_not_stalled() {
        let mut reviewer = n("reviewer", Some("L"), true, "idle", 600);
        reviewer.task = String::new();
        let t = vec![n("L", None, true, "busy", 0), reviewer];
        let ds = build(&t, &[], &Settings::default());
        for d in &ds {
            for line in &d.lines {
                assert!(!line.contains("STALLED"), "unexpected: {line}");
            }
        }
    }

    #[test]
    fn live_set_takes_effect_next_tick() {
        let mut bundle = Inner::default();
        let mut t = two_by_three();
        t[3].idle_s = 100;
        let prev = t.clone();
        // Default stall_after (300s): 100s idle is not a stall.
        assert!(build(&t, &prev, &bundle.settings).is_empty());
        apply_set(
            &mut bundle.settings,
            &json!({"stall_after": 60, "cadence": 30}),
        );
        assert_eq!(bundle.settings.cadence_s, 30);
        let ds = build(&t, &prev, &bundle.settings);
        assert!(digest_for(&ds, "A").unwrap().lines[0].contains("STALLED"));
    }

    #[test]
    fn quiet_mode_sends_nothing_without_change() {
        let t = two_by_three();
        let ds = build(&t, &t, &Settings::default());
        assert!(ds.is_empty());
    }

    #[test]
    fn lead_without_ai_digest_goes_to_board() {
        let mut t = two_by_three();
        t[1].has_ai = false; // A has no AI
        let ds = build(&t, &[], &Settings::default());
        assert!(digest_for(&ds, "A").is_none());
        assert_eq!(digest_for(&ds, BOARD).unwrap().lines.len(), 3);
    }

    #[test]
    fn register_team_registers_cadencer() {
        let mut reg = fantastic_kernel::BundleRegistry::new();
        crate::register_team(&mut reg);
        let names: Vec<&str> = reg.catalog().map(|(n, _)| n).collect();
        assert!(names.contains(&HANDLER_MODULE));
    }

    // ---- dispatch, timer and status tests ----

    fn shared() -> Arc<Mutex<Inner>> {
        Arc::new(Mutex::new(Inner::default()))
    }

    fn status(b: &Arc<Mutex<Inner>>) -> Value {
        dispatch(b, &json!({"type": "status"}))
    }

    /// Advance paused time by `secs`, then one more ms so timer tasks due at
    /// the boundary have run.
    async fn run_for(secs: u64) {
        tokio::time::sleep(Duration::from_secs(secs)).await;
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    fn tick_with(a1_idle: u64, a1_task: &str) -> Value {
        json!({"type": "tick", "tree": [
            {"id": "root", "has_ai": true, "state": "busy", "task": "x", "idle_s": 0},
            {"id": "A", "parent": "root", "has_ai": true, "state": "busy", "task": "x", "idle_s": 0},
            {"id": "A1", "parent": "A", "has_ai": true, "state": "busy", "task": a1_task, "idle_s": a1_idle}
        ]})
    }

    fn lines_for(reply: &Value, to: &str) -> Vec<String> {
        reply["digests"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| d["to"] == to)
            .flat_map(|d| d["lines"].as_array().cloned().unwrap_or_default())
            .map(|l| l.as_str().unwrap_or("").to_string())
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn dispatcher_ticks_on_its_own_timer() {
        let b = shared();
        dispatch(&b, &json!({"type": "set", "sender": "user", "cadence": 60}));
        run_for(180).await;
        assert_eq!(status(&b)["ticks"], 3);
    }

    #[tokio::test(start_paused = true)]
    async fn dispatcher_off_runs_no_timer() {
        let b = shared();
        dispatch(
            &b,
            &json!({"type": "set", "sender": "user", "enabled": false}),
        );
        run_for(600).await;
        let s = status(&b);
        assert_eq!(s["ticks"], 0);
        assert!(s["next_beat_s"].is_null());
    }

    #[tokio::test(start_paused = true)]
    async fn parent_driver_sets_child_cadence_live() {
        let b = shared();
        dispatch(&b, &json!({"type": "configure", "parent": "lead"}));
        dispatch(&b, &json!({"type": "set", "sender": "user", "cadence": 60}));
        run_for(30).await;
        let before = status(&b)["ticks"].as_u64().unwrap();
        dispatch(&b, &json!({"type": "set", "sender": "lead", "cadence": 10}));
        run_for(30).await;
        assert_eq!(status(&b)["ticks"].as_u64().unwrap(), before + 3);
    }

    #[tokio::test(start_paused = true)]
    async fn set_refused_from_a_non_parent() {
        let b = shared();
        dispatch(&b, &json!({"type": "configure", "parent": "lead"}));
        dispatch(&b, &json!({"type": "set", "sender": "user", "cadence": 60}));
        let r = dispatch(
            &b,
            &json!({"type": "set", "sender": "stranger", "cadence": 5}),
        );
        assert_eq!(r, json!({"error": "team_cadencer: set refused"}));
        assert_eq!(status(&b)["settings"]["cadence"], 60);
    }

    #[test]
    fn stalled_worker_flagged_once_then_while_still_stalled() {
        let b = shared();
        let d1 = dispatch(&b, &tick_with(400, "x"));
        let l1 = lines_for(&d1, "A");
        assert_eq!(l1.len(), 1);
        assert!(l1[0].contains("STALLED"));
        // Still idle and growing: reported again.
        let d2 = dispatch(&b, &tick_with(460, "x"));
        let l2 = lines_for(&d2, "A");
        assert_eq!(l2.len(), 1);
        assert!(l2[0].contains("STALLED"));
        // It moves: stall gone, nothing sent for A.
        let d3 = dispatch(&b, &tick_with(0, "x"));
        assert!(lines_for(&d3, "A").is_empty());
    }

    #[test]
    fn idle_member_without_a_task_is_not_stalled_live() {
        let b = shared();
        let tree = json!({"type": "tick", "tree": [
            {"id": "L", "has_ai": true, "state": "busy", "task": "x", "idle_s": 0},
            {"id": "reviewer", "parent": "L", "has_ai": true, "state": "idle", "task": "", "idle_s": 600}
        ]});
        for _ in 0..2 {
            let d = dispatch(&b, &tree);
            assert!(lines_for(&d, "L").iter().all(|l| !l.contains("STALLED")));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn status_reports_last_and_next_beat_and_digests_sent() {
        let b = shared();
        dispatch(&b, &json!({"type": "set", "sender": "user", "cadence": 60}));
        let first = dispatch(&b, &tick_with(0, "x"));
        let n1 = first["digests"].as_array().unwrap().len() as u64;
        run_for(60).await;
        let s = status(&b);
        assert_eq!(s["ticks"], 2);
        assert_eq!(s["digests_sent"].as_u64().unwrap(), n1);
        assert_eq!(
            s["next_beat_s"].as_u64().unwrap() - s["last_beat_s"].as_u64().unwrap(),
            60
        );
    }
}
