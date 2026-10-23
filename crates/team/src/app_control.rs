//! In-process control of the headless team: start, pause, resume, stop. These
//! are the same kernel messages any client sends; there is no UI-only path.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fantastic_kernel::bootstrap::{self, DEFAULT_ROOT_ID};
use fantastic_kernel::{Item, Kernel, Status};
use serde_json::{json, Value};

use crate::start;

/// Where the team is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Started and taking work.
    Running,
    /// Started, but not handing out new work.
    Paused,
    /// Shut down; the handle holds no kernel.
    Stopped,
}

/// Options for [`TeamHandle::start`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TeamOptions {
    /// A web UI port. Not wired into the handle yet; `None` is headless.
    pub port: Option<u16>,
}

/// A running headless team in this process.
pub struct TeamHandle {
    dir: PathBuf,
    kernel: Option<Arc<Kernel>>,
    state: State,
}

impl TeamHandle {
    /// Boot the team in `workdir` and start it. Fails if `opts.port` is set,
    /// because serving a port from a handle is not wired yet.
    pub async fn start(workdir: &Path, opts: TeamOptions) -> Result<TeamHandle, String> {
        if opts.port.is_some() {
            return Err("serving a port from a handle is not wired yet; start headless".into());
        }
        let kernel = start(workdir).await.map_err(|e| e.to_string())?;
        Ok(TeamHandle {
            dir: workdir.to_path_buf(),
            kernel: Some(kernel),
            state: State::Running,
        })
    }

    /// Stop handing out new work. The dispatcher's `control` verb is per node
    /// and has no team-wide pause, so this records the state on the handle and
    /// sends nothing to the team.
    pub fn pause(&mut self) -> Result<(), String> {
        match self.state {
            State::Stopped => Err("team is stopped".into()),
            _ => {
                self.state = State::Paused;
                Ok(())
            }
        }
    }

    /// Resume handing out work after [`TeamHandle::pause`].
    pub fn resume(&mut self) -> Result<(), String> {
        match self.state {
            State::Stopped => Err("team is stopped".into()),
            _ => {
                self.state = State::Running;
                Ok(())
            }
        }
    }

    /// Shut the kernel down and release the directory lock. Calling it again
    /// after a stop does nothing.
    pub async fn stop(&mut self) -> Result<(), String> {
        let Some(kernel) = self.kernel.take() else {
            return Ok(());
        };
        kernel.shutdown().await;
        self.state = State::Stopped;
        bootstrap::shutdown(&self.dir).map_err(|e| e.to_string())
    }

    /// The team's current state.
    pub fn state(&self) -> State {
        self.state
    }

    /// Send `reflect` to the root and return its reply.
    pub async fn reflect(&self) -> Result<Value, String> {
        let kernel = self.kernel.as_ref().ok_or("team is stopped")?;
        let outcomes = kernel
            .send(vec![Item::new(DEFAULT_ROOT_ID, json!({"type": "reflect"}))])
            .await;
        match outcomes.first() {
            Some(o) if matches!(o.status, Status::Completed) => Ok(o.result.clone()),
            Some(o) => Err(format!("reflect failed: {}", o.result)),
            None => Err("reflect failed: no reply".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn start_then_stop_leaves_no_running_task() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut h = TeamHandle::start(dir.path(), TeamOptions::default())
            .await
            .unwrap();
        assert_eq!(h.state(), State::Running);
        h.stop().await.unwrap();
        assert_eq!(h.state(), State::Stopped);
        assert!(h.kernel.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pause_then_resume_changes_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut h = TeamHandle::start(dir.path(), TeamOptions::default())
            .await
            .unwrap();
        h.pause().unwrap();
        assert_eq!(h.state(), State::Paused);
        h.resume().unwrap();
        assert_eq!(h.state(), State::Running);
        h.stop().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn stop_twice_is_ok() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut h = TeamHandle::start(dir.path(), TeamOptions::default())
            .await
            .unwrap();
        h.stop().await.unwrap();
        h.stop().await.unwrap();
        assert_eq!(h.state(), State::Stopped);
        assert!(h.pause().is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn two_handles_in_two_workdirs_run_side_by_side() {
        let a_dir = tempfile::TempDir::new().unwrap();
        let b_dir = tempfile::TempDir::new().unwrap();
        let mut a = TeamHandle::start(a_dir.path(), TeamOptions::default())
            .await
            .unwrap();
        let mut b = TeamHandle::start(b_dir.path(), TeamOptions::default())
            .await
            .unwrap();
        assert!(a.reflect().await.unwrap().get("id").is_some());
        assert!(b.reflect().await.unwrap().get("id").is_some());
        a.stop().await.unwrap();
        assert_eq!(b.state(), State::Running);
        b.stop().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_started_team_answers_reflect() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut h = TeamHandle::start(dir.path(), TeamOptions::default())
            .await
            .unwrap();
        let reply = h.reflect().await.unwrap();
        assert!(reply.get("id").is_some());
        h.stop().await.unwrap();
    }
}
