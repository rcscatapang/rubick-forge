//! The terminal WebSocket, against real tmux sessions and real ptys.

mod harness;

use std::time::Duration;

use forge_daemon::runtime::SessionRuntime;
use futures_util::{SinkExt, StreamExt};
use harness::{repo, Harness, Socket};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

/// A started task, its session, and a served daemon.
struct Fixture {
    harness: Harness,
    _repo: tempfile::TempDir,
    _worktrees: tempfile::TempDir,
    addr: std::net::SocketAddr,
    session_id: i64,
    tmux_name: String,
}

impl Fixture {
    async fn new() -> Self {
        let harness = Harness::new();
        let repo = repo();
        let project_id = harness.register(&repo).await;
        let worktrees = tempfile::tempdir().unwrap();
        harness.set_setting(
            "worktree_root",
            &std::fs::canonicalize(worktrees.path())
                .unwrap()
                .display()
                .to_string(),
        );

        let (_, task) = harness
            .post(
                "/tasks",
                json!({
                    "project_id": project_id,
                    "title": "Watch me",
                    "adapter": "claude-code",
                }),
            )
            .await;
        let task_id = task["id"].as_i64().unwrap();

        let stored = harness.store().task(task_id).unwrap().unwrap();
        let project = harness.store().project(project_id).unwrap().unwrap();
        let session = harness
            .state
            .sessions
            .start(&stored, &project, &[])
            .await
            .unwrap();

        let addr = harness.serve().await;

        Self {
            harness,
            _repo: repo,
            _worktrees: worktrees,
            addr,
            session_id: session.id,
            tmux_name: session.tmux_name,
        }
    }

    fn url(&self, query: &str) -> String {
        format!(
            "ws://{}/ws/sessions/{}/terminal?token={}&{query}",
            self.addr, self.session_id, self.harness.token
        )
    }

    async fn connect(&self, query: &str) -> Socket {
        let (socket, _) = tokio_tungstenite::connect_async(self.url(query))
            .await
            .expect("the upgrade should be accepted");
        socket
    }

    /// How many `tmux attach` processes exist for this harness's server.
    fn attach_processes(&self) -> usize {
        let output = std::process::Command::new("ps")
            .args(["-Ao", "args="])
            .output()
            .unwrap();

        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.contains(&self.harness.tmux_label) && line.contains("attach"))
            .count()
    }
}

/// tmux's own status line, which every attach draws whatever shell is inside.
/// Waiting on a shell prompt would depend on whose dotfiles are installed.
const ATTACHED: &str = "[forge-";

/// Read until `needle` shows up, or fail.
async fn read_until(socket: &mut Socket, needle: &str) -> String {
    let mut seen = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(15);

    while std::time::Instant::now() < deadline {
        let Ok(Some(Ok(message))) =
            tokio::time::timeout(Duration::from_secs(5), socket.next()).await
        else {
            break;
        };

        if let Message::Binary(bytes) = message {
            seen.push_str(&String::from_utf8_lossy(&bytes));
            if seen.contains(needle) {
                return seen;
            }
        }
    }

    panic!("never saw {needle:?}; got {seen:?}");
}

#[tokio::test]
async fn a_viewer_sees_what_the_session_is_showing() {
    let fixture = Fixture::new().await;

    let mut socket = fixture.connect("cols=100&rows=30").await;

    // Written into the session, not the socket: this is a window onto tmux.
    fixture
        .harness
        .state
        .sessions
        .runtime()
        .paste(&fixture.tmux_name, "printf 'MARKER-FROM-THE-PANE\\n'\n")
        .await
        .unwrap();

    read_until(&mut socket, "MARKER-FROM-THE-PANE").await;
}

#[tokio::test]
async fn keystrokes_reach_the_session() {
    let fixture = Fixture::new().await;
    let mut socket = fixture.connect("cols=100&rows=30").await;

    socket
        .send(Message::Binary(
            b"printf 'TYPED-THROUGH-THE-SOCKET\\n'\n".to_vec().into(),
        ))
        .await
        .unwrap();

    read_until(&mut socket, "TYPED-THROUGH-THE-SOCKET").await;
}

#[tokio::test]
async fn a_read_only_viewer_cannot_type() {
    let fixture = Fixture::new().await;
    let mut watcher = fixture.connect("cols=100&rows=30&read_only=true").await;

    watcher
        .send(Message::Binary(
            b"printf 'SHOULD-NOT-APPEAR\\n'\n".to_vec().into(),
        ))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let pane = fixture
        .harness
        .state
        .sessions
        .runtime()
        .capture(&fixture.tmux_name, 200)
        .await
        .unwrap();

    assert!(
        !pane.contains("SHOULD-NOT-APPEAR"),
        "read-only input reached the session: {pane}"
    );
}

#[tokio::test]
async fn two_viewers_watch_the_same_session_independently() {
    let fixture = Fixture::new().await;

    let mut first = fixture.connect("cols=100&rows=30").await;
    let mut second = fixture.connect("cols=100&rows=30").await;

    fixture
        .harness
        .state
        .sessions
        .runtime()
        .paste(&fixture.tmux_name, "printf 'SEEN-BY-BOTH\\n'\n")
        .await
        .unwrap();

    read_until(&mut first, "SEEN-BY-BOTH").await;
    read_until(&mut second, "SEEN-BY-BOTH").await;
}

#[tokio::test]
async fn one_viewer_leaving_disturbs_neither_the_other_nor_the_agent() {
    let fixture = Fixture::new().await;

    let mut leaving = fixture.connect("cols=100&rows=30").await;
    let mut staying = fixture.connect("cols=100&rows=30").await;
    read_until(&mut leaving, ATTACHED).await;

    leaving.close(None).await.unwrap();
    drop(leaving);
    tokio::time::sleep(Duration::from_millis(500)).await;

    fixture
        .harness
        .state
        .sessions
        .runtime()
        .paste(&fixture.tmux_name, "printf 'STILL-HERE\\n'\n")
        .await
        .unwrap();

    read_until(&mut staying, "STILL-HERE").await;
    assert!(
        fixture
            .harness
            .state
            .sessions
            .runtime()
            .exists(&fixture.tmux_name)
            .await
            .unwrap(),
        "detaching must not end the session"
    );
}

#[tokio::test]
async fn viewers_leave_no_processes_behind() {
    let fixture = Fixture::new().await;
    assert_eq!(fixture.attach_processes(), 0);

    for _ in 0..20 {
        let mut socket = fixture.connect("cols=100&rows=30").await;
        read_until(&mut socket, ATTACHED).await;
        socket.close(None).await.unwrap();
        drop(socket);
    }

    // Reaping happens when the handler notices the socket is gone.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while fixture.attach_processes() > 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "{} attach processes left behind",
            fixture.attach_processes()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert!(
        fixture
            .harness
            .state
            .sessions
            .runtime()
            .exists(&fixture.tmux_name)
            .await
            .unwrap(),
        "and the session outlived all of them"
    );
}

#[tokio::test]
async fn a_resize_reaches_tmux() {
    let fixture = Fixture::new().await;
    let mut socket = fixture.connect("cols=100&rows=30").await;
    read_until(&mut socket, ATTACHED).await;

    socket
        .send(Message::text(
            json!({ "type": "resize", "cols": 132, "rows": 43 }).to_string(),
        ))
        .await
        .unwrap();

    let width = async || {
        let output = std::process::Command::new("tmux")
            .args([
                "-L",
                &fixture.harness.tmux_label,
                "display-message",
                "-p",
                "-t",
                &fixture.tmux_name,
                "#{window_width}x#{window_height}",
            ])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };

    // One row shorter than the client: tmux's status line takes it.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while width().await != "132x42" {
        assert!(
            std::time::Instant::now() < deadline,
            "the window never resized; it is {}",
            width().await
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn read_only_can_be_turned_on_and_off_without_losing_the_terminal() {
    let fixture = Fixture::new().await;
    let mut socket = fixture.connect("cols=100&rows=30").await;
    read_until(&mut socket, ATTACHED).await;

    socket
        .send(Message::text(
            json!({ "type": "read_only", "value": true }).to_string(),
        ))
        .await
        .unwrap();
    socket
        .send(Message::Binary(
            b"printf 'WHILE-READ-ONLY\n'\n".to_vec().into(),
        ))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let pane = fixture
        .harness
        .state
        .sessions
        .runtime()
        .capture(&fixture.tmux_name, 200)
        .await
        .unwrap();
    assert!(!pane.contains("WHILE-READ-ONLY"), "{pane}");

    // The same socket, still attached, can type again once allowed.
    socket
        .send(Message::text(
            json!({ "type": "read_only", "value": false }).to_string(),
        ))
        .await
        .unwrap();
    socket
        .send(Message::Binary(
            b"printf 'ALLOWED-AGAIN\n'\n".to_vec().into(),
        ))
        .await
        .unwrap();

    read_until(&mut socket, "ALLOWED-AGAIN").await;
}

#[tokio::test]
async fn an_unreadable_control_message_is_ignored_rather_than_fatal() {
    let fixture = Fixture::new().await;
    let mut socket = fixture.connect("cols=100&rows=30").await;

    socket
        .send(Message::text("{\"type\":\"nonsense\"}"))
        .await
        .unwrap();
    socket.send(Message::text("not json at all")).await.unwrap();

    socket
        .send(Message::Binary(
            b"printf 'STILL-WORKS\\n'\n".to_vec().into(),
        ))
        .await
        .unwrap();

    read_until(&mut socket, "STILL-WORKS").await;
}

#[tokio::test]
async fn an_unauthenticated_viewer_is_refused() {
    let fixture = Fixture::new().await;

    let result = tokio_tungstenite::connect_async(format!(
        "ws://{}/ws/sessions/{}/terminal",
        fixture.addr, fixture.session_id
    ))
    .await;

    assert!(result.is_err(), "the upgrade should not be accepted");
}

#[tokio::test]
async fn a_session_that_does_not_exist_cannot_be_watched() {
    let harness = Harness::new();
    let addr = harness.serve().await;

    let result = tokio_tungstenite::connect_async(format!(
        "ws://{addr}/ws/sessions/404/terminal?token={}",
        harness.token
    ))
    .await;

    assert!(result.is_err(), "there is nothing to attach to");
}

#[tokio::test]
async fn a_session_that_has_ended_cannot_be_watched() {
    let fixture = Fixture::new().await;
    let task_id = fixture
        .harness
        .store()
        .session(fixture.session_id)
        .unwrap()
        .unwrap()
        .task_id;
    fixture.harness.state.sessions.stop(task_id).await.unwrap();

    let result = tokio_tungstenite::connect_async(fixture.url("cols=100&rows=30")).await;

    assert!(result.is_err(), "an ended session has no terminal");
}
