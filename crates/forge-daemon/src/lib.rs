//! The Rubick Forge daemon.
//!
//! This crate is the only place that may touch agents, tmux, git or SQLite;
//! the desktop app reaches all of it over HTTP/WS.

pub mod binaries;
pub mod bus;
pub mod cli;
pub mod config;
pub mod exec;
pub mod http;
pub mod launchd;
pub mod logging;
pub mod paths;
pub mod server;
pub mod store;
pub mod token;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
