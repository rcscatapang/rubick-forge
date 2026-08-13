//! Argument parsing for `forge-daemon`.
//!
//! Hand-rolled: the daemon takes a handful of flags and nothing else, and the
//! launchd plist passes them literally.

use std::fmt;

pub const USAGE: &str = "\
forge-daemon — the Rubick Forge control-plane daemon

USAGE:
    forge-daemon [OPTIONS]

OPTIONS:
    -f, --foreground        Run in the foreground and log to stderr (dev mode)
        --install-launchd   Install and start the LaunchAgent, then exit
        --uninstall-launchd Stop and remove the LaunchAgent, then exit
    -V, --version           Print version and exit
    -h, --help              Print this help and exit
";

/// What the process should do, once the arguments are understood.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Start the daemon.
    Run { foreground: bool },
    /// Write the LaunchAgent plist and bootstrap it.
    InstallLaunchd,
    /// Bootout the LaunchAgent and delete its plist.
    UninstallLaunchd,
    /// Print the version and exit.
    Version,
    /// Print usage and exit.
    Help,
}

/// An argument the daemon does not recognise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownArg(pub String);

impl fmt::Display for UnknownArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown argument: {}", self.0)
    }
}

impl std::error::Error for UnknownArg {}

/// Parse the arguments *after* argv[0].
pub fn parse<I, S>(args: I) -> Result<Command, UnknownArg>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut foreground = false;

    for arg in args {
        match arg.as_ref() {
            "-f" | "--foreground" => foreground = true,
            "--install-launchd" => return Ok(Command::InstallLaunchd),
            "--uninstall-launchd" => return Ok(Command::UninstallLaunchd),
            "-V" | "--version" => return Ok(Command::Version),
            "-h" | "--help" => return Ok(Command::Help),
            other => return Err(UnknownArg(other.to_owned())),
        }
    }

    Ok(Command::Run { foreground })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_runs_in_the_background() {
        let empty: [&str; 0] = [];
        assert_eq!(parse(empty).unwrap(), Command::Run { foreground: false });
    }

    #[test]
    fn foreground_flag_has_a_short_form() {
        for arg in ["--foreground", "-f"] {
            assert_eq!(parse([arg]).unwrap(), Command::Run { foreground: true });
        }
    }

    #[test]
    fn version_and_help_short_circuit() {
        assert_eq!(parse(["--version"]).unwrap(), Command::Version);
        assert_eq!(parse(["-V"]).unwrap(), Command::Version);
        assert_eq!(parse(["--help"]).unwrap(), Command::Help);
        assert_eq!(parse(["-h"]).unwrap(), Command::Help);
        // Even when they follow other flags.
        assert_eq!(
            parse(["--foreground", "--version"]).unwrap(),
            Command::Version
        );
    }

    #[test]
    fn the_launchd_subcommands_are_their_own_mode() {
        assert_eq!(
            parse(["--install-launchd"]).unwrap(),
            Command::InstallLaunchd
        );
        assert_eq!(
            parse(["--uninstall-launchd"]).unwrap(),
            Command::UninstallLaunchd
        );
    }

    #[test]
    fn unknown_arguments_are_rejected() {
        assert_eq!(
            parse(["--daemonize"]),
            Err(UnknownArg("--daemonize".to_owned()))
        );
    }
}
