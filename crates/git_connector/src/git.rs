//! The only place a `git` process is made. An HTTPS token travels only in
//! the child's environment, read by a one-shot credential helper. It never
//! appears in argv, a URL, a config file or a reply.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use fantastic_ai_core::credentials::key_reader;
use fantastic_kernel::secret::SHOWN;
use fantastic_kernel::{AgentId, Kernel};
use serde_json::{json, Value};

use crate::paths::{inside_root, is_worktree_of, not_worktree};
use crate::{bad, missing_arg, Cfg, NAME, URL_CREDS};

/// A token read from the credential store. It has no `Debug`, `Display` or
/// `Serialize`, so it cannot be printed by accident.
pub struct Auth {
    /// The HTTPS user name sent with the token.
    pub username: String,
    /// The token. Never printed; passes through `redact` before any reply.
    pub token: String,
    /// The host (with port) the token may be sent to.
    pub host: String,
    /// The scheme the token may be sent over (`https`, or `http` to loopback).
    pub proto: String,
}

/// Resets every configured helper, then supplies ours. Reads the token from
/// the child's environment only.
pub(crate) const HELPER: &str = r#"credential.helper=!f() { test "$1" = get || exit 0; p=; h=; while IFS='=' read -r k v; do case "$k" in protocol) p=$v;; host) h=$v;; esac; done; test "$h" = "$SHIFT_GIT_HOST" && test "$p" = "$SHIFT_GIT_PROTO" || exit 0; echo "username=$SHIFT_GIT_USERNAME"; echo "password=$SHIFT_GIT_TOKEN"; }; f"#;

const PLAIN_HTTP: &str = "shift_git: credentials are sent only over https (or http to 127.0.0.1, ::1 or localhost) — refused";
const FORCE: &str = "shift_git push: a force push is never made — push a new branch name instead";
const REWRITES: &str = "git connector: the repository rewrites its remote URL (insteadOf), refusing to send credentials";

/// True when a `url.<x>.insteadOf` or `url.<x>.pushInsteadOf` rule in `dir`
/// matches `url` as a prefix, so git could send the request elsewhere.
pub(crate) async fn rewrites_url(dir: &Path, url: &str) -> bool {
    let Some(out) = plain(
        dir,
        &["config", "--get-regexp", "^url\\..*\\.(push)?insteadof$"],
    )
    .await
    else {
        return false;
    };
    out.lines().any(|line| {
        line.split_once(' ')
            .map(|(_, value)| !value.is_empty() && url.starts_with(value))
            .unwrap_or(false)
    })
}
const TRAILER: &str = "shift_git commit: each trailer is one `Key: value` line";

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Replace every occurrence of the token with the shown marker.
pub fn redact(text: &str, auth: Option<&Auth>) -> String {
    match auth {
        Some(a) if !a.token.is_empty() => text.replace(a.token.as_str(), SHOWN),
        _ => text.to_string(),
    }
}

/// Redact every string inside a JSON value.
pub fn scrub(v: &mut Value, auth: Option<&Auth>) {
    match v {
        Value::String(text) => {
            let clean = redact(text, auth);
            *text = clean;
        }
        Value::Array(items) => {
            for x in items.iter_mut() {
                scrub(x, auth);
            }
        }
        Value::Object(map) => {
            for (_, x) in map.iter_mut() {
                scrub(x, auth);
            }
        }
        _ => {}
    }
}

fn finish(r: Result<Value, Value>, auth: Option<&Auth>) -> Result<Value, Value> {
    match r {
        Ok(mut v) => {
            scrub(&mut v, auth);
            Ok(v)
        }
        Err(mut v) => {
            scrub(&mut v, auth);
            Err(v)
        }
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(t) => !t.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

fn arg<'a>(p: &'a Value, verb: &str, name: &str) -> Result<&'a str, Value> {
    match p.get(name).and_then(|v| v.as_str()) {
        Some(t) if !t.is_empty() => Ok(t),
        _ => Err(missing_arg(verb, name)),
    }
}

fn dash(verb: &str, name: &str, value: &str) -> Result<(), Value> {
    if value.starts_with('-') {
        Err(bad(
            format!("shift_git {verb}: `{name}` must not start with `-`"),
            name,
            json!(name),
        ))
    } else {
        Ok(())
    }
}

fn looks_like_url(s: &str) -> bool {
    if s.contains("://") || s.starts_with('/') {
        return true;
    }
    match s.find(':') {
        Some(i) => !s[..i].contains('/'),
        None => false,
    }
}

fn host_of(authority: &str) -> String {
    if authority.starts_with('[') {
        match authority.find(']') {
            Some(i) => authority[..=i].to_ascii_lowercase(),
            None => authority.to_ascii_lowercase(),
        }
    } else {
        authority
            .split(':')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
    }
}

/// Run one git command in `dir` with the optional one-shot credential. Stdout is
/// returned redacted; stderr is redacted on failure.
pub async fn run(
    cfg: &Cfg,
    dir: &Path,
    pre: &[String],
    args: &[&str],
    auth: Option<&Auth>,
    verb: &str,
) -> Result<String, Value> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C").arg(dir);
    for entry in pre {
        cmd.arg("-c").arg(entry);
    }
    if let Some(a) = auth {
        cmd.args([
            "-c",
            "credential.helper=",
            "-c",
            HELPER,
            "-c",
            "core.hooksPath=/dev/null",
        ]);
        cmd.env("SHIFT_GIT_USERNAME", &a.username)
            .env("SHIFT_GIT_TOKEN", &a.token)
            .env("SHIFT_GIT_HOST", &a.host)
            .env("SHIFT_GIT_PROTO", &a.proto);
    }
    cmd.args(args);
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", &cfg.ssh_command);
    for k in [
        "GIT_ASKPASS",
        "SSH_ASKPASS",
        "GIT_TRACE",
        "GIT_TRACE_CURL",
        "GIT_CURL_VERBOSE",
        "GIT_TRACE_PACKET",
        "GIT_AUTHOR_NAME",
        "GIT_AUTHOR_EMAIL",
        "GIT_COMMITTER_NAME",
        "GIT_COMMITTER_EMAIL",
    ] {
        cmd.env_remove(k);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let out = tokio::time::timeout(Duration::from_secs(cfg.timeout_s), cmd.output())
        .await
        .map_err(|_| json!({"error": format!("shift_git {verb}: git timed out after {}s", cfg.timeout_s)}))?
        .map_err(|e| json!({"error": redact(&format!("shift_git {verb}: could not run git: {e}"), auth)}))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let code = out.status.code().unwrap_or(-1);
        return Err(json!({"error": redact(
            &format!("shift_git {verb}: git failed (exit {code}): {stderr}"),
            auth,
        )}));
    }
    Ok(redact(&String::from_utf8_lossy(&out.stdout), auth))
}

/// A quiet git query with no auth and no timeout. None on any failure.
async fn plain(dir: &Path, args: &[&str]) -> Option<String> {
    let out = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The canonical git common dir of `dir`, if it is inside a repository.
pub(crate) async fn common_dir(dir: &Path) -> Option<PathBuf> {
    let t = plain(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .await?;
    PathBuf::from(t).canonicalize().ok()
}

/// The canonical top level of the worktree containing `dir`.
pub(crate) async fn toplevel(dir: &Path) -> Option<PathBuf> {
    let t = plain(dir, &["rev-parse", "--show-toplevel"]).await?;
    PathBuf::from(t).canonicalize().ok()
}

/// The URL of the record's remote: itself when it is a URL, else the name's URL.
pub async fn remote_url(cfg: &Cfg) -> Result<String, Value> {
    if looks_like_url(&cfg.remote) {
        return Ok(cfg.remote.clone());
    }
    let url = run(
        cfg,
        &cfg.repo_dir,
        &[],
        &["remote", "get-url", cfg.remote.as_str()],
        None,
        "remote",
    )
    .await?;
    Ok(url.trim().to_string())
}

/// The credential for `url`, or None when no credential applies. Refuses
/// plain http to a remote host, and any URL that carries a user or password.
pub(crate) async fn auth_for(
    cfg: &Cfg,
    url: &str,
    id: &AgentId,
    kernel: &Arc<Kernel>,
) -> Result<Option<Auth>, Value> {
    let Some(pos) = url.find("://") else {
        return Ok(None);
    };
    let scheme = url[..pos].to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Ok(None);
    }
    let rest = &url[pos + 3..];
    let authority = rest.split('/').next().unwrap_or("");
    if authority.contains('@') {
        return Err(bad(
            URL_CREDS.to_string(),
            "remote",
            json!("[url withheld]"),
        ));
    }
    if !cfg.has_credentials {
        return Ok(None);
    }
    let host = host_of(authority);
    let loopback = matches!(host.as_str(), "127.0.0.1" | "[::1]" | "localhost");
    if scheme == "http" && !loopback {
        return Err(bad(
            PLAIN_HTTP.to_string(),
            "remote",
            json!("[url withheld]"),
        ));
    }
    match key_reader(id, kernel, NAME, false).read(url).await {
        Ok(Some(token)) => Ok(Some(Auth {
            username: cfg.username.clone(),
            token,
            host: authority.to_ascii_lowercase(),
            proto: scheme.clone(),
        })),
        Ok(None) => Ok(None),
        Err(e) => Err(json!({"error": e.0})),
    }
}

/// Check the path is inside the root and is a worktree of `repo_dir`.
async fn gate(cfg: &Cfg, verb: &str, raw: &str) -> Result<PathBuf, Value> {
    let path = inside_root(&cfg.workspace_root, raw, verb)?;
    if !is_worktree_of(cfg, &path).await {
        return Err(not_worktree(verb, raw, &cfg.repo_dir));
    }
    Ok(path)
}

async fn check_branch(cfg: &Cfg, verb: &str, branch: &str) -> Result<(), Value> {
    run(
        cfg,
        &cfg.repo_dir,
        &[],
        &["check-ref-format", "--branch", branch],
        None,
        verb,
    )
    .await
    .map(|_| ())
    .map_err(|_| {
        bad(
            format!("shift_git {verb}: `{branch}` is not a valid branch name"),
            "branch",
            json!(branch),
        )
    })
}

/// Make a worktree `path` on `branch` from `base`.
pub(crate) async fn worktree_add(cfg: &Cfg, p: &Value) -> Result<Value, Value> {
    const V: &str = "worktree_add";
    let raw = arg(p, V, "path")?;
    let branch = arg(p, V, "branch")?;
    let base = arg(p, V, "base")?;
    dash(V, "branch", branch)?;
    dash(V, "base", base)?;
    let path = inside_root(&cfg.workspace_root, raw, V)?;
    if path.exists() {
        return Err(bad(
            format!("shift_git worktree_add: `{raw}` already exists"),
            "path",
            json!(raw),
        ));
    }
    check_branch(cfg, V, branch).await?;
    let target = s(&path);
    run(
        cfg,
        &cfg.repo_dir,
        &[],
        &["worktree", "add", "-b", branch, target.as_str(), base],
        None,
        V,
    )
    .await?;
    Ok(json!({"path": target, "branch": branch, "base": base, "created": true}))
}

/// Remove a worktree, never forcing.
pub(crate) async fn worktree_remove(cfg: &Cfg, p: &Value) -> Result<Value, Value> {
    const V: &str = "worktree_remove";
    let raw = arg(p, V, "path")?;
    let path = gate(cfg, V, raw).await?;
    let target = s(&path);
    run(
        cfg,
        &cfg.repo_dir,
        &[],
        &["worktree", "remove", target.as_str()],
        None,
        V,
    )
    .await?;
    Ok(json!({"path": target, "removed": true}))
}

fn valid_trailer(t: &str) -> bool {
    if t.contains('\n') {
        return false;
    }
    match t.split_once(": ") {
        Some((key, value)) => {
            !key.is_empty()
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !value.is_empty()
        }
        None => false,
    }
}

/// Stage everything and commit with sign-off, the author, and any trailers.
pub(crate) async fn commit(cfg: &Cfg, p: &Value) -> Result<Value, Value> {
    const V: &str = "commit";
    let raw = arg(p, V, "path")?;
    let message = arg(p, V, "message")?;
    let path = gate(cfg, V, raw).await?;
    let trailers: Vec<String> = match p.get("trailers") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => {
            let mut out = Vec::new();
            for item in items {
                match item.as_str() {
                    Some(t) if valid_trailer(t) => out.push(t.to_string()),
                    _ => return Err(bad(TRAILER.to_string(), "trailers", json!(TRAILER))),
                }
            }
            out
        }
        Some(_) => return Err(bad(TRAILER.to_string(), "trailers", json!(TRAILER))),
    };
    let target = s(&path);
    run(cfg, &path, &[], &["add", "-A"], None, V).await?;
    let st = run(cfg, &path, &[], &["status", "--porcelain"], None, V).await?;
    if st.trim().is_empty() {
        return Err(bad(
            format!("shift_git commit: nothing to commit in `{raw}`"),
            "path",
            json!(raw),
        ));
    }
    let pre: Vec<String> = match &cfg.author {
        Some((name, email)) => vec![format!("user.name={name}"), format!("user.email={email}")],
        None => Vec::new(),
    };
    let mut args: Vec<String> = vec![
        "commit".into(),
        "-s".into(),
        "-m".into(),
        message.to_string(),
    ];
    for t in &trailers {
        args.push("--trailer".into());
        args.push(t.clone());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run(cfg, &path, &pre, &refs, None, V).await?;
    let sha = run(cfg, &path, &[], &["rev-parse", "HEAD"], None, V).await?;
    Ok(json!({"path": target, "commit": sha.trim(), "committed": true}))
}

/// The push checks, in order: force, branch name, protected branch, the path
/// inside the root and a real worktree. Returns the branch and the worktree.
pub async fn check_push(cfg: &Cfg, p: &Value) -> Result<(String, PathBuf), Value> {
    const V: &str = "push";
    if p.get("force").map(truthy).unwrap_or(false) {
        return Err(bad(FORCE.to_string(), "force", json!(true)));
    }
    let branch = arg(p, V, "branch")?;
    if branch.starts_with('+') {
        return Err(bad(FORCE.to_string(), "force", json!(true)));
    }
    if branch.starts_with("refs/") {
        return Err(bad(
            format!("branch names are short names (no refs/ prefix): {branch}"),
            "branch",
            json!(branch),
        ));
    }
    if branch == cfg.default_branch.as_str() || branch == "main" || branch == "master" {
        return Err(bad(
            format!("shift_git push: `{branch}` is a protected branch (the default branch, main or master) — push a work branch instead"),
            "branch",
            json!(branch),
        ));
    }
    dash(V, "branch", branch)?;
    check_branch(cfg, V, branch).await?;
    let raw = arg(p, V, "path")?;
    let path = gate(cfg, V, raw).await?;
    Ok((branch.to_string(), path))
}

async fn push_inner(
    cfg: &Cfg,
    p: &Value,
    id: &AgentId,
    kernel: &Arc<Kernel>,
    slot: &mut Option<Auth>,
) -> Result<Value, Value> {
    const V: &str = "push";
    let (branch, path) = check_push(cfg, p).await?;
    let url = remote_url(cfg).await?;
    if rewrites_url(&path, &url).await {
        return Err(bad(REWRITES.to_string(), "remote", json!("[url withheld]")));
    }
    let auth = auth_for(cfg, &url, id, kernel).await?;
    *slot = auth;
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    run(
        cfg,
        &path,
        &[],
        &["push", cfg.remote.as_str(), refspec.as_str()],
        slot.as_ref(),
        V,
    )
    .await?;
    Ok(json!({"path": s(&path), "branch": branch, "remote": cfg.remote, "pushed": true}))
}

/// Push a work branch to the record's remote. Never forces; refuses the default branch.
pub(crate) async fn push(
    cfg: &Cfg,
    p: &Value,
    id: &AgentId,
    kernel: &Arc<Kernel>,
) -> Result<Value, Value> {
    let mut slot: Option<Auth> = None;
    let r = push_inner(cfg, p, id, kernel, &mut slot).await;
    finish(r, slot.as_ref())
}

async fn fetch_inner(
    cfg: &Cfg,
    id: &AgentId,
    kernel: &Arc<Kernel>,
    slot: &mut Option<Auth>,
) -> Result<Value, Value> {
    const V: &str = "fetch";
    let url = remote_url(cfg).await?;
    if rewrites_url(&cfg.repo_dir, &url).await {
        return Err(bad(REWRITES.to_string(), "remote", json!("[url withheld]")));
    }
    let auth = auth_for(cfg, &url, id, kernel).await?;
    *slot = auth;
    if looks_like_url(&cfg.remote) {
        run(
            cfg,
            &cfg.repo_dir,
            &[],
            &[
                "fetch",
                "--prune",
                cfg.remote.as_str(),
                "refs/heads/*:refs/remotes/shift/*",
            ],
            slot.as_ref(),
            V,
        )
        .await?;
    } else {
        run(
            cfg,
            &cfg.repo_dir,
            &[],
            &["fetch", "--prune", cfg.remote.as_str()],
            slot.as_ref(),
            V,
        )
        .await?;
    }
    Ok(json!({"remote": cfg.remote, "fetched": true}))
}

/// Fetch the record's remote. The token, if any, is used only inside the child.
pub(crate) async fn fetch(cfg: &Cfg, id: &AgentId, kernel: &Arc<Kernel>) -> Result<Value, Value> {
    let mut slot: Option<Auth> = None;
    let r = fetch_inner(cfg, id, kernel, &mut slot).await;
    finish(r, slot.as_ref())
}

/// Show the diffstat of a worktree against `base`, or against HEAD.
pub(crate) async fn diff_stat(cfg: &Cfg, p: &Value) -> Result<Value, Value> {
    const V: &str = "diff_stat";
    let raw = arg(p, V, "path")?;
    let path = gate(cfg, V, raw).await?;
    let (against, args): (String, Vec<String>) = match p.get("base").and_then(|v| v.as_str()) {
        Some(b) if !b.is_empty() => {
            dash(V, "base", b)?;
            (
                b.to_string(),
                vec!["diff".into(), "--stat".into(), format!("{b}...HEAD")],
            )
        }
        _ => (
            "HEAD".into(),
            vec!["diff".into(), "--stat".into(), "HEAD".into()],
        ),
    };
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let stat = run(cfg, &path, &[], &refs, None, V).await?;
    Ok(json!({"path": s(&path), "against": against, "stat": stat}))
}

/// The worktree's branch, whether it is clean, and its changed entries.
pub(crate) async fn status(cfg: &Cfg, p: &Value) -> Result<Value, Value> {
    const V: &str = "status";
    let raw = arg(p, V, "path")?;
    let path = gate(cfg, V, raw).await?;
    let out = run(
        cfg,
        &path,
        &[],
        &["status", "--porcelain=v1", "--branch"],
        None,
        V,
    )
    .await?;
    let mut branch = String::new();
    let mut empty = false;
    let mut entries: Vec<String> = Vec::new();
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            branch = rest.split("...").next().unwrap_or("").to_string();
            if let Some(name) = branch.strip_prefix("No commits yet on ") {
                branch = name.to_string();
                empty = true;
            }
        } else if !line.is_empty() {
            entries.push(line.to_string());
        }
    }
    Ok(json!({
        "path": s(&path),
        "branch": branch,
        "empty": empty,
        "clean": entries.is_empty(),
        "entries": entries,
    }))
}
