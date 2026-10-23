//! App shell for the headless team binary's web UI (#37, part a).
//! No logic here: resolve a target, start or attach, show its URL.

use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// The headless binary name (`[[bin]] name` in crates/team/Cargo.toml).
pub const HEADLESS_BINARY: &str = "fantastic_team";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Start { binary: String, port: u16 },
    Attach { url: String },
}

/// A port the OS says is free on loopback right now.
pub fn free_port() -> std::io::Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

fn next_value<'a, I: Iterator<Item = &'a String>>(
    it: &mut I,
    flag: &str,
) -> Result<String, String> {
    it.next()
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// Parse `--attach <url>` / `--port <n>` / `--binary <path>`.
/// With no flags: start the headless binary on a free port.
pub fn target_from_args(args: &[String]) -> Result<Target, String> {
    let mut attach = None;
    let mut port: Option<u16> = None;
    let mut binary = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--attach" => attach = Some(next_value(&mut it, arg)?),
            "--port" => {
                let v = next_value(&mut it, arg)?;
                let p: u16 = v.parse().map_err(|_| format!("bad --port: {v}"))?;
                if p == 0 {
                    return Err("--port must be 1-65535".into());
                }
                port = Some(p);
            }
            "--binary" => binary = Some(next_value(&mut it, arg)?),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if let Some(url) = attach {
        if port.is_some() || binary.is_some() {
            return Err("--attach cannot be combined with --port or --binary".into());
        }
        validate_loopback(&url)?;
        return Ok(Target::Attach { url });
    }
    let port = match port {
        Some(p) => p,
        None => free_port().map_err(|e| e.to_string())?,
    };
    Ok(Target::Start {
        binary: binary.unwrap_or_else(|| HEADLESS_BINARY.to_string()),
        port,
    })
}

/// Split `http://authority/path` into (authority, path).
fn split_http(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("http://")?;
    Some(match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    })
}

/// Attach URLs must be plain http on loopback (tunnel/relay = loopback).
pub fn validate_loopback(url: &str) -> Result<(), String> {
    let (auth, _) =
        split_http(url).ok_or_else(|| "attach URL must start with http://".to_string())?;
    let host = if let Some(rest) = auth.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        auth.split(':').next().unwrap_or("")
    };
    if host == "127.0.0.1" || host == "localhost" || host == "::1" {
        Ok(())
    } else {
        Err(format!(
            "attach URL host must be 127.0.0.1 or localhost, got {host}"
        ))
    }
}

/// The headless command line: `--port N` and nothing else.
pub fn start_args(port: u16) -> Vec<String> {
    vec!["--port".to_string(), port.to_string()]
}

/// The web UI URL the window should show.
pub fn web_url(target: &Target) -> String {
    match target {
        Target::Start { port, .. } => format!("http://127.0.0.1:{port}/"),
        Target::Attach { url } => url.clone(),
    }
}

async fn http_status(addr: &str, host: &str, path: &str) -> std::io::Result<u16> {
    let mut s = TcpStream::connect(addr).await?;
    let req = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await?;
    String::from_utf8_lossy(&buf)
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| std::io::Error::other("bad HTTP response"))
}

/// Poll `url` until it answers 200 or `timeout` passes.
pub async fn wait_ready(url: &str, timeout: Duration) -> Result<(), String> {
    let (auth, path) = split_http(url).ok_or_else(|| "not an http:// URL".to_string())?;
    let addr = if auth.contains(':') {
        auth.to_string()
    } else {
        format!("{auth}:80")
    };
    let deadline = Instant::now() + timeout;
    loop {
        let attempt = tokio::time::timeout(Duration::from_secs(2), http_status(&addr, auth, path));
        if let Ok(Ok(200)) = attempt.await {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("no 200 from {url} within {timeout:?}"));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn default_is_start_on_a_free_port() {
        match target_from_args(&[]).unwrap() {
            Target::Start { binary, port } => {
                assert_eq!(binary, HEADLESS_BINARY);
                assert_ne!(port, 0);
            }
            other => panic!("expected Start, got {other:?}"),
        }
    }

    #[test]
    fn attach_parses_a_loopback_url() {
        let t = target_from_args(&args(&["--attach", "http://127.0.0.1:8080/"])).unwrap();
        assert_eq!(
            t,
            Target::Attach {
                url: "http://127.0.0.1:8080/".into()
            }
        );
        assert!(target_from_args(&args(&["--attach", "http://localhost:9/"])).is_ok());
        assert!(target_from_args(&args(&["--attach", "http://[::1]:8080/"])).is_ok());
    }

    #[test]
    fn attach_refuses_a_non_loopback_url() {
        assert!(target_from_args(&args(&["--attach", "http://example.com/"])).is_err());
        assert!(target_from_args(&args(&["--attach", "https://127.0.0.1/"])).is_err());
    }

    #[test]
    fn start_args_are_only_the_port() {
        assert_eq!(start_args(4321), args(&["--port", "4321"]));
    }

    #[tokio::test]
    async fn wait_ready_sees_a_local_server() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = s.read(&mut buf).await;
                    let _ = s
                        .write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nok")
                        .await;
                    let _ = s.shutdown().await;
                });
            }
        });
        wait_ready(&format!("http://127.0.0.1:{port}/"), Duration::from_secs(5))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn wait_ready_times_out_on_a_closed_port() {
        let port = free_port().unwrap();
        let r = wait_ready(
            &format!("http://127.0.0.1:{port}/"),
            Duration::from_millis(300),
        )
        .await;
        assert!(r.is_err());
    }
}
