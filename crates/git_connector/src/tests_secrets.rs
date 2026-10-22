//! Secret tests: the HTTPS token never leaves the agent. Uses a fake HTTP
//! server on 127.0.0.1 that the test itself runs.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Value};

use crate::git::{redact, Auth};
use crate::tests::{fx, sh_git};

const TOKEN: &str = "ghp_FAKEsecret0123456789abcdefABCDEF";

/// A 401 server on 127.0.0.1. Stores each request head it receives.
fn fake_forge() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut stream) = conn else { break };
            stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
            let mut head = Vec::new();
            let mut buf = [0u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => head.extend_from_slice(&buf[..n]),
                }
            }
            seen2
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&head).into_owned());
            let _ = stream.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"shift\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    (port, seen)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_https_token_never_leaves_the_agent() {
    let (port, seen) = fake_forge();
    let url = format!("http://127.0.0.1:{port}/org/repo.git");
    let f = fx(
        json!({"remote": url, "username": "shift-bot"}),
        Some((&url, TOKEN)),
    )
    .await;
    let mut replies: Vec<Value> = Vec::new();
    replies.push(f.call(json!({"type": "reflect"})).await);
    replies.push(
        f.call(json!({"type": "worktree_add", "path": "w", "branch": "work/secret", "base": "origin/main"}))
            .await,
    );
    std::fs::write(f.ws.join("w").join("secret.txt"), "s\n").unwrap();
    replies.push(
        f.call(json!({"type": "commit", "path": "w", "message": "s"}))
            .await,
    );
    replies.push(f.call(json!({"type": "status", "path": "w"})).await);
    replies.push(f.call(json!({"type": "diff_stat", "path": "w"})).await);
    replies.push(
        f.call(json!({"type": "push", "path": "w", "branch": "work/secret"}))
            .await,
    );
    replies.push(f.call(json!({"type": "fetch"})).await);
    replies.push(
        f.call(json!({"type": "push", "path": "w", "branch": "main"}))
            .await,
    );

    // 1. No reply carries the token.
    for r in &replies {
        assert!(!serde_json::to_string(r).unwrap().contains(TOKEN), "{r}");
    }
    // 2. The forge saw the token in the Basic header, and only there.
    let want = base64::engine::general_purpose::STANDARD.encode(format!("shift-bot:{TOKEN}"));
    let heads = seen.lock().unwrap().clone();
    assert!(
        heads
            .iter()
            .any(|h| h.contains(&format!("Authorization: Basic {want}"))),
        "forge saw no auth: {heads:?}"
    );
    // 3. No git config or config.worktree holds the token.
    let mut config_files = Vec::new();
    collect_configs(&f.repo.join(".git"), &mut config_files);
    for path in config_files {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(!text.contains(TOKEN), "token in {}", path.display());
    }
    assert!(!sh_git(&f.repo, &["config", "--list", "--show-origin"]).contains(TOKEN));
    // 4. Every row about the git agent, except the credential store's own
    // `get` reply (which fantastic records in clear), is token-free.
    let rows =
        fantastic_kernel::test_support::rows_about(&f.k, &fantastic_kernel::AgentId::from("git"));
    let mut skipped = 0;
    for row in &rows {
        let text = serde_json::to_string(row).unwrap();
        if text.contains("\"api_key\"") {
            skipped += 1;
            continue;
        }
        assert!(!text.contains(TOKEN), "token in a row: {text}");
    }
    eprintln!("secrets test: skipped {skipped} credential-store get rows");
}

fn collect_configs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_configs(&p, out);
        } else if matches!(
            p.file_name().and_then(|n| n.to_str()),
            Some("config") | Some("config.worktree")
        ) {
            out.push(p);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn plain_http_to_a_remote_host_is_refused() {
    let url = "http://example.com/org/repo.git";
    let f = fx(json!({"remote": url}), Some((url, TOKEN))).await;
    f.call(json!({"type": "worktree_add", "path": "w", "branch": "work/p", "base": "origin/main"}))
        .await;
    std::fs::write(f.ws.join("w").join("p.txt"), "p\n").unwrap();
    f.call(json!({"type": "commit", "path": "w", "message": "p"}))
        .await;
    let r = f
        .call(json!({"type": "push", "path": "w", "branch": "work/p"}))
        .await;
    assert_eq!(
        r["error"],
        "shift_git: credentials are sent only over https (or http to 127.0.0.1, ::1 or localhost) — refused"
    );
    assert!(!serde_json::to_string(&r).unwrap().contains(TOKEN));
}

#[test]
fn redact_hides_the_token() {
    let auth = Auth {
        username: "u".into(),
        token: TOKEN.into(),
    };
    assert_eq!(redact(&format!("x {TOKEN} y"), Some(&auth)), "x [secret] y");
    assert_eq!(redact("x y", None), "x y");
}
