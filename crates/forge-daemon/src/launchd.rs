//! LaunchAgent installation — the thing that makes the daemon always-on.
//!
//! `KeepAlive` is what turns "the daemon crashed" into "the daemon restarted",
//! which is why the desktop app is allowed to be a mere viewer.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::paths::{StateDir, STATE_DIR_ENV};

pub const LABEL: &str = "tech.cloverly.rubick-forge.daemon";

/// Where launchd looks for per-user agents.
pub fn plist_path() -> Result<PathBuf, LaunchdError> {
    let home = std::env::var_os("HOME").ok_or(LaunchdError::NoHome)?;
    Ok(Path::new(&home)
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

/// Write the plist and hand it to launchd. Idempotent: re-running repairs a
/// broken install, which is exactly what the app's "repair daemon" button does.
pub fn install(state_dir: &StateDir) -> Result<PathBuf, LaunchdError> {
    let exe = std::env::current_exe().map_err(LaunchdError::CurrentExe)?;
    let path = plist_path()?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| LaunchdError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    state_dir.ensure()?;

    std::fs::write(&path, plist(&exe, state_dir)).map_err(|source| LaunchdError::Write {
        path: path.clone(),
        source,
    })?;

    // Booting out first makes this a reinstall rather than a conflict; the
    // agent is usually not loaded, so its failure is expected and ignored.
    let _ = launchctl(&["bootout", &domain()?]);
    launchctl(&["bootstrap", &domain_target()?, &path.display().to_string()])?;

    Ok(path)
}

/// Stop the agent and forget it. Leaves the state directory alone.
pub fn uninstall() -> Result<PathBuf, LaunchdError> {
    let path = plist_path()?;

    let _ = launchctl(&["bootout", &domain()?]);

    match std::fs::remove_file(&path) {
        Ok(()) => Ok(path),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(path),
        Err(source) => Err(LaunchdError::Write { path, source }),
    }
}

/// `gui/<uid>/<label>` — the loaded service.
fn domain() -> Result<String, LaunchdError> {
    Ok(format!("{}/{LABEL}", domain_target()?))
}

/// `gui/<uid>` — the user's launchd domain.
fn domain_target() -> Result<String, LaunchdError> {
    let output = Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .map_err(LaunchdError::Uid)?;
    let uid = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || uid.is_empty() {
        return Err(LaunchdError::Uid(io::Error::other(
            "`id -u` did not report a user id",
        )));
    }
    Ok(format!("gui/{uid}"))
}

fn launchctl(args: &[&str]) -> Result<(), LaunchdError> {
    let output = Command::new("/bin/launchctl")
        .args(args)
        .output()
        .map_err(|source| LaunchdError::Launchctl {
            command: args.join(" "),
            detail: source.to_string(),
        })?;

    if output.status.success() {
        return Ok(());
    }

    Err(LaunchdError::Launchctl {
        command: args.join(" "),
        detail: String::from_utf8_lossy(&output.stderr)
            .trim()
            .lines()
            .next()
            .unwrap_or("launchctl gave no reason")
            .to_owned(),
    })
}

fn plist(exe: &Path, state_dir: &StateDir) -> String {
    let logs = state_dir.logs_dir();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>{state_env}</key>
        <string>{state_dir}</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ProcessType</key>
    <string>Background</string>
    <key>StandardOutPath</key>
    <string>{out}</string>
    <key>StandardErrorPath</key>
    <string>{err}</string>
</dict>
</plist>
"#,
        label = LABEL,
        exe = escape(&exe.display().to_string()),
        state_env = STATE_DIR_ENV,
        state_dir = escape(&state_dir.root().display().to_string()),
        out = escape(&logs.join("launchd.out.log").display().to_string()),
        err = escape(&logs.join("launchd.err.log").display().to_string()),
    )
}

/// Paths can legally contain `&` and `<`; a plist is XML.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[derive(Debug, thiserror::Error)]
pub enum LaunchdError {
    #[error("HOME is not set, so ~/Library/LaunchAgents cannot be located")]
    NoHome,
    #[error("cannot determine this binary's own path: {0}")]
    CurrentExe(#[source] io::Error),
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot determine the current user id: {0}")]
    Uid(#[source] io::Error),
    #[error("`launchctl {command}` failed: {detail}")]
    Launchctl { command: String, detail: String },
    #[error(transparent)]
    StateDir(#[from] crate::paths::StateDirError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plist_keeps_the_daemon_alive_and_points_at_this_binary() {
        let text = plist(
            Path::new("/usr/local/bin/forge-daemon"),
            &StateDir::at("/state"),
        );

        assert!(text.contains("<string>/usr/local/bin/forge-daemon</string>"));
        assert!(text.contains("<key>KeepAlive</key>\n    <true/>"));
        assert!(text.contains("<key>RunAtLoad</key>\n    <true/>"));
        assert!(text.contains(&format!("<string>{LABEL}</string>")));
    }

    #[test]
    fn the_agent_inherits_the_state_directory_it_was_installed_from() {
        let text = plist(
            Path::new("/bin/forge-daemon"),
            &StateDir::at("/custom/state"),
        );

        assert!(text.contains(&format!("<key>{STATE_DIR_ENV}</key>")));
        assert!(text.contains("<string>/custom/state</string>"));
    }

    #[test]
    fn logs_land_in_the_state_directory() {
        let text = plist(Path::new("/bin/forge-daemon"), &StateDir::at("/state"));

        assert!(text.contains("<string>/state/logs/launchd.out.log</string>"));
        assert!(text.contains("<string>/state/logs/launchd.err.log</string>"));
    }

    #[test]
    fn xml_metacharacters_in_a_path_cannot_break_the_document() {
        let text = plist(
            Path::new("/Users/a&b/<forge>/forge-daemon"),
            &StateDir::at("/state"),
        );

        assert!(text.contains("/Users/a&amp;b/&lt;forge&gt;/forge-daemon"));
        assert!(!text.contains("<forge>"));
    }
}
