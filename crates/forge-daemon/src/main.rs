use std::process::ExitCode;

use forge_daemon::cli::{self, Command};
use forge_daemon::paths::StateDir;
use forge_daemon::server::Daemon;
use forge_daemon::{launchd, logging, VERSION};

fn main() -> ExitCode {
    let command = match cli::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(err) => {
            eprintln!("forge-daemon: {err}\n");
            eprint!("{}", cli::USAGE);
            return ExitCode::FAILURE;
        }
    };

    match command {
        Command::Version => {
            println!("forge-daemon {VERSION}");
            ExitCode::SUCCESS
        }
        Command::Help => {
            print!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Command::InstallLaunchd => exit_code(install_launchd()),
        Command::UninstallLaunchd => exit_code(uninstall_launchd()),
        Command::Run { foreground } => exit_code(run(foreground)),
    }
}

fn run(foreground: bool) -> Result<(), Box<dyn std::error::Error>> {
    let state_dir = StateDir::locate()?;
    state_dir.ensure()?;

    let _log_guard = logging::init(&state_dir.logs_dir(), foreground);

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let daemon = Daemon::bootstrap(&state_dir)?;
        daemon.reconcile().await;
        daemon.watch();
        daemon.serve().await
    })?;

    Ok(())
}

fn install_launchd() -> Result<(), Box<dyn std::error::Error>> {
    let state_dir = StateDir::locate()?;
    let plist = launchd::install(&state_dir)?;

    println!("installed {}", plist.display());
    println!("state dir {}", state_dir.root().display());
    println!("launchd will keep `{}` running.", launchd::LABEL);
    Ok(())
}

fn uninstall_launchd() -> Result<(), Box<dyn std::error::Error>> {
    let plist = launchd::uninstall()?;

    println!("removed {}", plist.display());
    println!("state and worktrees were left alone.");
    Ok(())
}

/// Print the whole error chain, since the top-level message alone rarely
/// says which file or command actually failed.
fn exit_code(result: Result<(), Box<dyn std::error::Error>>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("forge-daemon: {err}");
            let mut source = err.source();
            while let Some(cause) = source {
                eprintln!("  caused by: {cause}");
                source = cause.source();
            }
            ExitCode::FAILURE
        }
    }
}
