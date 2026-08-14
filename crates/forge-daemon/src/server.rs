//! Bootstrapping: state directory, config, token, database, then serve.

use std::future::IntoFuture;
use std::net::SocketAddr;

use tokio::net::TcpListener;
use tokio::signal;

use std::sync::Arc;

use crate::bus::Bus;
use crate::config::{Config, ConfigError};
use crate::http::{router, AppState};
use crate::paths::{StateDir, StateDirError};
use crate::runtime::TmuxRuntime;
use crate::sessions::SessionManager;
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

        let bus = Bus::new(store.clone());
        let shared = Arc::new(config);
        let sessions = Arc::new(SessionManager::new(
            TmuxRuntime::new(),
            store.clone(),
            bus.clone(),
            shared.clone(),
        ));

        Ok(Self {
            addr: shared.socket_addr(),
            state: AppState::new(bus, store, sessions, token, shared, VERSION),
        })
    }

    /// Bring the sessions table back in line with what tmux actually has.
    ///
    /// The daemon may have been stopped, crashed or upgraded while sessions
    /// kept running; this is what makes them survive that.
    pub async fn reconcile(&self) {
        match self.state.sessions.reconcile().await {
            Ok(_) => {}
            // Not fatal: without tmux the daemon still serves history and
            // settings, and `/health` says why agents cannot start.
            Err(error) => tracing::warn!(%error, "cannot reconcile sessions on boot"),
        }
    }

    /// Start the Telegram bot and the GitHub poller, if either is configured.
    pub fn attend(&self) {
        crate::telegram::spawn(self.state.clone());

        // Always started: it reads the token itself, so one saved later begins
        // polling without a restart, and a revoked one stops.
        tokio::spawn(crate::github::poll::watch(self.state.clone()));
    }

    /// Watch live sessions until the daemon shuts down.
    ///
    /// Without this the daemon only notices a session ending when it next
    /// boots, so a task whose agent exited would sit there claiming to run.
    pub fn watch(&self) {
        let sessions = self.state.sessions.clone();
        let interval = self.state.config.poll_interval();

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                ticker.tick().await;
                if let Err(error) = sessions.poll().await {
                    tracing::warn!(%error, "a session poll failed");
                }
            }
        });
    }

    /// Every address to listen on: the configured bind, and the tailnet when
    /// `tailscale_bind` asks for one.
    ///
    /// Loopback stays whatever happens to the tailnet: the app on this Mac
    /// must not lose its daemon because Tailscale is down.
    pub async fn listen_addrs(&self) -> Result<Vec<SocketAddr>, StartupError> {
        let mut addrs = vec![self.addr];

        if let Some(tailnet) = self.state.config.tailnet_addr().await? {
            if tailnet != self.addr {
                addrs.push(tailnet);
            }
        }

        Ok(addrs)
    }

    /// Serve until SIGINT or SIGTERM.
    pub async fn serve(self) -> Result<(), StartupError> {
        self.serve_until(shutdown_signal()).await
    }

    /// Serve every configured address until `shutdown` resolves.
    ///
    /// Split out from [`serve`](Self::serve) so a test can end it without
    /// signalling the process running it.
    pub async fn serve_until(
        self,
        shutdown: impl std::future::Future<Output = ()>,
    ) -> Result<(), StartupError> {
        let addrs = self.listen_addrs().await?;
        let router = router(self.state);

        // One signal, many servers: each waits on its own receiver so that a
        // SIGTERM drains all of them rather than the first one to notice.
        let (stop_all, _) = tokio::sync::broadcast::channel::<()>(1);
        let mut servers = Vec::with_capacity(addrs.len());

        for addr in addrs {
            let listener = TcpListener::bind(addr)
                .await
                .map_err(|source| StartupError::Bind { addr, source })?;

            tracing::info!(%addr, version = VERSION, "listening");

            let mut stop = stop_all.subscribe();
            servers.push(tokio::spawn(
                axum::serve(listener, router.clone())
                    .with_graceful_shutdown(async move {
                        let _ = stop.recv().await;
                    })
                    .into_future(),
            ));
        }

        shutdown.await;
        let _ = stop_all.send(());

        for server in servers {
            match server.await {
                Ok(result) => result.map_err(StartupError::Serve)?,
                Err(source) => return Err(StartupError::Serve(std::io::Error::other(source))),
            }
        }

        Ok(())
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

    #[tokio::test]
    async fn loopback_is_the_only_listener_until_the_tailnet_is_asked_for() {
        let temp = tempfile::tempdir().unwrap();
        let state_dir = StateDir::at(temp.path());

        let daemon = Daemon::bootstrap(&state_dir).unwrap();

        assert_eq!(daemon.listen_addrs().await.unwrap(), vec![daemon.addr]);
    }

    #[tokio::test]
    async fn a_tailnet_bind_adds_a_listener_and_keeps_loopback() {
        let temp = tempfile::tempdir().unwrap();
        let state_dir = StateDir::at(temp.path());
        state_dir.ensure().unwrap();
        std::fs::write(
            state_dir.config_path(),
            "tailscale_bind = \"100.101.102.103\"\n",
        )
        .unwrap();

        let daemon = Daemon::bootstrap(&state_dir).unwrap();
        let addrs = daemon.listen_addrs().await.unwrap();

        assert_eq!(addrs.len(), 2);
        assert_eq!(addrs[0].ip().to_string(), "127.0.0.1");
        assert_eq!(addrs[1].ip().to_string(), "100.101.102.103");
        assert_eq!(addrs[0].port(), addrs[1].port());
    }

    #[tokio::test]
    async fn a_tailnet_bind_that_repeats_the_first_one_is_not_bound_twice() {
        let temp = tempfile::tempdir().unwrap();
        let state_dir = StateDir::at(temp.path());
        state_dir.ensure().unwrap();
        std::fs::write(state_dir.config_path(), "tailscale_bind = \"127.0.0.1\"\n").unwrap();

        let daemon = Daemon::bootstrap(&state_dir).unwrap();

        assert_eq!(daemon.listen_addrs().await.unwrap().len(), 1);
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
