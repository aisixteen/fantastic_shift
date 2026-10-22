//! The key, the JWT and the installation token never leave the agent: not in a
//! reply, a row (except the credential store's own `get` reply), a state event or
//! the saved state.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

use crate::tests::{fx, mock_app, requests_to, test_key, token};

#[tokio::test(flavor = "multi_thread")]
async fn the_key_the_jwt_and_the_token_never_leave_the_agent() {
    let f = fx(json!({})).await;
    mock_app(&f.gh).await;
    Mock::given(method("GET"))
        .and(path("/app"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": 1, "slug": "s", "name": "n"})),
        )
        .mount(&f.gh)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/issues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&f.gh)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/issues/3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"number": 3, "title": "t", "state": "open", "body": "b", "labels": []}),
        ))
        .mount(&f.gh)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/issues/3/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&f.gh)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/issues/99"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"message": "Not Found"})))
        .mount(&f.gh)
        .await;

    let states: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let tap = states.clone();
    f.k.add_state_subscriber(Arc::new(move |ev, body| {
        let mut s = ev.to_string();
        if let Some(b) = body {
            s.push_str(&String::from_utf8_lossy(&b.bytes));
        }
        tap.lock().unwrap().push(s);
    }));

    let calls = vec![
        json!({"type": "reflect"}),
        json!({"type": "directive"}),
        json!({"type": "whoami"}),
        json!({"type": "issues", "repo": "acme/widgets"}),
        json!({"type": "issue_view", "repo": "acme/widgets", "number": 3}),
        json!({"type": "issue_view", "repo": "acme/widgets", "number": 99}),
        json!({"type": "issues", "repo": "evil/other"}),
    ];
    let mut replies: Vec<Value> = Vec::new();
    for c in calls {
        replies.push(f.call(c).await);
    }

    let (pem, _, _) = test_key();
    let mut secrets: Vec<String> = vec![token(), pem.clone()];
    secrets.extend(pem.lines().filter(|l| l.len() >= 40).map(str::to_string));
    // Every JWT the mock saw.
    let all = f.gh.received_requests().await.unwrap();
    for r in &all {
        if let Some(a) = r.headers.get("authorization").and_then(|h| h.to_str().ok()) {
            if let Some(j) = a.strip_prefix("Bearer ") {
                secrets.push(j.to_string());
            }
        }
    }
    assert!(!secrets.is_empty());
    assert!(requests_to(&f.gh, "/app").await.len() >= 1);

    for (i, s) in secrets.iter().enumerate() {
        for r in &replies {
            assert!(
                !serde_json::to_string(r).unwrap().contains(s.as_str()),
                "secret {i} in a reply"
            );
        }
    }

    // Rows: skip the credential store's own `get` reply, count the skips.
    let rows = fantastic_kernel::test_support::rows_about(
        &f.k,
        &fantastic_kernel::AgentId::from("github"),
    );
    let mut skipped = 0;
    for row in &rows {
        let text = serde_json::to_string(row).unwrap();
        if text.contains("app_id") && text.contains("private_key") {
            skipped += 1;
            continue;
        }
        for (i, s) in secrets.iter().enumerate() {
            assert!(!text.contains(s.as_str()), "secret {i} in a row");
        }
    }
    eprintln!("secrets test: skipped {skipped} credential-store get rows");

    for st in states.lock().unwrap().iter() {
        if st.contains("app_id") && st.contains("private_key") {
            continue;
        }
        for (i, s) in secrets.iter().enumerate() {
            assert!(!st.contains(s.as_str()), "secret {i} in a state event");
        }
    }

    let saved = f.k.save_json();
    let saved_text = serde_json::to_string(&saved).unwrap();
    for (i, s) in secrets.iter().enumerate() {
        assert!(
            !saved_text.contains(s.as_str()),
            "secret {i} in the saved state"
        );
    }
}
