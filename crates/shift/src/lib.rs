//! fantastic_shift as a library: a headless fantastic host that boots in a
//! directory and runs until stopped. `main.rs` is a thin wrapper around `run`.
#![deny(missing_docs)]

pub mod config;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fantastic_kernel::bootstrap::{self, BootstrapOptions, DEFAULT_ROOT_ID};
use fantastic_kernel::{lock, BundleRegistry, Item, Kernel, Status};
use serde_json::json;

pub mod worker_exec;

/// The bundle registry the shift boots with. fantastic's default set comes
/// from fantastic-host; this repo's own bundles (queue, connectors) are added
/// here as they land.
pub fn registry() -> BundleRegistry {
    let mut reg = fantastic_host::default_bundles();
    reg.register(
        shift_git_connector::HANDLER_MODULE,
        shift_git_connector::GitConnectorBundle,
    );
    reg
}

/// Usage line, printed on `--help` and on bad arguments.
pub const USAGE: &str = "usage: fantastic_shift [--dir PATH]";

/// Parsed command line.
pub struct Args {
    /// The directory the shift lives in.
    pub dir: PathBuf,
}

/// Parse `--dir PATH` (default: the current directory).
pub fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut dir: Option<PathBuf> = None;
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dir" => {
                let v = it
                    .next()
                    .ok_or_else(|| format!("--dir needs a value\n{USAGE}"))?;
                dir = Some(PathBuf::from(v));
            }
            "--help" | "-h" => return Err(USAGE.to_string()),
            other => return Err(format!("unknown argument `{other}`\n{USAGE}")),
        }
    }
    let dir = match dir {
        Some(d) => d,
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    Ok(Args { dir })
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

/// Run the shift until Ctrl-C or a stop request, then tear down.
pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    let kernel = start(&args.dir).await?;
    println!("[shift] up");
    std::io::stdout().flush()?;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = kernel.shutdown_requested() => {}
    }
    println!("[shift] tearing down");
    kernel.shutdown().await;
    bootstrap::shutdown(&args.dir)?;
    println!("[shift] down");
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
        assert!(names.contains(&"shift_git.tools"));
    }

    #[test]
    fn args_default_to_the_current_directory() {
        let a = parse_args(&[]).unwrap();
        assert_eq!(a.dir, std::env::current_dir().unwrap());
    }

    #[test]
    fn args_take_a_dir() {
        let a = parse_args(&["--dir".to_string(), "/x".to_string()]).unwrap();
        assert_eq!(a.dir, PathBuf::from("/x"));
    }

    #[test]
    fn unknown_args_and_help_are_refused() {
        let e = parse_args(&["--nope".to_string()]).err().unwrap();
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
