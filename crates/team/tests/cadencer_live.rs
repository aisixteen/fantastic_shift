//! Live test (#112): the cadencer delivers through a real kernel. Boots the
//! team registry the way the binary does, creates a board with a group, a
//! cadencer and a driver, and checks the timer beats and the board is read.

use std::sync::Arc;

use fantastic_kernel::bootstrap::{self, DEFAULT_ROOT_ID};
use fantastic_kernel::{AgentId, Item, Kernel};
use serde_json::{json, Value};

async fn call(k: &Arc<Kernel>, to: &str, payload: Value) -> Value {
    k.send(vec![Item::new(AgentId::from(to), payload)])
        .await
        .into_iter()
        .next()
        .map(|o| o.result)
        .unwrap_or(Value::Null)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_board_and_cadencer_deliver_through_a_real_kernel() {
    let dir = tempfile::tempdir().unwrap();
    let kernel = fantastic_team::start(dir.path()).await.unwrap();

    call(
        &kernel,
        DEFAULT_ROOT_ID,
        json!({
            "type": "create_agent",
            "parent_id": DEFAULT_ROOT_ID,
            "id": "board",
            "handler_module": "team_board.tools",
            "github": "gh_absent",
            "store": "store",
            "team_groups": {"rtx": {"driver": "rtx-driver", "members": ["rtx-w1", "rtx-w2"]}}
        }),
    )
    .await;
    call(
        &kernel,
        DEFAULT_ROOT_ID,
        json!({
            "type": "create_agent",
            "parent_id": DEFAULT_ROOT_ID,
            "id": "cad",
            "handler_module": "team_cadencer.tools"
        }),
    )
    .await;
    call(
        &kernel,
        "cad",
        json!({"type": "configure", "board": "board", "parent": "mcp"}),
    )
    .await;

    // Bug 1: the timer must run right after create + configure, with no set.
    let st = call(&kernel, "cad", json!({"type": "status"})).await;
    assert!(
        !st["next_beat_s"].is_null(),
        "timer not started after create: {st}"
    );

    // Bug 2: a set from the operator (no origin) must be accepted.
    let set = call(&kernel, "cad", json!({"type": "set", "cadence": 1})).await;
    assert!(set.get("error").is_none(), "set refused: {set}");

    // Bug 3: the timer beats, reads the board's rows and delivers a digest.
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    let st = call(&kernel, "cad", json!({"type": "status"})).await;
    assert!(st["ticks"].as_u64().unwrap_or(0) >= 1, "no beat: {st}");
    assert!(
        st["digests_sent"].as_u64().unwrap_or(0) >= 1,
        "board rows not read: {st}"
    );

    kernel.shutdown().await;
    bootstrap::shutdown(dir.path()).unwrap();
}
