//! Behaviour tests on a temp bare remote, a temp clone and a temp workspace.
//! No network: the remote is a path on disk.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};

use fantastic_kernel::bootstrap::{self, BootstrapOptions};
use fantastic_kernel::bundle::Bundle as _;
use fantastic_kernel::test_support::register_root_loader;
use fantastic_kernel::{AgentId, BundleRegistry, Item, Kernel};
use serde_json::{json, Value};
use tempfile::TempDir;

use crate::GitConnectorBundle;

pub(crate) const STORE: &str = "creds";

static ISOLATE: Once = Once::new();

/// Keep the user's git config (helpers, hooks, signing, insteadOf) out of tests.
pub(crate) fn isolate() {
    ISOLATE.call_once(|| {
        let p = std::env::temp_dir().join("shift-git-tests.gitconfig");
        std::fs::write(&p, "").unwrap();
        std::env::set_var("GIT_CONFIG_GLOBAL", &p);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    });
}

/// Run git synchronously in `dir`, assert success, return trimmed stdout.
pub(crate) fn sh_git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Seed", "-c", "user.email=seed@shift.test"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub(crate) struct Fx {
    pub k: Arc<Kernel>,
    pub tmp: TempDir,
    pub bare: PathBuf,
    pub repo: PathBuf,
    pub ws: PathBuf,
}

/// A kernel with the git agent `git`, its temp remote, clone and workspace.
pub(crate) async fn fx(extra: Value, token: Option<(&str, &str)>) -> Fx {
    isolate();
    let tmp = TempDir::new().unwrap();
    let bare = tmp.path().join("remote.git");
    let seed = tmp.path().join("seed");
    let repo = tmp.path().join("repo");
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    sh_git(
        tmp.path(),
        &["init", "--bare", "-b", "main", bare.to_str().unwrap()],
    );
    sh_git(tmp.path(), &["init", "-b", "main", seed.to_str().unwrap()]);
    std::fs::write(seed.join("README.md"), "hello\n").unwrap();
    sh_git(&seed, &["add", "-A"]);
    sh_git(&seed, &["commit", "-m", "init"]);
    sh_git(&seed, &["push", bare.to_str().unwrap(), "main"]);
    sh_git(
        tmp.path(),
        &["clone", bare.to_str().unwrap(), repo.to_str().unwrap()],
    );

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
    reg.register(crate::HANDLER_MODULE, GitConnectorBundle);
    let booted = bootstrap::bootstrap(reg, BootstrapOptions::in_memory()).unwrap();
    let k = booted.kernel.clone();
    k.agents
        .get(&AgentId::from(bootstrap::DEFAULT_ROOT_ID))
        .unwrap()
        .meta
        .write()
        .unwrap()
        .insert("guid".to_string(), json!("g1"));

    let mut meta = json!({
        "repo_dir": repo.to_str().unwrap(),
        "workspace_root": ws.to_str().unwrap(),
        "remote": "origin",
        "default_branch": "main",
        "author_name": "Shift Bot",
        "author_email": "bot@shift.test",
    });
    if let Value::Object(extra) = extra {
        for (key, value) in extra {
            if value.is_null() {
                meta.as_object_mut().unwrap().remove(&key);
            } else {
                meta[key] = value;
            }
        }
    }
    if let Some((url, value)) = token {
        fantastic_credential_store::test_support::set_key(
            &k,
            tmp.path(),
            "git",
            "git/https",
            url,
            value,
        )
        .await;
        fantastic_credential_store::test_support::add_credentials(
            meta.as_object_mut().unwrap(),
            "git/https",
        );
    }
    // create_agent takes the record's fields at the top level, next to the verb.
    let mut payload = meta.clone();
    payload["type"] = json!("create_agent");
    payload["parent_id"] = json!(bootstrap::DEFAULT_ROOT_ID);
    payload["handler_module"] = json!(crate::HANDLER_MODULE);
    payload["id"] = json!("git");
    let reply = k
        .send(vec![Item::new("kernel", payload)])
        .await
        .remove(0)
        .result;
    assert_eq!(reply["id"], "git", "create_agent: {reply}");
    Fx {
        k,
        tmp,
        bare,
        repo,
        ws,
    }
}

impl Fx {
    /// Send one payload to the git agent and return its reply.
    pub(crate) async fn call(&self, payload: Value) -> Value {
        self.k
            .send(vec![Item::new(AgentId::from("git"), payload)])
            .await
            .remove(0)
            .result
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn worktree_add_makes_a_branch_from_base() {
    let f = fx(json!({}), None).await;
    let r = f
        .call(json!({"type": "worktree_add", "path": "w1", "branch": "work/one", "base": "origin/main"}))
        .await;
    assert_eq!(r["created"], true, "{r}");
    let expect = f.ws.canonicalize().unwrap().join("w1");
    assert_eq!(r["path"], expect.to_str().unwrap());
    assert_eq!(
        sh_git(&expect, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "work/one"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn status_and_diff_stat_show_a_change() {
    let f = fx(json!({}), None).await;
    f.call(
        json!({"type": "worktree_add", "path": "w1", "branch": "work/one", "base": "origin/main"}),
    )
    .await;
    let w1 = f.ws.join("w1");
    std::fs::write(w1.join("README.md"), "hello\nmore\n").unwrap();
    let st = f.call(json!({"type": "status", "path": "w1"})).await;
    assert_eq!(st["branch"], "work/one", "{st}");
    assert_eq!(st["clean"], false);
    assert!(st["entries"][0].as_str().unwrap().ends_with("README.md"));
    let d = f.call(json!({"type": "diff_stat", "path": "w1"})).await;
    assert_eq!(d["against"], "HEAD");
    assert!(d["stat"].as_str().unwrap().contains("README.md"));
}

#[tokio::test(flavor = "multi_thread")]
async fn commit_signs_off_with_the_identity_and_trailers() {
    let f = fx(json!({}), None).await;
    f.call(
        json!({"type": "worktree_add", "path": "w1", "branch": "work/one", "base": "origin/main"}),
    )
    .await;
    let w1 = f.ws.join("w1");
    std::fs::write(w1.join("new.txt"), "x\n").unwrap();
    let c = f
        .call(json!({"type": "commit", "path": "w1", "message": "add new",
                     "trailers": ["Co-Authored-By: Bot <bot@example.com>"]}))
        .await;
    let sha = c["commit"].as_str().unwrap();
    assert!(
        sha.len() == 40 && sha.chars().all(|ch| ch.is_ascii_hexdigit()),
        "{c}"
    );
    let body = sh_git(&w1, &["log", "-1", "--format=%B"]);
    assert!(body.contains("Signed-off-by: Shift Bot <bot@shift.test>"));
    assert!(body.contains("Co-Authored-By: Bot <bot@example.com>"));
    assert_eq!(
        sh_git(&w1, &["log", "-1", "--format=%an <%ae>"]),
        "Shift Bot <bot@shift.test>"
    );
    let again = f
        .call(json!({"type": "commit", "path": "w1", "message": "again"}))
        .await;
    assert_eq!(
        again["error"],
        "shift_git commit: nothing to commit in `w1`"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn push_sends_a_work_branch() {
    let f = fx(json!({}), None).await;
    f.call(
        json!({"type": "worktree_add", "path": "w1", "branch": "work/one", "base": "origin/main"}),
    )
    .await;
    let w1 = f.ws.join("w1");
    std::fs::write(w1.join("new.txt"), "x\n").unwrap();
    let c = f
        .call(json!({"type": "commit", "path": "w1", "message": "add"}))
        .await;
    let p = f
        .call(json!({"type": "push", "path": "w1", "branch": "work/one"}))
        .await;
    assert_eq!(p["pushed"], true, "{p}");
    let remote = sh_git(&f.bare, &["rev-parse", "refs/heads/work/one"]);
    assert_eq!(remote, c["commit"].as_str().unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_updates_the_remote_refs() {
    let f = fx(json!({}), None).await;
    let other = f.tmp.path().join("other");
    sh_git(
        f.tmp.path(),
        &["clone", f.bare.to_str().unwrap(), other.to_str().unwrap()],
    );
    std::fs::write(other.join("later.txt"), "y\n").unwrap();
    sh_git(&other, &["add", "-A"]);
    sh_git(&other, &["commit", "-m", "later"]);
    sh_git(&other, &["push", "origin", "main"]);
    let want = sh_git(&other, &["rev-parse", "HEAD"]);
    let r = f.call(json!({"type": "fetch"})).await;
    assert_eq!(r["fetched"], true, "{r}");
    assert_eq!(sh_git(&f.repo, &["rev-parse", "origin/main"]), want);
}

#[tokio::test(flavor = "multi_thread")]
async fn worktree_remove_removes_it() {
    let f = fx(json!({}), None).await;
    f.call(
        json!({"type": "worktree_add", "path": "w1", "branch": "work/one", "base": "origin/main"}),
    )
    .await;
    let r = f
        .call(json!({"type": "worktree_remove", "path": "w1"}))
        .await;
    assert_eq!(r["removed"], true, "{r}");
    assert!(!f.ws.join("w1").exists());
    assert!(!sh_git(&f.repo, &["worktree", "list"]).contains("work/one"));
}

#[tokio::test(flavor = "multi_thread")]
async fn push_refuses_the_protected_branches() {
    let f = fx(json!({"default_branch": "develop"}), None).await;
    f.call(
        json!({"type": "worktree_add", "path": "w1", "branch": "work/one", "base": "origin/main"}),
    )
    .await;
    for b in ["develop", "main", "master"] {
        let r = f
            .call(json!({"type": "push", "path": "w1", "branch": b}))
            .await;
        assert_eq!(
            r["error"],
            format!("shift_git push: `{b}` is a protected branch (the default branch, main or master) — push a work branch instead")
        );
    }
    assert_eq!(
        sh_git(&f.bare, &["for-each-ref", "refs/heads"])
            .lines()
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn push_never_forces() {
    let f = fx(json!({}), None).await;
    f.call(
        json!({"type": "worktree_add", "path": "w1", "branch": "work/one", "base": "origin/main"}),
    )
    .await;
    let a = f
        .call(json!({"type": "push", "path": "w1", "branch": "work/one", "force": true}))
        .await;
    assert_eq!(
        a["error"],
        "shift_git push: a force push is never made — push a new branch name instead"
    );
    let b = f
        .call(json!({"type": "push", "path": "w1", "branch": "+work/one"}))
        .await;
    assert_eq!(
        b["error"],
        "shift_git push: a force push is never made — push a new branch name instead"
    );
    assert!(sh_git(&f.bare, &["for-each-ref", "refs/heads"])
        .lines()
        .all(|l| !l.ends_with("work/one")));
}

#[tokio::test(flavor = "multi_thread")]
async fn paths_outside_the_root_are_refused() {
    let f = fx(json!({}), None).await;
    f.call(
        json!({"type": "worktree_add", "path": "w1", "branch": "work/one", "base": "origin/main"}),
    )
    .await;
    #[cfg(unix)]
    std::os::unix::fs::symlink(f.tmp.path(), f.ws.join("link")).unwrap();
    let mut bad_paths = vec!["../escape", "/tmp"];
    #[cfg(unix)]
    bad_paths.push("link");
    for bad_path in bad_paths {
        let checks = [
            json!({"type": "status", "path": bad_path}),
            json!({"type": "commit", "path": bad_path, "message": "m"}),
            json!({"type": "push", "path": bad_path, "branch": "work/x"}),
            json!({"type": "diff_stat", "path": bad_path}),
            json!({"type": "worktree_remove", "path": bad_path}),
            json!({"type": "worktree_add", "path": bad_path, "branch": "work/x", "base": "origin/main"}),
        ];
        for payload in checks {
            let verb = payload["type"].as_str().unwrap().to_string();
            let r = f.call(payload).await;
            assert_eq!(
                r["error"],
                format!(
                    "shift_git {verb}: path `{bad_path}` is outside the workspace root `{}`",
                    f.ws.canonicalize().unwrap().display()
                ),
                "{verb} {bad_path}"
            );
        }
    }
    let plain = f.ws.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    sh_git(&plain, &["init", "-b", "main"]);
    let r = f.call(json!({"type": "status", "path": "plain"})).await;
    assert_eq!(
        r["error"],
        format!(
            "shift_git status: `plain` is not a worktree of `{}`",
            f.repo.display()
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn options_cannot_be_injected() {
    let f = fx(json!({}), None).await;
    let a = f
        .call(json!({"type": "worktree_add", "path": "w1", "branch": "-D", "base": "origin/main"}))
        .await;
    assert_eq!(
        a["error"],
        "shift_git worktree_add: `branch` must not start with `-`"
    );
    assert_eq!(a["reason"], "bad_arg");
    let b = f
        .call(json!({"type": "worktree_add", "path": "w2", "branch": "work/x", "base": "--upload-pack=x"}))
        .await;
    assert_eq!(
        b["error"],
        "shift_git worktree_add: `base` must not start with `-`"
    );
    let c = f
        .call(
            json!({"type": "worktree_add", "path": "w3", "branch": "a..b", "base": "origin/main"}),
        )
        .await;
    assert_eq!(
        c["error"],
        "shift_git worktree_add: `a..b` is not a valid branch name"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_record_is_validated() {
    isolate();
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let base = json!({
        "repo_dir": repo.to_str().unwrap(),
        "workspace_root": tmp.path().to_str().unwrap(),
        "remote": "origin",
        "default_branch": "main",
    });
    let cases: Vec<(Value, &str)> = vec![
        (json!({"repo_dir": null}), "missing:"),
        (json!({"workspace_root": "rel/dir"}), "`workspace_root` must be an absolute path"),
        (
            json!({"remote": "https://bob:hunter2@example.com/r.git"}),
            "shift_git: the remote URL carries a user or password — remove it and name a credential store ref in credentials instead",
        ),
        (json!({"author_name": "A"}), "`author_name` and `author_email` go together"),
        (json!({"timeout_s": 0}), "`timeout_s` must be a positive int"),
        (json!({"credentials": {"store": "creds"}}), "credentials"),
    ];
    for (over, expect) in cases {
        let mut meta = base.clone();
        for (k, v) in over.as_object().unwrap() {
            if v.is_null() {
                meta.as_object_mut().unwrap().remove(k);
            } else {
                meta[k] = v.clone();
            }
        }
        let empty = serde_json::Map::new();
        let err = GitConnectorBundle
            .validate_meta(meta.as_object().unwrap(), &empty)
            .unwrap_or_default();
        assert!(err.contains(expect), "expected `{expect}` in `{err}`");
        assert!(!err.contains("hunter2"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_arg_and_an_unknown_verb_are_refused() {
    let f = fx(json!({}), None).await;
    let r = f.call(json!({"type": "push", "path": "w1"})).await;
    assert_eq!(r["reason"], "missing_arg");
    assert_eq!(r["arg"], "branch");
    let u = f.call(json!({"type": "rebase"})).await;
    assert_eq!(u["reason"], "unhandled");
}
