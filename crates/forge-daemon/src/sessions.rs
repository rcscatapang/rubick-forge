//! Starting, stopping and re-adopting the sessions tasks run in.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use forge_core::{tmux_session_name, AgentStatus, ForgeEvent, Project, Session, StopReason, Task};

use crate::adapters;
use crate::adapters::markers::tail_of;
use crate::adapters::status::{Outcome, Tracker};
use crate::bus::Bus;
use crate::config::Config;
use crate::runtime::{SessionRuntime, CAPTURE_LINES};
use crate::store::{Store, StoreError, TaskPatch};

/// Sessions the daemon owns are named `forge-<task-id>`, which is also the
/// name a human types into `tmux attach`.
pub const SESSION_PREFIX: &str = "forge-";

/// How often a stop checks whether the agent has taken the hint.
const GRACE_POLL: Duration = Duration::from_millis(100);

/// How much of a pane goes into an event payload.
///
/// Enough to see the question being asked, and short enough to fit a
/// notification. ANSI is already gone: tmux captures plain text unless asked
/// otherwise.
const PAYLOAD_LINES: usize = 12;
const PAYLOAD_CHARS: usize = 800;

/// The part of a pane worth showing a person, length-capped.
fn pane_tail(pane: &str) -> String {
    let tail = tail_of(pane, PAYLOAD_LINES);

    let trimmed: Vec<&str> = tail.lines().skip_while(|line| line.is_empty()).collect();

    let text = trimmed.join("\n");
    if text.chars().count() > PAYLOAD_CHARS {
        // Keep the end: the question is at the bottom.
        let skip = text.chars().count() - PAYLOAD_CHARS;
        return String::from("…") + &text.chars().skip(skip).collect::<String>();
    }
    text
}

pub struct SessionManager<R: SessionRuntime> {
    runtime: R,
    store: Store,
    bus: Bus,
    config: Arc<Config>,
    /// What the poller believes about each live session, keyed by session id.
    /// Deliberately not persisted: a restart re-derives it from the pane.
    trackers: Mutex<HashMap<i64, Tracker>>,
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
            trackers: Mutex::new(HashMap::new()),
        }
    }

    /// Send text to a running agent as if it had been typed.
    pub async fn send_instruction(
        &self,
        task_id: i64,
        text: &str,
    ) -> Result<(), SessionManagerError> {
        let session = self
            .store
            .live_session(task_id)?
            .ok_or(SessionManagerError::NotRunning(task_id))?;

        // Through a paste buffer, then a separate Enter: the text may contain
        // newlines and anything else, and none of it may be read as keys.
        self.runtime
            .paste(&session.tmux_name, text)
            .await
            .map_err(|err| SessionManagerError::Runtime(err.to_string()))?;

        self.runtime
            .send_keys(&session.tmux_name, &["Enter"])
            .await
            .map_err(|err| SessionManagerError::Runtime(err.to_string()))
    }

    /// Answer a dialog the agent is showing, as keystrokes.
    ///
    /// Deliberately not [`send_instruction`](Self::send_instruction): that
    /// pastes text and presses Enter, which is right for a prompt and wrong
    /// for a numbered list where `1` selects and Escape cancels.
    pub async fn send_answer(
        &self,
        task_id: i64,
        keys: &[&str],
    ) -> Result<(), SessionManagerError> {
        let session = self
            .store
            .live_session(task_id)?
            .ok_or(SessionManagerError::NotRunning(task_id))?;

        self.runtime
            .send_keys(&session.tmux_name, keys)
            .await
            .map_err(|err| SessionManagerError::Runtime(err.to_string()))
    }

    /// What is on a task's screen now, trimmed the way an event payload is.
    pub async fn pane_tail(&self, task_id: i64) -> Result<String, SessionManagerError> {
        let session = self
            .store
            .live_session(task_id)?
            .ok_or(SessionManagerError::NotRunning(task_id))?;

        let pane = self
            .runtime
            .capture(&session.tmux_name, CAPTURE_LINES)
            .await
            .map_err(|err| SessionManagerError::Runtime(err.to_string()))?;

        Ok(pane_tail(&pane))
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

    /// Read every live session once and act on what it says.
    ///
    /// Ordered exactly as documented: a dead process decides the outcome
    /// whatever the screen shows, then the adapter's markers are tried, and an
    /// unrecognised screen keeps the current belief up to a staleness cap.
    ///
    /// One session's failure does not abandon the rest: whatever went wrong is
    /// logged and the next session is read.
    async fn read_sessions(&self) -> Result<usize, SessionManagerError> {
        let mut examined = 0;

        for session in self.store.live_sessions()? {
            match self.read_session(&session).await {
                Ok(()) => examined += 1,
                Err(error) => {
                    tracing::warn!(session = session.id, %error, "cannot read a session");
                }
            }
        }

        Ok(examined)
    }

    async fn read_session(&self, session: &Session) -> Result<(), SessionManagerError> {
        let pane = self
            .runtime
            .pane_state(&session.tmux_name)
            .await
            .map_err(|err| SessionManagerError::Runtime(err.to_string()))?;

        let outcome = self.examine(session, &pane).await?;

        // The agent's process has gone. Its pane is still standing because
        // `remain-on-exit` is what let us read how it ended, so take the
        // session down before the row says it is over.
        if outcome.process_ended {
            if let Err(error) = self.runtime.kill(&session.tmux_name).await {
                tracing::warn!(session = %session.tmux_name, %error, "cannot remove a finished session");
            }
        }

        self.act_on(session, &outcome).await
    }

    /// Work out what the session's state is now.
    async fn examine(
        &self,
        session: &Session,
        pane: &crate::runtime::PaneState,
    ) -> Result<Outcome, SessionManagerError> {
        let patterns = self.patterns_for(session.task_id)?;

        // Capturing is the expensive part of a poll, so it is skipped when the
        // pane is dead — whose verdict does not depend on the screen — and
        // when tmux says the pane has produced nothing since last time.
        if !pane.alive {
            let mut trackers = self.trackers();
            let tracker = self.tracker_for(&mut trackers, session);
            return Ok(tracker.poll(pane, None, patterns));
        }

        let activity = self
            .runtime
            .activity(&session.tmux_name)
            .await
            .ok()
            .flatten();

        {
            let mut trackers = self.trackers();
            let tracker = self.tracker_for(&mut trackers, session);
            if tracker.is_unchanged_since(activity) {
                return Ok(tracker.skipped());
            }
        }

        let output = self
            .runtime
            .capture(&session.tmux_name, CAPTURE_LINES)
            .await
            .ok();

        let mut trackers = self.trackers();
        let tracker = self.tracker_for(&mut trackers, session);
        tracker.saw_activity(activity);
        Ok(tracker.poll(pane, output.as_deref(), patterns))
    }

    fn tracker_for<'a>(
        &self,
        trackers: &'a mut HashMap<i64, Tracker>,
        session: &Session,
    ) -> &'a mut Tracker {
        trackers
            .entry(session.id)
            .or_insert_with(|| Tracker::new(session.status))
    }

    /// Persist what a reading means, then announce it.
    ///
    /// Writes come before events: a client that hears about a status and then
    /// reads a different one from the API has been lied to.
    async fn act_on(
        &self,
        session: &Session,
        outcome: &Outcome,
    ) -> Result<(), SessionManagerError> {
        if outcome.process_ended {
            self.close(session, outcome.status, StopReason::Exited)?;
            self.report_ending(session, outcome)?;
            return Ok(());
        }

        let Some(from) = outcome.changed_from else {
            return Ok(());
        };

        self.store.set_session_status(session.id, outcome.status)?;
        self.set_task_status(session.task_id, outcome.status)?;

        self.bus.publish(ForgeEvent::StatusChanged {
            task_id: session.task_id,
            session_id: session.id,
            from,
            to: outcome.status,
        })?;

        if outcome.entered_waiting {
            self.bus.publish(ForgeEvent::AgentWaiting {
                task_id: session.task_id,
                session_id: session.id,
                tail: self.last_tail(session).await,
            })?;
        }

        if outcome.status == AgentStatus::Error {
            self.bus.publish(ForgeEvent::AgentError {
                task_id: session.task_id,
                session_id: session.id,
                detail: self.last_tail(session).await,
            })?;
        }

        Ok(())
    }

    /// Say how a session ended, once its row is closed.
    fn report_ending(
        &self,
        session: &Session,
        outcome: &Outcome,
    ) -> Result<(), SessionManagerError> {
        if let Some(from) = outcome.changed_from {
            self.bus.publish(ForgeEvent::StatusChanged {
                task_id: session.task_id,
                session_id: session.id,
                from,
                to: outcome.status,
            })?;
        }

        match outcome.status {
            AgentStatus::Error => {
                self.bus.publish(ForgeEvent::AgentError {
                    task_id: session.task_id,
                    session_id: session.id,
                    detail: format!("{} exited unexpectedly", session.tmux_name),
                })?;
            }
            // A task that only ever sat at a prompt did not "finish"; saying
            // so would notify the user about nothing happening.
            _ if outcome
                .changed_from
                .is_some_and(|from| from != AgentStatus::Idle) =>
            {
                self.bus.publish(ForgeEvent::TaskFinished {
                    task_id: session.task_id,
                    session_id: session.id,
                })?;
            }
            _ => {}
        }

        Ok(())
    }

    /// The pane as it last looked, for an event payload.
    async fn last_tail(&self, session: &Session) -> String {
        match self
            .runtime
            .capture(&session.tmux_name, CAPTURE_LINES)
            .await
        {
            Ok(pane) => pane_tail(&pane),
            Err(_) => String::new(),
        }
    }

    /// The markers for whichever adapter a task runs.
    fn patterns_for(
        &self,
        task_id: i64,
    ) -> Result<&'static crate::adapters::markers::StatusPatterns, SessionManagerError> {
        let task = self
            .store
            .task(task_id)?
            .ok_or_else(|| StoreError::Corrupt(format!("task {task_id} is gone")))?;

        Ok(adapters::adapter(task.adapter).status_patterns())
    }

    fn trackers(&self) -> std::sync::MutexGuard<'_, HashMap<i64, Tracker>> {
        self.trackers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn forget(&self, session_id: i64) {
        self.trackers().remove(&session_id);
    }

    /// One pass of the watch loop.
    pub async fn poll(&self) -> Result<Reconciliation, SessionManagerError> {
        let found = self.reconcile().await?;
        self.read_sessions().await?;
        Ok(found)
    }

    /// End a session row, mirror the status onto its task, and announce it.
    fn close(
        &self,
        session: &Session,
        status: AgentStatus,
        reason: StopReason,
    ) -> Result<Session, SessionManagerError> {
        // Somebody else may have closed it between reading and deciding; the
        // row is theirs to announce, not ours.
        if !self.store.close_session(session.id, status)? {
            self.forget(session.id);
            return Ok(self.store.session(session.id)?.unwrap_or(session.clone()));
        }

        let ended = self.store.session(session.id)?.unwrap_or(session.clone());

        self.set_task_status(session.task_id, status)?;
        self.bus.publish(ForgeEvent::SessionStopped {
            task_id: session.task_id,
            session_id: session.id,
            reason,
        })?;
        self.forget(session.id);

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
