//! Running external commands with a timeout.
//!
//! Every external tool the daemon drives — `git`, `tmux`, the agent CLIs —
//! goes through here. Arguments are passed as argv, never through a shell, so
//! a task title containing `;` or `$(…)` is data and stays data.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

/// Long enough for a cold command on a large repository, short enough that a
/// wedged tool does not wedge a request.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// stderr's first non-empty line, which is what a tool puts its actual
    /// complaint on. Callers surface this instead of a wall of output.
    pub fn first_error_line(&self) -> &str {
        self.stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("the command failed without saying why")
    }
}

/// Run `program` with `args`, capturing both streams.
///
/// A non-zero exit is an `Ok` [`Output`], not an error; only failing to run the
/// program at all is an [`ExecError`].
pub async fn run<S: AsRef<OsStr>>(
    program: &str,
    args: &[S],
    cwd: Option<&Path>,
    timeout: Duration,
) -> Result<Output, ExecError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }

    let child = command.spawn().map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            ExecError::NotFound(program.to_owned())
        } else {
            ExecError::Spawn {
                program: program.to_owned(),
                source,
            }
        }
    })?;

    // `kill_on_drop` means the timeout also reaps the process.
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| ExecError::TimedOut {
            program: program.to_owned(),
            timeout,
        })?
        .map_err(|source| ExecError::Spawn {
            program: program.to_owned(),
            source,
        })?;

    Ok(Output {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Resolve `program` against `PATH`, the way a shell would.
pub fn which(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let path = PathBuf::from(program);
        return is_executable(&path).then_some(path);
    }

    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(program))
            .find(|candidate| is_executable(candidate))
    })
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("`{0}` was not found on PATH")]
    NotFound(String),
    #[error("cannot run `{program}`: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`{program}` did not finish within {}s", timeout.as_secs())]
    TimedOut { program: String, timeout: Duration },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn output_is_captured_from_both_streams() {
        let output = run(
            "/bin/sh",
            &["-c", "echo out; echo err >&2"],
            None,
            DEFAULT_TIMEOUT,
        )
        .await
        .unwrap();

        assert!(output.success());
        assert_eq!(output.stdout.trim(), "out");
        assert_eq!(output.stderr.trim(), "err");
    }

    #[tokio::test]
    async fn a_non_zero_exit_is_a_result_not_an_error() {
        let output = run("/bin/sh", &["-c", "exit 3"], None, DEFAULT_TIMEOUT)
            .await
            .unwrap();

        assert!(!output.success());
        assert_eq!(output.code, Some(3));
    }

    #[tokio::test]
    async fn arguments_are_argv_and_never_reach_a_shell() {
        // If this went through a shell, the subshell would run and the output
        // would be "pwned".
        let hostile = "$(echo pwned); rm -rf /";
        let output = run("/bin/echo", &[hostile], None, DEFAULT_TIMEOUT)
            .await
            .unwrap();

        assert_eq!(output.stdout.trim(), hostile);
    }

    #[tokio::test]
    async fn the_working_directory_is_respected() {
        let temp = tempfile::tempdir().unwrap();
        let output = run(
            "/bin/pwd",
            &[] as &[&str],
            Some(temp.path()),
            DEFAULT_TIMEOUT,
        )
        .await
        .unwrap();

        // macOS temp dirs live under a symlinked /var, so compare canonically.
        assert_eq!(
            std::fs::canonicalize(output.stdout.trim()).unwrap(),
            std::fs::canonicalize(temp.path()).unwrap()
        );
    }

    #[tokio::test]
    async fn a_missing_program_is_named_in_the_error() {
        let err = run("forge-no-such-tool", &[] as &[&str], None, DEFAULT_TIMEOUT)
            .await
            .unwrap_err();

        assert!(matches!(err, ExecError::NotFound(name) if name == "forge-no-such-tool"));
    }

    #[tokio::test]
    async fn a_hung_command_times_out_instead_of_hanging_the_request() {
        let err = run("/bin/sleep", &["30"], None, Duration::from_millis(50))
            .await
            .unwrap_err();

        assert!(matches!(err, ExecError::TimedOut { .. }));
    }

    #[test]
    fn which_finds_a_real_binary_and_not_an_imaginary_one() {
        let sh = which("sh").expect("every Mac has sh on PATH");
        assert_eq!(sh.file_name().unwrap(), "sh");
        assert!(sh.is_absolute());
        assert!(which("forge-no-such-tool").is_none());
    }

    #[test]
    fn which_accepts_an_explicit_path() {
        assert_eq!(which("/bin/sh").unwrap(), Path::new("/bin/sh"));
        assert!(which("/bin/definitely-not-here").is_none());
    }

    #[test]
    fn the_first_error_line_skips_the_blank_ones() {
        let output = Output {
            code: Some(1),
            stdout: String::new(),
            stderr: "\n\nfatal: not a git repository\nhint: …\n".into(),
        };
        assert_eq!(output.first_error_line(), "fatal: not a git repository");
    }
}
