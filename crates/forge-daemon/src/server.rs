//! Bootstrapping: state directory, config, token, database, then serve.

use std::net::SocketAddr;

use tokio::net::TcpListener;
use tokio::signal;

use crate::bus::Bus;
use crate::config::{Config, ConfigError};
use crate::http::{router, AppState};
use crate::paths::{StateDir, StateDirError};
use crate::store::{Store, StoreError};
use crate::token::{Token, TokenError};
use crate::VERSION;

/// Everything the daemon needs, assembled but not yet listening.
pub struct Daemon {
    pub state: AppState,
    pub addr: SocketAddr,
}

impl Daemon {
    /// Create the state directory if needed and load everything in it.
    /// Idempotent: a second start finds what the first one wrote.
    pub fn bootstrap(state_dir: &StateDir) -> Result<Self, StartupError> {
        state_dir.ensure()?;

        let config = Config::load_or_create(&state_dir.config_path())?;
        let token = Token::load_or_create(&state_dir.token_path())?;
        let store = Store::open(&state_dir.db_path())?;

        tracing::info!(
            state_dir = %state_dir.root().display(),
            schema_version = store.schema_version()?,
            "state loaded"
        );

        Ok(Self {
            addr: config.socket_addr(),
            state: AppState::new(Bus::new(store.clone()), store, token, config, VERSION),
        })
    }

    /// Serve until SIGINT or SIGTERM.
    pub async fn serve(self) -> Result<(), StartupError> {
        let listener = TcpListener::bind(self.addr)
            .await
            .map_err(|source| StartupError::Bind {
                addr: self.addr,
                source,
            })?;

        tracing::info!(addr = %self.addr, version = VERSION, "listening");

        axum::serve(listener, router(self.state))
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(StartupError::Serve)
    }
}

async fn shutdown_signal() {
    let interrupt = async {
        let _ = signal::ctrl_c().await;
    };
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            // Without SIGTERM we still have ctrl-c; launchd's stop will fall
            // back to SIGKILL, which WAL mode survives.
            Err(error) => {
                tracing::warn!(%error, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };

    tokio::select! {
        () = interrupt => tracing::info!("interrupted; shutting down"),
        () = terminate => tracing::info!("terminated; shutting down"),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error(transparent)]
    StateDir(#[from] StateDirError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Token(#[from] TokenError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("cannot bind {addr}: {source}. Another daemon may already be running.")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("the server stopped unexpectedly: {0}")]
    Serve(#[source] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrapping_creates_the_whole_state_directory() {
        let temp = tempfile::tempdir().unwrap();
        let state_dir = StateDir::at(temp.path().join("state"));

        let daemon = Daemon::bootstrap(&state_dir).unwrap();

        assert!(state_dir.config_path().exists());
        assert!(state_dir.token_path().exists());
        assert!(state_dir.db_path().exists());
        assert!(state_dir.logs_dir().is_dir());
        assert_eq!(daemon.addr.port(), crate::config::DEFAULT_PORT);
    }

    #[test]
    fn a_second_bootstrap_reuses_the_first_ones_token() {
        let temp = tempfile::tempdir().unwrap();
        let state_dir = StateDir::at(temp.path());

        let first = Daemon::bootstrap(&state_dir).unwrap();
        let second = Daemon::bootstrap(&state_dir).unwrap();

        assert_eq!(first.state.token.expose(), second.state.token.expose());
    }
}
