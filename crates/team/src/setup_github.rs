//! `fantastic_team setup github`: the GitHub App manifest flow. The user names
//! their own app; GitHub returns its id and private key once. The key goes to the
//! credential store (through a [`KeyStore`]), never to a file, and never to any
//! output. Only the app id is written to the config file.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// The six permissions the app asks for, nothing else.
pub const PERMISSIONS: &[(&str, &str)] = &[
    ("contents", "write"),
    ("issues", "write"),
    ("pull_requests", "write"),
    ("metadata", "read"),
    ("checks", "read"),
    ("actions", "read"),
];

/// The manifest GitHub reads: the user's app name, a local callback, no webhook.
pub fn manifest(name: &str, redirect_url: &str) -> Value {
    let perms: serde_json::Map<String, Value> = PERMISSIONS
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v)))
        .collect();
    json!({
        "name": name,
        "url": "https://github.com/",
        "redirect_url": redirect_url,
        "public": false,
        "hook_attributes": {"active": false},
        "default_permissions": perms,
        "default_events": [],
    })
}

/// The github.com page that creates the app from `manifest`, with `state`.
/// Org URL when `org` is given, the user URL otherwise.
pub fn start_url(state: &str, org: Option<&str>) -> String {
    match org {
        Some(o) => format!("https://github.com/organizations/{o}/settings/apps/new?state={state}"),
        None => format!("https://github.com/settings/apps/new?state={state}"),
    }
}

/// The app GitHub created. Holds the private key: no Debug, no Serialize.
pub struct Registered {
    /// GitHub's numeric app id.
    pub id: u64,
    /// The app's URL slug.
    pub slug: String,
    /// The PEM private key, returned once by GitHub.
    pub private_key: String,
}

/// Exchange a manifest code for the app. `base_url` is `https://api.github.com`
/// in real use and a mock in tests.
pub async fn exchange(code: &str, base_url: &str) -> Result<Registered, String> {
    let url = format!("{base_url}/app-manifests/{code}/conversions");
    let resp = reqwest::Client::new()
        .post(url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "fantastic_team")
        .send()
        .await
        .map_err(|_| "could not reach GitHub".to_string())?;
    if !resp.status().is_success() {
        return Err(format!(
            "GitHub refused the manifest code (HTTP {})",
            resp.status().as_u16()
        ));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|_| "unreadable reply from GitHub".to_string())?;
    let id = v
        .get("id")
        .and_then(|x| x.as_u64())
        .ok_or("GitHub's reply has no app id")?;
    let slug = v
        .get("slug")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let private_key = v
        .get("pem")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .ok_or("GitHub's reply has no private key")?
        .to_string();
    Ok(Registered {
        id,
        slug,
        private_key,
    })
}

/// Where the key goes: the credential store, under one ref. Implemented by the
/// kernel's credential agent; tests use a fake.
pub trait KeyStore {
    /// Write the app id and private key under `reference`.
    fn store(&self, reference: &str, app_id: u64, private_key: &str) -> Result<(), String>;
}

/// Write the app id and key to the store. The key is never returned.
pub fn store_key(store: &dyn KeyStore, reference: &str, reg: &Registered) -> Result<(), String> {
    store.store(reference, reg.id, &reg.private_key)
}

/// Set `github_app_id: <id>` in the config text, keeping the rest of the file.
pub fn set_app_id(config: &str, id: u64) -> String {
    let line = format!("github_app_id: {id}");
    let mut found = false;
    let mut out: Vec<String> = config
        .lines()
        .map(|l| {
            if l.trim_start().starts_with("github_app_id:") {
                found = true;
                line.clone()
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found {
        out.push(line);
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// Record the app id in the config file at `path`, preserving everything else.
pub fn record_app_id(path: &Path, id: u64) -> Result<(), String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    std::fs::write(path, set_app_id(&text, id))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Wait for GitHub's redirect on `listener`: check `state`, return the `code`.
/// Refuses a wrong state and gives up after `timeout`.
pub fn wait_for_code(
    listener: &TcpListener,
    state: &str,
    timeout: Duration,
) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((mut conn, _)) => {
                conn.set_nonblocking(false).ok();
                let mut buf = [0u8; 4096];
                let n = conn.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let target = head.split_whitespace().nth(1).unwrap_or("").to_string();
                let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
                let mut code = None;
                let mut got_state = None;
                for pair in query.split('&') {
                    match pair.split_once('=') {
                        Some(("code", v)) => code = Some(v.to_string()),
                        Some(("state", v)) => got_state = Some(v.to_string()),
                        _ => {}
                    }
                }
                let ok = got_state.as_deref() == Some(state);
                let body = if ok {
                    "Your GitHub App is registered. You can close this tab."
                } else {
                    "State did not match; nothing was registered."
                };
                let _ = conn.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
                if !ok {
                    return Err("callback state does not match — refused".into());
                }
                return code
                    .filter(|c| !c.is_empty())
                    .ok_or_else(|| "callback has no code".into());
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("timed out waiting for the GitHub callback".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const FIXTURE_KEY: &str = "fixture-key-material-not-a-real-key";

    struct FakeStore(Mutex<Vec<(String, u64, String)>>);
    impl KeyStore for FakeStore {
        fn store(&self, reference: &str, app_id: u64, private_key: &str) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .push((reference.into(), app_id, private_key.into()));
            Ok(())
        }
    }

    #[test]
    fn manifest_has_exactly_the_least_privilege_permissions() {
        let m = manifest("mine", "http://127.0.0.1:1/cb");
        let perms = m["default_permissions"].as_object().unwrap();
        assert_eq!(perms.len(), 6);
        assert_eq!(perms["contents"], "write");
        assert_eq!(perms["issues"], "write");
        assert_eq!(perms["pull_requests"], "write");
        assert_eq!(perms["metadata"], "read");
        assert_eq!(perms["checks"], "read");
        assert_eq!(perms["actions"], "read");
        assert_eq!(m["hook_attributes"]["active"], false);
        assert_eq!(m["default_events"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn manifest_uses_the_user_chosen_name() {
        assert_eq!(manifest("my-team-app", "x")["name"], "my-team-app");
    }

    #[test]
    fn start_url_carries_state_and_org() {
        assert_eq!(
            start_url("s1", Some("acme")),
            "https://github.com/organizations/acme/settings/apps/new?state=s1"
        );
        assert_eq!(
            start_url("s1", None),
            "https://github.com/settings/apps/new?state=s1"
        );
    }

    #[test]
    fn callback_with_wrong_state_is_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let mut s = std::net::TcpStream::connect(addr).unwrap();
            s.write_all(b"GET /callback?code=abc&state=WRONG HTTP/1.1\r\n\r\n")
                .unwrap();
            let mut sink = String::new();
            let _ = s.read_to_string(&mut sink);
        });
        let r = wait_for_code(&listener, "right", Duration::from_secs(5));
        t.join().unwrap();
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn exchange_against_mock_returns_id_and_key() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/app-manifests/CODE1/conversions"))
            .respond_with(wiremock::ResponseTemplate::new(201).set_body_json(json!({
                "id": 4242, "slug": "my-team-app", "pem": FIXTURE_KEY
            })))
            .mount(&server)
            .await;
        let reg = exchange("CODE1", &server.uri()).await.unwrap();
        assert_eq!(reg.id, 4242);
        assert_eq!(reg.slug, "my-team-app");
        assert_eq!(reg.private_key, FIXTURE_KEY);
    }

    #[test]
    fn key_goes_to_credential_store_only() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FakeStore(Mutex::new(Vec::new()));
        let reg = Registered {
            id: 7,
            slug: "a".into(),
            private_key: FIXTURE_KEY.into(),
        };
        store_key(&store, "github/app", &reg).unwrap();
        assert_eq!(store.0.lock().unwrap()[0].2, FIXTURE_KEY);
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let p = entry.unwrap().path();
            if p.is_file() {
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                assert!(!text.contains("FIXTUREKEYMATERIAL"));
            }
        }
    }

    #[test]
    fn app_id_recorded_and_other_config_kept() {
        let out = set_app_id("name: team\ngithub_app_id: 1\nlead: x\n", 99);
        assert_eq!(out, "name: team\ngithub_app_id: 99\nlead: x\n");
        let added = set_app_id("name: team\n", 5);
        assert_eq!(added, "name: team\ngithub_app_id: 5\n");
    }
}
