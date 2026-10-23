//! fantastic_team as a library: this repo's bundles ([`register_team`]) for
//! fantastic's terminal-shell library (`fantastic_tui::main_with`), the command
//! line that maps the team's surface flags onto that shell's modes
//! ([`parse_args`]), and an in-process kernel ([`start`]) for tests. `main.rs`
//! is a thin wrapper around `fantastic_tui::main_with`.
#![deny(missing_docs)]

pub mod board;
pub mod cadence_feed;
pub mod cadencer;
pub mod config;
pub mod dispatcher;
pub mod driver;
pub mod driver_config;
pub mod driver_rules;
pub mod instructions;
pub mod pack_manifest;
pub mod pack_scan;
pub mod puller;
pub mod repo_config;
pub mod setup_github;
pub mod tree;
pub mod webhook;
pub mod worker_view;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fantastic_kernel::bootstrap::{self, BootstrapOptions, DEFAULT_ROOT_ID};
use fantastic_kernel::{lock, BundleRegistry, Item, Kernel, Status};
use serde_json::json;

pub mod worker_exec;

/// This repo's bundles, added to the registry of fantastic's shell AFTER its
/// stock set (an `fantastic_tui::Extra`): the git and github connectors. The
/// worker exec restriction ([`worker_exec`]) and the config and directives
/// ([`config`], [`directives`]) are records and files, not bundles, so nothing
/// else registers.
pub fn register_team(reg: &mut BundleRegistry) {
    reg.register(
        team_git_connector::HANDLER_MODULE,
        team_git_connector::GitConnectorBundle,
    );
    reg.register(
        team_github_connector::HANDLER_MODULE,
        team_github_connector::GithubConnectorBundle,
    );
    reg.register(puller::HANDLER_MODULE, puller::PullerBundle);
    reg.register(webhook::HANDLER_MODULE, webhook::WebhookBundle);
    reg.register(board::HANDLER_MODULE, board::BoardBundle);
    reg.register(dispatcher::HANDLER_MODULE, dispatcher::DispatcherBundle);
    reg.register(
        cadencer::HANDLER_MODULE,
        cadencer::CadencerBundle::default(),
    );
    reg.register(
        worker_view::HANDLER_MODULE,
        worker_view::WorkerViewBundle::default(),
    );
}

/// A kernel registry for [`start`] (tests, embedding): the bundles the base
/// tree needs (the root's state, the file store, the RAM memory store, the
/// exec bridge) plus [`register_team`]. fantastic's shell builds its own, fuller
/// set when the binary runs; that one is private to it.
pub fn registry() -> BundleRegistry {
    let mut reg = BundleRegistry::new();
    reg.register(
        fantastic_core::HANDLER_MODULE,
        fantastic_core::KernelStateBundle::default(),
    );
    reg.register("file_bridge.tools", fantastic_file::FileBundle::default());
    reg.register(
        fantastic_ram_store::HANDLER_MODULE,
        fantastic_ram_store::RamStoreBundle::default(),
    );
    reg.register(
        fantastic_exec_bridge::HANDLER_MODULE,
        fantastic_exec_bridge::ExecBridgeBundle,
    );
    register_team(&mut reg);
    reg
}

/// The README for AIs (`docs/ai-readme.md`), built into the binary. It is printed
/// by `fantastic_team ai-readme`.
pub const AI_README: &str = include_str!("../../../docs/ai-readme.md");

/// Usage text, printed on `--help` and on bad arguments.
pub const USAGE: &str = "usage: fantastic_team [--dir PATH] [--port N | --tui]\n  no surface flag: fully headless (no port, no terminal UI)\n  --port N: serve the web UI on 127.0.0.1:N\n  --tui: the terminal UI (needs a terminal)\n  (`mcp`, `send` and `readme` as the first word run fantastic's own commands)";

/// The first words that are fantastic's own commands, passed through untouched.
const PASSTHROUGH: &[&str] = &["mcp", "send", "readme"];

/// Whether `argv` is one of fantastic's own commands (`mcp`, `send`, `readme`).
pub fn is_passthrough(argv: &[String]) -> bool {
    argv.first()
        .is_some_and(|a| PASSTHROUGH.contains(&a.as_str()))
}

/// What the team shows, if anything. Surfaces are opt-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// No port, no terminal UI: fantastic's headless mode.
    Headless,
    /// The web UI on `127.0.0.1:<port>`: fantastic's canvas mode.
    Port(u16),
    /// The terminal UI.
    Tui,
}

/// Parsed command line.
#[derive(Debug)]
pub struct Args {
    /// The directory the team lives in.
    pub dir: PathBuf,
    /// The surface to serve.
    pub surface: Surface,
}

impl Args {
    /// The arguments for `fantastic_tui::main_with` that run this surface:
    /// `--headless`, `canvas --port N`, or none (its terminal UI).
    pub fn fantastic_args(&self) -> Vec<String> {
        match self.surface {
            Surface::Headless => vec!["--headless".to_string()],
            Surface::Port(p) => vec!["canvas".to_string(), "--port".to_string(), p.to_string()],
            Surface::Tui => Vec::new(),
        }
    }
}

fn parse_port(v: &str) -> Result<u16, String> {
    v.parse::<u16>()
        .ok()
        .filter(|p| *p != 0)
        .ok_or_else(|| format!("--port needs a number from 1 to 65535, got `{v}`\n{USAGE}"))
}

/// Parse `--dir PATH` (default: the current directory) and the surface flags
/// `--port N` and `--tui` (default: headless).
pub fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut dir: Option<PathBuf> = None;
    let mut port: Option<u16> = None;
    let mut tui = false;
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dir" => {
                let v = it
                    .next()
                    .ok_or_else(|| format!("--dir needs a value\n{USAGE}"))?;
                dir = Some(PathBuf::from(v));
            }
            "--port" => {
                let v = it
                    .next()
                    .ok_or_else(|| format!("--port needs a value\n{USAGE}"))?;
                port = Some(parse_port(v)?);
            }
            "--tui" => tui = true,
            "--help" | "-h" => return Err(USAGE.to_string()),
            other => return Err(format!("unknown argument `{other}`\n{USAGE}")),
        }
    }
    let dir = match dir {
        Some(d) => d,
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    if tui && port.is_some() {
        return Err(format!(
            "--port and --tui cannot be combined: fantastic serves the web UI without a terminal UI\n{USAGE}"
        ));
    }
    let surface = match (port, tui) {
        (Some(p), _) => Surface::Port(p),
        (None, true) => Surface::Tui,
        (None, false) => Surface::Headless,
    };
    Ok(Args { dir, surface })
}

/// Seed a fresh directory's base tree: a RAM memory store for the kernel's
/// execution rows, a file-bridge store so records persist, and the root
/// pointed at the memory store. A root that already names a `memory_store_id`
/// is left alone. Returns `Ok(true)` when it seeded, `Ok(false)` when there
/// was nothing to do. Mirrors the blueprint in fantastic's terminal app.
pub async fn seed_base_tree(kernel: &Arc<Kernel>) -> Result<bool, String> {
    let root = kernel
        .send(vec![Item::new(DEFAULT_ROOT_ID, json!({"type": "reflect"}))])
        .await;
    let first = match root.first() {
        Some(o) if matches!(o.status, Status::Completed) => o,
        Some(o) => return Err(format!("base tree: reflect failed: {}", o.result)),
        None => return Err("base tree: reflect failed: no reply".to_string()),
    };
    let reflected = first.result.clone();
    if reflected
        .get("memory_store_id")
        .and_then(|v| v.as_str())
        .is_some()
    {
        return Ok(false);
    }
    let steps = [
        (
            DEFAULT_ROOT_ID,
            json!({"type": "create_agent", "parent_id": DEFAULT_ROOT_ID,
                   "id": "kernel_memory", "handler_module": "ram_store.tools",
                   "admins": ["kernel"]}),
        ),
        (
            DEFAULT_ROOT_ID,
            json!({"type": "create_agent", "parent_id": DEFAULT_ROOT_ID,
                   "id": "store", "handler_module": "file_bridge.tools",
                   "root": ".fantastic", "allow": ["."]}),
        ),
        (
            "kernel",
            json!({"type": "update_agent", "id": DEFAULT_ROOT_ID,
                   "memory_store_id": "kernel_memory"}),
        ),
    ];
    for (target, payload) in steps {
        for o in kernel.send(vec![Item::new(target, payload)]).await {
            if !matches!(o.status, Status::Completed) {
                return Err(format!("base tree seeding failed: {}", o.result));
            }
        }
    }
    kernel.flush_records().await;
    Ok(true)
}

/// Boot a headless kernel in `dir` and start it: the daemon sequence of
/// fantastic-cli mode 5, with a base tree seeded into a fresh directory. The
/// caller stops it with `kernel.shutdown()` and then `bootstrap::shutdown(dir)`.
pub async fn start(dir: &Path) -> Result<Arc<Kernel>, Box<dyn std::error::Error>> {
    let mut booted = bootstrap::bootstrap(registry(), BootstrapOptions::one_shot(dir))?;
    lock::acquire(dir)?;
    let kernel = booted.kernel.clone();
    let outcome = match booted.write_seed() {
        Ok(()) => boot_kernel(&kernel).await,
        Err(e) => Err(e.to_string()),
    };
    match outcome {
        Ok(()) => Ok(kernel),
        Err(msg) => {
            kernel.shutdown().await;
            bootstrap::shutdown(dir)?;
            Err(msg.into())
        }
    }
}

/// Everything after the lock: seed the base tree, then start the kernel.
/// Any error here makes `start` shut down and release the lock.
async fn boot_kernel(kernel: &Arc<Kernel>) -> Result<(), String> {
    seed_base_tree(kernel).await?;
    let started = kernel.start(true).await;
    if started
        .get("error")
        .filter(|e| fantastic_bundle::py_truthy(e))
        .is_some()
    {
        return Err(fantastic_bundle::py_str(&started["error"]));
    }
    Ok(())
}

pub mod directives;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// Boot the root and write its record without starting the kernel, so a
    /// test can seed a fresh tree and inspect it.
    fn boot_unstarted(dir: &Path) -> Arc<Kernel> {
        let mut booted = bootstrap::bootstrap(registry(), BootstrapOptions::one_shot(dir)).unwrap();
        lock::acquire(dir).unwrap();
        booted.write_seed().unwrap();
        booted.kernel.clone()
    }

    #[test]
    fn the_registry_has_fantastic_s_bundles() {
        let reg = registry();
        let names: Vec<&str> = reg.catalog().map(|(n, _)| n).collect();
        assert!(names.contains(&"file_bridge.tools"));
        assert!(names.contains(&"kernel_state.tools"));
    }

    #[test]
    fn the_registry_has_the_git_connector() {
        let reg = registry();
        let names: Vec<&str> = reg.catalog().map(|(n, _)| n).collect();
        assert!(names.contains(&"team_git.tools"));
    }

    #[test]
    fn the_registry_has_the_github_connector() {
        let reg = registry();
        let names: Vec<&str> = reg.catalog().map(|(n, _)| n).collect();
        assert!(names.contains(&"team_github.tools"));
    }

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn register_team_adds_the_team_bundles_to_any_registry() {
        let mut reg = BundleRegistry::new();
        register_team(&mut reg);
        let names: Vec<&str> = reg.catalog().map(|(n, _)| n).collect();
        assert!(names.contains(&"team_git.tools"));
        assert!(names.contains(&"team_github.tools"));
        // An `fantastic_tui::Extra` is a plain fn over the registry.
        let _extra: fantastic_tui::Extra = register_team;
    }

    #[test]
    fn register_team_registers_puller() {
        let mut reg = BundleRegistry::new();
        register_team(&mut reg);
        let names: Vec<&str> = reg.catalog().map(|(n, _)| n).collect();
        assert!(names.contains(&"team_puller.tools"));
    }

    #[test]
    fn args_default_to_the_current_directory() {
        let a = parse_args(&[]).unwrap();
        assert_eq!(a.dir, std::env::current_dir().unwrap());
    }

    #[test]
    fn no_surface_flag_is_fully_headless() {
        let a = parse_args(&[]).unwrap();
        assert_eq!(a.surface, Surface::Headless);
        assert_eq!(a.fantastic_args(), v(&["--headless"]));
        let d = parse_args(&v(&["--dir", "/x"])).unwrap();
        assert_eq!(d.surface, Surface::Headless);
    }

    #[test]
    fn port_serves_the_web_ui_on_that_port() {
        let a = parse_args(&v(&["--port", "8123"])).unwrap();
        assert_eq!(a.surface, Surface::Port(8123));
        assert_eq!(a.fantastic_args(), v(&["canvas", "--port", "8123"]));
        let b = parse_args(&v(&["--port", "9000", "--dir", "/x"])).unwrap();
        assert_eq!(b.surface, Surface::Port(9000));
        assert_eq!(b.dir, PathBuf::from("/x"));
    }

    #[test]
    fn tui_opens_the_terminal_ui() {
        let a = parse_args(&v(&["--tui"])).unwrap();
        assert_eq!(a.surface, Surface::Tui);
        assert!(a.fantastic_args().is_empty());
    }

    #[test]
    fn bad_surface_flags_are_refused() {
        for bad in [
            &["--port"][..],
            &["--port", "abc"],
            &["--port", "0"],
            &["--port", "70000"],
            &["--port", "80", "--tui"],
            &["--tui", "--port", "80"],
            &["--canvas"],
            &["--headless"],
            &["canvas"],
        ] {
            let e = parse_args(&v(bad))
                .err()
                .unwrap_or_else(|| panic!("{bad:?} must be refused"));
            assert!(e.contains("usage:"), "{bad:?}: {e}");
        }
    }

    #[test]
    fn fantastic_s_own_commands_pass_through() {
        assert!(is_passthrough(&v(&["mcp"])));
        assert!(is_passthrough(&v(&["send", "{}"])));
        assert!(is_passthrough(&v(&["readme"])));
        assert!(!is_passthrough(&[]));
        assert!(!is_passthrough(&v(&["--port", "1"])));
    }

    #[test]
    fn args_take_a_dir() {
        let a = parse_args(&["--dir".to_string(), "/x".to_string()]).unwrap();
        assert_eq!(a.dir, PathBuf::from("/x"));
    }

    #[test]
    fn unknown_args_and_help_are_refused() {
        let e = parse_args(&["--nope".to_string()]).err().unwrap();
        assert!(!is_passthrough(&v(&["--nope"])));
        assert!(e.contains("unknown argument") && e.contains("usage:"));
        assert_eq!(parse_args(&["--help".to_string()]).err().unwrap(), USAGE);
        assert!(parse_args(&["--dir".to_string()]).is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_fresh_dir_gets_a_base_tree() {
        let dir = tempfile::TempDir::new().unwrap();
        let kernel = boot_unstarted(dir.path());
        assert!(seed_base_tree(&kernel).await.unwrap());
        let root = kernel
            .send(vec![Item::new(DEFAULT_ROOT_ID, json!({"type": "reflect"}))])
            .await;
        assert_eq!(
            root[0]
                .result
                .get("memory_store_id")
                .and_then(|v| v.as_str()),
            Some("kernel_memory")
        );
        kernel.shutdown().await;
        bootstrap::shutdown(dir.path()).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_seeded_dir_is_left_alone() {
        let dir = tempfile::TempDir::new().unwrap();
        let kernel = boot_unstarted(dir.path());
        assert!(seed_base_tree(&kernel).await.unwrap());
        assert!(!seed_base_tree(&kernel).await.unwrap());
        kernel.shutdown().await;
        bootstrap::shutdown(dir.path()).unwrap();
    }

    /// Reflect an agent and return its record.
    async fn reflect_of(kernel: &Arc<Kernel>, id: &str) -> Value {
        let outcomes = kernel
            .send(vec![Item::new(id, json!({"type": "reflect"}))])
            .await;
        outcomes[0].result.clone()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn seeding_twice_is_a_no_op() {
        let dir = tempfile::TempDir::new().unwrap();
        let kernel = boot_unstarted(dir.path());
        assert!(seed_base_tree(&kernel).await.unwrap());
        let before = reflect_of(&kernel, DEFAULT_ROOT_ID).await;
        assert!(seed_base_tree(&kernel).await.is_ok());
        let after = reflect_of(&kernel, DEFAULT_ROOT_ID).await;
        assert_eq!(before, after);
        kernel.shutdown().await;
        bootstrap::shutdown(dir.path()).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_reflect_is_reported_not_reseeded() {
        // A kernel with no root: the reflect is refused, so nothing is seeded.
        let kernel = Arc::new(Kernel::new());
        let err = seed_base_tree(&kernel).await.unwrap_err();
        assert!(
            err.starts_with("base tree: reflect failed"),
            "wrong error: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_seeded_store_has_no_ingress_rule() {
        let dir = tempfile::TempDir::new().unwrap();
        let kernel = boot_unstarted(dir.path());
        assert!(seed_base_tree(&kernel).await.unwrap());
        let store = reflect_of(&kernel, "store").await;
        // The reflect reports the resolved posture: sealed is `deny_ingress`.
        assert_eq!(store["ingress_rule"], "deny_ingress");
        assert_eq!(store["sealed"], true);
        kernel.shutdown().await;
        bootstrap::shutdown(dir.path()).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_seed_releases_the_lock() {
        let dir = tempfile::TempDir::new().unwrap();
        // A persisted `store` agent already exists, so the seed's own
        // `create_agent store` fails — after the lock is taken.
        let agent = dir.path().join(".fantastic/agents/store");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(
            agent.join("agent.yaml"),
            "id: store\nhandler_module: file_bridge.tools\nroot: .\n",
        )
        .unwrap();
        let r = start(dir.path()).await;
        let err = r.err().expect("start must fail").to_string();
        assert!(
            err.contains("base tree seeding failed"),
            "wrong failure: {err}"
        );
        assert!(lock::acquire(dir.path()).is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_headless_host_boots_and_answers_reflect() {
        let dir = tempfile::TempDir::new().unwrap();
        let kernel = start(dir.path()).await.unwrap();
        let outcomes = kernel
            .send(vec![Item::new(DEFAULT_ROOT_ID, json!({"type": "reflect"}))])
            .await;
        let first = &outcomes[0];
        assert!(matches!(first.status, Status::Completed));
        assert!(first.result.get("id").is_some());
        kernel.shutdown().await;
        bootstrap::shutdown(dir.path()).unwrap();
    }
}
