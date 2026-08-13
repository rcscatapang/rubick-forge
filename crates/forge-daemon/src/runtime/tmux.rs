//! Driving tmux through its CLI.

use std::path::Path;
use std::time::Duration;

use super::{PaneState, SessionRuntime};
use crate::exec::{self, ExecError, Output};

const BINARY: &str = "tmux";

/// Below this, the `#{pane_dead}` and `#{pane_dead_status}` formats this code
/// relies on are missing or unreliable.
pub const MINIMUM_VERSION: (u32, u32) = (3, 0);

/// tmux answers locally and instantly; anything slower means it is wedged.
const TIMEOUT: Duration = Duration::from_secs(10);

/// A local fork takes milliseconds; this is the point at which it has failed.
const RESPAWN_TIMEOUT: Duration = Duration::from_secs(5);
const RESPAWN_POLL: Duration = Duration::from_millis(10);

/// How tmux says "that is not here" — as opposed to failing for some other
/// reason, which must not be read as an absent session.
const ABSENT: [&str; 4] = [
    "can't find session",
    "no such session",
    "no server running",
    "error connecting to",
];

/// The tmux server the daemon talks to.
///
/// A socket name isolates a server completely, which is how tests avoid
/// touching the user's own tmux.
#[derive(Debug, Clone)]
pub struct TmuxRuntime {
    binary: String,
    socket: Option<String>,
}

impl Default for TmuxRuntime {
    fn default() -> Self {
        Self {
            binary: BINARY.to_owned(),
            socket: None,
        }
    }
}

impl TmuxRuntime {
    /// The user's own tmux server — the one `tmux attach` reaches.
    pub fn new() -> Self {
        Self::default()
    }

    /// A private server under `-L <label>`.
    pub fn with_socket(label: impl Into<String>) -> Self {
        Self {
            socket: Some(label.into()),
            ..Self::default()
        }
    }

    /// Run a specific tmux rather than whatever `PATH` finds first.
    pub fn with_binary(self, binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
            ..self
        }
    }

    async fn run<S: AsRef<std::ffi::OsStr>>(&self, args: &[S]) -> Result<Output, TmuxError> {
        let mut full: Vec<std::ffi::OsString> = Vec::with_capacity(args.len() + 2);
        if let Some(socket) = &self.socket {
            full.push("-L".into());
            full.push(socket.into());
        }
        full.extend(args.iter().map(|arg| arg.as_ref().to_owned()));

        exec::run(&self.binary, &full, None, TIMEOUT)
            .await
            .map_err(|source| match source {
                ExecError::NotFound(_) => TmuxError::Missing,
                other => TmuxError::Failed {
                    detail: other.to_string(),
                },
            })
    }

    /// Run a command that must succeed, turning a non-zero exit into an error.
    async fn require<S: AsRef<std::ffi::OsStr>>(&self, args: &[S]) -> Result<Output, TmuxError> {
        let output = self.run(args).await?;
        if output.success() {
            return Ok(output);
        }
        Err(TmuxError::Failed {
            detail: output.first_error_line().to_owned(),
        })
    }

    /// The server's version, as tmux reports it.
    pub async fn version(&self) -> Result<Version, TmuxError> {
        let output = self.require(&["-V"]).await?;
        Version::parse(output.stdout.trim()).ok_or(TmuxError::UnreadableVersion(
            output.stdout.trim().to_owned(),
        ))
    }

    /// Everything after the session itself exists.
    async fn finish_create(
        &self,
        name: &str,
        cwd: std::ffi::OsString,
        command: &[String],
    ) -> Result<(), TmuxError> {
        self.require(&["set-option", "-t", name, "-w", "remain-on-exit", "on"])
            .await?;

        if command.is_empty() {
            return Ok(());
        }

        let placeholder = self.pane_state(name).await?.pid;

        // Everything after `--` is the command and its arguments, so no part
        // of it can be read as a tmux option or reach a shell.
        let mut args: Vec<std::ffi::OsString> = vec![
            "respawn-pane".into(),
            "-k".into(),
            "-t".into(),
            name.into(),
            "-c".into(),
            cwd,
            "--".into(),
        ];
        args.extend(command.iter().map(Into::into));
        self.require(&args).await?;

        self.await_respawn(name, placeholder).await
    }

    /// Block until the pane is running something other than `previous`.
    ///
    /// Respawning kills one process and forks another; input sent into that
    /// gap goes nowhere. Callers that start a session and immediately send it
    /// a prompt depend on this having closed.
    async fn await_respawn(&self, name: &str, previous: Option<i64>) -> Result<(), TmuxError> {
        let deadline = tokio::time::Instant::now() + RESPAWN_TIMEOUT;

        loop {
            let state = self.pane_state(name).await?;
            if state.pid.is_some() && state.pid != previous {
                return Ok(());
            }
            // A command that exits instantly is up as far as anyone can tell.
            if !state.alive {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(TmuxError::Failed {
                    detail: format!("the pane in session {name} never started its command"),
                });
            }
            tokio::time::sleep(RESPAWN_POLL).await;
        }
    }

    /// Ask tmux one question about a session and get its answer back.
    ///
    /// A failed query is only reported as a missing session once that has been
    /// confirmed; otherwise a wedged server would be indistinguishable from an
    /// agent that finished.
    async fn format(&self, name: &str, format: &str) -> Result<String, TmuxError> {
        let output = self
            .run(&["display-message", "-p", "-t", name, format])
            .await?;

        if output.success() {
            return Ok(output.stdout.trim().to_owned());
        }
        Err(self.explain_failure(name, &output).await)
    }

    /// Decide whether a failed command means "no such session" or something
    /// worse.
    async fn explain_failure(&self, name: &str, output: &Output) -> TmuxError {
        match self.run(&["has-session", "-t", name]).await {
            Ok(probe) if !probe.success() => TmuxError::NoSuchSession(name.to_owned()),
            _ => TmuxError::Failed {
                detail: output.first_error_line().to_owned(),
            },
        }
    }
}

/// A tmux version, compared as a tuple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
}

/// Whether a reported tmux version is usable, and why not when it is not.
///
/// One place decides this, so `/health` and a refused start never disagree.
pub fn version_complaint(reported: &str) -> Option<String> {
    match Version::parse(reported) {
        Some(version) if version.is_at_least(MINIMUM_VERSION) => None,
        Some(version) => Some(
            TmuxError::TooOld {
                found: version,
                needed: MINIMUM_VERSION,
            }
            .to_string(),
        ),
        None => Some(TmuxError::UnreadableVersion(reported.to_owned()).to_string()),
    }
}

impl Version {
    /// Reads `tmux 3.4`, `tmux 3.7b`, `tmux next-3.6` and `tmux openbsd-7.5`.
    ///
    /// Point releases carry a letter suffix rather than a third number, and it
    /// never matters for a minimum check, so it is dropped.
    pub fn parse(reported: &str) -> Option<Self> {
        let digits = reported
            .split(|c: char| !(c.is_ascii_digit() || c == '.'))
            .find(|part| part.contains('.'))?;

        let (major, minor) = digits.split_once('.')?;
        Some(Self {
            major: major.parse().ok()?,
            minor: minor.trim_end_matches('.').parse().ok()?,
        })
    }

    pub fn is_at_least(self, (major, minor): (u32, u32)) -> bool {
        (self.major, self.minor) >= (major, minor)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl SessionRuntime for TmuxRuntime {
    type Error = TmuxError;

    async fn preflight(&self) -> Result<(), TmuxError> {
        let reported = self.require(&["-V"]).await?.stdout.trim().to_owned();

        match version_complaint(&reported) {
            None => Ok(()),
            Some(_) => match Version::parse(&reported) {
                Some(found) => Err(TmuxError::TooOld {
                    found,
                    needed: MINIMUM_VERSION,
                }),
                None => Err(TmuxError::UnreadableVersion(reported)),
            },
        }
    }

    /// Start the session empty, then respawn the pane with the real command.
    ///
    /// The detour buys `remain-on-exit`, which has to be set *before* the
    /// command runs: a command that exits immediately would otherwise take the
    /// pane, the session, and the exit code that says how it ended with it.
    /// Forge kills the session itself once it is done with it.
    async fn create(&self, name: &str, cwd: &Path, command: &[String]) -> Result<(), TmuxError> {
        let cwd: std::ffi::OsString = cwd.into();

        self.require(&[
            std::ffi::OsString::from("new-session"),
            "-d".into(),
            "-s".into(),
            name.into(),
            "-c".into(),
            cwd.clone(),
        ])
        .await?;

        // From here the session exists, so anything that goes wrong has to
        // take it back down: a session with no row to stop it is unreachable
        // from the API and blocks the task from ever starting again.
        match self.finish_create(name, cwd, command).await {
            Ok(()) => Ok(()),
            Err(err) => {
                if let Err(cleanup) = self.kill(name).await {
                    tracing::error!(session = %name, error = %cleanup, "cannot remove a half-built session");
                }
                Err(err)
            }
        }
    }

    async fn kill(&self, name: &str) -> Result<(), TmuxError> {
        let output = self.run(&["kill-session", "-t", name]).await?;

        // Killing something already gone is the state the caller wanted.
        if output.success() || !self.exists(name).await? {
            return Ok(());
        }
        Err(TmuxError::Failed {
            detail: output.first_error_line().to_owned(),
        })
    }

    async fn exists(&self, name: &str) -> Result<bool, TmuxError> {
        // `has-session` exits non-zero both for "no such session" and for a
        // server that will not answer, and only the first is a `false`.
        let output = self.run(&["has-session", "-t", name]).await?;
        if output.success() {
            return Ok(true);
        }

        let complaint = output.first_error_line().to_lowercase();
        if ABSENT.iter().any(|phrase| complaint.contains(phrase)) {
            return Ok(false);
        }

        Err(TmuxError::Failed {
            detail: output.first_error_line().to_owned(),
        })
    }

    async fn list(&self) -> Result<Vec<String>, TmuxError> {
        let output = self
            .run(&["list-sessions", "-F", "#{session_name}"])
            .await?;

        // No server running at all means no sessions, not a failure.
        if !output.success() {
            return Ok(Vec::new());
        }

        Ok(output
            .stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    }

    async fn send_keys(&self, name: &str, keys: &[&str]) -> Result<(), TmuxError> {
        let mut args = vec!["send-keys", "-t", name, "--"];
        args.extend_from_slice(keys);
        self.require(&args).await.map(|_| ())
    }

    async fn paste(&self, name: &str, text: &str) -> Result<(), TmuxError> {
        // Through a buffer rather than `send-keys`, so newlines stay newlines
        // and nothing in the text is read as a key name.
        let buffer = format!("forge-{name}");
        self.require(&["set-buffer", "-b", &buffer, "--", text])
            .await?;
        self.require(&["paste-buffer", "-d", "-b", &buffer, "-t", name])
            .await
            .map(|_| ())
    }

    async fn capture(&self, name: &str, lines: u32) -> Result<String, TmuxError> {
        let start = format!("-{lines}");
        let output = self
            .run(&["capture-pane", "-p", "-t", name, "-S", &start])
            .await?;

        if !output.success() {
            return Err(self.explain_failure(name, &output).await);
        }
        Ok(output.stdout)
    }

    async fn pane_state(&self, name: &str) -> Result<PaneState, TmuxError> {
        let reported = self
            .format(name, "#{pane_pid} #{pane_dead} #{pane_dead_status}")
            .await?;

        let mut fields = reported.split_whitespace();
        let pid = fields.next().and_then(|pid| pid.parse().ok());
        let dead = fields.next() == Some("1");
        let exit_code = fields.next().and_then(|code| code.parse().ok());

        Ok(PaneState {
            pid,
            alive: !dead,
            // tmux reports a status only once the process has actually exited.
            exit_code: dead.then_some(exit_code).flatten(),
        })
    }

    async fn activity(&self, name: &str) -> Result<Option<u64>, TmuxError> {
        let reported = self.format(name, "#{window_activity}").await?;
        Ok(reported.parse().ok())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TmuxError {
    #[error("`tmux` was not found on PATH; Forge needs it to run agents")]
    Missing,
    #[error("tmux {found} is too old; Forge needs {}.{} or newer", needed.0, needed.1)]
    TooOld { found: Version, needed: (u32, u32) },
    #[error("cannot read the tmux version from {0:?}")]
    UnreadableVersion(String),
    #[error("there is no tmux session named {0}")]
    NoSuchSession(String),
    #[error("tmux failed: {detail}")]
    Failed { detail: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_from_what_tmux_actually_prints() {
        for (reported, expected) in [
            ("tmux 3.4", Version { major: 3, minor: 4 }),
            ("tmux 3.7b", Version { major: 3, minor: 7 }),
            ("tmux next-3.6", Version { major: 3, minor: 6 }),
            ("tmux openbsd-7.5", Version { major: 7, minor: 5 }),
            ("tmux 2.9a", Version { major: 2, minor: 9 }),
        ] {
            assert_eq!(Version::parse(reported), Some(expected), "{reported}");
        }
    }

    #[test]
    fn nonsense_is_not_guessed_at() {
        assert_eq!(Version::parse("tmux"), None);
        assert_eq!(Version::parse(""), None);
    }

    #[test]
    fn the_floor_is_decided_in_one_place() {
        assert_eq!(version_complaint("tmux 3.4"), None);
        assert_eq!(version_complaint("tmux 3.7b"), None);
        assert!(version_complaint("tmux 2.9a").unwrap().contains("too old"));
        // Unreadable is unusable, not "probably fine".
        assert!(version_complaint("tmux").is_some());
    }

    #[test]
    fn the_minimum_is_compared_as_numbers_not_text() {
        assert!(Version {
            major: 3,
            minor: 10
        }
        .is_at_least((3, 9)));
        assert!(Version { major: 3, minor: 0 }.is_at_least(MINIMUM_VERSION));
        assert!(!Version { major: 2, minor: 9 }.is_at_least(MINIMUM_VERSION));
    }
}
