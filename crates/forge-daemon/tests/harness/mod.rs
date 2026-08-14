//! An in-process daemon for integration tests: real router, real database,
//! throwaway token.

#![allow(dead_code)] // Each test binary uses a different slice of this.

use std::net::SocketAddr;
use std::path::Path;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use forge_daemon::bus::Bus;
use forge_daemon::config::Config;
use forge_daemon::http::{router, AppState};
use forge_daemon::runtime::TmuxRuntime;
use forge_daemon::sessions::SessionManager;
use forge_daemon::store::Store;
use forge_daemon::token::Token;
use serde_json::Value;
use tower::ServiceExt;

pub type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub struct Harness {
    pub state: AppState,
    /// The private tmux server this harness drives, so nothing here can touch
    /// the developer's own sessions.
    pub tmux_label: String,
    pub token: String,
    /// Kept so the database outlives the harness, and so fixtures can open
    /// their own connection to it.
    temp: tempfile::TempDir,
}

impl Harness {
    pub fn new() -> Self {
        Self::build(None)
    }

    /// A daemon pointed at a tmux that is not there, for the paths that have
    /// to answer rather than crash when the runtime is unusable.
    pub fn with_tmux_binary(binary: &str) -> Self {
        Self::build(Some(binary.to_owned()))
    }

    fn build(tmux_binary: Option<String>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let token = Token::load_or_create(&temp.path().join("token")).unwrap();
        let secret = token.expose().to_owned();
        // On disk rather than in memory, so a fixture can reach the same
        // database over its own connection.
        let store = Store::open(&temp.path().join("forge.db")).unwrap();
        let bus = Bus::new(store.clone());

        // The label carries the process id so a server leaked by a failed run
        // cannot be adopted by the next one.
        let tmux_label = format!(
            "forge-harness-{}-{}",
            std::process::id(),
            temp.path().file_name().unwrap().to_string_lossy()
        );
        let config = std::sync::Arc::new(Config::default());
        let mut runtime = TmuxRuntime::with_socket(&tmux_label);
        if let Some(binary) = tmux_binary {
            runtime = runtime.with_binary(binary);
        }

        let sessions = std::sync::Arc::new(SessionManager::new(
            runtime,
            store.clone(),
            bus.clone(),
            config.clone(),
        ));

        Self {
            state: AppState::new(bus, store, sessions, token, config, "0.1.0-test"),
            tmux_label,
            token: secret,
            temp,
        }
    }

    pub fn bus(&self) -> &Bus {
        &self.state.bus
    }

    pub fn store(&self) -> &Store {
        &self.state.store
    }

    pub async fn send(&self, request: Request<Body>) -> (StatusCode, Value) {
        let response = router(self.state.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// A request carrying the daemon's token.
    async fn request(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {}", self.token));

        let request = match body {
            Some(body) => builder
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };

        self.send(request).await
    }

    pub async fn get(&self, uri: &str) -> (StatusCode, Value) {
        self.request("GET", uri, None).await
    }

    pub async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.request("POST", uri, Some(body)).await
    }

    pub async fn patch(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.request("PATCH", uri, Some(body)).await
    }

    pub async fn delete(&self, uri: &str) -> (StatusCode, Value) {
        self.request("DELETE", uri, None).await
    }

    /// A request with no credentials at all.
    pub async fn anonymous_get(&self, uri: &str) -> (StatusCode, Value) {
        self.send(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
    }

    /// The status of a request of any method with no credentials at all.
    pub async fn unauthenticated(&self, method: &str, uri: &str) -> StatusCode {
        self.send(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .0
    }

    /// Register a repository and return its project id.
    pub async fn register(&self, repo: &tempfile::TempDir) -> i64 {
        let root = std::fs::canonicalize(repo.path()).unwrap();
        let (status, project) = self
            .post(
                "/projects",
                serde_json::json!({ "path": root.display().to_string() }),
            )
            .await;

        assert_eq!(status, StatusCode::CREATED, "{project}");
        project["id"].as_i64().unwrap()
    }

    /// A task row, written directly, for guards that only care about a
    /// task's status.
    pub fn insert_task(&self, project_id: i64, status: &str) {
        self.connection()
            .execute(
            "INSERT INTO tasks (project_id, title, adapter, base_branch, branch, status, created_at, updated_at)
             VALUES (?1, 'a task', 'claude-code', 'main', 'main', ?2, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                rusqlite::params![project_id, status],
            )
            .unwrap();
    }

    /// A live session row, written directly, for guards that only care that
    /// one exists.
    pub fn insert_session(&self, task_id: i64) {
        self.connection()
            .execute(
                "INSERT INTO sessions (task_id, tmux_name, status, started_at)
                 VALUES (?1, ?2, 'working', '2026-01-01T00:00:00Z')",
                rusqlite::params![task_id, format!("forge-{task_id}")],
            )
            .unwrap();
    }

    pub fn set_setting(&self, key: &str, value: &str) {
        self.connection()
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                rusqlite::params![key, value],
            )
            .unwrap();
    }

    fn connection(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.temp.path().join("forge.db")).unwrap()
    }

    /// Bind a real socket, so WebSocket clients have something to connect to.
    pub async fn serve(&self) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router(self.state.clone());

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        addr
    }

    /// Open a WebSocket and wait until its handler has really subscribed.
    ///
    /// The handshake completes before the upgraded task runs, so publishing
    /// straight after a connect would otherwise race the subscription.
    pub async fn connect(&self, addr: SocketAddr, query: &str) -> Socket {
        let before = self.bus().subscriber_count();
        let (socket, _) =
            tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events?{query}"))
                .await
                .expect("the upgrade should be accepted");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while self.bus().subscriber_count() <= before {
            assert!(
                std::time::Instant::now() < deadline,
                "handler never subscribed"
            );
            tokio::task::yield_now().await;
        }

        socket
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::process::Command::new("tmux")
            .args(["-L", &self.tmux_label, "kill-server"])
            .output();
    }
}

/// Run a git command, failing the test if git itself fails.
pub fn git(repo: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git should be installed");

    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A repository with one commit on `main` and its own identity, so tests do
/// not depend on the machine's git config.
pub fn repo() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path();

    git(path, &["init", "--initial-branch=main", "--quiet"]);
    git(path, &["config", "user.email", "test@example.com"]);
    git(path, &["config", "user.name", "Test"]);
    std::fs::write(path.join("README.md"), "hello").unwrap();
    git(path, &["add", "README.md"]);
    git(path, &["commit", "--quiet", "-m", "initial"]);

    temp
}
