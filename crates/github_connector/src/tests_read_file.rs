//! `read_file`: one file through the Contents API, on the mock GitHub.

use base64::Engine;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

use crate::tests::{fx, mock_app, token};

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
}

#[tokio::test]
async fn read_file_returns_decoded_text() {
    let f = fx(json!({})).await;
    mock_app(&f.gh).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/contents/.github/team.yaml"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "encoding": "base64",
            "content": b64("task_labels: [do-it]\n"),
        })))
        .mount(&f.gh)
        .await;
    let r = f
        .call(json!({"type": "read_file", "repo": "acme/widgets", "path": ".github/team.yaml"}))
        .await;
    assert_eq!(r["text"], "task_labels: [do-it]\n", "{r}");
    assert_eq!(r["path"], ".github/team.yaml");
    assert!(!r.to_string().contains(&token()), "token leaked: {r}");
}

#[tokio::test]
async fn read_file_passes_ref_as_query() {
    let f = fx(json!({})).await;
    mock_app(&f.gh).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/contents/team.yaml"))
        .and(query_param("ref", "dev"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "encoding": "base64",
            "content": b64("x\n"),
        })))
        .mount(&f.gh)
        .await;
    let r = f
        .call(
            json!({"type": "read_file", "repo": "acme/widgets", "path": "team.yaml", "ref": "dev"}),
        )
        .await;
    assert_eq!(r["text"], "x\n", "{r}");
    assert_eq!(r["ref"], "dev");
}

#[tokio::test]
async fn read_file_missing_is_named_not_found() {
    let f = fx(json!({})).await;
    mock_app(&f.gh).await;
    Mock::given(method("GET"))
        .and(path("/repos/acme/widgets/contents/.github/team.yaml"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"message": "Not Found"})))
        .mount(&f.gh)
        .await;
    let r = f
        .call(json!({"type": "read_file", "repo": "acme/widgets", "path": ".github/team.yaml"}))
        .await;
    assert_eq!(r["not_found"], true, "{r}");
    assert!(r.get("text").is_none(), "{r}");
}

#[tokio::test]
async fn read_file_refuses_parent_dir() {
    let f = fx(json!({})).await;
    let r = f
        .call(json!({"type": "read_file", "repo": "acme/widgets", "path": "../secrets"}))
        .await;
    assert!(r.get("error").is_some(), "{r}");
    assert!(!r.to_string().contains(&token()), "{r}");
}

#[tokio::test]
async fn read_file_refuses_repo_outside_allowlist() {
    let f = fx(json!({})).await;
    let r = f
        .call(json!({"type": "read_file", "repo": "evil/repo", "path": "team.yaml"}))
        .await;
    assert!(r.get("error").is_some(), "{r}");
}
