//! The Rubick Forge daemon.
//!
//! This crate is the only place that may touch agents, tmux, git or SQLite;
//! the desktop app reaches all of it over HTTP/WS.

pub mod adapters;
pub mod binaries;
pub mod bus;
pub mod cli;
pub mod config;
pub mod exec;
pub mod fleet;
pub mod git;
pub mod github;
pub mod http;
pub mod hub;
pub mod launchd;
pub mod logging;
pub mod paths;
pub mod runtime;
pub mod server;
pub mod sessions;
pub mod slug;
pub mod store;
pub mod tailnet;
pub mod telegram;
pub mod terminal;
pub mod token;
pub mod worktree;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The keychain service every secret this daemon holds is filed under.
pub const KEYCHAIN_SERVICE: &str = "tech.cloverly.rubick-forge";

/// One secret from the login keychain, by account name.
///
/// Config files name accounts, never secrets: a `daemon.toml` should be safe to
/// paste into a bug report.
pub fn secret(account: &str) -> Result<String, String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, account)
        .map_err(|err| format!("cannot reach the keychain: {err}"))?
        .get_password()
        .map_err(|err| format!("no keychain entry `{account}`: {err}"))
}
