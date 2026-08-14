//! Presence and version checks for the tools the daemon shells out to.
//!
//! A missing tool is reported on `/health`, never fatal: the daemon still
//! serves history and settings, it just cannot start agents.

use std::time::Duration;

use forge_core::BinaryStatus;

use crate::exec::{self, ExecError};
use crate::runtime::tmux::version_complaint;

/// Version flags answer instantly or not at all.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// The tools without which nothing works.
pub const REQUIRED: [&str; 2] = ["tmux", "git"];

/// Look `name` up on `PATH` and ask it for its version.
pub async fn probe(name: &str) -> BinaryStatus {
    probe_with(name, &["--version".to_owned()]).await
}

/// Probe with the arguments a manifest says produce a version.
///
/// Not every CLI spells it `--version`, and a manifest that had to is a
/// manifest that could not describe some real agent.
pub async fn probe_with(name: &str, version_args: &[String]) -> BinaryStatus {
    let Some(path) = exec::which(name) else {
        return BinaryStatus::missing(name);
    };

    let found = path.display().to_string();

    match exec::run(name, version_args, None, PROBE_TIMEOUT).await {
        Ok(output) if output.success() => {
            let version = first_line(&output.stdout);

            // Being present is not the same as being usable: an old tmux is
            // missing the pane formats the status poller reads.
            if let Some(complaint) = unusable_version(name, version.as_deref()) {
                return BinaryStatus {
                    name: name.to_owned(),
                    path: Some(found),
                    version,
                    ok: false,
                    detail: Some(complaint),
                };
            }

            BinaryStatus {
                name: name.to_owned(),
                path: Some(found),
                version,
                ok: true,
                detail: None,
            }
        }
        Ok(output) => unusable(
            name,
            found,
            format!("`{name} --version` failed: {}", output.first_error_line()),
        ),
        Err(err) => unusable(name, found, describe(err)),
    }
}

/// Present but not usable: the path is still worth reporting, so the user can
/// see *which* copy is broken.
fn unusable(name: &str, path: String, detail: String) -> BinaryStatus {
    BinaryStatus {
        name: name.to_owned(),
        path: Some(path),
        version: None,
        ok: false,
        detail: Some(detail),
    }
}

/// Probe everything the daemon depends on, in a stable order.
pub async fn probe_required() -> Vec<BinaryStatus> {
    let mut statuses = Vec::with_capacity(REQUIRED.len());
    for name in REQUIRED {
        statuses.push(probe(name).await);
    }
    statuses
}

/// The one binary with a version floor. The runtime owns what that floor is,
/// so a green `/health` and a refused start cannot disagree.
fn unusable_version(name: &str, version: Option<&str>) -> Option<String> {
    if name != "tmux" {
        return None;
    }

    match version {
        Some(reported) => version_complaint(reported),
        None => Some("tmux did not report a version".to_owned()),
    }
}

fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

fn describe(err: ExecError) -> String {
    match err {
        ExecError::NotFound(name) => format!("`{name}` disappeared between the lookup and the run"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_present_binary_reports_its_path_and_version() {
        let status = probe("git").await;

        assert!(status.ok, "git should be on PATH in a dev environment");
        assert!(status.path.is_some());
        assert!(status.version.unwrap().starts_with("git version"));
        assert!(status.detail.is_none());
    }

    #[tokio::test]
    async fn a_missing_binary_is_a_warning_with_a_reason() {
        let status = probe("forge-no-such-tool").await;

        assert!(!status.ok);
        assert!(status.path.is_none());
        assert!(status.detail.unwrap().contains("not found on PATH"));
    }

    #[tokio::test]
    async fn the_required_set_is_probed_in_order() {
        let statuses = probe_required().await;

        let names: Vec<&str> = statuses.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, REQUIRED);
    }

    #[test]
    fn an_old_tmux_is_present_but_not_usable() {
        assert!(unusable_version("tmux", Some("tmux 2.9a"))
            .unwrap()
            .contains("too old"));
        assert_eq!(unusable_version("tmux", Some("tmux 3.0")), None);
        assert_eq!(unusable_version("tmux", Some("tmux 3.7b")), None);
    }

    #[test]
    fn only_tmux_has_a_version_floor() {
        assert_eq!(unusable_version("git", Some("git version 1.0")), None);
    }

    #[test]
    fn a_tmux_whose_version_cannot_be_read_is_not_called_healthy() {
        // Starting a session would refuse; health must say the same thing.
        assert!(unusable_version("tmux", Some("tmux")).is_some());
        assert!(unusable_version("tmux", None).is_some());
    }

    #[test]
    fn the_first_line_is_the_version_line() {
        assert_eq!(
            first_line("\ngit version 2.44.0\nextra\n").unwrap(),
            "git version 2.44.0"
        );
        assert_eq!(first_line("   \n"), None);
    }
}
