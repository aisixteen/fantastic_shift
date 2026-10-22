//! The git connector agent (`shift_git.tools`): one local clone and one
//! remote. It makes worktrees, commits and pushes work branches. It never
//! pushes the default branch, `main` or `master`, and never forces.
//!
//! THE SECRET RULE: an HTTPS token is read from the credential store at each
//! fetch or push and handed to git only through the child's environment and a
//! one-shot credential helper. It is never in argv, a URL, a git config, a
//! reply, an error or a log line. Every string returned passes through `redact`.
#![deny(missing_docs)]

pub mod git;
pub mod paths;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_secrets;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use fantastic_ai_core::schema::{self, credentials_field, Field};
use fantastic_kernel::bundle::{Bundle, BundleError, Reply};
use fantastic_kernel::refusal::{missing, refusal, with};
use fantastic_kernel::{AgentId, Kernel};
use serde_json::{json, Map, Value};

/// The handler module name this bundle registers under.
pub const HANDLER_MODULE: &str = "shift_git.tools";
/// The bundle name.
const NAME: &str = "shift_git";
/// The readme shown by `reflect` and the kernel.
pub const README: &str = include_str!("readme.md");
/// One-line sentence of what the agent is.
pub const SENTENCE: &str = "git connector — worktrees, commits and pushes of work branches to one remote; never the default branch, never a force.";

const BAD_ARG: &str = "bad_arg";
const UNHANDLED: &str = "unhandled";

/// Refusal text: the record's remote carries a user or password.
pub(crate) const URL_CREDS: &str = "shift_git: the remote URL carries a user or password — remove it and name a credential store ref in credentials instead";

/// The git connector bundle.
#[derive(Default)]
pub struct GitConnectorBundle;

/// The record schema of a `shift_git` agent, as the catalog publishes it.
pub(crate) fn record_schema() -> &'static Value {
    static S: OnceLock<Value> = OnceLock::new();
    S.get_or_init(|| {
        json!({"fields": [
            Field::new("repo_dir", "str", "absolute path of the local clone every worktree is made from").required().json(),
            Field::new("workspace_root", "str", "absolute directory every worktree path must be inside").required().json(),
            Field::new("remote", "str", "a remote name in repo_dir (origin) or a URL (https://…, ssh://…, git@host:org/repo.git)").required().json(),
            Field::new("default_branch", "str", "the remote's default branch — push refuses it (and main, master)").required().json(),
            credentials_field(false).json(),
            Field::new("username", "str", "the HTTPS user name sent with the token (GitHub: x-access-token, GitLab: oauth2)").default(json!("x-access-token")).json(),
            Field::new("author_name", "str", "commit author and sign-off name (with author_email)").json(),
            Field::new("author_email", "str", "commit author and sign-off email (with author_name)").json(),
            Field::new("ssh_command", "str", "GIT_SSH_COMMAND for SSH remotes").default(json!("ssh -o BatchMode=yes")).json(),
            Field::new("timeout_s", "int", "seconds one git command may run").default(json!(300)).json(),
        ]})
    })
}

/// The config of the agent, read from its meta at every call.
pub struct Cfg {
    /// Absolute path of the local clone every worktree is made from.
    pub repo_dir: PathBuf,
    /// Absolute directory every worktree path must be inside.
    pub workspace_root: PathBuf,
    /// A remote name in `repo_dir`, or a URL.
    pub remote: String,
    /// The remote's default branch; push refuses it.
    pub default_branch: String,
    /// The HTTPS user name sent with a token.
    pub username: String,
    /// Commit author and sign-off `(name, email)`, when both are set.
    pub author: Option<(String, String)>,
    /// GIT_SSH_COMMAND for SSH remotes.
    pub ssh_command: String,
    /// Seconds one git command may run.
    pub timeout_s: u64,
    /// Whether the record names a credential store ref.
    pub has_credentials: bool,
}

fn text_of(meta: &Value, key: &str) -> Option<String> {
    meta.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

impl GitConnectorBundle {
    /// Read the config from the agent's merged meta.
    pub(crate) fn cfg(meta: &Value) -> Cfg {
        Cfg {
            repo_dir: PathBuf::from(text_of(meta, "repo_dir").unwrap_or_default()),
            workspace_root: PathBuf::from(text_of(meta, "workspace_root").unwrap_or_default()),
            remote: text_of(meta, "remote").unwrap_or_default(),
            default_branch: text_of(meta, "default_branch").unwrap_or_default(),
            username: text_of(meta, "username").unwrap_or_else(|| "x-access-token".into()),
            author: match (text_of(meta, "author_name"), text_of(meta, "author_email")) {
                (Some(n), Some(e)) => Some((n, e)),
                _ => None,
            },
            ssh_command: text_of(meta, "ssh_command")
                .unwrap_or_else(|| "ssh -o BatchMode=yes".into()),
            timeout_s: meta
                .get("timeout_s")
                .and_then(|v| v.as_u64())
                .unwrap_or(300),
            has_credentials: meta
                .get("credentials")
                .map(|c| !c.is_null())
                .unwrap_or(false),
        }
    }
}

/// The config of the git agent `id`, read from its record the way `handle` reads it.
pub fn cfg_of(kernel: &Kernel, id: &AgentId) -> Result<Cfg, Value> {
    let entry = kernel
        .agents
        .get(id)
        .ok_or_else(|| json!({"error": format!("shift_git: no agent `{id}`")}))?;
    let meta = entry.meta.read().expect("meta").clone();
    Ok(GitConnectorBundle::cfg(&Value::Object(meta)))
}

/// A refusal reply: `error` text plus the arg that caused it.
pub(crate) fn bad(text: String, arg: &str, value: Value) -> Value {
    let mut r = refusal(BAD_ARG, text);
    with(&mut r, arg, value);
    r
}

/// A missing-argument refusal for `verb` and `arg`.
pub(crate) fn missing_arg(verb: &str, arg: &str) -> Value {
    missing(verb, arg)
}

#[async_trait::async_trait]
impl Bundle for GitConnectorBundle {
    fn name(&self) -> &str {
        NAME
    }

    fn readme(&self) -> Option<&'static str> {
        Some(README)
    }

    fn verb_docs(&self) -> Value {
        json!({
            "reflect": "reflect → {id, sentence, repo_dir, workspace_root, remote, default_branch, credentials}",
            "boot": "boot → nothing",
            "worktree_add": "worktree_add {path, branch, base} → {path, branch, base, created}",
            "worktree_remove": "worktree_remove {path} → {path, removed}",
            "commit": "commit {path, message, trailers?} → {path, commit, committed}",
            "push": "push {path, branch} → {path, branch, remote, pushed}",
            "fetch": "fetch {} → {remote, fetched}",
            "diff_stat": "diff_stat {path, base?} → {path, against, stat}",
            "status": "status {path} → {path, branch, clean, entries}",
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
        let meta_v = Value::Object(meta.clone());
        let record_v = Value::Object(record.clone());
        for key in ["repo_dir", "workspace_root"] {
            if let Some(v) = meta.get(key).filter(|v| !v.is_null()) {
                let ok = v
                    .as_str()
                    .map(|s| std::path::Path::new(s).is_absolute())
                    .unwrap_or(false);
                if !ok {
                    return Some(format!("shift_git: `{key}` must be an absolute path"));
                }
            }
        }
        if let Some(v) = meta.get("remote").filter(|v| !v.is_null()) {
            match v.as_str() {
                Some(s) if !s.is_empty() && !s.starts_with('-') => {
                    let lower = s.to_ascii_lowercase();
                    if lower.starts_with("http://") || lower.starts_with("https://") {
                        let rest = &s[s.find("//").unwrap() + 2..];
                        let authority = rest.split('/').next().unwrap_or("");
                        if authority.contains('@') {
                            return Some(URL_CREDS.to_string());
                        }
                    }
                }
                _ => return Some("shift_git: `remote` must be a remote name or a URL".into()),
            }
        }
        if let Some(v) = meta.get("default_branch").filter(|v| !v.is_null()) {
            match v.as_str() {
                Some(s) if !s.is_empty() && !s.starts_with('-') => {}
                _ => return Some("shift_git: `default_branch` must be a branch name".into()),
            }
        }
        for key in ["username", "ssh_command"] {
            if let Some(v) = meta.get(key).filter(|v| !v.is_null()) {
                match v.as_str() {
                    Some(s) if !s.is_empty() => {}
                    _ => return Some(format!("shift_git: `{key}` must be a non-empty string")),
                }
            }
        }
        if let Some(v) = meta.get("timeout_s").filter(|v| !v.is_null()) {
            let ok = !v.is_boolean() && v.as_u64().map(|n| n > 0).unwrap_or(false);
            if !ok {
                return Some("shift_git: `timeout_s` must be a positive int".into());
            }
        }
        // Checked on the MERGED record: record plus meta.
        let set = |k: &str| {
            meta_v.get(k).filter(|v| !v.is_null()).is_some()
                || record_v.get(k).filter(|v| !v.is_null()).is_some()
        };
        if set("author_name") != set("author_email") {
            return Some(
                "shift_git: `author_name` and `author_email` go together — set both or neither"
                    .into(),
            );
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
        let meta = kernel
            .agents
            .get(agent_id)
            .map(|e| e.meta.read().expect("meta").clone())
            .unwrap_or_default();
        let cfg = Self::cfg(&Value::Object(meta));
        let out: Value = match t {
            "reflect" => json!({
                "id": agent_id.to_string(),
                "sentence": SENTENCE,
                "repo_dir": cfg.repo_dir,
                "workspace_root": cfg.workspace_root,
                "remote": cfg.remote,
                "default_branch": cfg.default_branch,
                "credentials": cfg.has_credentials,
            }),
            "boot" => return Ok(None),
            "worktree_add" => match git::worktree_add(&cfg, payload).await {
                Ok(v) | Err(v) => v,
            },
            "worktree_remove" => match git::worktree_remove(&cfg, payload).await {
                Ok(v) | Err(v) => v,
            },
            "commit" => match git::commit(&cfg, payload).await {
                Ok(v) | Err(v) => v,
            },
            "push" => match git::push(&cfg, payload, agent_id, kernel).await {
                Ok(v) | Err(v) => v,
            },
            "fetch" => match git::fetch(&cfg, agent_id, kernel).await {
                Ok(v) | Err(v) => v,
            },
            "diff_stat" => match git::diff_stat(&cfg, payload).await {
                Ok(v) | Err(v) => v,
            },
            "status" => match git::status(&cfg, payload).await {
                Ok(v) | Err(v) => v,
            },
            other => {
                let mut r = refusal(UNHANDLED, format!("shift_git: unknown type '{other}'"));
                with(&mut r, "verb", json!(other));
                r
            }
        };
        Ok(Some(out))
    }
}
