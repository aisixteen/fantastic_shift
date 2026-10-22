//! The github connector agent (`shift_github.tools`): one GitHub App, bound by
//! configuration. It pushes work branches through a git connector agent, opens
//! draft pull requests, and reads and writes issues, comments, labels and PR status
//! on an allowlist of repositories.
//!
//! NO APP IDENTITY IN CODE: the app id, key, owner and repo names all come from the
//! record and the credential store.
//!
//! THE SECRET RULE: the app's private key is read from the credential store at each
//! call. A JWT and an installation token are minted per call, live only in local
//! variables of that call, and never appear in a reply, an error, a log line, a
//! record, a payload sent to another agent, argv, a URL or a file. Every error
//! string passes through `redact` before it is returned.
#![deny(missing_docs)]

mod api;
mod app;
mod push;
mod verbs;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_secrets;

use std::sync::{Arc, OnceLock};

use fantastic_ai_core::schema::{self, Field};
use fantastic_kernel::bundle::{Bundle, BundleError, Reply};
use fantastic_kernel::refusal::{refusal, with};
use fantastic_kernel::{AgentId, Kernel};
use serde_json::{json, Map, Value};

/// The handler module name this bundle registers under.
pub const HANDLER_MODULE: &str = "shift_github.tools";
/// The bundle name.
const NAME: &str = "shift_github";
/// The readme shown by `reflect` and the kernel.
pub const README: &str = include_str!("readme.md");
/// One-line sentence of what the agent is.
pub const SENTENCE: &str = "github connector — a GitHub App bound by configuration: push via a git agent, draft pull requests, issues, comments, labels and PR status on allowlisted repos.";

const BAD_ARG: &str = "bad_arg";
const UNHANDLED: &str = "unhandled";
const DEFAULT_API: &str = "https://api.github.com";

/// The github connector bundle.
#[derive(Default)]
pub struct GithubConnectorBundle;

/// The record schema of a `shift_github` agent, as the catalog publishes it.
pub(crate) fn record_schema() -> &'static Value {
    static S: OnceLock<Value> = OnceLock::new();
    S.get_or_init(|| {
        json!({"fields": [
            Field::new("app", "map", "{credentials: {store, ref}} — the ref holds fields app_id and private_key (PEM), bound to api_url's host").required().json(),
            Field::new("repos", "list", "the allowlist: [\"owner/name\", …] — every other repo is refused").required().json(),
            Field::new("api_url", "str", "the GitHub API root (GitHub Enterprise: https://<host>/api/v3)").default(json!(DEFAULT_API)).json(),
            Field::new("git", "str", "the id of a git connector agent (shift_git.tools) whose worktrees push uses").json(),
            Field::new("directive", "str", "what the shifter and workers may do with this connector, and how — returned by the directive verb").required().json(),
            Field::new("timeout_s", "int", "seconds one GitHub API request may take").default(json!(30)).json(),
        ]})
    })
}

/// The config of the agent, read from its meta at every call.
pub(crate) struct Cfg {
    /// The credential store and ref of the app's key.
    pub app_cred: (String, String),
    /// The allowlist of `owner/name` repositories.
    pub repos: Vec<String>,
    /// The API root, without a trailing slash.
    pub api_url: String,
    /// The id of the git connector agent push uses.
    pub git: Option<String>,
    /// What the connector may be used for.
    pub directive: String,
    /// Seconds one request may take.
    pub timeout_s: u64,
}

/// True for `owner/name` where both parts are plain names.
pub(crate) fn valid_repo(s: &str) -> bool {
    let mut parts = s.split('/');
    let (Some(owner), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let ok = |p: &str| {
        !p.is_empty()
            && p != "."
            && p != ".."
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    ok(owner) && ok(name)
}

fn text(meta: &Value, key: &str) -> Option<String> {
    meta.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

impl GithubConnectorBundle {
    /// Read the config from the agent's merged meta.
    pub(crate) fn cfg(meta: &Value) -> Cfg {
        let cred = meta.get("app").and_then(|a| a.get("credentials"));
        let app_cred = (
            cred.and_then(|c| c["store"].as_str())
                .unwrap_or("")
                .to_string(),
            cred.and_then(|c| c["ref"].as_str())
                .unwrap_or("")
                .to_string(),
        );
        Cfg {
            app_cred,
            repos: meta
                .get("repos")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            api_url: text(meta, "api_url")
                .unwrap_or_else(|| DEFAULT_API.into())
                .trim_end_matches('/')
                .to_string(),
            git: text(meta, "git").filter(|s| !s.is_empty()),
            directive: text(meta, "directive").unwrap_or_default(),
            timeout_s: meta.get("timeout_s").and_then(|v| v.as_u64()).unwrap_or(30),
        }
    }
}

/// A refusal reply: `error` text plus the arg that caused it.
pub(crate) fn bad(text: String, arg: &str, value: Value) -> Value {
    let mut r = refusal(BAD_ARG, text);
    with(&mut r, arg, value);
    r
}

fn bad_field(field: &str) -> String {
    format!("shift_github: `{field}` must be a non-empty string")
}

fn validate_app(app: &Value) -> Option<String> {
    let ok = app.as_object().is_some_and(|o| {
        o.len() == 1
            && o.get("credentials").is_some_and(|c| {
                c.as_object().is_some_and(|c| {
                    c.get("store")
                        .and_then(|v| v.as_str())
                        .is_some_and(|s| !s.is_empty())
                        && c.get("ref")
                            .and_then(|v| v.as_str())
                            .is_some_and(|s| !s.is_empty())
                })
            })
    });
    if ok {
        None
    } else {
        Some("shift_github: `app` must be {credentials: {store, ref}}".into())
    }
}

fn validate_repos(repos: &Value) -> Option<String> {
    let ok = repos
        .as_array()
        .is_some_and(|a| !a.is_empty() && a.iter().all(|r| r.as_str().is_some_and(valid_repo)));
    if ok {
        None
    } else {
        Some("shift_github: `repos` must be a non-empty list of \"owner/name\"".into())
    }
}

fn validate_api_url(v: &Value) -> Option<String> {
    let bad = || {
        Some(
            "shift_github: `api_url` must be an https URL (or http to 127.0.0.1, ::1 or localhost)"
                .into(),
        )
    };
    let Some(s) = v.as_str() else { return bad() };
    let Ok(u) = reqwest::Url::parse(s) else {
        return bad();
    };
    let host = u.host_str().unwrap_or("");
    let local = matches!(host, "127.0.0.1" | "::1" | "[::1]" | "localhost");
    let scheme_ok = u.scheme() == "https" || (u.scheme() == "http" && local);
    if scheme_ok && !host.is_empty() && u.query().is_none() && u.fragment().is_none() {
        None
    } else {
        bad()
    }
}

#[async_trait::async_trait]
impl Bundle for GithubConnectorBundle {
    fn name(&self) -> &str {
        NAME
    }

    fn readme(&self) -> Option<&'static str> {
        Some(README)
    }

    fn verb_docs(&self) -> Value {
        json!({
            "reflect": "reflect → {id, sentence, repos, api_url, git, directive}",
            "boot": "boot → nothing",
            "directive": "read this before using the connector → {directive}",
            "whoami": "whoami → {app: {id, slug, name}, repos, api_url}",
            "push": "push {repo, branch, path} → {repo, branch, path, pushed}",
            "pr_create": "pr_create {repo, branch, title, body?} → {repo, number, url, draft, base} (draft only)",
            "issues": "issues {repo, label?, state?} → {repo, issues: [{number, title, state, labels}]}",
            "issue_view": "issue_view {repo, number} → {repo, number, title, state, labels, body, comments}",
            "comment": "comment {repo, number, body} → {repo, number, comment_id, url}",
            "label": "label {repo, number, add?, remove?} → {repo, number, labels}",
            "pr_status": "pr_status {repo, number} → {repo, number, state, draft, head_sha, checks, checks_summary, reviews, review_state}",
        })
    }

    fn record_schema(&self) -> Option<Value> {
        Some(record_schema().clone())
    }

    fn validate_meta(
        &self,
        meta: &Map<String, Value>,
        record: &Map<String, Value>,
    ) -> Option<String> {
        let get = |k: &str| meta.get(k).filter(|v| !v.is_null());
        if let Some(v) = get("app") {
            if let Some(e) = validate_app(v) {
                return Some(e);
            }
        }
        if let Some(v) = get("repos") {
            if let Some(e) = validate_repos(v) {
                return Some(e);
            }
        }
        if let Some(v) = get("api_url") {
            if let Some(e) = validate_api_url(v) {
                return Some(e);
            }
        }
        for key in ["git", "directive"] {
            if let Some(v) = get(key) {
                let ok = v.as_str().is_some_and(|s| !s.trim().is_empty());
                if !ok {
                    return Some(bad_field(key));
                }
            }
        }
        if let Some(v) = get("timeout_s") {
            let ok = !v.is_boolean() && v.as_u64().is_some_and(|n| n > 0);
            if !ok {
                return Some("shift_github: `timeout_s` must be a positive int".into());
            }
        }
        schema::validate(record_schema(), NAME, meta, record)
    }

    async fn handle(
        &self,
        agent_id: &AgentId,
        payload: &Value,
        kernel: &Arc<Kernel>,
    ) -> Result<Reply, BundleError> {
        let t = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if t == "boot" {
            return Ok(None);
        }
        let meta = kernel
            .agents
            .get(agent_id)
            .map(|e| e.meta.read().expect("meta").clone())
            .unwrap_or_default();
        let cfg = Self::cfg(&Value::Object(meta));
        let out = verbs::dispatch(t, agent_id, payload, kernel, &cfg).await;
        Ok(Some(out))
    }
}
