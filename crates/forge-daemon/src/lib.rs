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
pub mod git;
pub mod http;
pub mod launchd;
pub mod logging;
pub mod paths;
pub mod runtime;
pub mod server;
pub mod sessions;
pub mod slug;
pub mod store;
pub mod tailnet;
pub mod terminal;
pub mod token;
pub mod worktree;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
