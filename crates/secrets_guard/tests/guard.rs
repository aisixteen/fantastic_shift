use async_trait::async_trait;
use base64::Engine as _;
use fantastic_kernel::{Agent, AgentId, Bundle, Item, Kernel, Reply};

/// The kernel's bundle error type (same alias as `fantastic_kernel::bundle`).
type BundleError = Box<dyn std::error::Error + Send + Sync>;
use secrets_guard::{percent_encode, Leak, Probe};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

const SECRET: &str = "s3cret-Value/42+x";

/// A dummy agent that leaks its secret in every way the guard must catch.
struct Leaky {
    secret: String,
    shelf: Arc<Mutex<Vec<Value>>>,
    logs: PathBuf,
}

#[async_trait]
impl Bundle for Leaky {
    fn name(&self) -> &str {
        "leaky"
    }

    async fn handle(
        &self,
        agent_id: &AgentId,
        payload: &Value,
        kernel: &Arc<Kernel>,
    ) -> Result<Reply, BundleError> {
        let s = &self.secret;
        let reply = match payload["type"].as_str().unwrap_or("") {
            "clean" => json!({"ok": true}),
            "emit_clean" => {
                let _ = kernel.emit(agent_id, json!({"type": "note", "n": 1})).await;
                json!({"ok": true})
            }
            "reply" => json!({"token": s}),
            "reply_b64" => json!({
                "auth": format!(
                    "Basic {}",
                    base64::engine::general_purpose::STANDARD
                        .encode(format!("x-access-token:{}", s))
                )
            }),
            "url" => json!({
                "url": format!(
                    "https://example.test/cb?token={}",
                    percent_encode(s, true)
                )
            }),
            "error" => json!({
                "error": format!("login failed for token {}", s),
                "reason": "transport_error"
            }),
            "emit" => {
                let _ = kernel.emit(agent_id, json!({"type": "note", "t": s})).await;
                json!({"ok": true})
            }
            "store" => {
                self.shelf.lock().expect("shelf").push(json!({"row": s}));
                json!({"ok": true})
            }
            "file" => {
                std::fs::create_dir_all(&self.logs)?;
                std::fs::write(self.logs.join("agent.log"), format!("token={}\n", s))?;
                json!({"ok": true})
            }
            "record" => {
                if let Some(a) = kernel.agents.get(agent_id) {
                    a.meta
                        .write()
                        .expect("meta")
                        .insert("note".into(), json!(s));
                }
                json!({"ok": true})
            }
            _ => json!({"error": "unknown verb", "reason": "unhandled"}),
        };
        Ok(Some(reply))
    }
}

/// The store the dummy agent writes its rows to.
struct Shelf {
    rows: Arc<Mutex<Vec<Value>>>,
}

#[async_trait]
impl Bundle for Shelf {
    fn name(&self) -> &str {
        "shelf"
    }

    async fn handle(
        &self,
        _agent_id: &AgentId,
        payload: &Value,
        _kernel: &Arc<Kernel>,
    ) -> Result<Reply, BundleError> {
        let reply = match payload["type"].as_str().unwrap_or("") {
            "dump" => json!({"tables": {"rows": self.rows.lock().expect("rows").clone()}}),
            _ => json!({"ok": true}),
        };
        Ok(Some(reply))
    }
}

/// A kernel with the dummy bundles, a root, and the `leaky` and `shelf` agents.
async fn kernel_with(tmp: &TempDir) -> Arc<Kernel> {
    let shelf = Arc::new(Mutex::new(Vec::new()));
    let mut k = Kernel::new();
    k.bundles.register(
        "leaky.tools",
        Leaky {
            secret: SECRET.into(),
            shelf: shelf.clone(),
            logs: tmp.path().join("logs"),
        },
    );
    k.bundles.register("shelf.tools", Shelf { rows: shelf });
    let k = Arc::new(k);
    let root = Agent::new(
        AgentId::from("kernel_state"),
        None,
        None,
        Map::new(),
        tmp.path().join(".fantastic"),
        false,
    );
    k.register(Arc::clone(&root));
    k.set_root(Arc::clone(&root));
    for (id, module) in [("leaky", "leaky.tools"), ("shelf", "shelf.tools")] {
        let out = k
            .send(vec![Item::new(
                AgentId::from("kernel_state"),
                json!({
                    "type": "create_agent",
                    "parent_id": "kernel",
                    "handler_module": module,
                    "id": id,
                }),
            )])
            .await
            .remove(0);
        assert!(
            out.result.get("error").is_none(),
            "create {id}: {}",
            out.result
        );
    }
    k
}

fn probe(k: &Arc<Kernel>, tmp: &TempDir, verbs: &[&str]) -> Probe {
    Probe::new(
        k,
        "leaky",
        verbs.iter().map(|v| json!({"type": v})).collect(),
        &[SECRET],
    )
    .store("shelf")
    .dir(tmp.path().join("logs"))
}

fn has(leaks: &[Leak], channel: &str, form: &str) -> bool {
    leaks.iter().any(|l| l.channel == channel && l.form == form)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_clean_agent_passes() {
    let tmp = TempDir::new().expect("tmp");
    let k = kernel_with(&tmp).await;
    let p = probe(&k, &tmp, &["clean", "emit_clean"]);
    assert!(p.leaks().await.is_empty());
    p.assert_clean().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_secret_in_a_reply_is_caught() {
    let tmp = TempDir::new().expect("tmp");
    let k = kernel_with(&tmp).await;
    let l = probe(&k, &tmp, &["reply"]).leaks().await;
    assert!(has(&l, "reply", "plain"), "{l:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_base64_secret_in_a_reply_is_caught() {
    let tmp = TempDir::new().expect("tmp");
    let k = kernel_with(&tmp).await;
    let l = probe(&k, &tmp, &["reply_b64"]).leaks().await;
    assert!(has(&l, "reply", "base64"), "{l:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_url_encoded_secret_is_caught() {
    let tmp = TempDir::new().expect("tmp");
    let k = kernel_with(&tmp).await;
    let l = probe(&k, &tmp, &["url"]).leaks().await;
    assert!(has(&l, "reply", "url-encoded"), "{l:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_secret_in_an_error_is_caught() {
    let tmp = TempDir::new().expect("tmp");
    let k = kernel_with(&tmp).await;
    let l = probe(&k, &tmp, &["error"]).leaks().await;
    assert!(has(&l, "error", "plain"), "{l:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_secret_in_an_emitted_event_is_caught() {
    let tmp = TempDir::new().expect("tmp");
    let k = kernel_with(&tmp).await;
    let l = probe(&k, &tmp, &["emit"]).leaks().await;
    assert!(has(&l, "event", "plain"), "{l:?}");
    assert!(has(&l, "state", "plain"), "{l:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_secret_in_a_store_row_is_caught() {
    let tmp = TempDir::new().expect("tmp");
    let k = kernel_with(&tmp).await;
    let l = probe(&k, &tmp, &["store"]).leaks().await;
    assert!(has(&l, "store", "plain"), "{l:?}");
    assert!(
        !l.iter()
            .any(|x| x.channel == "reply" || x.channel == "state"),
        "{l:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_secret_in_a_file_is_caught() {
    let tmp = TempDir::new().expect("tmp");
    let k = kernel_with(&tmp).await;
    let l = probe(&k, &tmp, &["file"]).leaks().await;
    assert!(has(&l, "file", "plain"), "{l:?}");
    assert!(
        l.iter()
            .any(|x| x.channel == "file" && x.at.ends_with("agent.log")),
        "{l:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_secret_in_the_agent_record_is_caught() {
    let tmp = TempDir::new().expect("tmp");
    let k = kernel_with(&tmp).await;
    let l = probe(&k, &tmp, &["record"]).leaks().await;
    assert!(has(&l, "records", "plain"), "{l:?}");
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "secret #0 as plain in reply (payload #0)")]
async fn assert_clean_panics_naming_the_leak_not_the_secret() {
    let tmp = TempDir::new().expect("tmp");
    let k = kernel_with(&tmp).await;
    probe(&k, &tmp, &["reply"]).assert_clean().await;
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "payload #0 carries secret #0")]
async fn a_payload_carrying_the_secret_is_refused() {
    let tmp = TempDir::new().expect("tmp");
    let k = kernel_with(&tmp).await;
    Probe::new(
        &k,
        "leaky",
        vec![json!({"type": "clean", "x": SECRET})],
        &[SECRET],
    )
    .leaks()
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "shorter than 8")]
async fn a_short_secret_is_refused() {
    let tmp = TempDir::new().expect("tmp");
    let k = kernel_with(&tmp).await;
    Probe::new(&k, "leaky", vec![json!({"type": "clean"})], &["abc"])
        .leaks()
        .await;
}
