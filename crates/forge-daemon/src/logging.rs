//! Structured logging: stderr in the foreground, rotated files under launchd.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

const DEFAULT_FILTER: &str = "info";

/// Keeps the background log writer alive; dropping it flushes and stops it.
pub struct LogGuard(#[allow(dead_code)] Option<WorkerGuard>);

/// Install the global subscriber.
///
/// `RUST_LOG` wins when set, in both modes, so a foreground debugging session
/// needs no config file edit.
pub fn init(logs_dir: &Path, foreground: bool) -> LogGuard {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    if foreground {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
        return LogGuard(None);
    }

    let appender = tracing_appender::rolling::daily(logs_dir, "forge-daemon.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .init();

    LogGuard(Some(guard))
}
