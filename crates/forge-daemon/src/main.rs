//! Scaffold binary: parses arguments, reports what it would do, exits.
//!
//! The HTTP/WS server, tmux runtime, SQLite store and adapters are not built
//! yet; this crate is the only place any of them may live (D3).

mod cli;

use std::process::ExitCode;

use cli::Command;
use forge_core::AdapterId;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    match cli::parse(std::env::args().skip(1)) {
        Ok(Command::Version) => {
            println!("forge-daemon {VERSION}");
            ExitCode::SUCCESS
        }
        Ok(Command::Help) => {
            print!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Ok(Command::Run { foreground }) => {
            println!("forge-daemon {VERSION}");
            println!(
                "mode: {}",
                if foreground {
                    "foreground"
                } else {
                    "background"
                }
            );
            println!(
                "adapters: {}",
                AdapterId::ALL
                    .iter()
                    .map(|a| a.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            println!("scaffold only: no server, no sessions, nothing started");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("forge-daemon: {err}\n");
            eprint!("{}", cli::USAGE);
            ExitCode::FAILURE
        }
    }
}
