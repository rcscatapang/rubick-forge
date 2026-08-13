//! The tmux runtime against a real tmux server.
//!
//! Every test runs on its own `-L` socket, so nothing here can see, disturb or
//! be disturbed by the tmux the developer is sitting in.

use std::path::Path;
use std::time::Duration;

use forge_daemon::runtime::{SessionRuntime, TmuxError, TmuxRuntime, CAPTURE_LINES};

/// A server nobody else shares.
///
/// The label carries the process id as well as the test's name: a label reused
/// across runs would let a server leaked by a failed run be adopted by the
/// next one, which fails in ways that look like real bugs.
struct Server {
    runtime: TmuxRuntime,
    label: String,
}

impl Server {
    fn new(label: &str) -> Self {
        let label = format!("forge-test-{}-{label}", std::process::id());
        Self {
            runtime: TmuxRuntime::with_socket(&label),
            label,
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::process::Command::new("tmux")
            .args(["-L", &self.label, "kill-server"])
            .output();
    }
}

/// Wait for a condition the session reaches on its own schedule.
async fn until(mut check: impl AsyncFnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the session never reached the expected state");
}

fn shell(script: &str) -> Vec<String> {
    vec!["/bin/sh".into(), "-c".into(), script.into()]
}

#[tokio::test]
async fn a_real_tmux_passes_preflight() {
    let server = Server::new("preflight");

    server.runtime.preflight().await.unwrap();
    assert!(server.runtime.version().await.unwrap().major >= 3);
}

#[tokio::test]
async fn a_missing_tmux_is_reported_as_missing() {
    // A socket label cannot make tmux itself absent, so this checks the error
    // mapping through a runtime pointed at a binary that is not there.
    let err = forge_daemon::exec::run("tmux-does-not-exist", &["-V"], None, Duration::from_secs(5))
        .await
        .unwrap_err();

    assert!(matches!(err, forge_daemon::exec::ExecError::NotFound(_)));
}

#[tokio::test]
async fn a_session_lives_where_it_was_told_to() {
    let server = Server::new("cwd");
    let dir = tempfile::tempdir().unwrap();
    let cwd = std::fs::canonicalize(dir.path()).unwrap();

    server
        .runtime
        .create("forge-1", &cwd, &shell("pwd > where.txt; sleep 30"))
        .await
        .unwrap();

    assert!(server.runtime.exists("forge-1").await.unwrap());
    until(async || cwd.join("where.txt").is_file()).await;

    let recorded = std::fs::read_to_string(cwd.join("where.txt")).unwrap();
    assert_eq!(
        std::fs::canonicalize(recorded.trim()).unwrap(),
        cwd,
        "the session should start in its own directory"
    );
}

#[tokio::test]
async fn sessions_are_listed_created_and_killed() {
    let server = Server::new("lifecycle");
    let dir = tempfile::tempdir().unwrap();

    assert!(server.runtime.list().await.unwrap().is_empty());
    assert!(!server.runtime.exists("forge-1").await.unwrap());

    server
        .runtime
        .create("forge-1", dir.path(), &shell("sleep 30"))
        .await
        .unwrap();
    server
        .runtime
        .create("forge-2", dir.path(), &shell("sleep 30"))
        .await
        .unwrap();

    let mut listed = server.runtime.list().await.unwrap();
    listed.sort();
    assert_eq!(listed, ["forge-1", "forge-2"]);

    server.runtime.kill("forge-1").await.unwrap();
    assert!(!server.runtime.exists("forge-1").await.unwrap());
    assert_eq!(server.runtime.list().await.unwrap(), ["forge-2"]);
}

#[tokio::test]
async fn killing_a_session_that_is_already_gone_is_not_an_error() {
    let server = Server::new("kill-twice");
    let dir = tempfile::tempdir().unwrap();
    server
        .runtime
        .create("forge-1", dir.path(), &shell("sleep 30"))
        .await
        .unwrap();

    server.runtime.kill("forge-1").await.unwrap();
    server.runtime.kill("forge-1").await.unwrap();
    server.runtime.kill("never-existed").await.unwrap();
}

#[tokio::test]
async fn creating_the_same_session_twice_is_refused() {
    let server = Server::new("duplicate");
    let dir = tempfile::tempdir().unwrap();
    let command = shell("sleep 30");

    server
        .runtime
        .create("forge-1", dir.path(), &command)
        .await
        .unwrap();

    assert!(server
        .runtime
        .create("forge-1", dir.path(), &command)
        .await
        .is_err());

    // The refusal must not take the existing session with it.
    assert!(server.runtime.exists("forge-1").await.unwrap());
    assert!(server.runtime.pane_state("forge-1").await.unwrap().alive);
}

#[tokio::test]
async fn output_is_captured_from_the_pane() {
    let server = Server::new("capture");
    let dir = tempfile::tempdir().unwrap();

    server
        .runtime
        .create(
            "forge-1",
            dir.path(),
            &shell("echo hello-from-the-pane; sleep 30"),
        )
        .await
        .unwrap();

    until(async || {
        server
            .runtime
            .capture("forge-1", CAPTURE_LINES)
            .await
            .unwrap()
            .contains("hello-from-the-pane")
    })
    .await;
}

#[tokio::test]
async fn capturing_a_session_that_is_not_there_says_so() {
    let server = Server::new("capture-missing");

    assert!(matches!(
        server.runtime.capture("forge-1", CAPTURE_LINES).await,
        Err(TmuxError::NoSuchSession(_))
    ));
}

#[tokio::test]
async fn keys_reach_the_process_in_the_pane() {
    let server = Server::new("send-keys");
    let dir = tempfile::tempdir().unwrap();
    let cwd = std::fs::canonicalize(dir.path()).unwrap();

    server
        .runtime
        .create(
            "forge-1",
            &cwd,
            &shell("read line; echo \"got:$line\" > out.txt; sleep 30"),
        )
        .await
        .unwrap();

    server
        .runtime
        .send_keys("forge-1", &["typed", "Enter"])
        .await
        .unwrap();

    until(async || cwd.join("out.txt").is_file()).await;
    assert_eq!(
        std::fs::read_to_string(cwd.join("out.txt")).unwrap().trim(),
        "got:typed"
    );
}

#[tokio::test]
async fn pasted_text_arrives_verbatim_however_hostile() {
    let server = Server::new("paste");
    let dir = tempfile::tempdir().unwrap();
    let cwd = std::fs::canonicalize(dir.path()).unwrap();

    // Every one of these means something to a shell, to tmux's key parser, or
    // to both. All of it must land as data.
    let hostile = r#"$(touch pwned); `touch pwned2`; "quoted" 'single' C-c ; rm -rf /"#;

    server
        .runtime
        .create(
            "forge-1",
            &cwd,
            &shell("read -r line; printf '%s' \"$line\" > out.txt; sleep 30"),
        )
        .await
        .unwrap();

    server.runtime.paste("forge-1", hostile).await.unwrap();
    server
        .runtime
        .send_keys("forge-1", &["Enter"])
        .await
        .unwrap();

    until(async || cwd.join("out.txt").is_file()).await;

    assert_eq!(
        std::fs::read_to_string(cwd.join("out.txt")).unwrap(),
        hostile
    );
    assert!(!cwd.join("pwned").exists(), "a subshell ran");
    assert!(!cwd.join("pwned2").exists(), "a subshell ran");
}

#[tokio::test]
async fn multiline_text_keeps_its_newlines() {
    let server = Server::new("paste-multiline");
    let dir = tempfile::tempdir().unwrap();
    let cwd = std::fs::canonicalize(dir.path()).unwrap();

    server
        .runtime
        .create("forge-1", &cwd, &shell("cat > out.txt; sleep 30"))
        .await
        .unwrap();

    server
        .runtime
        .paste("forge-1", "first line\nsecond line\n")
        .await
        .unwrap();
    // End the here-doc-ish read.
    server.runtime.send_keys("forge-1", &["C-d"]).await.unwrap();

    until(async || {
        std::fs::read_to_string(cwd.join("out.txt"))
            .map(|text| text.lines().count() >= 2)
            .unwrap_or(false)
    })
    .await;

    let written = std::fs::read_to_string(cwd.join("out.txt")).unwrap();
    assert_eq!(
        written.lines().collect::<Vec<_>>(),
        ["first line", "second line"]
    );
}

#[tokio::test]
async fn a_live_pane_reports_a_pid_and_no_exit_code() {
    let server = Server::new("pane-live");
    let dir = tempfile::tempdir().unwrap();

    server
        .runtime
        .create("forge-1", dir.path(), &shell("sleep 30"))
        .await
        .unwrap();

    let state = server.runtime.pane_state("forge-1").await.unwrap();

    assert!(state.alive);
    assert!(state.pid.is_some_and(|pid| pid > 0));
    assert_eq!(state.exit_code, None);
}

#[tokio::test]
async fn a_dead_pane_reports_how_it_exited() {
    let server = Server::new("pane-dead");
    let dir = tempfile::tempdir().unwrap();

    // `create` turns on remain-on-exit, which is what keeps the pane around
    // to be asked how it ended.
    server
        .runtime
        .create("forge-1", dir.path(), &shell("exit 3"))
        .await
        .unwrap();

    until(async || {
        server
            .runtime
            .pane_state("forge-1")
            .await
            .map(|state| !state.alive)
            .unwrap_or(false)
    })
    .await;

    let state = server.runtime.pane_state("forge-1").await.unwrap();
    assert!(!state.alive);
    assert_eq!(state.exit_code, Some(3));
}

#[tokio::test]
async fn asking_about_a_session_that_is_not_there_says_so() {
    let server = Server::new("pane-missing");

    assert!(matches!(
        server.runtime.pane_state("forge-1").await,
        Err(TmuxError::NoSuchSession(_))
    ));
}

#[tokio::test]
async fn a_session_name_that_looks_like_a_flag_is_still_a_name() {
    let server = Server::new("hostile-name");
    let dir = tempfile::tempdir().unwrap();

    // Forge never generates such a name, but nothing here may treat one as an
    // option if it ever did.
    let result = server
        .runtime
        .create("-d", dir.path(), &shell("sleep 30"))
        .await;

    // Either tmux refuses it or it becomes a session — never a silently
    // reinterpreted option.
    if result.is_ok() {
        assert!(server
            .runtime
            .list()
            .await
            .unwrap()
            .contains(&"-d".to_owned()));
    }
}

#[tokio::test]
async fn work_continues_while_a_human_is_attached() {
    let server = Server::new("attached");
    let dir = tempfile::tempdir().unwrap();
    let cwd = std::fs::canonicalize(dir.path()).unwrap();

    // Every line, not just the first: an attaching client may deliver a
    // newline of its own, and that must not be mistaken for the input.
    server
        .runtime
        .create(
            "forge-1",
            &cwd,
            &shell(
                "while read -r line; do [ -n \"$line\" ] && echo \"got:$line\" >> out.txt; done",
            ),
        )
        .await
        .unwrap();

    // Stand in for a person running `tmux attach`: a second client on the same
    // session, in its own pty. Its stdin is a pipe held open for the duration,
    // because a client whose stdin ends immediately sends EOF into the pane.
    let mut attached = std::process::Command::new("script")
        .args([
            "-q",
            "/dev/null",
            "tmux",
            "-L",
            &server.label,
            "attach",
            "-t",
            "forge-1",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("script should be available on macOS");
    let held_open = attached.stdin.take();

    // Wait for the client to really be attached rather than guessing.
    until(async || {
        std::process::Command::new("tmux")
            .args(["-L", &server.label, "list-clients", "-t", "forge-1"])
            .output()
            .map(|out| !out.stdout.is_empty())
            .unwrap_or(false)
    })
    .await;

    assert!(
        server.runtime.pane_state("forge-1").await.unwrap().alive,
        "the attach should not have killed the pane"
    );

    // Capture and send-keys must both still work with a client attached.
    server
        .runtime
        .capture("forge-1", CAPTURE_LINES)
        .await
        .unwrap();
    server
        .runtime
        .send_keys("forge-1", &["typed", "Enter"])
        .await
        .unwrap();

    until(async || {
        std::fs::read_to_string(cwd.join("out.txt"))
            .map(|text| text.contains("got:typed"))
            .unwrap_or(false)
    })
    .await;

    drop(held_open);
    let _ = attached.kill();
    let _ = attached.wait();
}

#[tokio::test]
async fn activity_moves_when_the_pane_produces_output() {
    let server = Server::new("activity");
    let dir = tempfile::tempdir().unwrap();

    server
        .runtime
        .create(
            "forge-1",
            dir.path(),
            &shell("while true; do echo tick; sleep 0.1; done"),
        )
        .await
        .unwrap();

    let before = server.runtime.activity("forge-1").await.unwrap();
    assert!(before.is_some());

    until(async || server.runtime.activity("forge-1").await.unwrap() != before).await;
}

#[tokio::test]
async fn each_socket_is_its_own_world() {
    let first = Server::new("isolation-a");
    let second = Server::new("isolation-b");
    let dir = tempfile::tempdir().unwrap();

    first
        .runtime
        .create("forge-1", dir.path(), &shell("sleep 30"))
        .await
        .unwrap();

    assert!(first.runtime.exists("forge-1").await.unwrap());
    assert!(!second.runtime.exists("forge-1").await.unwrap());
    assert!(second.runtime.list().await.unwrap().is_empty());
}

/// The runtime must not care that `Path` is not `Send`-friendly across awaits.
#[tokio::test]
async fn the_runtime_is_usable_from_a_spawned_task() {
    let server = Server::new("spawned");
    let dir = tempfile::tempdir().unwrap();
    let runtime = server.runtime.clone();
    let path = dir.path().to_path_buf();

    tokio::spawn(async move {
        runtime
            .create("forge-1", Path::new(&path), &shell("sleep 30"))
            .await
    })
    .await
    .unwrap()
    .unwrap();

    assert!(server.runtime.exists("forge-1").await.unwrap());
}
