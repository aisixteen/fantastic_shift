//! The verbs. Each API verb checks its arguments and the repo allowlist first,
//! then mints a token for that one repo, then makes its requests. Every reply is
//! scrubbed of the token before it is returned.

use std::sync::Arc;

use fantastic_kernel::refusal::{missing, refusal, with};
use fantastic_kernel::{AgentId, Kernel};
use reqwest::Method;
use serde_json::{json, Value};

use crate::api::{redact, Api};
use crate::{app, bad, push, Cfg, SENTENCE, UNHANDLED};

const STATE_TEXT: &str = "team_github issues: `state` must be open, closed or all";
const LABEL_TEXT: &str =
    "team_github label: give `add` and/or `remove` — each a non-empty list of label names";
const DRAFT_TEXT: &str =
    "team_github pr_create: pull requests are opened as drafts only — a person marks one ready";

/// Route one verb to its handler.
pub(crate) async fn dispatch(
    t: &str,
    id: &AgentId,
    p: &Value,
    kernel: &Arc<Kernel>,
    cfg: &Cfg,
) -> Value {
    match t {
        "reflect" => json!({
            "id": id.to_string(),
            "sentence": SENTENCE,
            "repos": cfg.repos,
            "api_url": cfg.api_url,
            "git": cfg.git,
            "directive": cfg.directive,
        }),
        "directive" => json!({"directive": cfg.directive}),
        "whoami" => whoami(id, kernel, cfg).await,
        "push" => push::push(id, kernel, cfg, p).await,
        "pr_create" => pr_create(id, kernel, cfg, p).await,
        "issues" => issues(id, kernel, cfg, p).await,
        "issue_view" => issue_view(id, kernel, cfg, p).await,
        "comment" => comment(id, kernel, cfg, p).await,
        "label" => label(id, kernel, cfg, p).await,
        "pr_status" => pr_status(id, kernel, cfg, p).await,
        other => {
            let mut r = refusal(UNHANDLED, format!("team_github: unknown type '{other}'"));
            with(&mut r, "verb", json!(other));
            r
        }
    }
}

fn ge(r: Result<Value, (u16, Value)>) -> Result<Value, Value> {
    r.map_err(|(_, v)| v)
}

fn str_arg<'a>(p: &'a Value, verb: &str, name: &str) -> Result<&'a str, Value> {
    match p.get(name).and_then(|v| v.as_str()) {
        Some(t) if !t.is_empty() => Ok(t),
        _ => Err(missing(verb, name)),
    }
}

fn repo_arg(p: &Value, verb: &str, cfg: &Cfg) -> Result<String, Value> {
    let raw = str_arg(p, verb, "repo")?;
    match cfg.repos.iter().find(|r| r.eq_ignore_ascii_case(raw)) {
        Some(r) => Ok(r.clone()),
        None => Err(bad(
            format!(
                "team_github {verb}: `{raw}` is not in this connector's repos ({})",
                cfg.repos.join(", ")
            ),
            "repo",
            json!(raw),
        )),
    }
}

fn number_arg(p: &Value, verb: &str) -> Result<u64, Value> {
    match p.get("number").and_then(|v| v.as_u64()) {
        Some(n) if n > 0 => Ok(n),
        _ => Err(bad(
            format!("team_github {verb}: `number` must be a positive int"),
            "number",
            p.get("number").cloned().unwrap_or(Value::Null),
        )),
    }
}

fn names_of(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|l| l["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A non-empty list of non-empty strings, or None.
fn names(v: &Value) -> Option<Vec<String>> {
    let a = v.as_array()?;
    if a.is_empty() {
        return None;
    }
    a.iter()
        .map(|x| x.as_str().filter(|s| !s.is_empty()).map(str::to_string))
        .collect()
}

/// Percent-encode a label name for a path segment.
fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn scrub(v: &mut Value, token: &str) {
    match v {
        Value::String(s) => *s = redact(s.as_str(), &[token]),
        Value::Array(a) => a.iter_mut().for_each(|x| scrub(x, token)),
        Value::Object(m) => m.values_mut().for_each(|x| scrub(x, token)),
        _ => {}
    }
}

fn finish(r: Result<Value, Value>, token: &str) -> Value {
    let mut v = match r {
        Ok(v) | Err(v) => v,
    };
    scrub(&mut v, token);
    v
}

fn base_refusal(branch: &str) -> Value {
    bad(
        format!("team_github pr_create: `{branch}` is a base branch (the default branch, main or master) — open the pull request from a work branch"),
        "branch",
        json!(branch),
    )
}

async fn whoami(id: &AgentId, kernel: &Arc<Kernel>, cfg: &Cfg) -> Value {
    let jwt = match app::app_jwt(id, kernel, cfg).await {
        Ok(j) => j,
        Err(e) => return e,
    };
    let api = Api::new(cfg);
    let r: Result<Value, Value> = async {
        let app = ge(api
            .call("whoami", Method::GET, "/app", &[], &jwt, None)
            .await)?;
        Ok(json!({
            "app": {"id": app["id"], "slug": app["slug"], "name": app["name"]},
            "repos": cfg.repos,
            "api_url": cfg.api_url,
        }))
    }
    .await;
    finish(r, &jwt)
}

async fn pr_create(id: &AgentId, kernel: &Arc<Kernel>, cfg: &Cfg, p: &Value) -> Value {
    const V: &str = "pr_create";
    if p.get("draft")
        .is_some_and(|d| !d.is_null() && *d != Value::Bool(true))
    {
        return bad(DRAFT_TEXT.into(), "draft", p["draft"].clone());
    }
    let branch = match str_arg(p, V, "branch") {
        Ok(b) => b,
        Err(e) => return e,
    };
    if branch == "main" || branch == "master" {
        return base_refusal(branch);
    }
    let title = match str_arg(p, V, "title") {
        Ok(t) => t.to_string(),
        Err(e) => return e,
    };
    let repo = match repo_arg(p, V, cfg) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let api = Api::new(cfg);
    let token = match app::token_for(id, kernel, cfg, &api, V, &repo).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let r: Result<Value, Value> = async {
        let info = ge(api
            .call(V, Method::GET, &format!("/repos/{repo}"), &[], &token, None)
            .await)?;
        let default = info["default_branch"].as_str().unwrap_or("").to_string();
        if branch == default {
            return Err(base_refusal(branch));
        }
        let body_text = p.get("body").and_then(|v| v.as_str()).unwrap_or("");
        let payload = json!({
            "title": title,
            "head": branch,
            "base": default,
            "body": body_text,
            "draft": true,
        });
        let pr = ge(api
            .call(
                V,
                Method::POST,
                &format!("/repos/{repo}/pulls"),
                &[],
                &token,
                Some(&payload),
            )
            .await)?;
        Ok(json!({
            "repo": repo,
            "number": pr["number"],
            "url": pr["html_url"],
            "draft": pr["draft"],
            "base": default,
        }))
    }
    .await;
    finish(r, &token)
}

async fn issues(id: &AgentId, kernel: &Arc<Kernel>, cfg: &Cfg, p: &Value) -> Value {
    const V: &str = "issues";
    let repo = match repo_arg(p, V, cfg) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let state = match p.get("state") {
        None | Some(Value::Null) => "open".to_string(),
        Some(Value::String(s)) if matches!(s.as_str(), "open" | "closed" | "all") => s.clone(),
        Some(other) => return bad(STATE_TEXT.into(), "state", other.clone()),
    };
    let label = p
        .get("label")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let api = Api::new(cfg);
    let token = match app::token_for(id, kernel, cfg, &api, V, &repo).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let r: Result<Value, Value> = async {
        let mut q: Vec<(&str, String)> = vec![("state", state.clone()), ("per_page", "100".into())];
        if let Some(l) = &label {
            q.push(("labels", l.clone()));
        }
        let items = ge(api
            .call(
                V,
                Method::GET,
                &format!("/repos/{repo}/issues"),
                &q,
                &token,
                None,
            )
            .await)?;
        let list: Vec<Value> = items
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|i| i.get("pull_request").is_none())
                    .map(|i| {
                        json!({
                            "number": i["number"],
                            "title": i["title"],
                            "state": i["state"],
                            "labels": names_of(&i["labels"]),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(json!({"repo": repo, "issues": list}))
    }
    .await;
    finish(r, &token)
}

async fn issue_view(id: &AgentId, kernel: &Arc<Kernel>, cfg: &Cfg, p: &Value) -> Value {
    const V: &str = "issue_view";
    let repo = match repo_arg(p, V, cfg) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let number = match number_arg(p, V) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let api = Api::new(cfg);
    let token = match app::token_for(id, kernel, cfg, &api, V, &repo).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let r: Result<Value, Value> = async {
        let issue = ge(api
            .call(
                V,
                Method::GET,
                &format!("/repos/{repo}/issues/{number}"),
                &[],
                &token,
                None,
            )
            .await)?;
        let comments = ge(api
            .call(
                V,
                Method::GET,
                &format!("/repos/{repo}/issues/{number}/comments"),
                &[("per_page", "100".into())],
                &token,
                None,
            )
            .await)?;
        let list: Vec<Value> = comments
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|c| {
                        json!({
                            "author": c["user"]["login"],
                            "created_at": c["created_at"],
                            "body": c["body"],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(json!({
            "repo": repo,
            "number": number,
            "title": issue["title"],
            "state": issue["state"],
            "labels": names_of(&issue["labels"]),
            "body": issue["body"].as_str().unwrap_or(""),
            "comments": list,
        }))
    }
    .await;
    finish(r, &token)
}

async fn comment(id: &AgentId, kernel: &Arc<Kernel>, cfg: &Cfg, p: &Value) -> Value {
    const V: &str = "comment";
    let repo = match repo_arg(p, V, cfg) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let number = match number_arg(p, V) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let body = match str_arg(p, V, "body") {
        Ok(b) => b.to_string(),
        Err(e) => return e,
    };
    let api = Api::new(cfg);
    let token = match app::token_for(id, kernel, cfg, &api, V, &repo).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let r: Result<Value, Value> = async {
        let made = ge(api
            .call(
                V,
                Method::POST,
                &format!("/repos/{repo}/issues/{number}/comments"),
                &[],
                &token,
                Some(&json!({"body": body})),
            )
            .await)?;
        Ok(json!({
            "repo": repo,
            "number": number,
            "comment_id": made["id"],
            "url": made["html_url"],
        }))
    }
    .await;
    finish(r, &token)
}

async fn label(id: &AgentId, kernel: &Arc<Kernel>, cfg: &Cfg, p: &Value) -> Value {
    const V: &str = "label";
    let repo = match repo_arg(p, V, cfg) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let number = match number_arg(p, V) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let parse = |key: &str| -> Result<Option<Vec<String>>, Value> {
        match p.get(key).filter(|v| !v.is_null()) {
            None => Ok(None),
            Some(v) => names(v)
                .map(Some)
                .ok_or_else(|| bad(LABEL_TEXT.into(), key, json!(null))),
        }
    };
    let add = match parse("add") {
        Ok(a) => a,
        Err(e) => return e,
    };
    let remove = match parse("remove") {
        Ok(r) => r,
        Err(e) => return e,
    };
    if add.is_none() && remove.is_none() {
        return bad(LABEL_TEXT.into(), "add", json!(null));
    }
    let api = Api::new(cfg);
    let token = match app::token_for(id, kernel, cfg, &api, V, &repo).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let r: Result<Value, Value> = async {
        if let Some(a) = &add {
            ge(api
                .call(
                    V,
                    Method::POST,
                    &format!("/repos/{repo}/issues/{number}/labels"),
                    &[],
                    &token,
                    Some(&json!({"labels": a})),
                )
                .await)?;
        }
        if let Some(rm) = &remove {
            for name in rm {
                match api
                    .call(
                        V,
                        Method::DELETE,
                        &format!("/repos/{repo}/issues/{number}/labels/{}", pct(name)),
                        &[],
                        &token,
                        None,
                    )
                    .await
                {
                    Ok(_) => {}
                    Err((404, _)) => {}
                    Err((_, v)) => return Err(v),
                }
            }
        }
        let now = ge(api
            .call(
                V,
                Method::GET,
                &format!("/repos/{repo}/issues/{number}/labels"),
                &[],
                &token,
                None,
            )
            .await)?;
        Ok(json!({"repo": repo, "number": number, "labels": names_of(&now)}))
    }
    .await;
    finish(r, &token)
}

fn checks_summary(checks: &[Value]) -> &'static str {
    if checks.is_empty() {
        return "none";
    }
    let failed = |c: &Value| {
        matches!(
            c["conclusion"].as_str(),
            Some("failure" | "timed_out" | "cancelled" | "action_required" | "startup_failure")
        )
    };
    if checks.iter().any(failed) {
        return "failure";
    }
    if checks
        .iter()
        .any(|c| c["status"].as_str() != Some("completed"))
    {
        return "pending";
    }
    "success"
}

fn review_summary(reviews: &Value) -> (Value, &'static str) {
    // Each author's LAST review, in first-seen order.
    let mut last: Vec<(String, String)> = Vec::new();
    for r in reviews.as_array().into_iter().flatten() {
        let author = r["user"]["login"].as_str().unwrap_or("").to_string();
        let state = r["state"].as_str().unwrap_or("").to_string();
        match last.iter_mut().find(|(a, _)| *a == author) {
            Some(e) => e.1 = state,
            None => last.push((author, state)),
        }
    }
    let any = |s: &str| last.iter().any(|(_, st)| st == s);
    let summary = if any("CHANGES_REQUESTED") {
        "changes_requested"
    } else if any("APPROVED") {
        "approved"
    } else if !last.is_empty() {
        "commented"
    } else {
        "none"
    };
    let list: Vec<Value> = last
        .iter()
        .map(|(a, s)| json!({"author": a, "state": s}))
        .collect();
    (Value::Array(list), summary)
}

async fn pr_status(id: &AgentId, kernel: &Arc<Kernel>, cfg: &Cfg, p: &Value) -> Value {
    const V: &str = "pr_status";
    let repo = match repo_arg(p, V, cfg) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let number = match number_arg(p, V) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let api = Api::new(cfg);
    let token = match app::token_for(id, kernel, cfg, &api, V, &repo).await {
        Ok(t) => t,
        Err(e) => return e,
    };
    let r: Result<Value, Value> = async {
        let pr = ge(api
            .call(
                V,
                Method::GET,
                &format!("/repos/{repo}/pulls/{number}"),
                &[],
                &token,
                None,
            )
            .await)?;
        let sha = pr["head"]["sha"].as_str().unwrap_or("").to_string();
        let runs = ge(api
            .call(
                V,
                Method::GET,
                &format!("/repos/{repo}/commits/{sha}/check-runs"),
                &[("per_page", "100".into())],
                &token,
                None,
            )
            .await)?;
        let reviews = ge(api
            .call(
                V,
                Method::GET,
                &format!("/repos/{repo}/pulls/{number}/reviews"),
                &[("per_page", "100".into())],
                &token,
                None,
            )
            .await)?;
        let checks: Vec<Value> = runs["check_runs"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|c| {
                        json!({
                            "name": c["name"],
                            "status": c["status"],
                            "conclusion": c["conclusion"],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let (review_list, review_state) = review_summary(&reviews);
        Ok(json!({
            "repo": repo,
            "number": number,
            "state": pr["state"],
            "draft": pr["draft"],
            "head_sha": sha,
            "checks": checks,
            "checks_summary": checks_summary(&checks),
            "reviews": review_list,
            "review_state": review_state,
        }))
    }
    .await;
    finish(r, &token)
}
