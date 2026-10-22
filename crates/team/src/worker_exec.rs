//! The exec record a team gives its workers.
//!
//! A worker runs its commands through a fantastic `exec_bridge.tools` agent
//! built from [`worker_exec_record`]. Its HOME is a sandbox folder inside the
//! team with no credentials in it; its environment is a fixed list of names,
//! and no token, key or SSH agent is among them; its `allowed` pattern refuses
//! forge CLIs (`gh`, `glab`, `hub`), `git push`, `git credential`, `git remote
//! set-url`, any `credential.helper` setting and `api.github.com`.
//!
//! The forge-CLI refusal is a clear, early error, NOT a wall: a shell can spell
//! any command in many ways. The boundary is that no credential is reachable
//! through the worker's environment or HOME, so a push from a worker fails.
//!
//! This is NOT an OS sandbox. The worker runs as the user, so files the user
//! can read (`~/.ssh` keys, the login keychain) stay readable to a worker that
//! looks for them. Only a separate user or a container closes that.

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// The fantastic bundle a worker's exec agent runs on.
pub const EXEC_HANDLER: &str = "exec_bridge.tools";
/// The worker's HOME, relative to the team folder.
pub const SANDBOX_HOME: &str = "sandbox/home";
/// Where a worker's commands run, relative to the team folder.
pub const WORK_DIR: &str = "work";

/// The ONLY names a worker's command inherits from the team's environment.
pub const INHERIT_ENV: &[&str] = &[
    "PATH", "LANG", "LC_ALL", "LC_CTYPE", "TERM", "TMPDIR", "TZ", "USER", "LOGNAME", "SHELL",
];

/// Names a worker's environment must never hold, whatever their value.
pub const FORBIDDEN_ENV: &[&str] = &[
    "SSH_AUTH_SOCK",
    "SSH_AGENT_PID",
    "SSH_ASKPASS",
    "GIT_ASKPASS",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "GITLAB_TOKEN",
    "GLAB_TOKEN",
];

/// Files and folders that would give a HOME a credential; a sandbox home holds none.
pub const CREDENTIAL_FILES: &[&str] = &[
    ".gitconfig",
    ".git-credentials",
    ".netrc",
    ".ssh",
    ".config/gh",
    ".config/git",
    ".config/glab-cli",
];

/// ssh for git with no config, no agent and no key files, never asking.
pub const NO_SSH: &str =
    "ssh -F /dev/null -o IdentityAgent=none -o IdentitiesOnly=yes -o IdentityFile=/dev/null -o BatchMode=yes";

/// The one `allowed` pattern: any command line EXCEPT one that names a forge CLI
/// (`gh`, `glab`, `hub` as a word), runs `git push`, `git credential` or
/// `git remote set-url` (options between `git` and the subcommand included),
/// mentions `credential.helper`, or reaches `api.github.com`.
pub const WORKER_ALLOWED: &str = r#"(?!.*(?:(?:^|[\s/'"`;&|(])(?:gh|glab|hub)(?:$|[\s'"`;&|)])|\bgit(?:\s+(?:-[Cc]\s+\S+|--?[\w.-]+(?:=\S*)?))*\s+(?:push|credential|remote\s+set-url)\b|credential\.helper|api\.github\.com)).*"#;

/// The worker's HOME inside the team folder.
pub fn sandbox_home(team_dir: &Path) -> PathBuf {
    team_dir.join(SANDBOX_HOME)
}

/// The folder a worker's commands run in, inside the team folder.
pub fn work_dir(team_dir: &Path) -> PathBuf {
    team_dir.join(WORK_DIR)
}

/// True for a name that must not reach a worker's environment: a listed forge
/// or SSH name, or one that looks like a secret.
pub fn forbidden_env_name(name: &str) -> bool {
    FORBIDDEN_ENV.contains(&name)
        || ["_TOKEN", "_KEY", "_SECRET", "_PASSWORD", "_PASS"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
        || name.contains("CREDENTIAL")
}

/// Make the sandbox home and the work dir, and refuse a home that already holds
/// a credential. It never deletes anything.
pub fn prepare_sandbox(team_dir: &Path) -> std::io::Result<PathBuf> {
    let home = sandbox_home(team_dir);
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(work_dir(team_dir))?;
    for name in CREDENTIAL_FILES {
        if home.join(name).exists() {
            return Err(std::io::Error::other(format!(
                "sandbox home {} holds {name} — a worker's HOME must hold no credentials; remove it",
                home.display()
            )));
        }
    }
    Ok(home)
}

/// The exec record for a worker. Pure: touches no disk. Call [`prepare_sandbox`] first.
pub fn worker_exec_record(team_dir: &Path) -> Value {
    let home = sandbox_home(team_dir);
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let mut env = Map::new();
    env.insert("HOME".into(), json!(s(&home)));
    env.insert("XDG_CONFIG_HOME".into(), json!(s(&home.join(".config"))));
    env.insert("GIT_CONFIG_NOSYSTEM".into(), json!("1"));
    env.insert("GIT_TERMINAL_PROMPT".into(), json!("0"));
    env.insert("GCM_INTERACTIVE".into(), json!("never"));
    env.insert("GIT_SSH_COMMAND".into(), json!(NO_SSH));
    json!({
        "handler_module": EXEC_HANDLER,
        "ingress_rule": "allow_all_ingress",
        "cwd": s(&work_dir(team_dir)),
        "allowed": [WORKER_ALLOWED],
        "inherit_env": INHERIT_ENV,
        "env": env,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fantastic_kernel::Bundle;

    fn argv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn refused(rec: &Value, argv: &[&str]) -> bool {
        fantastic_exec_bridge::command_permitted(
            rec.as_object().unwrap(),
            &fantastic_exec_bridge::shlex_join(&argv_owned(argv)),
        )
        .is_some()
    }

    fn argv_owned(v: &[&str]) -> Vec<String> {
        argv(v)
    }

    #[test]
    fn the_record_names_no_credential_variable() {
        let dir = tempfile::TempDir::new().unwrap();
        let rec = worker_exec_record(dir.path());
        let inherit = rec["inherit_env"].as_array().unwrap();
        assert!(!inherit.iter().any(|v| v.as_str() == Some("*")));
        for name in inherit {
            let name = name.as_str().unwrap();
            assert!(!forbidden_env_name(name), "{name} must not be inherited");
            assert!(!FORBIDDEN_ENV.contains(&name));
        }
        for key in rec["env"].as_object().unwrap().keys() {
            assert!(!forbidden_env_name(key), "{key} must not be set");
        }
    }

    #[test]
    fn the_record_points_home_into_the_team() {
        let dir = tempfile::TempDir::new().unwrap();
        let rec = worker_exec_record(dir.path());
        let home = sandbox_home(dir.path()).to_string_lossy().into_owned();
        let env = &rec["env"];
        assert_eq!(env["HOME"], json!(home));
        assert_eq!(env["XDG_CONFIG_HOME"], json!(format!("{home}/.config")));
        assert_eq!(env["GIT_CONFIG_NOSYSTEM"], json!("1"));
        assert_eq!(env["GIT_TERMINAL_PROMPT"], json!("0"));
        assert_eq!(
            rec["cwd"],
            json!(work_dir(dir.path()).to_string_lossy().into_owned())
        );
        assert_eq!(rec["handler_module"], json!("exec_bridge.tools"));
    }

    #[test]
    fn forbidden_names_are_recognised() {
        for name in [
            "GH_TOKEN",
            "SSH_AUTH_SOCK",
            "ANTHROPIC_API_KEY",
            "MY_SERVICE_TOKEN",
            "DB_PASSWORD",
            "AWS_SECRET",
            "GIT_CREDENTIAL_X",
        ] {
            assert!(forbidden_env_name(name), "{name}");
        }
        for name in ["PATH", "HOME", "GIT_SSH_COMMAND", "KEYBOARD"] {
            assert!(!forbidden_env_name(name), "{name}");
        }
    }

    #[test]
    fn prepare_sandbox_makes_an_empty_home_and_a_work_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = prepare_sandbox(dir.path()).unwrap();
        assert!(work_dir(dir.path()).is_dir());
        assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0);
        for name in CREDENTIAL_FILES {
            assert!(!home.join(name).exists(), "{name}");
        }
        assert!(prepare_sandbox(dir.path()).is_ok());
    }

    #[test]
    fn prepare_sandbox_refuses_a_home_holding_credentials() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = sandbox_home(dir.path());
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        let err = prepare_sandbox(dir.path()).unwrap_err();
        assert!(err.to_string().contains(".ssh"));
        assert!(home.join(".ssh").is_dir());
    }

    #[test]
    fn the_record_passes_exec_validation() {
        let dir = tempfile::TempDir::new().unwrap();
        let rec = worker_exec_record(dir.path());
        assert!(fantastic_exec_bridge::ExecBridgeBundle
            .validate_meta(rec.as_object().unwrap(), &Map::new())
            .is_none());
    }

    #[test]
    fn forge_commands_are_refused_by_the_allowlist() {
        let dir = tempfile::TempDir::new().unwrap();
        let rec = worker_exec_record(dir.path());
        for a in [
            &["gh", "--version"][..],
            &["/opt/homebrew/bin/gh", "pr", "list"],
            &["glab", "mr", "list"],
            &["hub", "pull-request"],
            &["git", "push"],
            &["git", "push", "origin", "main"],
            &["git", "-C", "/tmp/x", "push"],
            &["git", "--git-dir=/x/.git", "push"],
            &[
                "git",
                "remote",
                "set-url",
                "origin",
                "https://example.test/r.git",
            ],
            &["git", "-c", "credential.helper=store", "fetch"],
            &["git", "credential", "fill"],
            &["sh", "-c", "git push origin main"],
            &["sh", "-c", "cd x && git push"],
            &["sh", "-c", "gh auth token"],
            &["sh", "-c", "ls; gh repo list"],
            &["bash", "-lc", "echo hi\ngit push"],
            &["curl", "-s", "https://api.github.com/user"],
        ] {
            assert!(refused(&rec, a), "should be refused: {a:?}");
        }
    }

    #[test]
    fn ordinary_commands_are_allowed() {
        let dir = tempfile::TempDir::new().unwrap();
        let rec = worker_exec_record(dir.path());
        for a in [
            &["sh", "-c", "echo $HOME; env"][..],
            &["git", "status"],
            &["git", "commit", "-m", "push the fix"],
            &["git", "-C", "/x", "status"],
            &["git", "log", "--oneline"],
            &["git", "pull"],
            &["cargo", "test", "-p", "fantastic-team"],
            &["echo", "github"],
            &["grep", "-rn", "ghost", "."],
            &["python3", "-c", "print(1)"],
        ] {
            assert!(!refused(&rec, a), "should be allowed: {a:?}");
        }
    }
}
