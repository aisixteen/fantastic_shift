use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/check-secrets.sh")
}

fn run(dir: &Path) -> (i32, String) {
    let out = Command::new("sh")
        .arg(script())
        .arg(dir)
        .output()
        .expect("run check-secrets.sh");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn git_repo() -> TempDir {
    let d = TempDir::new().expect("tempdir");
    let st = Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(d.path())
        .status()
        .expect("git init");
    assert!(st.success());
    d
}

// Built from pieces, never as one literal, so this file passes its own check.
fn samples() -> Vec<(&'static str, String)> {
    vec![
        (
            "GitHub token",
            format!("{}{}{}", "gh", "p_", "A".repeat(36)),
        ),
        (
            "GitHub token",
            format!("{}{}{}", "gh", "s_", "B".repeat(36)),
        ),
        (
            "GitHub fine-grained token",
            format!("{}{}{}", "github", "_pat_", "C".repeat(30)),
        ),
        (
            "Anthropic key",
            format!(
                "{}{}{}",
                "sk-",
                "ant-",
                "api03-".to_string() + &"D".repeat(30)
            ),
        ),
        (
            "private key",
            format!("{}{}", "-----", "BEGIN OPENSSH PRIVATE KEY-----"),
        ),
        (
            "token in a URL",
            format!(
                "https://{}{}{}@github.com/o/r.git",
                "x-access",
                "-token:",
                "E".repeat(20)
            ),
        ),
    ]
}

#[test]
fn a_clean_tree_passes() {
    let d = git_repo();
    std::fs::write(
        d.path().join("hello.txt"),
        "hello\nthe gh cli and an sk-ant- prefix, named\n",
    )
    .unwrap();
    let (code, err) = run(d.path());
    assert_eq!(code, 0, "{err}");
}

#[test]
fn every_planted_secret_is_caught() {
    for (kind, token) in samples() {
        let d = git_repo();
        std::fs::write(
            d.path().join("leak.txt"),
            format!("line one\nkey = {}\n", token),
        )
        .unwrap();
        let (code, err) = run(d.path());
        assert_eq!(code, 1, "{kind}: {err}");
        assert!(err.contains("leak.txt:2"), "{kind}: {err}");
        assert!(err.contains(kind), "{kind}: {err}");
        assert!(
            !err.contains(&token),
            "{kind}: the match leaked into stderr"
        );
    }
}

#[test]
fn a_dir_that_is_not_a_repo_is_refused() {
    let d = TempDir::new().unwrap();
    let (code, _) = run(d.path());
    assert_eq!(code, 2);
}

#[test]
fn this_repo_is_clean() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let (code, err) = run(root.as_path());
    assert_eq!(code, 0, "{err}");
}
