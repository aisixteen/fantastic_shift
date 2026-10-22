//! Path rules: every worktree path stays inside `workspace_root`, and a
//! verb that works on a worktree first checks that the path really is one of
//! `repo_dir`'s worktrees.

use std::path::{Component, Path, PathBuf};

use serde_json::{json, Value};

use crate::git;
use crate::{bad, Cfg};

/// Refusal: the path is outside the workspace root.
pub(crate) fn outside(verb: &str, path: &str, root: &Path) -> Value {
    bad(
        format!(
            "shift_git {verb}: path `{path}` is outside the workspace root `{}`",
            root.display()
        ),
        "path",
        json!(path),
    )
}

/// Refusal: the path is not a worktree of `repo_dir`.
pub(crate) fn not_worktree(verb: &str, path: &str, repo_dir: &Path) -> Value {
    bad(
        format!(
            "shift_git {verb}: `{path}` is not a worktree of `{}`",
            repo_dir.display()
        ),
        "path",
        json!(path),
    )
}

/// Resolve `raw` against the root and clamp it. Returns the canonical path,
/// which must be strictly inside the canonical root.
/// Resolve `raw` under `root` and check it stays strictly inside it.
pub fn inside_root(root: &Path, raw: &str, verb: &str) -> Result<PathBuf, Value> {
    if Path::new(raw)
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(outside(verb, raw, root));
    }
    let abs = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        root.join(raw)
    };
    let canon_root = root.canonicalize().map_err(|_| {
        json!({"error": format!("shift_git: workspace_root `{}` does not exist", root.display())})
    })?;
    // Canonicalize the nearest existing ancestor, then re-join the missing tail.
    let mut existing = abs.clone();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(name) => tail.push(name.to_os_string()),
            None => break,
        }
        if !existing.pop() {
            break;
        }
    }
    let mut resolved = existing.canonicalize().unwrap_or(existing);
    for part in tail.iter().rev() {
        resolved.push(part);
    }
    if !resolved.starts_with(&canon_root) || resolved == canon_root {
        return Err(outside(verb, raw, root));
    }
    Ok(resolved)
}

/// True when `path` is a worktree of `cfg.repo_dir`: same git common dir, and
/// its own top level is `path`. Any git failure is false.
/// True when `path` is a worktree of `cfg.repo_dir`.
pub async fn is_worktree_of(cfg: &Cfg, path: &Path) -> bool {
    let common_here = git::common_dir(path).await;
    let common_repo = git::common_dir(&cfg.repo_dir).await;
    let top = git::toplevel(path).await;
    match (common_here, common_repo, top) {
        (Some(a), Some(b), Some(t)) => {
            a == b && t == path.canonicalize().unwrap_or(path.to_path_buf())
        }
        _ => false,
    }
}
