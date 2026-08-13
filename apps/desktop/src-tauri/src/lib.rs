//! The desktop shell: a window, and the little that only a native process can
//! do.
//!
//! This process is strictly an API client. It never opens SQLite, never talks
//! to tmux or git, and never runs an agent. Closing it changes nothing about
//! running agents.
//!
//! Two things here are native by necessity, and both are about *reaching* the
//! daemon rather than doing its work: reading the bearer token it wrote, and
//! running its own `--install-launchd` when it is not installed. A webview can
//! do neither.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where the daemon keeps its state. Must match the daemon's own idea of it.
const STATE_DIR: &str = "Library/Application Support/rubick-forge";

/// Version of the app shell, surfaced to the frontend so the UI can show what
/// it is running next to the daemon version it talks to.
#[tauri::command]
fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The local daemon's bearer token, if it has ever run.
///
/// Read rather than stored: the daemon owns it, and a copy in the app would be
/// one more place for it to go stale or leak.
#[tauri::command]
fn daemon_token() -> Result<String, String> {
    let path = state_dir()?.join("token");

    std::fs::read_to_string(&path)
        .map(|token| token.trim().to_owned())
        .map_err(|err| format!("cannot read {}: {err}", path.display()))
}

/// Install the daemon as a LaunchAgent by running its own subcommand.
///
/// The app never supervises the daemon; it asks the daemon to install itself
/// and launchd takes over from there.
///
/// The path comes from the webview, so it is checked before it is run: it must
/// identify itself as forge-daemon. Otherwise a bug in the page could turn
/// this into "run any binary on this machine".
#[tauri::command]
fn install_daemon(binary: String) -> Result<String, String> {
    let path = PathBuf::from(&binary);
    if !path.is_file() {
        return Err(format!("there is no daemon binary at {binary}"));
    }

    let version = ask_daemon(&path, "--version")?;
    if !version.starts_with("forge-daemon ") {
        return Err(format!("{binary} is not the Forge daemon"));
    }

    ask_daemon(&path, "--install-launchd")
}

/// Run one of the daemon's own subcommands and return what it said.
fn ask_daemon(path: &Path, argument: &str) -> Result<String, String> {
    let output = Command::new(path)
        .arg(argument)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("cannot run {}: {err}", path.display()))?;

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned());
    }

    Err(String::from_utf8_lossy(&output.stderr)
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("the daemon would not say why it failed")
        .to_owned())
}

fn state_dir() -> Result<PathBuf, String> {
    if let Some(overridden) = std::env::var_os("FORGE_STATE_DIR") {
        return Ok(PathBuf::from(overridden));
    }

    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(STATE_DIR))
        .ok_or_else(|| "HOME is not set, so the daemon's state cannot be found".to_owned())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            app_version,
            daemon_token,
            install_daemon
        ])
        .run(tauri::generate_context!())
        .expect("error while running rubick-forge");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_version_is_the_crate_version() {
        assert_eq!(app_version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn installing_needs_a_binary_that_is_there() {
        let err = install_daemon("/definitely/not/here".to_owned()).unwrap_err();

        assert!(err.contains("no daemon binary"), "{err}");
    }

    #[test]
    fn installing_refuses_anything_that_is_not_the_daemon() {
        // A real, runnable binary that is not ours.
        let err = install_daemon("/bin/echo".to_owned()).unwrap_err();

        assert!(err.contains("not the Forge daemon"), "{err}");
    }

    #[test]
    fn a_missing_token_says_which_file_is_missing() {
        // The override is what the daemon itself honours, so the app follows
        // it rather than guessing.
        let temp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("FORGE_STATE_DIR", temp.path()) };

        let err = daemon_token().unwrap_err();
        assert!(err.contains("token"), "{err}");

        std::fs::write(temp.path().join("token"), "abc123\n").unwrap();
        assert_eq!(daemon_token().unwrap(), "abc123");

        unsafe { std::env::remove_var("FORGE_STATE_DIR") };
    }
}
