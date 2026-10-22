//! Behaviour tests on a mock GitHub (wiremock on 127.0.0.1). No network.

use std::sync::{Arc, Once, OnceLock};

use base64::Engine;
use fantastic_kernel::bootstrap::{self, BootstrapOptions};

use fantastic_kernel::test_support::register_root_loader;
use fantastic_kernel::{AgentId, BundleRegistry, Item, Kernel};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use rsa::pkcs1::EncodeRsaPrivateKey;
use rsa::traits::PublicKeyParts;
use serde_json::{json, Value};
use tempfile::TempDir;
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::app::Claims;
use crate::{GithubConnectorBundle, HANDLER_MODULE};

pub(crate) const APP_ID: &str = "12345";
pub(crate) const STORE: &str = "creds";
const REF: &str = "github/app";

static ISOLATE: Once = Once::new();

pub(crate) fn isolate() {
    ISOLATE.call_once(|| {
        let p = std::env::temp_dir().join("shift-github-tests.gitconfig");
        std::fs::write(&p, "").unwrap();
        std::env::set_var("GIT_CONFIG_GLOBAL", &p);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    });
}

fn b64url(bytes: Vec<u8>) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The test RSA key, generated once per test binary and never written to disk.
/// Returns (PEM, modulus, exponent).
pub(crate) fn test_key() -> &'static (String, String, String) {
    static KEY: OnceLock<(String, String, String)> = OnceLock::new();
    KEY.get_or_init(|| {
        let k = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
        (
            k.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
                .unwrap()
                .to_string(),
            b64url(k.n().to_bytes_be()),
            b64url(k.e().to_bytes_be()),
        )
    })
}

/// Decode and verify a JWT signed for `APP_ID`.
pub(crate) fn decode(jwt: &str) -> Claims {
    let (_, n, e) = test_key();
    let mut v = Validation::new(Algorithm::RS256);
    v.set_issuer(&[APP_ID]);
    jsonwebtoken::decode::<Claims>(jwt, &DecodingKey::from_rsa_components(n, e).unwrap(), &v)
        .unwrap()
        .claims
}

/// The installation token the mock mints. Built from pieces so no
/// token-shaped literal sits in the source.
pub(crate) fn token() -> String {
    format!("{}{}{}", "gh", "s_", "T0k".repeat(12))
}

/// Mount the installation lookup and the token mint for acme/widgets.
pub(crate) async fn mock_app(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/installation"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 42})))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/app/installations/42/access_tokens"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "token": token(),
            "expires_at": "2099-01-01T00:00:00Z"
        })))
        .mount(server)
        .await;
}

#[allow(dead_code)] // `tmp` keeps the store directory alive for the test
pub(crate) struct Fx {
    pub k: Arc<Kernel>,
    pub tmp: TempDir,
    pub gh: MockServer,
}

impl Fx {
    /// Send one payload to the github agent and return its reply.
    pub(crate) async fn call(&self, payload: Value) -> Value {
        self.k
            .send(vec![Item::new(AgentId::from("github"), payload)])
            .await
            .remove(0)
            .result
    }
}

/// Requests received by the mock GitHub whose path matches.
pub(crate) async fn requests_to(gh: &MockServer, p: &str) -> Vec<wiremock::Request> {
    gh.received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path() == p)
        .collect()
}

/// A kernel with the github agent and its app credential in the store.
pub(crate) async fn fx(extra: Value) -> Fx {
    isolate();
    let tmp = TempDir::new().unwrap();
    let gh = MockServer::start().await;
    let mut reg = BundleRegistry::new();
    register_root_loader(&mut reg);
    reg.register(
        "ram_store.tools",
        fantastic_ram_store::RamStoreBundle::default(),
    );
    reg.register(
        fantastic_credential_store::HANDLER_MODULE,
        fantastic_credential_store::test_support::bundle(),
    );
    reg.register(HANDLER_MODULE, GithubConnectorBundle);
    let booted = bootstrap::bootstrap(reg, BootstrapOptions::in_memory()).unwrap();
    let k = booted.kernel.clone();
    k.agents
        .get(&AgentId::from(bootstrap::DEFAULT_ROOT_ID))
        .unwrap()
        .meta
        .write()
        .unwrap()
        .insert("guid".to_string(), json!("g1"));

    fantastic_credential_store::test_support::ensure_store(&k, tmp.path(), STORE).await;
    k.send(vec![Item::new(
        "kernel",
        json!({"type": "update_agent", "id": STORE,
               "refs": {REF: {"host": gh.uri()}},
               "callable_by": {"$self": "*", "github": "*"}}),
    )])
    .await;
    let set = k
        .send(vec![Item::new(
            STORE,
            json!({"type": "set", "ref": REF, "fields": {
                "app_id": APP_ID,
                "private_key": test_key().0.clone(),
            }}),
        )])
        .await
        .remove(0);
    assert_eq!(set.result["stored"], json!(true), "set: {}", set.result);

    let mut payload = json!({
        "type": "create_agent",
        "parent_id": bootstrap::DEFAULT_ROOT_ID,
        "id": "github",
        "handler_module": HANDLER_MODULE,
        "app": {"credentials": {"store": STORE, "ref": REF}},
        "repos": ["acme/widgets"],
        "api_url": gh.uri(),
        "git": "git",
        "directive": "Open draft PRs only.",
        "can_call": {STORE: "*"},
    });
    if let Value::Object(extra) = extra {
        for (key, value) in extra {
            if value.is_null() {
                payload.as_object_mut().unwrap().remove(&key);
            } else {
                payload[key] = value;
            }
        }
    }
    let reply = k
        .send(vec![Item::new("kernel", payload)])
        .await
        .remove(0)
        .result;
    assert_eq!(reply["id"], "github", "create_agent: {reply}");
    Fx { k, tmp, gh }
}

#[test]
fn the_jwt_is_rs256_with_the_app_id_and_ten_minutes_at_most() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let jwt = crate::app::sign_jwt(APP_ID, &test_key().0, now).unwrap();
    assert_eq!(
        jsonwebtoken::decode_header(&jwt).unwrap().alg,
        Algorithm::RS256
    );
    let c = decode(&jwt);
    assert_eq!(c.iss, APP_ID);
    assert!(c.exp - c.iat <= 600);
    assert!(c.exp - now <= 600);
    assert!(c.iat <= now);
    let err = crate::app::sign_jwt(APP_ID, "not a key", now).unwrap_err();
    assert!(!err.contains("not a key"));
}

#[tokio::test(flavor = "multi_thread")]
async fn whoami_reads_the_app_with_a_jwt() {
    let f = fx(json!({})).await;
    Mock::given(method("GET"))
        .and(path("/app"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": 12345, "slug": "my-app", "name": "My App"})),
        )
        .mount(&f.gh)
        .await;
    let r = f.call(json!({"type": "whoami"})).await;
    assert_eq!(r["app"]["slug"], "my-app", "{r}");
    let reqs = f.gh.received_requests().await.unwrap();
    let auth = reqs[0]
        .headers
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap();
    let jwt = auth.strip_prefix("Bearer ").unwrap();
    assert_eq!(decode(jwt).iss, APP_ID);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_is_minted_per_call_for_the_repo() {
    let f = fx(json!({})).await;
    mock_app(&f.gh).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/issues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&f.gh)
        .await;
    for _ in 0..2 {
        let r = f
            .call(json!({"type": "issues", "repo": "acme/widgets"}))
            .await;
        assert!(r.get("error").is_none(), "{r}");
    }
    assert_eq!(
        requests_to(&f.gh, "/repos/acme/widgets/installation")
            .await
            .len(),
        2
    );
    let mints = requests_to(&f.gh, "/app/installations/42/access_tokens").await;
    assert_eq!(mints.len(), 2);
    let body: Value = serde_json::from_slice(&mints[0].body).unwrap();
    assert_eq!(body, json!({"repositories": ["widgets"]}));
}

#[tokio::test(flavor = "multi_thread")]
async fn issues_lists_numbers_titles_labels_without_prs() {
    let f = fx(json!({})).await;
    mock_app(&f.gh).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/issues"))
        .and(query_param("labels", "shift"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"number": 1, "title": "a", "state": "open", "labels": [{"name": "shift"}]},
            {"number": 2, "title": "pr", "state": "open", "labels": [], "pull_request": {}}
        ])))
        .mount(&f.gh)
        .await;
    let r = f
        .call(json!({"type": "issues", "repo": "acme/widgets", "label": "shift"}))
        .await;
    assert_eq!(
        r["issues"],
        json!([{"number": 1, "title": "a", "state": "open", "labels": ["shift"]}]),
        "{r}"
    );
    let bad = f
        .call(json!({"type": "issues", "repo": "acme/widgets", "state": "draft"}))
        .await;
    assert_eq!(
        bad["error"],
        "shift_github issues: `state` must be open, closed or all"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pr_create_is_a_draft_on_the_default_branch() {
    let f = fx(json!({})).await;
    mock_app(&f.gh).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"default_branch": "trunk"})))
        .mount(&f.gh)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/acme/widgets/pulls"))
        .and(body_json(json!({
            "title": "T", "head": "work/x", "base": "trunk", "body": "B", "draft": true
        })))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({"number": 9, "html_url": "https://x/pull/9", "draft": true})),
        )
        .mount(&f.gh)
        .await;
    let r = f
        .call(
            json!({"type": "pr_create", "repo": "acme/widgets", "branch": "work/x",
                     "title": "T", "body": "B"}),
        )
        .await;
    assert_eq!(r["number"], 9, "{r}");
    assert_eq!(r["draft"], true);
    assert_eq!(r["base"], "trunk");

    let before = f.gh.received_requests().await.unwrap().len();
    let not_draft = f
        .call(
            json!({"type": "pr_create", "repo": "acme/widgets", "branch": "work/x",
                     "title": "T", "draft": false}),
        )
        .await;
    assert!(not_draft["error"].as_str().unwrap().contains("drafts only"));
    let on_main = f
        .call(
            json!({"type": "pr_create", "repo": "acme/widgets", "branch": "main",
                     "title": "T"}),
        )
        .await;
    assert!(on_main["error"].as_str().unwrap().contains("base branch"));
    assert_eq!(f.gh.received_requests().await.unwrap().len(), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn repos_off_the_allowlist_are_refused_before_any_request() {
    let f = fx(json!({})).await;
    let r = f
        .call(json!({"type": "issues", "repo": "evil/other"}))
        .await;
    assert!(
        r["error"].as_str().unwrap().contains("(acme/widgets)"),
        "{r}"
    );
    assert_eq!(r["reason"], "bad_arg");
    assert!(f.gh.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_api_error_is_named_without_secrets() {
    let f = fx(json!({})).await;
    mock_app(&f.gh).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/issues/99"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"message": "Not Found"})))
        .mount(&f.gh)
        .await;
    let r = f
        .call(json!({"type": "issue_view", "repo": "acme/widgets", "number": 99}))
        .await;
    assert_eq!(
        r["error"],
        "shift_github issue_view: GitHub answered 404 to GET /repos/acme/widgets/issues/99: Not Found"
    );
    assert_eq!(r["status"], 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_record_is_validated() {
    let f = fx(json!({})).await;
    let no_app =
        f.k.send(vec![Item::new(
            "kernel",
            json!({"type": "create_agent", "parent_id": bootstrap::DEFAULT_ROOT_ID,
                   "id": "gh2", "handler_module": HANDLER_MODULE,
                   "repos": ["acme/widgets"], "directive": "x"}),
        )])
        .await
        .remove(0)
        .result;
    assert!(
        no_app["error"].as_str().unwrap().contains("app"),
        "{no_app}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn directive_reflect_and_unknown_verbs() {
    let f = fx(json!({})).await;
    assert_eq!(
        f.call(json!({"type": "directive"})).await,
        json!({"directive": "Open draft PRs only."})
    );
    let r = f.call(json!({"type": "reflect"})).await;
    assert_eq!(r["sentence"], crate::SENTENCE);
    assert_eq!(r["repos"], json!(["acme/widgets"]));
    let n = f
        .call(json!({"type": "issue_view", "repo": "acme/widgets", "number": "3"}))
        .await;
    assert!(n["error"]
        .as_str()
        .unwrap()
        .contains("`number` must be a positive int"));
    let m = f.call(json!({"type": "merge"})).await;
    assert_eq!(m["reason"], "unhandled");
}
