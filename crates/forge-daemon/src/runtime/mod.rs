//! Where agent sessions actually live.
//!
//! tmux is a hard dependency: `tmux attach` from any terminal is a supported
//! escape hatch, and sessions surviving a daemon restart falls straight out of
//! it. Everything above this module is written against the trait rather than
//! tmux itself, so what tmux is asked to do stays in one readable place.

pub mod tmux;

use std::future::Future;
use std::path::Path;

pub use tmux::{TmuxError, TmuxRuntime};

/// How much scrollback a status poll reads. Bounded on purpose: the poller
/// runs per session, and a full-scrollback capture would not be free.
pub const CAPTURE_LINES: u32 = 200;

/// A live session's process and activity, as the runtime sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneState {
    /// The process the pane is running.
    pub pid: Option<i64>,
    /// Whether that process is still alive.
    pub alive: bool,
    /// Exit status, once it is not.
    pub exit_code: Option<i32>,
}

/// Create, watch and destroy the sessions agents run in.
pub trait SessionRuntime: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Fail early and clearly when the runtime is unusable, rather than at the
    /// first attempt to start something.
    fn preflight(&self) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Start a detached session named `name`, running `command` in `cwd`.
    fn create(
        &self,
        name: &str,
        cwd: &Path,
        command: &[String],
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn kill(&self, name: &str) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn exists(&self, name: &str) -> impl Future<Output = Result<bool, Self::Error>> + Send;

    /// Every session the runtime currently has, by name.
    fn list(&self) -> impl Future<Output = Result<Vec<String>, Self::Error>> + Send;

    /// Send key *names* — `C-c`, `Enter` — interpreted by the runtime.
    fn send_keys(
        &self,
        name: &str,
        keys: &[&str],
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Send `text` verbatim, however hostile its contents.
    fn paste(&self, name: &str, text: &str)
        -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// The last `lines` lines of the session's visible output.
    fn capture(
        &self,
        name: &str,
        lines: u32,
    ) -> impl Future<Output = Result<String, Self::Error>> + Send;

    /// What the session's process is doing.
    fn pane_state(&self, name: &str)
        -> impl Future<Output = Result<PaneState, Self::Error>> + Send;

    /// A monotonic marker that changes when the session produces output.
    ///
    /// Comparing it against the last one seen is much cheaper than capturing
    /// the pane, so a poller can skip sessions that have done nothing.
    fn activity(&self, name: &str)
        -> impl Future<Output = Result<Option<u64>, Self::Error>> + Send;
}
