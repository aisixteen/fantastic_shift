//! HTTP to the GitHub API, and the one redaction rule for this connector.

use std::time::Duration;

use reqwest::Method;
use serde_json::{json, Value};
use shift_git_connector::git::{redact as git_redact, Auth};

/// Replace every given secret in `s` with the shown marker.
pub(crate) fn redact(s: &str, secrets: &[&str]) -> String {
    let mut out = s.to_string();
    for secret in secrets {
        if !secret.is_empty() {
            out = git_redact(
                &out,
                Some(&Auth {
                    username: String::new(),
                    token: secret.to_string(),
                }),
            );
        }
    }
    out
}

/// A client bound to one API root and one timeout.
pub(crate) struct Api {
    http: reqwest::Client,
    base: String,
}

impl Api {
    /// Build the client from the record's config.
    pub(crate) fn new(cfg: &crate::Cfg) -> Api {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_s))
            .user_agent("fantastic-shift-github-connector")
            .build()
            .expect("http client");
        Api {
            http,
            base: cfg.api_url.clone(),
        }
    }

    /// One request. A 2xx answer is its JSON body (null when empty). Any other
    /// answer is `(status, {error, status})`, redacted. Headers are never echoed.
    pub(crate) async fn call(
        &self,
        verb: &str,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        bearer: &str,
        body: Option<&Value>,
    ) -> Result<Value, (u16, Value)> {
        let url = format!("{}{}", self.base, path);
        let mut req = self
            .http
            .request(method.clone(), &url)
            .bearer_auth(bearer)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .query(query);
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                return Err((
                    0,
                    json!({
                        "error": redact(&format!("shift_github {verb}: GitHub unreachable: {e}"), &[bearer]),
                        "transport": true,
                    }),
                ))
            }
        };
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let parsed: Value = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::Null)
        };
        if status.is_success() {
            return Ok(parsed);
        }
        let code = status.as_u16();
        let message: String = parsed
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .chars()
            .take(300)
            .collect();
        Err((
            code,
            json!({
                "error": redact(
                    &format!("shift_github {verb}: GitHub answered {code} to {method} {path}: {message}"),
                    &[bearer],
                ),
                "status": code,
            }),
        ))
    }
}
