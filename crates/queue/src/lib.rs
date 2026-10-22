//! The queue agent of a fantastic shift: tasks as data.
//!
//! A task is data: an id, a title, instructions (the text, or a path to a file
//! the worker reads), a status, who holds it, and a result. The queue stores it
//! and hands it out. It never reads `instructions_file` and never interprets
//! what the work is. The order is "oldest pending first" and nothing more.
//!
//! Two rules make it safe to share. `take` is atomic: every verb runs under one
//! lock, so one pending task goes to exactly one taker. A taken task holds a
//! lease: when the lease ends, or the taker is no longer an agent on this
//! kernel, the task goes back to pending. Both are checked on every call; there
//! is no timer.
//!
//! The tasks persist in the agent's `tasks.json` sidecar through the root
//! loader, the way the scheduler persists its schedules.

#![deny(missing_docs)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use fantastic_bundle::py_repr_value;
use fantastic_kernel::bundle::{Bundle, BundleError, Reply};
use fantastic_kernel::refusal::{
    bad, missing, missing_hint, refusal, refusal_hint, with, BAD_ARG, EXISTS, NOT_FOUND, UNHANDLED,
};
use fantastic_kernel::{AgentId, Kernel};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// The `handler_module` this bundle answers to.
pub const HANDLER_MODULE: &str = "shift_queue.tools";
/// The readme the kernel shows for this bundle.
pub const README: &str = include_str!("readme.md");
/// The sidecar the tasks live in, beside the agent's own record.
pub const BLOB: &str = "tasks.json";
/// The lease a take gets when the caller names none, in seconds.
pub const DEFAULT_LEASE_S: f64 = 600.0;
/// The longest lease a caller may name, in seconds (one day).
pub const MAX_LEASE_S: f64 = 86400.0;
/// A task nobody holds yet.
pub const PENDING: &str = "pending";
/// A task held under a lease.
pub const TAKEN: &str = "taken";
/// A task finished with a result.
pub const DONE: &str = "done";
/// A task finished without success.
pub const FAILED: &str = "failed";
/// Every status a task can have.
pub const STATUSES: [&str; 4] = [PENDING, TAKEN, DONE, FAILED];

const VERB_DOCS: [(&str, &str); 10] = [
    (
        "reflect",
        "{type} — what the queue is, its counts. No args.",
    ),
    ("boot", "{type} — no-op."),
    (
        "add",
        "{type, title, instructions|instructions_file, id?, data?} — append a pending task.",
    ),
    ("list", "{type, status?} — tasks in insertion order."),
    ("get", "{type, id} — one task."),
    (
        "take",
        "{type, taker, lease_s?} — the oldest pending task, taken under a lease.",
    ),
    (
        "renew",
        "{type, id, taker, lease_s?} — extend the lease of a task you hold.",
    ),
    (
        "done",
        "{type, id, taker, status: done|failed, result} — finish a task you hold.",
    ),
    (
        "release",
        "{type, id, taker} — put a task you hold back to pending.",
    ),
    ("status", "{type} — counts per status and the total."),
];

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// One task. The queue stores it; it never interprets title, instructions or data.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    /// Unique within the queue.
    pub id: String,
    /// A short human name.
    pub title: String,
    /// The instructions as text, when given inline.
    #[serde(default)]
    pub instructions: Option<String>,
    /// A path to the instructions file the worker reads.
    #[serde(default)]
    pub instructions_file: Option<String>,
    /// Any JSON object the adder gave, else null.
    #[serde(default)]
    pub data: Value,
    /// One of the STATUSES.
    pub status: String,
    /// Who holds the task (taken), or who finished it (done or failed).
    #[serde(default)]
    pub taker: Option<String>,
    /// Unix seconds when the lease ends. Set only while taken.
    #[serde(default)]
    pub lease_until: Option<f64>,
    /// The result a finished task carries; null until done.
    #[serde(default)]
    pub result: Value,
    /// Unix seconds when the task was added.
    pub created_at: f64,
    /// Unix seconds of the last take.
    #[serde(default)]
    pub taken_at: Option<f64>,
    /// Unix seconds when the task finished.
    #[serde(default)]
    pub done_at: Option<f64>,
    /// How many times the task was taken.
    #[serde(default)]
    pub attempts: u64,
    /// How many times the task went back to pending.
    #[serde(default)]
    pub requeues: u64,
}

/// The whole sidecar document.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QueueDoc {
    /// The document format version.
    pub version: u64,
    /// The next number to mint a task id from.
    pub next_id: u64,
    /// Every task, in insertion order.
    pub tasks: Vec<Task>,
}

impl QueueDoc {
    fn new() -> Self {
        QueueDoc {
            version: 1,
            next_id: 1,
            tasks: Vec::new(),
        }
    }
}

/// The `shift_queue.tools` bundle: one loaded QueueDoc per agent id.
#[derive(Default)]
pub struct QueueBundle {
    // The lock IS the atomicity: every verb holds it from its first read until
    // its persist has finished. A tokio mutex, because it is held across awaits.
    docs: tokio::sync::Mutex<HashMap<String, QueueDoc>>,
}

type Res = Result<(Value, bool), Value>;

fn wire(t: &Task) -> Value {
    serde_json::to_value(t).unwrap_or(Value::Null)
}

fn fail(text: String, reply: &Value) -> Value {
    let mut out = json!({ "error": text });
    if let Some(r) = reply.get("reason") {
        out["reason"] = r.clone();
    }
    out
}

fn loader(kernel: &Kernel) -> Option<Arc<fantastic_kernel::Agent>> {
    kernel
        .root()
        .filter(|r| r.handler_module.as_deref() == Some("kernel_state.tools"))
}

async fn loader_call(
    kernel: &Arc<Kernel>,
    root: &Arc<fantastic_kernel::Agent>,
    payload: Value,
) -> Value {
    kernel.bookkeeping(root, payload, None).await.result
}

async fn load_doc(agent_id: &AgentId, kernel: &Arc<Kernel>, verb: &str) -> Result<QueueDoc, Value> {
    let Some(root) = loader(kernel) else {
        return Err(json!({"error": format!(
            "{verb}: cannot load tasks.json — the root is not a kernel_state loader, no store"
        )}));
    };
    let reply = loader_call(
        kernel,
        &root,
        json!({"type": "load_blob", "agent_id": agent_id.0, "name": BLOB}),
    )
    .await;
    if let Some(e) = reply.get("error").and_then(Value::as_str) {
        return Err(fail(
            format!("{verb}: cannot load tasks.json — {e}"),
            &reply,
        ));
    }
    match reply.get("content") {
        Some(Value::Null) => Ok(QueueDoc::new()),
        Some(Value::String(s)) => serde_json::from_str::<QueueDoc>(s).map_err(
            |e| json!({"error": format!("{verb}: tasks.json is not a queue document — {e}")}),
        ),
        _ => Err(json!({"error": format!(
            "{verb}: tasks.json is not a queue document — the loader sent no content"
        )})),
    }
}

async fn save_doc(
    agent_id: &AgentId,
    kernel: &Arc<Kernel>,
    verb: &str,
    doc: &QueueDoc,
) -> Result<(), Value> {
    let Some(root) = loader(kernel) else {
        return Err(json!({"error": format!(
            "{verb}: cannot save tasks.json — the root is not a kernel_state loader, no store"
        )}));
    };
    let content = serde_json::to_string_pretty(doc)
        .map_err(|e| json!({"error": format!("{verb}: cannot save tasks.json — {e}")}))?;
    let reply = loader_call(
        kernel,
        &root,
        json!({"type": "persist_blob", "agent_id": agent_id.0, "name": BLOB, "content": content}),
    )
    .await;
    match reply.get("error").and_then(Value::as_str) {
        Some(e) => Err(fail(
            format!("{verb}: cannot save tasks.json — {e}"),
            &reply,
        )),
        None => Ok(()),
    }
}

fn requeue(t: &mut Task, reason: &str, queue: &str, events: &mut Vec<Value>) {
    let old = t.taker.take();
    events.push(json!({"type": "task_requeued", "queue": queue, "id": t.id, "taker": old, "reason": reason}));
    t.status = PENDING.into();
    t.lease_until = None;
    t.taken_at = None;
    t.requeues += 1;
}

// Put back to pending every taken task whose lease ended or whose taker is gone.
fn sweep(doc: &mut QueueDoc, now: f64, queue: &str, live: impl Fn(&str) -> bool) -> Vec<Value> {
    let mut events = Vec::new();
    for t in doc.tasks.iter_mut() {
        if t.status != TAKEN {
            continue;
        }
        let reason = if t.lease_until.is_none_or(|u| u <= now) {
            "lease_expired"
        } else if !live(t.taker.as_deref().unwrap_or("")) {
            "taker_gone"
        } else {
            continue;
        };
        requeue(t, reason, queue, &mut events);
    }
    events
}

fn lease_of(verb: &str, p: &Value) -> Result<f64, Value> {
    match p.get("lease_s") {
        None | Some(Value::Null) => Ok(DEFAULT_LEASE_S),
        Some(v) => match v.as_f64() {
            Some(f) if f.is_finite() && f > 0.0 && f <= MAX_LEASE_S => Ok(f),
            _ => Err(bad(verb, "lease_s", "a number of seconds > 0 and <= 86400")),
        },
    }
}

fn str_arg<'a>(verb: &str, p: &'a Value, arg: &str) -> Result<&'a str, Value> {
    match p.get(arg) {
        None | Some(Value::Null) => Err(missing(verb, arg)),
        Some(Value::String(s)) if !s.is_empty() => Ok(s),
        Some(Value::String(_)) => Err(missing(verb, arg)),
        Some(_) => Err(bad(verb, arg, "a string")),
    }
}

fn opt_str(verb: &str, p: &Value, arg: &str) -> Result<Option<String>, Value> {
    match p.get(arg) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.is_empty() => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(bad(verb, arg, "a string")),
    }
}

fn not_held(verb: &str, t: &Task, taker: &str) -> Value {
    let mut r = if t.status == TAKEN {
        let holder = t.taker.as_deref().unwrap_or("nobody");
        refusal(
            BAD_ARG,
            format!("{verb}: task {} is taken by {holder}, not {taker}", t.id),
        )
    } else {
        refusal(
            BAD_ARG,
            format!("{verb}: task {} is {}, not taken", t.id, t.status),
        )
    };
    with(&mut r, "arg", json!("taker"));
    with(&mut r, "id", json!(t.id));
    with(&mut r, "status", json!(t.status));
    r
}

fn no_task(verb: &str, id: &str) -> Value {
    let mut r = refusal(NOT_FOUND, format!("{verb}: no task {id}"));
    with(&mut r, "id", json!(id));
    r
}

fn find_mut<'a>(doc: &'a mut QueueDoc, id: &str, verb: &str) -> Result<&'a mut Task, Value> {
    doc.tasks
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or_else(|| no_task(verb, id))
}

fn counts(doc: &QueueDoc) -> Value {
    let mut m = Map::new();
    for s in STATUSES {
        m.insert(
            s.to_string(),
            json!(doc.tasks.iter().filter(|t| t.status == s).count()),
        );
    }
    Value::Object(m)
}

fn add(doc: &mut QueueDoc, p: &Value, now: f64) -> Res {
    const V: &str = "add";
    let title = str_arg(V, p, "title")?.to_string();
    let instr = opt_str(V, p, "instructions")?;
    let file = opt_str(V, p, "instructions_file")?;
    if instr.is_some() && file.is_some() {
        let mut r = refusal(
            BAD_ARG,
            format!("{V}: give instructions or instructions_file, not both"),
        );
        with(&mut r, "arg", json!("instructions_file"));
        return Err(r);
    }
    if instr.is_none() && file.is_none() {
        return Err(missing_hint(
            V,
            "instructions",
            "give instructions (the text) or instructions_file (a path the worker reads)",
        ));
    }
    let data = match p.get("data") {
        None | Some(Value::Null) => Value::Null,
        Some(v @ Value::Object(_)) => v.clone(),
        Some(_) => return Err(bad(V, "data", "an object")),
    };
    let id = match p.get("id") {
        None | Some(Value::Null) => loop {
            let cand = format!("t{}", doc.next_id);
            doc.next_id += 1;
            if !doc.tasks.iter().any(|t| t.id == cand) {
                break cand;
            }
        },
        Some(Value::String(s)) if !s.is_empty() => {
            if doc.tasks.iter().any(|t| t.id == *s) {
                let mut r = refusal(EXISTS, format!("{V}: task {s} already exists"));
                with(&mut r, "id", json!(s));
                return Err(r);
            }
            s.clone()
        }
        Some(_) => return Err(bad(V, "id", "a non-empty string")),
    };
    let t = Task {
        id,
        title,
        instructions: instr,
        instructions_file: file,
        data,
        status: PENDING.into(),
        taker: None,
        lease_until: None,
        result: Value::Null,
        created_at: now,
        taken_at: None,
        done_at: None,
        attempts: 0,
        requeues: 0,
    };
    let w = wire(&t);
    doc.tasks.push(t);
    Ok((json!({ "task": w }), true))
}

fn list(doc: &QueueDoc, p: &Value) -> Res {
    let filter = match p.get("status") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if STATUSES.contains(&s.as_str()) => Some(s.clone()),
        Some(_) => return Err(bad("list", "status", "one of pending, taken, done, failed")),
    };
    let tasks: Vec<Value> = doc
        .tasks
        .iter()
        .filter(|t| filter.as_ref().is_none_or(|f| t.status == *f))
        .map(wire)
        .collect();
    let count = tasks.len();
    Ok((json!({ "tasks": tasks, "count": count }), false))
}

fn get(doc: &QueueDoc, p: &Value) -> Res {
    const V: &str = "get";
    let id = str_arg(V, p, "id")?;
    let t = doc
        .tasks
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| no_task(V, id))?;
    Ok((json!({ "task": wire(t) }), false))
}

fn take(
    doc: &mut QueueDoc,
    p: &Value,
    queue: &str,
    now: f64,
    live: &dyn Fn(&str) -> bool,
    events: &mut Vec<Value>,
) -> Res {
    const V: &str = "take";
    let taker = str_arg(V, p, "taker")?;
    if !live(taker) {
        let mut r = refusal_hint(
            NOT_FOUND,
            format!("take: taker {taker} is not an agent on this kernel"),
            "take for a live agent id — a task whose taker is gone goes back to pending",
        );
        with(&mut r, "arg", json!("taker"));
        return Err(r);
    }
    let lease = lease_of(V, p)?;
    let Some(t) = doc.tasks.iter_mut().find(|t| t.status == PENDING) else {
        return Ok((json!({ "task": null }), false));
    };
    t.status = TAKEN.into();
    t.taker = Some(taker.to_string());
    t.lease_until = Some(now + lease);
    t.taken_at = Some(now);
    t.attempts += 1;
    events.push(json!({"type": "task_taken", "queue": queue, "id": t.id, "taker": taker, "lease_until": now + lease}));
    Ok((json!({ "task": wire(t) }), true))
}

fn renew(doc: &mut QueueDoc, p: &Value, now: f64) -> Res {
    const V: &str = "renew";
    let id = str_arg(V, p, "id")?;
    let taker = str_arg(V, p, "taker")?;
    let lease = lease_of(V, p)?;
    let t = find_mut(doc, id, V)?;
    if !(t.status == TAKEN && t.taker.as_deref() == Some(taker)) {
        return Err(not_held(V, t, taker));
    }
    t.lease_until = Some(now + lease);
    Ok((json!({ "task": wire(t) }), true))
}

fn done(doc: &mut QueueDoc, p: &Value, queue: &str, now: f64, events: &mut Vec<Value>) -> Res {
    const V: &str = "done";
    let id = str_arg(V, p, "id")?;
    let taker = str_arg(V, p, "taker")?;
    let status = str_arg(V, p, "status")?;
    if status != DONE && status != FAILED {
        return Err(bad(V, "status", "done or failed"));
    }
    let result = match p.get("result") {
        None | Some(Value::Null) => return Err(missing(V, "result")),
        Some(v) => v.clone(),
    };
    let t = find_mut(doc, id, V)?;
    if !(t.status == TAKEN && t.taker.as_deref() == Some(taker)) {
        return Err(not_held(V, t, taker));
    }
    t.status = status.to_string();
    t.result = result;
    t.done_at = Some(now);
    t.lease_until = None;
    events.push(
        json!({"type": "task_done", "queue": queue, "id": t.id, "taker": taker, "status": status}),
    );
    Ok((json!({ "task": wire(t) }), true))
}

fn release(doc: &mut QueueDoc, p: &Value, queue: &str, events: &mut Vec<Value>) -> Res {
    const V: &str = "release";
    let id = str_arg(V, p, "id")?;
    let taker = str_arg(V, p, "taker")?;
    let t = find_mut(doc, id, V)?;
    if !(t.status == TAKEN && t.taker.as_deref() == Some(taker)) {
        return Err(not_held(V, t, taker));
    }
    requeue(t, "released", queue, events);
    Ok((json!({ "task": wire(t) }), true))
}

// Run one verb on the (already swept) document. Returns the reply and whether
// the verb changed the document. A refusal is an Err and changes nothing.
fn apply(
    verb: &str,
    doc: &mut QueueDoc,
    p: &Value,
    queue: &str,
    now: f64,
    live: &dyn Fn(&str) -> bool,
    events: &mut Vec<Value>,
) -> Res {
    match verb {
        "add" => add(doc, p, now),
        "list" => list(doc, p),
        "get" => get(doc, p),
        "take" => take(doc, p, queue, now, live, events),
        "renew" => renew(doc, p, now),
        "done" => done(doc, p, queue, now, events),
        "release" => release(doc, p, queue, events),
        _ => Ok((
            json!({ "counts": counts(doc), "total": doc.tasks.len() }),
            false,
        )),
    }
}

#[async_trait]
impl Bundle for QueueBundle {
    fn name(&self) -> &str {
        "shift_queue"
    }

    fn verb_docs(&self) -> Value {
        let mut verbs = Map::new();
        for (n, d) in VERB_DOCS {
            verbs.insert(n.to_string(), Value::from(d));
        }
        Value::Object(verbs)
    }

    fn emits_doc(&self) -> Option<Value> {
        Some(json!({
            "task_taken": "{type, queue, id, taker, lease_until} — on the queue's own inbox after a take",
            "task_done": "{type, queue, id, taker, status} — after done",
            "task_requeued": "{type, queue, id, taker, reason: lease_expired|taker_gone|released} — a task back to pending",
        }))
    }

    fn readme(&self) -> Option<&'static str> {
        Some(README)
    }

    async fn handle(
        &self,
        agent_id: &AgentId,
        payload: &Value,
        kernel: &Arc<Kernel>,
    ) -> Result<Reply, BundleError> {
        let verb = payload.get("type").and_then(Value::as_str).unwrap_or("");
        let out = match verb {
            "boot" => return Ok(None),
            "reflect" => self.reflect(agent_id, kernel).await,
            "add" | "list" | "get" | "take" | "renew" | "done" | "release" | "status" => {
                self.mutate(verb, agent_id, payload, kernel).await
            }
            _ => {
                let t = payload.get("type").cloned().unwrap_or(Value::Null);
                let mut r = refusal(
                    UNHANDLED,
                    format!("shift_queue: unknown type {}", py_repr_value(&t)),
                );
                with(&mut r, "verb", t);
                r
            }
        };
        Ok(Some(out))
    }

    async fn on_delete(
        &self,
        agent_id: &AgentId,
        _kernel: &Arc<Kernel>,
    ) -> Result<(), BundleError> {
        self.docs.lock().await.remove(&agent_id.0);
        Ok(())
    }
}

impl QueueBundle {
    async fn reflect(&self, agent_id: &AgentId, kernel: &Arc<Kernel>) -> Value {
        let mut cache = self.docs.lock().await;
        let loaded = match cache.get(&agent_id.0) {
            Some(d) => Ok(d.clone()),
            None => load_doc(agent_id, kernel, "reflect").await,
        };
        let mut out = json!({
            "id": agent_id.0,
            "sentence": "Task queue — tasks as data; take hands one pending task to one taker under a lease.",
            "file": BLOB,
        });
        match loaded {
            Ok(doc) => {
                out["total"] = json!(doc.tasks.len());
                out["counts"] = counts(&doc);
                cache.insert(agent_id.0.clone(), doc);
            }
            Err(e) => {
                out["total"] = Value::Null;
                out["counts"] = Value::Null;
                out["load_error"] = e.get("error").cloned().unwrap_or(Value::Null);
            }
        }
        out
    }

    async fn mutate(
        &self,
        verb: &str,
        agent_id: &AgentId,
        payload: &Value,
        kernel: &Arc<Kernel>,
    ) -> Value {
        let mut cache = self.docs.lock().await;
        let base = match cache.get(&agent_id.0) {
            Some(d) => d.clone(),
            None => match load_doc(agent_id, kernel, verb).await {
                Ok(d) => d,
                Err(e) => return e,
            },
        };
        let now = now_secs();
        let live = |t: &str| kernel.agents.contains_key(&AgentId::from(t));
        let mut next = base;
        let mut events = sweep(&mut next, now, &agent_id.0, &live);
        let (reply, mutated) = match apply(
            verb,
            &mut next,
            payload,
            &agent_id.0,
            now,
            &live,
            &mut events,
        ) {
            Ok(x) => x,
            Err(e) => (e, false),
        };
        if mutated || !events.is_empty() {
            if let Err(e) = save_doc(agent_id, kernel, verb, &next).await {
                return e;
            }
        }
        cache.insert(agent_id.0.clone(), next);
        drop(cache);
        for ev in events {
            kernel.emit(agent_id, ev).await;
        }
        reply
    }
}

#[cfg(test)]
mod tests;
