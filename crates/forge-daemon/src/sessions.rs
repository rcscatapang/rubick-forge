//! Starting, stopping and re-adopting the sessions tasks run in.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use forge_core::{tmux_session_name, AgentStatus, ForgeEvent, Project, Session, StopReason, Task};

use crate::bus::Bus;
use crate::config::Config;
use crate::runtime::SessionRuntime;
use crate::store::{Store, StoreError, TaskPatch};

/// Sessions the daemon owns are named `forge-<task-id>`, which is also the
/// name a human types into `tmux attach`.
pub const SESSION_PREFIX: &str = "forge-";

/// How often a stop checks whether the agent has taken the hint.
const GRACE_POLL: Duration = Duration::from_millis(100);

pub struct SessionManager<R: SessionRuntime> {
    runtime: R,
    store: Store,
    bus: Bus,
    config: Arc<Config>,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionManagerError {
    #[error("task {0} is already running")]
    AlreadyRunning(i64),
    #[error("task {0} is not running")]
    NotRunning(i64),
    /// Something the user can fix: tmux is missing, too old, or refusing.
    #[error("the session runtime is unusable: {0}")]
    Runtime(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Storage errors that are not the caller's fault keep their nature rather
/// than being reported as a broken tmux.
impl From<crate::store::SessionError> for SessionManagerError {
    fn from(err: crate::store::SessionError) -> Self {
        use crate::store::SessionError;
        match err {
            SessionError::AlreadyRunning(id) => Self::AlreadyRunning(id),
            SessionError::Store(store) => Self::Store(store),
            SessionError::NotFound(id) => {
                Self::Store(StoreError::Corrupt(format!("session {id} disappeared")))
            }
        }
    }
}

/// What a reconciliation pass found on boot.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Reconciliation {
    /// Sessions that were still there and are now being watched again.
    pub adopted: usize,
    /// Rows whose tmux session had disappeared.
    pub vanished: usize,
    /// `forge-` sessions with no row of their own, left alone.
    pub unclaimed: usize,
}

impl<R: SessionRuntime> SessionManager<R> {
    pub fn new(runtime: R, store: Store, bus: Bus, config: Arc<Config>) -> Self {
        Self {
            runtime,
            store,
            bus,
            config,
        }
    }

    pub fn runtime(&self) -> &R {
        &self.runtime
    }

    /// Whether the runtime can be used at all.
    pub async fn preflight(&self) -> Result<(), SessionManagerError> {
        self.runtime
            .preflight()
            .await
            .map_err(|err| SessionManagerError::Runtime(err.to_string()))
    }

    /// Start a session for `task`, running `command` in its working directory.
    pub async fn start(
        &self,
        task: &Task,
        project: &Project,
        command: &[String],
    ) -> Result<Session, SessionManagerError> {
        self.preflight().await?;

        let name = tmux_session_name(task.id);

        // Reality first: a session left over from a previous life would make
        // the row and the runtime disagree from the outset.
        if self.alive(&name).await? {
            return Err(SessionManagerError::AlreadyRunning(task.id));
        }
        if self.store.live_session(task.id)?.is_some() {
            return Err(SessionManagerError::AlreadyRunning(task.id));
        }

        let session = self.store.start_session(task.id, &name)?;

        let cwd = task.working_dir(project).to_owned();
        if let Err(err) = self.runtime.create(&name, Path::new(&cwd), command).await {
            // The row exists but nothing is running. `create` takes its own
            // half-built session back down, so closing the row is all that is
            // left; no session ever started, so nothing announces stopping.
            if let Err(cleanup) = self.store.end_session(session.id, AgentStatus::Error) {
                tracing::error!(session = session.id, error = %cleanup, "cannot close a failed session");
            }
            return Err(SessionManagerError::Runtime(err.to_string()));
        }

        self.record_pane(&session).await;

        self.set_task_status(task.id, AgentStatus::Idle)?;
        self.bus.publish(ForgeEvent::SessionStarted {
            task_id: task.id,
            session_id: session.id,
            tmux_name: name,
        })?;

        Ok(self.store.session(session.id)?.unwrap_or(session))
    }

    /// Ask the agent to stop, then insist.
    ///
    /// The interrupt gives a CLI the chance to shut down cleanly — flushing
    /// its own state, releasing its own locks — before the session is killed
    /// out from under it.
    pub async fn stop(&self, task_id: i64) -> Result<Session, SessionManagerError> {
        let session = self
            .store
            .live_session(task_id)?
            .ok_or(SessionManagerError::NotRunning(task_id))?;

        let name = session.tmux_name.clone();

        // A runtime that cannot answer must not strand the row: whatever tmux
        // is doing, the session is unreachable and the task is not running.
        match self.runtime.exists(&name).await {
            Ok(true) => {
                let _ = self
                    .runtime
                    .send_keys(&name, &self.config.stop_keys())
                    .await;
                self.await_exit(&name).await;

                self.runtime
                    .kill(&name)
                    .await
                    .map_err(|err| SessionManagerError::Runtime(err.to_string()))?;
            }
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(session = %name, %error, "cannot reach the runtime; closing the row anyway");
            }
        }

        self.close(&session, AgentStatus::Stopped, StopReason::Requested)
    }

    /// Stop and start again. The task keeps its history: this is a new
    /// session, not a reused one.
    pub async fn restart(
        &self,
        task: &Task,
        project: &Project,
        command: &[String],
    ) -> Result<Session, SessionManagerError> {
        if self.store.live_session(task.id)?.is_some() {
            self.stop(task.id).await?;
        }
        self.start(task, project, command).await
    }

    /// Reconcile the sessions table against what the runtime actually has.
    ///
    /// Run at boot, when the two can have drifted apart while the daemon was
    /// not there to watch: this is the whole of "sessions survive a restart".
    pub async fn reconcile(&self) -> Result<Reconciliation, SessionManagerError> {
        let running = self
            .runtime
            .list()
            .await
            .map_err(|err| SessionManagerError::Runtime(err.to_string()))?;

        let mut found = Reconciliation::default();
        let mut claimed: Vec<String> = Vec::new();

        for session in self.store.live_sessions()? {
            if running.iter().any(|name| name == &session.tmux_name) {
                found.adopted += 1;
                claimed.push(session.tmux_name.clone());
                self.record_pane(&session).await;
                continue;
            }

            found.vanished += 1;
            self.close(&session, AgentStatus::Stopped, StopReason::Vanished)?;
        }

        for name in running {
            // A session someone else named `forge-…` is not ours to adopt or
            // to kill; say so once and leave it alone.
            if name.starts_with(SESSION_PREFIX) && !claimed.contains(&name) {
                found.unclaimed += 1;
                tracing::info!(session = %name, "ignoring a forge- session this daemon has no record of");
            }
        }

        tracing::info!(
            adopted = found.adopted,
            vanished = found.vanished,
            unclaimed = found.unclaimed,
            "reconciled sessions"
        );

        Ok(found)
    }

    /// Record what the runtime says the session's process is.
    async fn record_pane(&self, session: &Session) {
        match self.runtime.pane_state(&session.tmux_name).await {
            Ok(state) => {
                if let Err(error) = self.store.set_session_pid(session.id, state.pid) {
                    tracing::warn!(session = session.id, %error, "cannot record a pane pid");
                }
            }
            Err(error) => {
                tracing::warn!(session = session.id, %error, "cannot read a pane's state");
            }
        }
    }

    /// Notice a session whose process has exited, without waiting for a boot.
    ///
    /// `remain-on-exit` keeps the pane alive after its process dies, so an
    /// agent that finished or crashed is still there to be asked how it went.
    async fn sweep_dead_panes(&self) -> Result<usize, SessionManagerError> {
        let mut ended = 0;

        for session in self.store.live_sessions()? {
            let Ok(state) = self.runtime.pane_state(&session.tmux_name).await else {
                continue;
            };
            if state.alive {
                continue;
            }

            // A non-zero exit is a crash, not a finish.
            let status = match state.exit_code {
                Some(0) | None => AgentStatus::Stopped,
                Some(_) => AgentStatus::Error,
            };

            let _ = self.runtime.kill(&session.tmux_name).await;
            self.close(&session, status, StopReason::Exited)?;
            ended += 1;
        }

        Ok(ended)
    }

    /// One pass of the watch loop.
    pub async fn poll(&self) -> Result<Reconciliation, SessionManagerError> {
        let found = self.reconcile().await?;
        self.sweep_dead_panes().await?;
        Ok(found)
    }

    /// End a session row, mirror the status onto its task, and announce it.
    fn close(
        &self,
        session: &Session,
        status: AgentStatus,
        reason: StopReason,
    ) -> Result<Session, SessionManagerError> {
        let ended = self.store.end_session(session.id, status)?;

        self.set_task_status(session.task_id, status)?;
        self.bus.publish(ForgeEvent::SessionStopped {
            task_id: session.task_id,
            session_id: session.id,
            reason,
        })?;

        Ok(ended)
    }

    /// Mirror a session's status onto its task.
    ///
    /// A task deleted while its session was being closed is not an error worth
    /// failing the close for — the row is already gone.
    fn set_task_status(&self, task_id: i64, status: AgentStatus) -> Result<(), StoreError> {
        match self.store.update_task(
            task_id,
            &TaskPatch {
                status: Some(status),
                ..TaskPatch::default()
            },
        ) {
            Ok(_) => Ok(()),
            Err(crate::store::TaskError::Store(store)) => Err(store),
            Err(gone) => {
                tracing::debug!(task = task_id, error = %gone, "not mirroring status onto a task that is gone");
                Ok(())
            }
        }
    }

    async fn alive(&self, name: &str) -> Result<bool, SessionManagerError> {
        self.runtime
            .exists(name)
            .await
            .map_err(|err| SessionManagerError::Runtime(err.to_string()))
    }

    /// Give the interrupted process a moment to leave on its own.
    async fn await_exit(&self, name: &str) {
        let deadline = tokio::time::Instant::now() + self.config.stop_grace();

        while tokio::time::Instant::now() < deadline {
            match self.runtime.pane_state(name).await {
                Ok(state) if !state.alive => return,
                Err(_) => return,
                Ok(_) => tokio::time::sleep(GRACE_POLL).await,
            }
        }
    }
}
