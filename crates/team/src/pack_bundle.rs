//! Worker packs: `pack` a worker into a directory, `install` it as a template.
//!
//! A pack is `manifest.yaml` (name, version), `worker.json` (the worker record
//! with bindings stripped), and `directives/`, `templates/`, `evals/`.
//! Install copies a pack under the canvas templates dir as a TEMPLATE. It never
//! creates an agent: every effect goes through the `files` agent.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::puller::{KernelPeers, Peers};

/// Handler module name of the pack bundle.
pub const HANDLER_MODULE: &str = "team_pack.tools";

/// Keys that bind a worker to a backend, model or credential. Never packed.
/// Matched by exact key name, case-insensitively: a secret kept under any
/// other key name is NOT stripped, so a worker record must not hold one.
const BINDING_KEYS: &[&str] = &[
    "backend",
    "backend_id",
    "model",
    "model_id",
    "credentials",
    "credential",
    "api_key",
    "token",
    "access_token",
    "secret",
    "secrets",
    "client_secret",
    "auth",
    "authorization",
    "password",
    "passphrase",
    "private_key",
];

fn is_binding_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    BINDING_KEYS.contains(&k.as_str())
}

/// The directories a pack carries, besides the manifest and the record.
const PACK_DIRS: &[&str] = &["directives", "templates", "evals"];

/// Remove binding and credential keys from a worker record, recursively.
pub fn strip_bindings(record: &Value) -> Value {
    match record {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| !is_binding_key(k))
                .map(|(k, v)| (k.clone(), strip_bindings(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(strip_bindings).collect()),
        other => other.clone(),
    }
}

/// The fixed files of a pack, relative to the pack directory.
pub fn layout_paths() -> Vec<String> {
    let mut out = vec!["manifest.yaml".to_string(), "worker.json".to_string()];
    for d in PACK_DIRS {
        out.push(format!("{d}/.keep"));
    }
    out
}

/// Where an installed pack lands: `<into>/<name>-<version>/`.
pub fn install_dir(into: &Path, name: &str, version: &str) -> PathBuf {
    into.join(format!("{name}-{version}"))
}

/// A name or version that is safe as one path segment and one manifest line:
/// letters, digits, `.`, `_`, `-`, `+`; never empty, never `..`.
pub fn valid_part(s: &str) -> bool {
    !s.is_empty()
        && !s.contains("..")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
}

/// The manifest text: minimal, name and version only.
pub fn manifest_yaml(name: &str, version: &str) -> String {
    format!("name: {name}\nversion: {version}\n")
}

/// Read `name` and `version` back out of a manifest written by `manifest_yaml`.
pub fn parse_manifest(text: &str) -> Option<(String, String)> {
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(": "))
            .map(|v| v.trim().to_string())
    };
    Some((field("name")?, field("version")?))
}

fn err(msg: &str) -> Value {
    json!({"error": msg})
}

/// Text of a `files` read reply: `content`, or the joined `lines` of a listing.
fn text_of(reply: &Value) -> Option<String> {
    if reply.get("error").is_some() {
        return None;
    }
    if let Some(c) = reply.get("content").and_then(Value::as_str) {
        return Some(c.to_string());
    }
    let lines = reply.get("lines")?.as_array()?;
    Some(
        lines
            .iter()
            .filter_map(|l| l.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

async fn read<P: Peers>(p: &P, path: &str) -> Option<String> {
    text_of(&p.ask("files", json!({"type": "read", "path": path})).await)
}

async fn write<P: Peers>(p: &P, path: &str, bytes: &str) -> bool {
    let r = p
        .ask(
            "files",
            json!({"type": "write", "path": path, "bytes": bytes}),
        )
        .await;
    r.get("error").is_none()
}

/// Entry names of a directory, from a `files` list reply.
async fn list<P: Peers>(p: &P, path: &str) -> Vec<String> {
    let r = p.ask("files", json!({"type": "list", "path": path})).await;
    if let Some(a) = r.get("files").and_then(Value::as_array) {
        return a
            .iter()
            .filter_map(|f| f.get("name").and_then(Value::as_str))
            .map(String::from)
            .collect();
    }
    r.get("entries")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Does a directory exist? A `files` list of it answers without an error.
async fn exists<P: Peers>(p: &P, path: &str) -> bool {
    p.ask("files", json!({"type": "list", "path": path}))
        .await
        .get("error")
        .is_none()
}

/// Write `<root>/<rel>`; record it under `ok`, or under `failed` when the write fails.
async fn put<P: Peers>(
    p: &P,
    root: &str,
    rel: &str,
    text: &str,
    ok: &mut Vec<String>,
    failed: &mut Vec<String>,
) {
    if write(p, &format!("{root}/{rel}"), text).await {
        ok.push(rel.to_string());
    } else {
        failed.push(rel.to_string());
    }
}

/// Pack the worker whose record is `<worker>/worker.json` into the directory `out`.
/// Refuses any existing `out` (a directory with anything in it or empty), a worker
/// with no name, a name or version that is not a safe path segment, and reports
/// every failed write.
pub async fn pack<P: Peers>(p: &P, worker: &str, out: &str) -> Value {
    if exists(p, out).await {
        return err("out already exists");
    }
    let Some(raw) = read(p, &format!("{worker}/worker.json")).await else {
        return err("worker record not found");
    };
    let Ok(record) = serde_json::from_str::<Value>(&raw) else {
        return err("worker record is not json");
    };
    let name = match record.get("name").and_then(Value::as_str) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return err("worker has no name"),
    };
    let version = record
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("0.1.0")
        .to_string();
    if !valid_part(&name) || !valid_part(&version) {
        return err("worker name or version is not a safe path segment");
    }
    let stripped = strip_bindings(&record);

    let mut written = Vec::new();

    let mut failed = Vec::new();
    put(
        p,
        out,
        "manifest.yaml",
        &manifest_yaml(&name, &version),
        &mut written,
        &mut failed,
    )
    .await;
    let record_text = serde_json::to_string_pretty(&stripped).unwrap_or_default();
    put(
        p,
        out,
        "worker.json",
        &record_text,
        &mut written,
        &mut failed,
    )
    .await;
    for d in PACK_DIRS {
        put(p, out, &format!("{d}/.keep"), "", &mut written, &mut failed).await;
        for entry in list(p, &format!("{worker}/{d}")).await {
            if let Some(text) = read(p, &format!("{worker}/{d}/{entry}")).await {
                put(
                    p,
                    out,
                    &format!("{d}/{entry}"),
                    &text,
                    &mut written,
                    &mut failed,
                )
                .await;
            }
        }
    }
    if !failed.is_empty() {
        return json!({"error": "write failed", "failed": failed, "files": written});
    }
    json!({"packed": out, "name": name, "version": version, "files": written})
}

/// Install the pack at `pack` as a template under `into`, as `<name>-<version>/`.
/// Refuses an existing version, a name or version that is not a safe path
/// segment (no `/`, `\`, `..`), and reports every failed write. Creates no agent.
pub async fn install<P: Peers>(p: &P, pack: &str, into: &str) -> Value {
    let Some(manifest) = read(p, &format!("{pack}/manifest.yaml")).await else {
        return err("pack manifest not found");
    };
    let Some((name, version)) = parse_manifest(&manifest) else {
        return err("pack manifest has no name and version");
    };
    if !valid_part(&name) || !valid_part(&version) {
        return err("pack name or version is not a safe path segment");
    }
    let dest = install_dir(Path::new(into), &name, &version);
    let dest_s = dest.to_string_lossy().to_string();
    if exists(p, &dest_s).await {
        return err("version already installed");
    }
    let Some(raw) = read(p, &format!("{pack}/worker.json")).await else {
        return err("pack worker record not found");
    };
    let record: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);

    let mut copied = Vec::new();
    let mut failed = Vec::new();
    for rel in layout_paths() {
        if let Some(text) = read(p, &format!("{pack}/{rel}")).await {
            put(p, &dest_s, &rel, &text, &mut copied, &mut failed).await;
        }
    }
    for d in PACK_DIRS {
        for entry in list(p, &format!("{pack}/{d}")).await {
            let rel = format!("{d}/{entry}");
            if copied.contains(&rel) {
                continue;
            }
            if let Some(text) = read(p, &format!("{pack}/{rel}")).await {
                put(p, &dest_s, &rel, &text, &mut copied, &mut failed).await;
            }
        }
    }
    if !failed.is_empty() {
        return json!({"error": "write failed", "failed": failed, "files": copied});
    }
    json!({
        "installed_as": "template",
        "path": dest_s,
        "name": name,
        "version": version,
        "ports": record.get("ports").cloned().unwrap_or(json!([])),
        "grants": record.get("grants").cloned().unwrap_or(json!([])),
        "files": copied,
    })
}

/// The `team_pack.tools` bundle: `pack` and `install`.
pub struct PackBundle;

#[async_trait::async_trait]
impl fantastic_kernel::bundle::Bundle for PackBundle {
    fn name(&self) -> &str {
        HANDLER_MODULE
    }

    fn verb_docs(&self) -> Value {
        json!({
            "pack": "pack {worker, out} → writes the pack layout; refuses an existing out",
            "install": "install {pack, into} → copies as a template to <into>/<name>-<version>/; refuses an existing version; creates no agent",
        })
    }

    async fn handle(
        &self,
        agent_id: &fantastic_kernel::AgentId,
        payload: &serde_json::Value,
        kernel: &Arc<fantastic_kernel::Kernel>,
    ) -> Result<fantastic_kernel::bundle::Reply, fantastic_kernel::bundle::BundleError> {
        let _ = agent_id;
        let t = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if t == "boot" {
            return Ok(None);
        }
        let s = |k: &str| {
            payload
                .get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let peers = KernelPeers(kernel.clone());
        let out = match t {
            "pack" => pack(&peers, &s("worker"), &s("out")).await,
            "install" => install(&peers, &s("pack"), &s("into")).await,
            other => err(&format!("unknown verb: {other}")),
        };
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// In-memory `files` agent. Records every target it was asked.
    #[derive(Default)]
    struct FakeFiles {
        files: Mutex<BTreeMap<String, String>>,
        targets: Mutex<Vec<String>>,
        /// Writes whose path contains one of these fail.
        fail: Mutex<Vec<String>>,
    }

    impl FakeFiles {
        fn with(path: &str, text: &str) -> Self {
            let f = Self::default();
            f.files
                .lock()
                .unwrap()
                .insert(path.to_string(), text.to_string());
            f
        }
        fn get(&self, path: &str) -> Option<String> {
            self.files.lock().unwrap().get(path).cloned()
        }
    }

    #[async_trait::async_trait]
    impl Peers for FakeFiles {
        async fn ask(&self, to: &str, p: Value) -> Value {
            self.targets.lock().unwrap().push(to.to_string());
            let path = p["path"].as_str().unwrap_or("").to_string();
            let mut files = self.files.lock().unwrap();
            match p["type"].as_str().unwrap_or("") {
                "read" => match files.get(&path) {
                    Some(s) => json!({"content": s}),
                    None => json!({"error": "not_found"}),
                },
                "write" => {
                    if self.fail.lock().unwrap().iter().any(|f| path.contains(f)) {
                        return json!({"error": "disk full"});
                    }
                    files.insert(path, p["bytes"].as_str().unwrap_or("").to_string());
                    json!({"edited": true})
                }
                "list" => {
                    let prefix = format!("{path}/");
                    let entries: Vec<String> = files
                        .keys()
                        .filter_map(|k| k.strip_prefix(&prefix))
                        .filter(|rest| !rest.contains('/'))
                        .map(String::from)
                        .collect();
                    if files.keys().any(|k| k.starts_with(&prefix)) {
                        json!({"entries": entries})
                    } else {
                        json!({"error": "not_found"})
                    }
                }
                _ => json!({"error": "bad type"}),
            }
        }
    }

    fn worker() -> FakeFiles {
        FakeFiles::with(
            "w/worker.json",
            r#"{"name":"w","version":"1.0","model":"m","credentials":{"k":"v"},"ports":["p"],"grants":["g"]}"#,
        )
    }

    #[test]
    fn pack_strips_bindings_and_credentials() {
        let rec = json!({"name": "w", "model": "m", "credentials": {"k": "v"}, "keep": 1});
        assert_eq!(strip_bindings(&rec), json!({"name": "w", "keep": 1}));
    }

    #[test]
    fn layout_and_paths_are_pure() {
        let paths = layout_paths();
        assert!(paths.contains(&"manifest.yaml".to_string()));
        assert!(paths.contains(&"worker.json".to_string()));
        assert!(paths.contains(&"directives/.keep".to_string()));
        assert_eq!(
            install_dir(Path::new("/t"), "w", "1.0"),
            PathBuf::from("/t/w-1.0")
        );
        assert_eq!(
            parse_manifest(&manifest_yaml("w", "1.0")),
            Some(("w".to_string(), "1.0".to_string()))
        );
    }

    #[tokio::test]
    async fn pack_writes_the_layout() {
        let f = worker();
        let r = pack(&f, "w", "out").await;
        assert!(r.get("error").is_none(), "{r}");
        assert!(f.get("out/manifest.yaml").is_some());
        assert!(f.get("out/worker.json").is_some());
        assert!(f.get("out/directives/.keep").is_some());
        assert!(f.get("out/templates/.keep").is_some());
        assert!(f.get("out/evals/.keep").is_some());
    }

    #[tokio::test]
    async fn pack_strips_bindings_on_disk() {
        let f = worker();
        pack(&f, "w", "out").await;
        let packed = f.get("out/worker.json").unwrap();
        assert!(!packed.contains("\"model\""));
        assert!(!packed.contains("credentials"));
        assert!(packed.contains("\"ports\""));
    }

    #[tokio::test]
    async fn pack_refuses_an_existing_out() {
        let f = worker();
        f.files
            .lock()
            .unwrap()
            .insert("out/manifest.yaml".into(), "name: x\nversion: 1\n".into());
        let r = pack(&f, "w", "out").await;
        assert!(r.get("error").is_some(), "{r}");
    }

    #[tokio::test]
    async fn install_copies_as_a_template_and_creates_no_agent() {
        let f = worker();
        pack(&f, "w", "pack").await;
        let r = install(&f, "pack", "templates").await;
        assert_eq!(r["installed_as"], "template", "{r}");
        assert_eq!(r["ports"], json!(["p"]));
        assert_eq!(r["grants"], json!(["g"]));
        assert!(f.get("templates/w-1.0/manifest.yaml").is_some());
        assert!(f.get("templates/w-1.0/worker.json").is_some());
        let targets = f.targets.lock().unwrap();
        assert!(targets.iter().all(|t| t == "files"), "{targets:?}");
    }

    #[tokio::test]
    async fn install_refuses_an_existing_version() {
        let f = worker();
        pack(&f, "w", "pack").await;
        install(&f, "pack", "templates").await;
        let r = install(&f, "pack", "templates").await;
        assert!(r.get("error").is_some(), "{r}");
    }

    fn seeded_pack(manifest: &str) -> FakeFiles {
        let f = FakeFiles::with("pack/manifest.yaml", manifest);
        f.files
            .lock()
            .unwrap()
            .insert("pack/worker.json".into(), "{}".into());
        f
    }

    #[test]
    fn valid_part_accepts_only_safe_segments() {
        for ok in ["w", "my-worker_2", "1.0", "0.1.0+b3"] {
            assert!(valid_part(ok), "{ok}");
        }
        for bad in [
            "", "..", "../x", "a/b", "a\\b", "x..y", "a\nb", "a b", "a:b",
        ] {
            assert!(!valid_part(bad), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn install_refuses_a_traversing_name() {
        let f = seeded_pack("name: ../../evil\nversion: 1.0\n");
        let r = install(&f, "pack", "templates").await;
        assert!(r.get("error").is_some(), "{r}");
        let files = f.files.lock().unwrap();
        assert!(files.keys().all(|k| !k.contains("evil")), "{files:?}");
        assert!(files.keys().all(|k| !k.starts_with("templates")));
    }

    #[tokio::test]
    async fn install_refuses_a_bad_version() {
        for v in ["../x", "1/2", "1\\2", "..", "a..b"] {
            let f = seeded_pack(&format!("name: w\nversion: {v}\n"));
            let r = install(&f, "pack", "templates").await;
            assert!(r.get("error").is_some(), "{v}: {r}");
            let files = f.files.lock().unwrap();
            assert!(files.keys().all(|k| !k.starts_with("templates")), "{v}");
        }
    }

    #[tokio::test]
    async fn install_refuses_a_backslash_name() {
        let f = seeded_pack("name: ..\\evil\nversion: 1.0\n");
        let r = install(&f, "pack", "templates").await;
        assert!(r.get("error").is_some(), "{r}");
    }

    #[tokio::test]
    async fn pack_refuses_any_existing_out_directory() {
        let f = worker();
        f.files
            .lock()
            .unwrap()
            .insert("out/notes.txt".into(), "hi".into());
        let r = pack(&f, "w", "out").await;
        assert_eq!(r["error"], "out already exists", "{r}");
        assert!(f.get("out/manifest.yaml").is_none());
    }

    #[tokio::test]
    async fn pack_reports_write_failures() {
        let f = worker();
        f.fail.lock().unwrap().push("worker.json".into());
        let r = pack(&f, "w", "out").await;
        assert_eq!(r["error"], "write failed", "{r}");
        assert_eq!(r["failed"], json!(["worker.json"]));
        assert!(r.get("packed").is_none());
    }

    #[tokio::test]
    async fn install_reports_write_failures() {
        let f = worker();
        pack(&f, "w", "pack").await;
        f.fail
            .lock()
            .unwrap()
            .push("templates/w-1.0/worker.json".into());
        let r = install(&f, "pack", "templates").await;
        assert_eq!(r["error"], "write failed", "{r}");
        assert_eq!(r["failed"], json!(["worker.json"]));
        assert!(r.get("installed_as").is_none());
    }

    #[tokio::test]
    async fn pack_refuses_a_worker_with_no_name() {
        for rec in [r#"{"version":"1.0"}"#, r#"{"name":"","version":"1.0"}"#] {
            let f = FakeFiles::with("w/worker.json", rec);
            let r = pack(&f, "w", "out").await;
            assert_eq!(r["error"], "worker has no name", "{rec}: {r}");
            assert!(f.get("out/manifest.yaml").is_none());
        }
    }

    #[tokio::test]
    async fn pack_refuses_a_traversing_name() {
        let f = FakeFiles::with("w/worker.json", r#"{"name":"../evil","version":"1.0"}"#);
        let r = pack(&f, "w", "out").await;
        assert!(r.get("error").is_some(), "{r}");
        assert!(f
            .files
            .lock()
            .unwrap()
            .keys()
            .all(|k| !k.starts_with("out")));
    }

    #[test]
    fn strip_bindings_widened_and_case_insensitive() {
        let rec = json!({
            "name": "w",
            "Secret": "s",
            "auth": {"user": "u"},
            "Authorization": "Bearer x",
            "password": "p",
            "nested": [{"private_key": "k", "keep": 1}],
            "ports": ["p"]
        });
        assert_eq!(
            strip_bindings(&rec),
            json!({"name": "w", "nested": [{"keep": 1}], "ports": ["p"]})
        );
    }

    #[test]
    fn register_team_registers_pack() {
        let mut reg = fantastic_kernel::bundle::BundleRegistry::new();
        crate::register_team(&mut reg);
        let names: Vec<&str> = reg.catalog().map(|(n, _)| n).collect();
        assert!(names.contains(&HANDLER_MODULE));
    }
}
