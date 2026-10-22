use super::*;
use fantastic_kernel::{Agent, Item, Kernel};
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

async fn make(k: &Arc<Kernel>, payload: Value) -> Value {
    k.send(vec![Item::new(AgentId::from("kernel_state"), payload)])
        .await
        .remove(0)
        .result
}

async fn call(k: &Arc<Kernel>, payload: Value) -> Value {
    k.send(vec![Item::new(AgentId::from("q"), payload)])
        .await
        .remove(0)
        .result
}

async fn mk_kernel(tmp: &TempDir, with_store: bool) -> Arc<Kernel> {
    let mut k = Kernel::new();
    k.bundles = fantastic_host::default_bundles();
    k.bundles.register(HANDLER_MODULE, QueueBundle::default());
    let root = Agent::new(
        AgentId::from("kernel_state"),
        Some("kernel_state.tools".into()),
        None,
        Map::new(),
        tmp.path().join(".fantastic"),
        false,
    );
    k.register(Arc::clone(&root));
    k.set_root(root);
    let k = Arc::new(k);
    if with_store {
        let r = make(
            &k,
            json!({"type": "create_agent", "parent_id": "kernel", "id": "store",
                   "handler_module": "file_bridge.tools", "allow": ["."],
                   "root": tmp.path().join(".fantastic").to_string_lossy(),
                   "ingress_rule": "allow_all_ingress"}),
        )
        .await;
        assert!(r.get("error").is_none(), "{r}");
    }
    let r = make(
        &k,
        json!({"type": "create_agent", "parent_id": "kernel", "id": "q", "handler_module": HANDLER_MODULE}),
    )
    .await;
    assert!(r.get("error").is_none(), "{r}");
    for w in ["w1", "w2"] {
        k.register(Agent::new(
            AgentId::from(w),
            None,
            Some(AgentId::from("kernel_state")),
            Map::new(),
            PathBuf::new(),
            true,
        ));
    }
    k
}

async fn add_task(k: &Arc<Kernel>, title: &str) -> Value {
    call(
        k,
        json!({"type": "add", "title": title, "instructions": "do it"}),
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn add_and_list() {
    let tmp = TempDir::new().unwrap();
    let k = mk_kernel(&tmp, true).await;
    assert_eq!(add_task(&k, "one").await["task"]["id"], "t1");
    let b = call(
        &k,
        json!({"type": "add", "title": "two", "id": "x7", "instructions_file": "/tmp/a.md"}),
    )
    .await;
    assert_eq!(b["task"]["id"], "x7");
    let c = call(
        &k,
        json!({"type": "add", "title": "three", "instructions": "x", "data": {"k": 1}}),
    )
    .await;
    assert_eq!(c["task"]["id"], "t2");
    assert_eq!(call(&k, json!({"type": "list"})).await["count"], 3);
    assert_eq!(
        call(&k, json!({"type": "list", "status": "done"})).await["count"],
        0
    );
    assert_eq!(
        call(&k, json!({"type": "get", "id": "x7"})).await["task"]["title"],
        "two"
    );
    let s = call(&k, json!({"type": "status"})).await;
    assert_eq!(s["total"], 3);
    assert_eq!(s["counts"]["pending"], 3);
    k.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn take_gives_the_oldest_pending_task_under_a_lease() {
    let tmp = TempDir::new().unwrap();
    let k = mk_kernel(&tmp, true).await;
    add_task(&k, "one").await;
    add_task(&k, "two").await;
    let t = call(&k, json!({"type": "take", "taker": "w1"})).await;
    assert_eq!(t["task"]["id"], "t1");
    assert_eq!(t["task"]["status"], "taken");
    assert_eq!(t["task"]["taker"], "w1");
    assert_eq!(t["task"]["attempts"], 1);
    let lease = t["task"]["lease_until"].as_f64().unwrap();
    assert!((lease - (now_secs() + 600.0)).abs() < 1.0);
    let t2 = call(&k, json!({"type": "take", "taker": "w2", "lease_s": 30})).await;
    assert_eq!(t2["task"]["id"], "t2");
    assert!(call(&k, json!({"type": "take", "taker": "w1"})).await["task"].is_null());
    k.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn take_is_atomic_under_concurrency() {
    let tmp = TempDir::new().unwrap();
    let k = mk_kernel(&tmp, true).await;
    for i in 0..20 {
        add_task(&k, &format!("task {i}")).await;
    }
    let mut handles = Vec::new();
    for w in ["w1", "w2"] {
        let k = k.clone();
        handles.push(tokio::spawn(async move {
            let mut got = Vec::new();
            loop {
                let r = call(&k, json!({"type": "take", "taker": w})).await;
                match r["task"]["id"].as_str() {
                    Some(id) => got.push(id.to_string()),
                    None => break,
                }
            }
            got
        }));
    }
    let a = handles.remove(0).await.unwrap();
    let b = handles.remove(0).await.unwrap();
    assert_eq!(a.len() + b.len(), 20);
    assert!(a.iter().all(|x| !b.contains(x)));
    let mut all: Vec<String> = a.into_iter().chain(b).collect();
    all.sort();
    let mut want: Vec<String> = (1..=20).map(|i| format!("t{i}")).collect();
    want.sort();
    assert_eq!(all, want);
    k.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_simultaneous_takes_of_one_task_give_it_to_one() {
    let tmp = TempDir::new().unwrap();
    let k = mk_kernel(&tmp, true).await;
    add_task(&k, "only").await;
    let (x, y) = tokio::join!(
        call(&k, json!({"type": "take", "taker": "w1"})),
        call(&k, json!({"type": "take", "taker": "w2"})),
    );
    let got = [&x, &y].iter().filter(|r| !r["task"].is_null()).count();
    assert_eq!(got, 1);
    k.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_lease_puts_the_task_back() {
    let tmp = TempDir::new().unwrap();
    let k = mk_kernel(&tmp, true).await;
    let mut inbox = k.ensure_inbox(&AgentId::from("q"));
    add_task(&k, "one").await;
    call(&k, json!({"type": "take", "taker": "w1", "lease_s": 0.2})).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let t = call(&k, json!({"type": "get", "id": "t1"})).await;
    assert_eq!(t["task"]["status"], "pending");
    assert!(t["task"]["taker"].is_null());
    assert_eq!(t["task"]["requeues"], 1);
    let mut found = false;
    while let Some(item) = inbox.try_recv() {
        if item.payload["type"] == "task_requeued"
            && item.payload["reason"] == "lease_expired"
            && item.payload["taker"] == "w1"
        {
            found = true;
        }
    }
    assert!(found);
    let again = call(&k, json!({"type": "take", "taker": "w2"})).await;
    assert_eq!(again["task"]["id"], "t1");
    assert_eq!(again["task"]["attempts"], 2);
    k.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_gone_taker_puts_the_task_back() {
    let tmp = TempDir::new().unwrap();
    let k = mk_kernel(&tmp, true).await;
    add_task(&k, "one").await;
    call(&k, json!({"type": "take", "taker": "w1"})).await;
    k.unregister(&AgentId::from("w1"));
    let s = call(&k, json!({"type": "status"})).await;
    assert_eq!(s["counts"]["pending"], 1);
    assert_eq!(s["counts"]["taken"], 0);
    let t = call(&k, json!({"type": "get", "id": "t1"})).await;
    assert!(t["task"]["taker"].is_null());
    assert_eq!(t["task"]["requeues"], 1);
    k.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_taker_may_renew_done_or_release() {
    let tmp = TempDir::new().unwrap();
    let k = mk_kernel(&tmp, true).await;
    add_task(&k, "one").await;
    call(&k, json!({"type": "take", "taker": "w1"})).await;
    let r = call(&k, json!({"type": "renew", "id": "t1", "taker": "w2"})).await;
    assert_eq!(r["error"], "renew: task t1 is taken by w1, not w2");
    assert_eq!(r["reason"], "bad_arg");
    let r = call(
        &k,
        json!({"type": "done", "id": "t1", "taker": "w2", "status": "done", "result": "x"}),
    )
    .await;
    assert_eq!(r["error"], "done: task t1 is taken by w1, not w2");
    let r = call(&k, json!({"type": "release", "id": "t1", "taker": "w2"})).await;
    assert_eq!(r["error"], "release: task t1 is taken by w1, not w2");
    let r = call(
        &k,
        json!({"type": "renew", "id": "t1", "taker": "w1", "lease_s": 120}),
    )
    .await;
    let lease = r["task"]["lease_until"].as_f64().unwrap();
    assert!((lease - (now_secs() + 120.0)).abs() < 1.0);
    let r = call(&k, json!({"type": "done", "id": "t1", "taker": "w1", "status": "failed", "result": {"why": "tests red"}})).await;
    assert_eq!(r["task"]["status"], "failed");
    assert_eq!(r["task"]["result"]["why"], "tests red");
    assert_eq!(r["task"]["taker"], "w1");
    assert!(!r["task"]["done_at"].is_null());
    let r = call(
        &k,
        json!({"type": "done", "id": "t1", "taker": "w1", "status": "done", "result": "x"}),
    )
    .await;
    assert_eq!(r["error"], "done: task t1 is failed, not taken");
    add_task(&k, "two").await;
    call(&k, json!({"type": "take", "taker": "w2"})).await;
    let r = call(&k, json!({"type": "release", "id": "t2", "taker": "w2"})).await;
    assert_eq!(r["task"]["status"], "pending");
    assert!(r["task"]["taker"].is_null());
    k.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn take_and_done_emit_events() {
    let tmp = TempDir::new().unwrap();
    let k = mk_kernel(&tmp, true).await;
    let mut inbox = k.ensure_inbox(&AgentId::from("q"));
    add_task(&k, "one").await;
    call(&k, json!({"type": "take", "taker": "w1"})).await;
    call(
        &k,
        json!({"type": "done", "id": "t1", "taker": "w1", "status": "done", "result": "ok"}),
    )
    .await;
    let mut taken = false;
    let mut done = false;
    while let Some(item) = inbox.try_recv() {
        let p = &item.payload;
        if p["type"] == "task_taken" && p["queue"] == "q" && p["id"] == "t1" && p["taker"] == "w1" {
            taken = p["lease_until"].is_number();
        }
        if p["type"] == "task_done"
            && p["id"] == "t1"
            && p["taker"] == "w1"
            && p["status"] == "done"
        {
            done = true;
        }
    }
    assert!(taken && done);
    k.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tasks_survive_a_restart() {
    let tmp = TempDir::new().unwrap();
    {
        let k = mk_kernel(&tmp, true).await;
        add_task(&k, "one").await;
        add_task(&k, "two").await;
        call(&k, json!({"type": "take", "taker": "w1"})).await;
        assert!(tmp.path().join(".fantastic/agents/q/tasks.json").exists());
        k.shutdown().await;
    }
    let k = mk_kernel(&tmp, true).await;
    let l = call(&k, json!({"type": "list"})).await;
    assert_eq!(l["count"], 2);
    assert_eq!(l["tasks"][0]["status"], "taken");
    assert_eq!(l["tasks"][0]["taker"], "w1");
    assert_eq!(l["tasks"][1]["status"], "pending");
    let r = add_task(&k, "three").await;
    assert_eq!(r["task"]["id"], "t3");
    k.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_corrupt_file_is_refused_not_overwritten() {
    let tmp = TempDir::new().unwrap();
    {
        let k = mk_kernel(&tmp, true).await;
        add_task(&k, "one").await;
        k.shutdown().await;
    }
    let path = tmp.path().join(".fantastic/agents/q/tasks.json");
    std::fs::write(&path, "not json").unwrap();
    let k = mk_kernel(&tmp, true).await;
    let r = call(&k, json!({"type": "list"})).await;
    assert!(r["error"]
        .as_str()
        .unwrap()
        .starts_with("list: tasks.json is not a queue document — "));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json");
    k.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_store_is_said() {
    let tmp = TempDir::new().unwrap();
    let k = mk_kernel(&tmp, false).await;
    let r = add_task(&k, "one").await;
    assert!(r["error"]
        .as_str()
        .unwrap()
        .starts_with("add: cannot load tasks.json — "));
    let rf = call(&k, json!({"type": "reflect"})).await;
    assert_eq!(rf["id"], "q");
    assert!(rf.get("load_error").is_some());
    k.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn refusals_have_exact_texts() {
    let tmp = TempDir::new().unwrap();
    let k = mk_kernel(&tmp, true).await;
    let e = |r: Value| r["error"].as_str().unwrap_or("").to_string();

    assert_eq!(
        e(call(&k, json!({"type": "add"})).await),
        "add: title required"
    );
    assert_eq!(
        e(call(&k, json!({"type": "add", "title": "a"})).await),
        "add: instructions required"
    );
    assert_eq!(
        e(call(
            &k,
            json!({"type": "add", "title": "a", "instructions": "x", "instructions_file": "/f"})
        )
        .await),
        "add: give instructions or instructions_file, not both"
    );
    assert_eq!(
        e(call(&k, json!({"type": "add", "title": "a", "instructions": 5})).await),
        "add: instructions must be a string"
    );
    assert_eq!(
        e(call(
            &k,
            json!({"type": "add", "title": "a", "instructions": "x", "data": [1]})
        )
        .await),
        "add: data must be an object"
    );
    call(
        &k,
        json!({"type": "add", "title": "a", "instructions": "x", "id": "d"}),
    )
    .await;
    assert_eq!(
        e(call(
            &k,
            json!({"type": "add", "title": "a", "instructions": "x", "id": "d"})
        )
        .await),
        "add: task d already exists"
    );
    assert_eq!(
        e(call(&k, json!({"type": "list", "status": "nope"})).await),
        "list: status must be one of pending, taken, done, failed"
    );
    assert_eq!(
        e(call(&k, json!({"type": "get", "id": "zz"})).await),
        "get: no task zz"
    );
    assert_eq!(
        e(call(&k, json!({"type": "take"})).await),
        "take: taker required"
    );
    assert_eq!(
        e(call(&k, json!({"type": "take", "taker": "ghost"})).await),
        "take: taker ghost is not an agent on this kernel"
    );
    for bad in [json!(0), json!(-1), json!(90000), json!("10")] {
        assert_eq!(
            e(call(&k, json!({"type": "take", "taker": "w1", "lease_s": bad})).await),
            "take: lease_s must be a number of seconds > 0 and <= 86400"
        );
    }
    call(&k, json!({"type": "take", "taker": "w1"})).await;
    call(&k, json!({"type": "release", "id": "d", "taker": "w1"})).await;
    assert_eq!(
        e(call(
            &k,
            json!({"type": "done", "id": "d", "taker": "w1", "status": "done", "result": "x"})
        )
        .await),
        "done: task d is pending, not taken"
    );
    add_task(&k, "held").await;
    call(&k, json!({"type": "take", "taker": "w1"})).await;
    assert_eq!(
        e(call(&k, json!({"type": "done", "id": "t2", "taker": "w1"})).await),
        "done: status required"
    );
    assert_eq!(
        e(call(
            &k,
            json!({"type": "done", "id": "t2", "taker": "w1", "status": "maybe"})
        )
        .await),
        "done: status must be done or failed"
    );
    assert_eq!(
        e(call(
            &k,
            json!({"type": "done", "id": "t2", "taker": "w1", "status": "done"})
        )
        .await),
        "done: result required"
    );
    assert_eq!(
        e(call(&k, json!({"type": "frobnicate"})).await),
        "shift_queue: unknown type 'frobnicate'"
    );
    k.shutdown().await;
}
