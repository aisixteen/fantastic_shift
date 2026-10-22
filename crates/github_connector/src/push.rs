//! Push a work branch: the git connector's own checks, then a token for one repo
//! handed to git only through the child's environment.

use std::sync::Arc;

use fantastic_kernel::{AgentId, Kernel};
use reqwest::Url;
use serde_json::{json, Value};
use shift_git_connector::git::{self, Auth};

use crate::api::{redact, Api};
use crate::app;
use crate::{bad, valid_repo, Cfg};

fn scrub_reply(mut v: Value, secrets: &[&str]) -> Value {
    fn walk(v: &mut Value, secrets: &[&str]) {
        match v {
            Value::String(s) => *s = redact(s, secrets),
            Value::Array(a) => a.iter_mut().for_each(|x| walk(x, secrets)),
            Value::Object(m) => m.values_mut().for_each(|x| walk(x, secrets)),
            _ => {}
        }
    }
    walk(&mut v, secrets);
    v
}

/// Push `branch` from the worktree `path` of the record's git agent to `repo`.
pub(crate) async fn push(id: &AgentId, kernel: &Arc<Kernel>, cfg: &Cfg, payload: &Value) -> Value {
    let Some(git_id) = cfg.git.clone() else {
        return bad(
            "shift_github push: no `git` on the record — name a git connector agent".into(),
            "git",
            json!(null),
        );
    };
    let git_id = AgentId::from(git_id.as_str());
    let is_git = kernel
        .agents
        .get(&git_id)
        .map(|e| e.handler_module.as_deref() == Some(shift_git_connector::HANDLER_MODULE))
        .unwrap_or(false);
    if !is_git {
        return bad(
            format!("shift_github push: `{git_id}` is not a git connector agent (shift_git.tools)"),
            "git",
            json!(git_id.to_string()),
        );
    }
    let gcfg = match shift_git_connector::cfg_of(kernel, &git_id) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let (branch, path) = match shift_git_connector::check_push(&gcfg, payload).await {
        Ok(v) => v,
        Err(e) => return e,
    };
    let url = match git::remote_url(&gcfg).await {
        Ok(u) => u,
        Err(e) => return e,
    };
    let not_https = || {
        bad(
            "shift_github push: the git agent's remote must be an https URL of a GitHub repository"
                .into(),
            "remote",
            json!("[url withheld]"),
        )
    };
    let Ok(parsed) = Url::parse(&url) else {
        return not_https();
    };
    let local = matches!(
        parsed.host_str(),
        Some("127.0.0.1" | "::1" | "[::1]" | "localhost")
    );
    if !(parsed.scheme() == "https" || (parsed.scheme() == "http" && local)) {
        return not_https();
    }
    let api_url = match Url::parse(&cfg.api_url) {
        Ok(u) => u,
        Err(_) => return not_https(),
    };
    let remote_host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    let api_host = api_url.host_str().unwrap_or("").to_ascii_lowercase();
    let git_host = if api_host == "api.github.com" {
        "github.com".to_string()
    } else {
        api_host
    };
    if remote_host != git_host {
        return bad(
            format!(
                "shift_github push: the git agent's remote is on `{remote_host}`, not on `{git_host}` — a token is sent only to the GitHub that minted it"
            ),
            "remote",
            json!("[url withheld]"),
        );
    }
    let repo_path = parsed
        .path()
        .trim_matches('/')
        .trim_end_matches(".git")
        .to_string();
    if !valid_repo(&repo_path) {
        return bad(
            "shift_github push: the git agent's remote does not name a repository as owner/name"
                .into(),
            "remote",
            json!("[url withheld]"),
        );
    }
    let Some(repo) = cfg
        .repos
        .iter()
        .find(|r| r.eq_ignore_ascii_case(&repo_path))
        .cloned()
    else {
        return bad(
            format!(
                "shift_github push: `{repo_path}` is not in this connector's repos ({})",
                cfg.repos.join(", ")
            ),
            "repo",
            json!(repo_path),
        );
    };
    let api = Api::new(cfg);
    let token = match app::token_for(id, kernel, cfg, &api, "push", &repo).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let jwt_text: String = app::app_jwt(id, kernel, cfg).await.unwrap_or_default();
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    let auth = Auth {
        username: "x-access-token".into(),
        token: token.clone(),
    };
    let reply = match git::run(
        &gcfg,
        &path,
        &[],
        &["push", url.as_str(), refspec.as_str()],
        Some(&auth),
        "push",
    )
    .await
    {
        Ok(_) => json!({
            "repo": repo,
            "branch": branch,
            "path": path.to_string_lossy(),
            "pushed": true,
        }),
        Err(e) => e,
    };
    scrub_reply(reply, &[token.as_str(), jwt_text.as_str()])
}
