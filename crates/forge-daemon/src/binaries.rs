//! Presence and version checks for the tools the daemon shells out to.
//!
//! A missing tool is reported on `/health`, never fatal: the daemon still
//! serves history and settings, it just cannot start agents.

use std::time::Duration;

use forge_core::BinaryStatus;

use crate::exec::{self, ExecError};

/// Version flags answer instantly or not at all.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// The tools without which nothing works.
pub const REQUIRED: [&str; 2] = ["tmux", "git"];

/// Look `name` up on `PATH` and ask it for its version.
pub async fn probe(name: &str) -> BinaryStatus {
    let Some(path) = exec::which(name) else {
        return BinaryStatus::missing(name);
    };

    let found = path.display().to_string();

    match exec::run(name, &["--version"], None, PROBE_TIMEOUT).await {
        Ok(output) if output.success() => BinaryStatus {
            name: name.to_owned(),
            path: Some(found),
            version: first_line(&output.stdout),
            ok: true,
            detail: None,
        },
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
    fn the_first_line_is_the_version_line() {
        assert_eq!(
            first_line("\ngit version 2.44.0\nextra\n").unwrap(),
            "git version 2.44.0"
        );
        assert_eq!(first_line("   \n"), None);
    }
}
