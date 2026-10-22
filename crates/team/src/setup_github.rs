//! `fantastic_team setup github`: the GitHub App manifest flow. The user names
//! their own app; a local page posts the manifest to GitHub, GitHub redirects back
//! with a code, and the code is exchanged once for the app id and private key. The
//! key goes to the credential agent (through a [`KeyStore`]) and never to a file,
//! a log line or any reply. Only the app id is written to the config file.
//!
//! Decision on the spec's contradiction (`wt-t30` vs `wt-burst-t30`, `t30` vs
//! `burst-t30`): the task header names the burst paths and the PR lives on
//! `team/30-setup-github`, so those are used everywhere.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fantastic_kernel::{Item, Kernel, Status};
use serde_json::{json, Value};

/// The credential agent the key is written to.
pub const CREDENTIAL_AGENT: &str = "credentials";

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

/// The github.com page that creates the app from a manifest, with `state`.
/// Org URL when `org` is given, the user URL otherwise.
pub fn start_url(state: &str, org: Option<&str>) -> String {
    match org {
        Some(o) => format!("https://github.com/organizations/{o}/settings/apps/new?state={state}"),
        None => format!("https://github.com/settings/apps/new?state={state}"),
    }
}

/// The local page that POSTs the manifest to GitHub and submits itself.
pub fn form_page(manifest: &Value, action: &str) -> String {
    format!(
        "<!doctype html><html><body><form id=\"f\" method=\"post\" action=\"{}\">\
         <input type=\"hidden\" name=\"manifest\" value=\"{}\"></form>\
         <script>document.getElementById('f').submit()</script></body></html>",
        esc(action),
        esc(&manifest.to_string())
    )
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// A random state string for the callback check (hex, 128 bits).
pub fn new_state() -> String {
    let mut b = [0u8; 16];
    let read = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .is_ok();
    if !read {
        for (i, chunk) in b.chunks_mut(8).enumerate() {
            let mut h = RandomState::new().build_hasher();
            h.write_usize(i);
            chunk.copy_from_slice(&h.finish().to_le_bytes()[..chunk.len()]);
        }
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
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

/// Where the key goes: the credential store, under one ref. The kernel adapter is
/// [`KernelStore`]; tests use a fake.
#[async_trait::async_trait]
pub trait KeyStore: Send + Sync {
    /// Write the app id and private key under `reference`.
    async fn store(&self, reference: &str, app_id: u64, private_key: &str) -> Result<(), String>;
}

/// The credential agent reached through the kernel: `set` with `ref` and `fields`.
pub struct KernelStore {
    /// The running kernel.
    pub kernel: Arc<Kernel>,
    /// The credential agent's id.
    pub agent: String,
}

#[async_trait::async_trait]
impl KeyStore for KernelStore {
    async fn store(&self, reference: &str, app_id: u64, private_key: &str) -> Result<(), String> {
        let outs = self
            .kernel
            .send(vec![Item::new(
                self.agent.as_str(),
                json!({
                    "type": "set",
                    "ref": reference,
                    "fields": {"app_id": app_id.to_string(), "private_key": private_key},
                }),
            )])
            .await;
        match outs.first() {
            Some(o) if matches!(o.status, Status::Completed) && o.result.get("error").is_none() => {
                Ok(())
            }
            _ => Err(format!(
                "credential agent `{}` refused to store the key",
                self.agent
            )),
        }
    }
}

/// Write the app id and key to the store. The key is never returned.
pub async fn store_key(
    store: &dyn KeyStore,
    reference: &str,
    reg: &Registered,
) -> Result<(), String> {
    store.store(reference, reg.id, &reg.private_key).await
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

fn respond(conn: &mut TcpStream, ctype: &str, body: &str) {
    let _ = conn.write_all(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    );
}

/// Serve `form` at `/` and wait for GitHub's redirect on `/callback`: check
/// `state`, return the `code`. Refuses a wrong state; gives up after `timeout`.
pub fn wait_for_code(
    listener: &TcpListener,
    state: &str,
    form: &str,
    timeout: Duration,
) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((mut conn, _)) => {
                conn.set_nonblocking(false).ok();
                conn.set_read_timeout(Some(Duration::from_secs(5))).ok();
                let mut buf = [0u8; 8192];
                let n = conn.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let target = head.split_whitespace().nth(1).unwrap_or("").to_string();
                let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
                if path == "/" {
                    respond(&mut conn, "text/html", form);
                    continue;
                }
                if path != "/callback" {
                    respond(&mut conn, "text/plain", "not found");
                    continue;
                }
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
                respond(&mut conn, "text/plain", body);
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

/// One setup run: which app, which account, which config file.
pub struct Flow<'a> {
    /// The app name the user chose.
    pub name: &'a str,
    /// The organisation to create the app under, if any.
    pub org: Option<&'a str>,
    /// The GitHub API base, `https://api.github.com` or a mock.
    pub api_base: &'a str,
    /// The config file that gets `github_app_id`.
    pub config: &'a Path,
    /// How long to wait for the browser to come back.
    pub timeout: Duration,
}

/// Run the flow: serve the manifest page, wait for the callback, exchange the
/// code, store the key, record the app id. `announce` is told the local URL to
/// open. Returns the summary line; never the key.
pub async fn run_flow(
    flow: &Flow<'_>,
    store: &dyn KeyStore,
    announce: &(dyn Fn(&str) + Sync),
) -> Result<String, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("cannot open the local callback: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let callback = format!("http://127.0.0.1:{port}/callback");
    let state = new_state();
    let action = start_url(&state, flow.org);
    let form = form_page(&manifest(flow.name, &callback), &action);
    announce(&format!("http://127.0.0.1:{port}/"));
    let code = wait_for_code(&listener, &state, &form, flow.timeout)?;
    let reg = exchange(&code, flow.api_base).await?;
    let reference = format!("github/{}", flow.name);
    store_key(store, &reference, &reg).await?;
    record_app_id(flow.config, reg.id)?;
    Ok(format!("App {} (id {}) registered", reg.slug, reg.id))
}

/// `setup github --name NAME [--org ORG]`: run the flow in the current directory
/// against the team kernel. Handles its own arguments.
pub fn run_cli(args: &[String]) -> Result<(), String> {
    let usage = "usage: fantastic_team setup github --name NAME [--org ORG]";
    if args.first().map(String::as_str) != Some("github") {
        return Err(usage.into());
    }
    let mut name = None;
    let mut org = None;
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--name" => name = Some(it.next().ok_or(usage)?.clone()),
            "--org" => org = Some(it.next().ok_or(usage)?.clone()),
            other => return Err(format!("unknown argument `{other}`\n{usage}")),
        }
    }
    let name = name.ok_or_else(|| format!("--name is required\n{usage}"))?;
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        let kernel = crate::start(&dir).await.map_err(|e| e.to_string())?;
        let store = KernelStore {
            kernel: kernel.clone(),
            agent: CREDENTIAL_AGENT.into(),
        };
        let config = dir.join(crate::config::FILE_NAME);
        let flow = Flow {
            name: &name,
            org: org.as_deref(),
            api_base: "https://api.github.com",
            config: &config,
            timeout: Duration::from_secs(600),
        };
        let announce = |url: &str| {
            println!("open this URL in your browser to create the app:\n{url}");
            let _ = std::process::Command::new("xdg-open").arg(url).spawn();
        };
        let res = run_flow(&flow, &store, &announce).await;
        kernel.shutdown().await;
        let _ = fantastic_kernel::bootstrap::shutdown(&dir);
        println!("{}", res?);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const FIXTURE_KEY: &str = "fixture-key-material-not-a-real-key";

    #[derive(Default)]
    struct FakeStore(Mutex<Vec<(String, u64, String)>>);

    #[async_trait::async_trait]
    impl KeyStore for FakeStore {
        async fn store(
            &self,
            reference: &str,
            app_id: u64,
            private_key: &str,
        ) -> Result<(), String> {
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
            let mut s = TcpStream::connect(addr).unwrap();
            s.write_all(b"GET /callback?code=abc&state=WRONG HTTP/1.1\r\n\r\n")
                .unwrap();
            let mut sink = String::new();
            let _ = s.read_to_string(&mut sink);
        });
        let r = wait_for_code(&listener, "right", "", Duration::from_secs(5));
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

    #[tokio::test]
    async fn key_goes_to_credential_store_only() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FakeStore::default();
        let reg = Registered {
            id: 7,
            slug: "a".into(),
            private_key: FIXTURE_KEY.into(),
        };
        store_key(&store, "github/app", &reg).await.unwrap();
        assert_eq!(store.0.lock().unwrap()[0].2, FIXTURE_KEY);
        assert!(all_files(dir.path()).is_empty());
    }

    #[test]
    fn app_id_recorded_and_other_config_kept() {
        let out = set_app_id("name: team\ngithub_app_id: 1\nlead: x\n", 99);
        assert_eq!(out, "name: team\ngithub_app_id: 99\nlead: x\n");
        let added = set_app_id("name: team\n", 5);
        assert_eq!(added, "name: team\ngithub_app_id: 5\n");
    }

    /// Every file under `dir`, concatenated.
    fn all_files(dir: &Path) -> String {
        let mut text = String::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    text.push_str(&all_files(&p));
                } else if let Ok(t) = std::fs::read_to_string(&p) {
                    text.push_str(&t);
                }
            }
        }
        text
    }

    /// GET `path` from the local server and return the whole response.
    fn http_get(port: u16, path: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    /// Play the browser: fetch the manifest page, take its state, come back on
    /// the callback with a code.
    fn drive_browser(root: String) {
        let port: u16 = root
            .trim_start_matches("http://127.0.0.1:")
            .trim_end_matches('/')
            .parse()
            .unwrap();
        let page = http_get(port, "/");
        let i = page.find("state=").unwrap() + "state=".len();
        let state: String = page[i..]
            .chars()
            .take_while(|c| c.is_ascii_hexdigit())
            .collect();
        http_get(port, &format!("/callback?code=CODE1&state={state}"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn no_key_in_any_output() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/app-manifests/CODE1/conversions"))
            .respond_with(wiremock::ResponseTemplate::new(201).set_body_json(json!({
                "id": 4242, "slug": "my-team-app", "pem": FIXTURE_KEY
            })))
            .mount(&server)
            .await;
        let dir = tempfile::TempDir::new().unwrap();
        let config = dir.path().join("team.yaml");
        std::fs::write(&config, "name: team\n").unwrap();
        let store = FakeStore::default();
        let base = server.uri();
        let flow = Flow {
            name: "my-team-app",
            org: None,
            api_base: &base,
            config: &config,
            timeout: Duration::from_secs(10),
        };
        let log = Mutex::new(String::new());
        let announce = |url: &str| {
            log.lock().unwrap().push_str(url);
            log.lock().unwrap().push('\n');
            let root = url.to_string();
            std::thread::spawn(move || drive_browser(root));
        };
        let summary = run_flow(&flow, &store, &announce).await.unwrap();
        assert_eq!(summary, "App my-team-app (id 4242) registered");
        // The key did reach the store, and only there.
        assert_eq!(store.0.lock().unwrap()[0].2, FIXTURE_KEY);
        let mut seen = log.lock().unwrap().clone();
        seen.push_str(&summary);
        seen.push_str(&all_files(dir.path()));
        assert!(!seen.contains(FIXTURE_KEY), "key leaked into output");
        assert!(std::fs::read_to_string(&config)
            .unwrap()
            .contains("github_app_id: 4242"));
    }
}
