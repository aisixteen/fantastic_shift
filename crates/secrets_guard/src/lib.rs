//! Test helper: proves that no secret leaves an agent.
//!
//! One [`Probe`] per agent under test. The secret is planted where the agent
//! reads it (its credential store, its env, its constructor), never in a
//! payload. The probe sends the agent its payloads, then searches every reply,
//! error, emitted event, state-stream event, store dump, kernel record and
//! named folder for each secret, in plain, base64, base64url, URL-encoded and
//! form-encoded form.
#![deny(missing_docs)]

use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use fantastic_kernel::{AgentId, Blob, Item, Kernel, Status};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long the guard waits after the last payload for late events.
pub const SETTLE: Duration = Duration::from_millis(300);
/// The shortest secret the guard searches for (shorter ones match by chance).
pub const MIN_SECRET_LEN: usize = 8;

/// One place a secret was found. It names the secret by its index, never by value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leak {
    /// Index of the secret in the probe's list.
    pub secret: usize,
    /// How it was written: plain, base64, base64url, url-encoded, form-encoded.
    pub form: &'static str,
    /// Where: reply, error, event, state, store, records, file.
    pub channel: &'static str,
    /// Which one: `payload #2`, `event #0`, a store id, a file path.
    pub at: String,
}

impl std::fmt::Display for Leak {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "secret #{} as {} in {} ({})",
            self.secret, self.form, self.channel, self.at
        )
    }
}

/// Percent-encode every byte except ASCII alphanumerics and `-._~`.
pub fn percent_encode(s: &str, upper: bool) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else if upper {
            out.push_str(&format!("%{b:02X}"));
        } else {
            out.push_str(&format!("%{b:02x}"));
        }
    }
    out
}

fn add(out: &mut Vec<(&'static str, String)>, name: &'static str, s: String) {
    if !s.is_empty() && !out.iter().any(|(_, x)| *x == s) {
        out.push((name, s));
    }
}

/// The needles searched for a secret, in order, without duplicates.
pub fn forms(secret: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    add(&mut out, "plain", secret.to_string());
    let sb = secret.as_bytes();
    let n = sb.len();
    for k in 0..3usize {
        // k zero bytes in front put the secret at byte offset k: the characters
        // between `start` and `end` depend only on the secret's own bytes.
        let mut buf = vec![0u8; k];
        buf.extend_from_slice(sb);
        let start = (8 * k).div_ceil(6);
        let end = (8 * (k + n)) / 6;
        let std_enc = STANDARD_NO_PAD.encode(&buf);
        let url_enc = URL_SAFE_NO_PAD.encode(&buf);
        if let Some(s) = std_enc.get(start..end) {
            add(&mut out, "base64", s.to_string());
        }
        if let Some(s) = url_enc.get(start..end) {
            add(&mut out, "base64url", s.to_string());
        }
    }
    add(&mut out, "url-encoded", percent_encode(secret, true));
    add(&mut out, "url-encoded", percent_encode(secret, false));
    add(
        &mut out,
        "form-encoded",
        percent_encode(secret, true).replace("%20", "+"),
    );
    out
}

/// Every string, object key and number of a JSON value, each followed by a newline.
pub fn texts_of(v: &Value) -> String {
    let mut out = String::new();
    walk(v, &mut out);
    out
}

fn walk(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => {
            out.push_str(s);
            out.push('\n');
        }
        Value::Number(n) => {
            out.push_str(&n.to_string());
            out.push('\n');
        }
        Value::Array(a) => {
            for x in a {
                walk(x, out);
            }
        }
        Value::Object(m) => {
            for (k, x) in m {
                out.push_str(k);
                out.push('\n');
                walk(x, out);
            }
        }
        _ => {}
    }
}

/// Search `text` for every secret; the first matching form of each is recorded.
pub fn search(
    text: &str,
    secrets: &[String],
    channel: &'static str,
    at: &str,
    out: &mut Vec<Leak>,
) {
    for (i, s) in secrets.iter().enumerate() {
        if let Some((form, _)) = forms(s)
            .into_iter()
            .find(|(_, needle)| text.contains(needle.as_str()))
        {
            out.push(Leak {
                secret: i,
                form,
                channel,
                at: at.to_string(),
            });
        }
    }
}

/// A readable report of leaks. It never contains a secret.
pub fn format_leaks(leaks: &[Leak]) -> String {
    let mut s = format!(
        "secrets_guard: {} leak(s) — no secret may leave an agent:\n",
        leaks.len()
    );
    for l in leaks {
        s.push_str(&format!("  - {l}\n"));
    }
    s
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            out.extend(files_under(&p));
        } else if ft.is_file() {
            out.push(p);
        }
    }
    out
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// One agent under test: its payloads, the secrets it holds, and where else to look.
pub struct Probe {
    kernel: Arc<Kernel>,
    agent: String,
    payloads: Vec<Value>,
    secrets: Vec<String>,
    stores: Vec<String>,
    dirs: Vec<PathBuf>,
}

impl Probe {
    /// A probe for `agent`, sending it `payloads` and guarding `secrets`.
    pub fn new(kernel: &Arc<Kernel>, agent: &str, payloads: Vec<Value>, secrets: &[&str]) -> Self {
        Self {
            kernel: kernel.clone(),
            agent: agent.to_string(),
            payloads,
            secrets: secrets.iter().map(|s| s.to_string()).collect(),
            stores: Vec::new(),
            dirs: Vec::new(),
        }
    }

    /// Also dump this store agent after the run and search its reply.
    pub fn store(mut self, id: &str) -> Self {
        self.stores.push(id.to_string());
        self
    }

    /// Also read every file under this folder after the run.
    pub fn dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.dirs.push(path.into());
        self
    }

    /// Run the probe and return every leak found.
    pub async fn leaks(&self) -> Vec<Leak> {
        for (j, s) in self.secrets.iter().enumerate() {
            if s.len() < MIN_SECRET_LEN {
                panic!("secrets_guard: secret #{j} is shorter than {MIN_SECRET_LEN} characters — too short to search for");
            }
        }
        for (i, p) in self.payloads.iter().enumerate() {
            let mut hits = Vec::new();
            search(&texts_of(p), &self.secrets, "payload", "", &mut hits);
            if let Some(h) = hits.first() {
                panic!(
                    "secrets_guard: payload #{i} carries secret #{} — plant secrets where the agent reads them (its credential store, its env), never in a payload",
                    h.secret
                );
            }
        }

        let agent = AgentId::from(self.agent.clone());
        let mut found: Vec<Leak> = Vec::new();

        // Taps: the state stream, and an inbox watching every payload sent or emitted.
        let state: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = state.clone();
        let token =
            self.kernel
                .add_state_subscriber(Arc::new(move |ev: &Value, body: Option<&Blob>| {
                    let mut text = texts_of(ev);
                    if let Some(b) = body {
                        text.push_str(&lossy(&b.bytes));
                    }
                    sink.lock().expect("state tap").push(text);
                }));
        let watcher = AgentId::from(format!("_secrets_guard_{}", self.agent));
        let mut inbox = self.kernel.ensure_inbox(&watcher);
        if let Some(r) = self.kernel.watch(&agent, watcher.clone()).await {
            panic!("secrets_guard: cannot watch {}: {r}", self.agent);
        }

        // Send every payload; search the reply or the error.
        for (i, p) in self.payloads.iter().enumerate() {
            let out = self
                .kernel
                .send(vec![Item::new(agent.clone(), p.clone())])
                .await
                .remove(0);
            let failed = !matches!(out.status, Status::Completed)
                || out
                    .result
                    .get("error")
                    .is_some_and(|e| !e.is_null() && e != &Value::Bool(false));
            let channel = if failed { "error" } else { "reply" };
            let mut text = texts_of(&out.result);
            if let Some(b) = &out.body {
                text.push_str(&lossy(&b.bytes));
            }
            search(
                &text,
                &self.secrets,
                channel,
                &format!("payload #{i}"),
                &mut found,
            );
        }

        tokio::time::sleep(SETTLE).await;

        // Events the watcher received.
        let mut n = 0;
        while let Some(item) = inbox.try_recv() {
            let mut text = texts_of(&item.payload);
            if let Some(b) = &item.body {
                text.push_str(&lossy(&b.bytes));
            }
            search(
                &text,
                &self.secrets,
                "event",
                &format!("event #{n}"),
                &mut found,
            );
            n += 1;
        }
        self.kernel.unwatch(&agent, &watcher).await;
        self.kernel.remove_inbox(&watcher);
        self.kernel.remove_state_subscriber(token);
        let states = std::mem::take(&mut *state.lock().expect("state tap"));
        for (n, t) in states.iter().enumerate() {
            search(
                t,
                &self.secrets,
                "state",
                &format!("state event #{n}"),
                &mut found,
            );
        }

        // Store dumps.
        for id in &self.stores {
            let out = self
                .kernel
                .send(vec![Item::new(
                    AgentId::from(id.clone()),
                    json!({"type": "dump"}),
                )])
                .await
                .remove(0);
            search(
                &texts_of(&out.result),
                &self.secrets,
                "store",
                id,
                &mut found,
            );
        }

        // Kernel records.
        search(
            &self.kernel.save_json().to_string(),
            &self.secrets,
            "records",
            "kernel.save_json()",
            &mut found,
        );

        // Files.
        for dir in &self.dirs {
            for f in files_under(dir) {
                let Ok(meta) = std::fs::metadata(&f) else {
                    continue;
                };
                if meta.len() > 8 * 1024 * 1024 {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&f) else {
                    continue;
                };
                search(
                    &lossy(&bytes),
                    &self.secrets,
                    "file",
                    &f.display().to_string(),
                    &mut found,
                );
            }
        }
        found
    }

    /// Panic with a readable report when any leak is found.
    pub async fn assert_clean(&self) {
        let l = self.leaks().await;
        if !l.is_empty() {
            panic!("{}", format_leaks(&l));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::{STANDARD, URL_SAFE};

    #[test]
    fn base64_finds_the_secret_at_every_alignment() {
        let secret = "s3cret-Value/42+x";
        let f = forms(secret);
        for prefix in ["", "a", "ab", "abc", "x-access-token:", "planted-user:"] {
            let text = format!("{prefix}{secret}");
            let std_text = STANDARD.encode(&text);
            let url_text = URL_SAFE.encode(&text);
            assert!(
                f.iter()
                    .any(|(k, n)| *k == "base64" && std_text.contains(n.as_str())),
                "prefix {prefix:?}"
            );
            assert!(
                f.iter()
                    .any(|(k, n)| (*k == "base64" || *k == "base64url")
                        && url_text.contains(n.as_str())),
                "prefix {prefix:?}"
            );
        }
    }

    #[test]
    fn url_encoding_is_found_in_both_cases() {
        assert_eq!(percent_encode("a b/c", true), "a%20b%2Fc");
        assert_eq!(percent_encode("a b/c", false), "a%20b%2fc");
        assert!(forms("s3cret-Value/42+x")
            .contains(&("url-encoded", "s3cret-Value%2F42%2Bx".to_string())));
    }

    #[test]
    fn forms_has_no_duplicates() {
        let f = forms("abcdefgh12345678");
        let mut needles: Vec<&String> = f.iter().map(|(_, n)| n).collect();
        let total = needles.len();
        needles.sort();
        needles.dedup();
        assert_eq!(needles.len(), total);
    }

    #[test]
    fn texts_of_reads_keys_values_and_numbers() {
        let t = texts_of(&json!({"k": ["v", 7, {"inner": "w"}]}));
        for part in ["k\n", "v\n", "7\n", "inner\n", "w\n"] {
            assert!(t.contains(part), "missing {part:?} in {t:?}");
        }
    }

    #[test]
    fn format_leaks_never_names_the_secret() {
        let l = Leak {
            secret: 0,
            form: "plain",
            channel: "reply",
            at: "payload #0".into(),
        };
        assert!(format_leaks(&[l]).contains("secret #0 as plain in reply (payload #0)"));
    }
}
