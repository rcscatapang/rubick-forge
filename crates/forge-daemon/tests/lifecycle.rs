//! Starting, stopping and re-adopting sessions, against a real tmux server.

mod harness;

use std::time::Duration;

use axum::http::StatusCode;
use forge_daemon::runtime::SessionRuntime;
use harness::{repo, Harness};
use serde_json::{json, Value};

/// A registered project with one task, ready to run.
struct Fixture {
    harness: Harness,
    _repo: tempfile::TempDir,
    _worktrees: tempfile::TempDir,
    task_id: i64,
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

        let (status, task) = harness
            .post(
                "/tasks",
                json!({
                    "project_id": project_id,
                    "title": "Run something",
                    "adapter": "claude-code",
                }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{task}");

        Self {
            harness,
            _repo: repo,
            _worktrees: worktrees,
            task_id: task["id"].as_i64().unwrap(),
        }
    }

    /// Start the session running a plain shell rather than the task's real
    /// agent CLI: what these tests exercise is the runtime and the poller, not
    /// Claude Code's interface.
    async fn start(&self) -> (StatusCode, Value) {
        self.start_running(&[]).await
    }

    async fn start_running(&self, command: &[String]) -> (StatusCode, Value) {
        let task = self.harness.store().task(self.task_id).unwrap().unwrap();
        let project = self
            .harness
            .store()
            .project(task.project_id)
            .unwrap()
            .unwrap();

        match self
            .harness
            .state
            .sessions
            .start(&task, &project, command)
            .await
        {
            Ok(session) => (StatusCode::CREATED, serde_json::to_value(session).unwrap()),
            Err(err) => (
                StatusCode::CONFLICT,
                json!({ "error": { "message": err.to_string() } }),
            ),
        }
    }

    async fn stop(&self) -> (StatusCode, Value) {
        self.harness
            .post(&format!("/tasks/{}/stop", self.task_id), json!({}))
            .await
    }

    async fn task(&self) -> Value {
        self.harness
            .get(&format!("/tasks/{}", self.task_id))
            .await
            .1
    }

    fn tmux_name(&self) -> String {
        format!("forge-{}", self.task_id)
    }

    async fn session_exists(&self) -> bool {
        self.harness
            .state
            .sessions
            .runtime()
            .exists(&self.tmux_name())
            .await
            .unwrap()
    }

    async fn event_kinds(&self) -> Vec<String> {
        let (_, feed) = self
            .harness
            .get(&format!("/events?task={}", self.task_id))
            .await;
        feed["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["kind"].as_str().unwrap().to_owned())
            .collect()
    }
}

/// Start a task's session the way [`Fixture::start`] does, for tests that own
/// a bare harness rather than a fixture: in process, running a plain shell.
///
/// The HTTP route looks the task's agent CLI up on `PATH` first, which is not
/// what these tests are about and is not installed on CI.
async fn start_bare(harness: &Harness, task_id: i64) {
    let task = harness.store().task(task_id).unwrap().unwrap();
    let project = harness.store().project(task.project_id).unwrap().unwrap();

    harness
        .state
        .sessions
        .start(&task, &project, &[])
        .await
        .unwrap();
}

#[tokio::test]
async fn starting_a_task_creates_a_session_in_its_worktree() {
    let fixture = Fixture::new().await;

    let (status, session) = fixture.start().await;

    assert_eq!(status, StatusCode::CREATED, "{session}");
    assert_eq!(session["task_id"], fixture.task_id);
    assert_eq!(session["tmux_name"], fixture.tmux_name());
    assert_eq!(session["status"], "idle");
    assert!(session["ended_at"].is_null());
    assert!(session["pid"].as_i64().is_some_and(|pid| pid > 0));

    assert!(fixture.session_exists().await);
    assert_eq!(fixture.task().await["status"], "idle");
}

#[tokio::test]
async fn a_session_starts_in_the_tasks_working_directory() {
    let fixture = Fixture::new().await;

    // The command reports its own directory, rather than asking tmux later:
    // an interactive shell's startup files may `cd` somewhere else, which
    // says nothing about where the session was started.
    fixture
        .start_running(&[
            "/bin/sh".into(),
            "-c".into(),
            "pwd > where.txt; sleep 30".into(),
        ])
        .await;

    let worktree = fixture.task().await["worktree_path"]
        .as_str()
        .unwrap()
        .to_owned();
    let recorded = std::path::Path::new(&worktree).join("where.txt");

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !recorded.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the session never reported where it started"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert_eq!(
        std::fs::canonicalize(std::fs::read_to_string(&recorded).unwrap().trim()).unwrap(),
        std::fs::canonicalize(&worktree).unwrap()
    );
}

#[tokio::test]
async fn starting_twice_is_refused() {
    let fixture = Fixture::new().await;
    fixture.start().await;

    let (status, body) = fixture.start().await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("already running"));
}

#[tokio::test]
async fn stopping_kills_the_session_and_closes_the_row() {
    let fixture = Fixture::new().await;
    let (_, started) = fixture.start().await;

    let (status, stopped) = fixture.stop().await;

    assert_eq!(status, StatusCode::OK, "{stopped}");
    assert_eq!(stopped["id"], started["id"]);
    assert_eq!(stopped["status"], "stopped");
    assert!(stopped["ended_at"].is_string());
    assert!(!fixture.session_exists().await);
    assert_eq!(fixture.task().await["status"], "stopped");
}

#[tokio::test]
async fn stopping_a_task_that_is_not_running_says_so() {
    let fixture = Fixture::new().await;

    let (status, body) = fixture.stop().await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("not running"));
}

#[tokio::test]
async fn restarting_gives_the_task_a_second_session_not_a_reused_one() {
    let fixture = Fixture::new().await;
    let (_, first) = fixture.start().await;

    let task = fixture
        .harness
        .store()
        .task(fixture.task_id)
        .unwrap()
        .unwrap();
    let project = fixture
        .harness
        .store()
        .project(task.project_id)
        .unwrap()
        .unwrap();
    let second = fixture
        .harness
        .state
        .sessions
        .restart(&task, &project, &[])
        .await
        .unwrap();
    let second = serde_json::to_value(second).unwrap();

    assert_ne!(second["id"], first["id"]);
    assert!(fixture.session_exists().await);

    let (_, history) = fixture
        .harness
        .get(&format!("/tasks/{}/sessions", fixture.task_id))
        .await;
    let sessions = history["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2, "the task keeps its history");
    assert!(sessions[0]["ended_at"].is_string());
    assert!(sessions[1]["ended_at"].is_null());
}

#[tokio::test]
async fn restarting_something_stopped_just_starts_it() {
    let fixture = Fixture::new().await;
    let task = fixture
        .harness
        .store()
        .task(fixture.task_id)
        .unwrap()
        .unwrap();
    let project = fixture
        .harness
        .store()
        .project(task.project_id)
        .unwrap()
        .unwrap();

    fixture
        .harness
        .state
        .sessions
        .restart(&task, &project, &[])
        .await
        .unwrap();

    assert!(fixture.session_exists().await);
}

#[tokio::test]
async fn the_lifecycle_is_announced_on_the_bus() {
    let fixture = Fixture::new().await;
    fixture.start().await;
    fixture.stop().await;

    let kinds = fixture.event_kinds().await;

    assert!(kinds.contains(&"session_started".to_owned()), "{kinds:?}");
    assert!(kinds.contains(&"session_stopped".to_owned()), "{kinds:?}");
    let started = kinds.iter().position(|k| k == "session_started").unwrap();
    let stopped = kinds.iter().position(|k| k == "session_stopped").unwrap();
    assert!(started < stopped);
}

#[tokio::test]
async fn a_session_killed_out_of_band_is_noticed_and_marked_stopped() {
    let fixture = Fixture::new().await;
    fixture.start().await;

    // Stand in for the user running `tmux kill-session` themselves.
    std::process::Command::new("tmux")
        .args([
            "-L",
            &fixture.harness.tmux_label,
            "kill-session",
            "-t",
            &fixture.tmux_name(),
        ])
        .output()
        .unwrap();

    let found = fixture.harness.state.sessions.reconcile().await.unwrap();

    assert_eq!(found.vanished, 1);
    assert_eq!(found.adopted, 0);
    assert_eq!(fixture.task().await["status"], "stopped");

    let (_, history) = fixture
        .harness
        .get(&format!("/tasks/{}/sessions", fixture.task_id))
        .await;
    assert!(history["sessions"][0]["ended_at"].is_string());

    let kinds = fixture.event_kinds().await;
    assert!(kinds.contains(&"session_stopped".to_owned()));
}

#[tokio::test]
async fn a_surviving_session_is_re_adopted_rather_than_restarted() {
    let fixture = Fixture::new().await;
    let (_, session) = fixture.start().await;

    // The daemon "restarting" is exactly this: reconcile against a tmux
    // server that kept running.
    let found = fixture.harness.state.sessions.reconcile().await.unwrap();

    assert_eq!(found.adopted, 1);
    assert_eq!(found.vanished, 0);
    assert!(fixture.session_exists().await);

    let (_, history) = fixture
        .harness
        .get(&format!("/tasks/{}/sessions", fixture.task_id))
        .await;
    let sessions = history["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1, "no duplicate row");
    assert_eq!(sessions[0]["id"], session["id"]);
    assert!(sessions[0]["ended_at"].is_null());
}

#[tokio::test]
async fn reconciling_repeatedly_changes_nothing_further() {
    let fixture = Fixture::new().await;
    fixture.start().await;

    let first = fixture.harness.state.sessions.reconcile().await.unwrap();
    let second = fixture.harness.state.sessions.reconcile().await.unwrap();

    assert_eq!(first, second);

    let (_, history) = fixture
        .harness
        .get(&format!("/tasks/{}/sessions", fixture.task_id))
        .await;
    assert_eq!(history["sessions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_forge_session_with_no_row_is_left_alone() {
    let fixture = Fixture::new().await;
    let dir = tempfile::tempdir().unwrap();

    // Someone else's session that happens to use the naming scheme.
    fixture
        .harness
        .state
        .sessions
        .runtime()
        .create("forge-999", dir.path(), &[])
        .await
        .unwrap();

    let found = fixture.harness.state.sessions.reconcile().await.unwrap();

    assert_eq!(found.unclaimed, 1);
    assert_eq!(found.adopted, 0);
    assert!(
        fixture
            .harness
            .state
            .sessions
            .runtime()
            .exists("forge-999")
            .await
            .unwrap(),
        "it must not be killed"
    );
}

/// Drive one poll pass until `done`, or fail.
async fn poll_until(fixture: &Fixture, what: &str, mut done: impl AsyncFnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);

    while !done().await {
        assert!(std::time::Instant::now() < deadline, "never {what}");
        fixture.harness.state.sessions.poll().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn stopping_interrupts_the_agent_before_killing_it() {
    let fixture = Fixture::new().await;
    fixture.start().await;

    // A script that records the interrupt it is sent. Had stop gone straight
    // to kill-session, nothing would be written.
    let worktree = fixture.task().await["worktree_path"]
        .as_str()
        .unwrap()
        .to_owned();
    let marker = std::path::Path::new(&worktree).join("interrupted.txt");
    let runtime = fixture.harness.state.sessions.runtime();

    let started = std::path::Path::new(&worktree).join("started.txt");

    runtime
        .paste(
            &fixture.tmux_name(),
            &format!(
                "sh -c 'trap \"echo yes > {} ; exit 0\" INT; touch {}; sleep 30'\n",
                marker.display(),
                started.display()
            ),
        )
        .await
        .unwrap();

    // The script itself creates this, so its presence means the trap is
    // installed and the sleep has begun.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !started.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the script never started"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    fixture.stop().await;

    assert!(
        marker.exists(),
        "stop should interrupt the agent before killing the session"
    );
}

#[tokio::test]
async fn an_agent_that_exits_cleanly_is_noticed_by_the_poller() {
    let fixture = Fixture::new().await;
    fixture.start().await;

    fixture
        .harness
        .state
        .sessions
        .runtime()
        .paste(&fixture.tmux_name(), "exit 0\n")
        .await
        .unwrap();

    poll_until(&fixture, "noticed the exit", async || {
        fixture.task().await["status"] == "stopped"
    })
    .await;

    assert!(!fixture.session_exists().await, "the session is cleaned up");
    assert!(fixture
        .event_kinds()
        .await
        .contains(&"session_stopped".to_owned()));
}

#[tokio::test]
async fn an_agent_that_crashes_is_recorded_as_an_error() {
    let fixture = Fixture::new().await;
    fixture.start().await;

    fixture
        .harness
        .state
        .sessions
        .runtime()
        .paste(&fixture.tmux_name(), "exit 3\n")
        .await
        .unwrap();

    poll_until(&fixture, "noticed the crash", async || {
        fixture.task().await["status"] != "idle"
    })
    .await;

    assert_eq!(
        fixture.task().await["status"],
        "error",
        "a non-zero exit is a crash, not a finish"
    );
}

#[tokio::test]
async fn polling_a_healthy_session_leaves_it_alone() {
    let fixture = Fixture::new().await;
    let (_, session) = fixture.start().await;

    for _ in 0..3 {
        fixture.harness.state.sessions.poll().await.unwrap();
    }

    assert!(fixture.session_exists().await);
    assert_eq!(fixture.task().await["status"], "idle");

    let (_, history) = fixture
        .harness
        .get(&format!("/tasks/{}/sessions", fixture.task_id))
        .await;
    let sessions = history["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["id"], session["id"]);
    assert!(sessions[0]["ended_at"].is_null());
}

#[tokio::test]
async fn reconciling_many_sessions_is_quick() {
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

    const SESSIONS: usize = 20;

    for index in 0..SESSIONS {
        let (_, task) = harness
            .post(
                "/tasks",
                json!({
                    "project_id": project_id,
                    "title": format!("Task {index}"),
                    "adapter": "claude-code",
                }),
            )
            .await;
        start_bare(&harness, task["id"].as_i64().unwrap()).await;
    }

    let started = std::time::Instant::now();
    let found = harness.state.sessions.reconcile().await.unwrap();
    let took = started.elapsed();

    assert_eq!(found.adopted, SESSIONS);
    assert!(took < Duration::from_secs(2), "took {took:?}");
}

#[tokio::test]
async fn a_running_task_cannot_be_deleted_or_have_its_worktree_removed() {
    let fixture = Fixture::new().await;
    fixture.start().await;

    let (status, _) = fixture
        .harness
        .post(
            &format!("/tasks/{}/worktree/cleanup", fixture.task_id),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, _) = fixture
        .harness
        .delete(&format!("/tasks/{}?force=true", fixture.task_id))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Stopping releases both.
    fixture.stop().await;
    let (status, _) = fixture
        .harness
        .delete(&format!("/tasks/{}?force=true", fixture.task_id))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_task_without_a_worktree_runs_in_the_repository_root() {
    let harness = Harness::new();
    let repo = repo();
    let project_id = harness.register(&repo).await;

    let (_, task) = harness
        .post(
            "/tasks",
            json!({
                "project_id": project_id,
                "title": "In the root",
                "adapter": "codex",
                "use_worktree": false,
            }),
        )
        .await;
    let id = task["id"].as_i64().unwrap();

    start_bare(&harness, id).await;

    assert!(harness
        .state
        .sessions
        .runtime()
        .exists(&format!("forge-{id}"))
        .await
        .unwrap());
}

#[tokio::test]
async fn an_instruction_is_typed_into_the_running_agent() {
    let fixture = Fixture::new().await;
    fixture.start().await;

    let worktree = fixture.task().await["worktree_path"]
        .as_str()
        .unwrap()
        .to_owned();
    let out = std::path::Path::new(&worktree).join("received.txt");
    let runtime = fixture.harness.state.sessions.runtime();

    // A pane that writes down whatever it is told, verbatim, and says when it
    // is ready to be told.
    let ready = std::path::Path::new(&worktree).join("ready.txt");
    runtime
        .paste(
            &fixture.tmux_name(),
            &format!("touch {}; cat > {}\n", ready.display(), out.display()),
        )
        .await
        .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !ready.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the pane never started"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Quotes, a subshell, a semicolon, a tmux key name, and a newline: every
    // one of them means something to something, and all of it is data.
    let instruction = "Fix $(whoami)'s \"bug\"; C-c\nand keep going";
    let (status, _) = fixture
        .harness
        .post(
            &format!("/tasks/{}/instruction", fixture.task_id),
            json!({ "text": instruction }),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    runtime
        .send_keys(&fixture.tmux_name(), &["C-d"])
        .await
        .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let received = std::fs::read_to_string(&out).unwrap_or_default();
        if received.contains("and keep going") {
            // Every character survived: the subshell never ran, the semicolon
            // split nothing, and `C-c` stayed three characters rather than
            // becoming an interrupt.
            assert!(
                received.contains("Fix $(whoami)'s \"bug\"; C-c"),
                "got {received:?}"
            );
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "nothing was received: {received:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn an_instruction_to_a_task_that_is_not_running_is_refused() {
    let fixture = Fixture::new().await;

    let (status, body) = fixture
        .harness
        .post(
            &format!("/tasks/{}/instruction", fixture.task_id),
            json!({ "text": "hello" }),
        )
        .await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("not running"));
}

#[tokio::test]
async fn an_empty_instruction_is_refused() {
    let fixture = Fixture::new().await;
    fixture.start().await;

    let (status, _) = fixture
        .harness
        .post(
            &format!("/tasks/{}/instruction", fixture.task_id),
            json!({ "text": "" }),
        )
        .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_status_change_is_announced_with_its_before_and_after() {
    let fixture = Fixture::new().await;
    // A pane whose text the claude-code markers read as "working".
    fixture
        .start_running(&[
            "/bin/sh".into(),
            "-c".into(),
            "printf 'esc to interrupt\\n'; sleep 30".into(),
        ])
        .await;

    poll_until(&fixture, "saw it working", async || {
        fixture.task().await["status"] == "working"
    })
    .await;

    let (_, feed) = fixture
        .harness
        .get(&format!("/events?task={}", fixture.task_id))
        .await;
    let changed = feed["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["kind"] == "status_changed")
        .expect("a status_changed event");

    assert_eq!(changed["from"], "idle");
    assert_eq!(changed["to"], "working");
    assert_eq!(changed["task_id"], fixture.task_id);
}

#[tokio::test]
async fn entering_waiting_announces_it_once_with_a_readable_tail() {
    let fixture = Fixture::new().await;
    fixture
        .start_running(&[
            "/bin/sh".into(),
            "-c".into(),
            "printf 'Do you want to make this edit?\\n'; sleep 30".into(),
        ])
        .await;

    poll_until(&fixture, "saw it waiting", async || {
        fixture.task().await["status"] == "waiting"
    })
    .await;

    // Several more passes must not restack the notification.
    for _ in 0..3 {
        fixture.harness.state.sessions.poll().await.unwrap();
    }

    let (_, feed) = fixture
        .harness
        .get(&format!("/events?task={}", fixture.task_id))
        .await;
    let waiting: Vec<&Value> = feed["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["kind"] == "agent_waiting")
        .collect();

    assert_eq!(waiting.len(), 1, "one notification per entry into waiting");
    assert!(
        waiting[0]["tail"]
            .as_str()
            .unwrap()
            .contains("Do you want to make this edit?"),
        "the tail should say what is being asked: {}",
        waiting[0]["tail"]
    );
}

#[tokio::test]
async fn an_agent_that_was_working_and_then_exits_reports_the_task_finished() {
    let fixture = Fixture::new().await;
    fixture
        .start_running(&[
            "/bin/sh".into(),
            "-c".into(),
            "printf 'esc to interrupt\\n'; sleep 1".into(),
        ])
        .await;

    poll_until(&fixture, "saw it working", async || {
        fixture.task().await["status"] == "working"
    })
    .await;
    poll_until(&fixture, "saw it finish", async || {
        fixture.task().await["status"] == "stopped"
    })
    .await;

    let kinds = fixture.event_kinds().await;
    assert!(kinds.contains(&"task_finished".to_owned()), "{kinds:?}");
}

#[tokio::test]
async fn a_task_that_only_ever_sat_at_a_prompt_does_not_claim_to_have_finished() {
    let fixture = Fixture::new().await;
    fixture
        .start_running(&["/bin/sh".into(), "-c".into(), "exit 0".into()])
        .await;

    poll_until(&fixture, "saw it stop", async || {
        fixture.task().await["status"] == "stopped"
    })
    .await;

    let kinds = fixture.event_kinds().await;
    assert!(
        !kinds.contains(&"task_finished".to_owned()),
        "nothing happened, so nothing finished: {kinds:?}"
    );
    assert!(kinds.contains(&"session_stopped".to_owned()));
}

#[tokio::test]
async fn an_error_on_screen_does_not_take_the_session_down() {
    let fixture = Fixture::new().await;
    fixture
        .start_running(&[
            "/bin/sh".into(),
            "-c".into(),
            "printf 'API Error: overloaded\\n'; sleep 30".into(),
        ])
        .await;

    poll_until(&fixture, "saw the error", async || {
        fixture.task().await["status"] == "error"
    })
    .await;

    assert!(
        fixture.session_exists().await,
        "an unhappy agent is still a running agent"
    );
    let (_, history) = fixture
        .harness
        .get(&format!("/tasks/{}/sessions", fixture.task_id))
        .await;
    assert!(
        history["sessions"][0]["ended_at"].is_null(),
        "its session is still open"
    );
    assert!(fixture
        .event_kinds()
        .await
        .contains(&"agent_error".to_owned()));
}

#[tokio::test]
async fn the_session_routes_need_a_token() {
    let harness = Harness::new();

    for uri in [
        "/tasks/1/start",
        "/tasks/1/stop",
        "/tasks/1/restart",
        "/tasks/1/instruction",
    ] {
        assert_eq!(
            harness.unauthenticated("POST", uri).await,
            StatusCode::UNAUTHORIZED,
            "{uri}"
        );
    }
    assert_eq!(
        harness.unauthenticated("GET", "/tasks/1/sessions").await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_runtime_that_cannot_be_reached_refuses_a_start_rather_than_crashing() {
    let fixture = Fixture::new().await;

    // Point the daemon at a tmux that is not there. `preflight` is what turns
    // that into an answer instead of a panic.
    let broken = Harness::with_tmux_binary("tmux-does-not-exist");
    let repo = repo();
    let project_id = broken.register(&repo).await;
    let worktrees = tempfile::tempdir().unwrap();
    broken.set_setting(
        "worktree_root",
        &std::fs::canonicalize(worktrees.path())
            .unwrap()
            .display()
            .to_string(),
    );
    let (_, task) = broken
        .post(
            "/tasks",
            json!({ "project_id": project_id, "title": "No tmux", "adapter": "codex" }),
        )
        .await;
    let id = task["id"].as_i64().unwrap();

    let (status, body) = broken.post(&format!("/tasks/{id}/start"), json!({})).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["code"], "runtime_unavailable");

    // And the daemon carries on serving everything else.
    let (status, _) = broken.get("/tasks").await;
    assert_eq!(status, StatusCode::OK);
    drop(fixture);
}

#[tokio::test]
async fn acting_on_a_task_that_does_not_exist_is_a_404() {
    let harness = Harness::new();

    for uri in ["/tasks/404/start", "/tasks/404/stop", "/tasks/404/restart"] {
        let (status, _) = harness.post(uri, json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
}
