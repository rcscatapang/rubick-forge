//! Event hooks against real scripts.
//!
//! Hooks run other people's executables, so the things worth proving are the
//! limits: what they are told, where they run, and that they cannot outlast
//! their timeout or pile up without bound.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use forge_core::{EventKind, EventRecord, ForgeEvent, Timestamp};
use forge_daemon::hooks::{self, Hooks, HooksConfig};

/// Write an executable shell script into `dir`.
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn finished(task_id: i64) -> EventRecord {
    EventRecord {
        id: 1,
        ts: Timestamp::now(),
        event: ForgeEvent::TaskFinished {
            task_id,
            session_id: 3,
        },
    }
}

#[tokio::test]
async fn a_hook_is_handed_the_event_on_stdin() {
    let temp = tempfile::tempdir().unwrap();
    let hook = script(temp.path(), "echo.sh", "cat");

    let payload = serde_json::to_string(&finished(12)).unwrap();
    let outcome = hooks::run(&hook, &payload, None, Duration::from_secs(5))
        .await
        .unwrap();

    assert_eq!(outcome.code, Some(0));
    // The whole event, so a hook can act on any of it.
    assert!(outcome.output.contains("task_finished"), "{outcome:?}");
    assert!(outcome.output.contains("\"task_id\":12"), "{outcome:?}");
}

#[tokio::test]
async fn a_hook_runs_in_the_worktree_when_the_event_has_one() {
    let temp = tempfile::tempdir().unwrap();
    let worktree = temp.path().join("tree");
    std::fs::create_dir(&worktree).unwrap();
    let hook = script(temp.path(), "where.sh", "pwd");

    let outcome = hooks::run(&hook, "{}", Some(&worktree), Duration::from_secs(5))
        .await
        .unwrap();

    // macOS puts a symlink in front of the temp directory.
    let reported = std::fs::canonicalize(outcome.output.trim()).unwrap();
    assert_eq!(reported, std::fs::canonicalize(&worktree).unwrap());
}

#[tokio::test]
async fn a_hook_whose_worktree_is_gone_still_runs() {
    let temp = tempfile::tempdir().unwrap();
    let hook = script(temp.path(), "ok.sh", "echo fine");

    let outcome = hooks::run(
        &hook,
        "{}",
        Some(&temp.path().join("cleaned-up")),
        Duration::from_secs(5),
    )
    .await
    .unwrap();

    assert_eq!(outcome.output, "fine");
}

#[tokio::test]
async fn a_hanging_hook_is_killed_at_the_timeout() {
    let temp = tempfile::tempdir().unwrap();
    let hook = script(temp.path(), "hang.sh", "sleep 30");

    let started = Instant::now();
    let error = hooks::run(&hook, "{}", None, Duration::from_millis(200))
        .await
        .unwrap_err();

    assert!(error.contains("killed"), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "it was not waited on"
    );
}

#[tokio::test]
async fn a_hook_that_reads_nothing_does_not_hang_the_daemon() {
    // stdin is closed after the payload, so a script that never reads it — or
    // one that reads to end-of-input — finishes either way.
    let temp = tempfile::tempdir().unwrap();
    let hook = script(temp.path(), "ignore.sh", "echo done");

    let outcome = hooks::run(&hook, &"x".repeat(100_000), None, Duration::from_secs(5))
        .await
        .unwrap();

    assert_eq!(outcome.output, "done");
}

#[tokio::test]
async fn a_failing_hook_is_recorded_rather_than_acted_on() {
    // Fire and forget: the exit code is reported and changes nothing.
    let temp = tempfile::tempdir().unwrap();
    let hook = script(temp.path(), "fail.sh", "echo went wrong >&2; exit 3");

    let outcome = hooks::run(&hook, "{}", None, Duration::from_secs(5))
        .await
        .unwrap();

    assert_eq!(outcome.code, Some(3));
    assert!(outcome.output.contains("went wrong"));
}

#[tokio::test]
async fn a_hook_that_is_not_there_is_an_error_not_a_panic() {
    let temp = tempfile::tempdir().unwrap();

    let error = hooks::run(
        &temp.path().join("missing.sh"),
        "{}",
        None,
        Duration::from_secs(5),
    )
    .await
    .unwrap_err();

    assert!(error.contains("cannot run it"), "{error}");
}

#[tokio::test]
async fn only_the_configured_event_runs_a_hook() {
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("ran");
    script(
        temp.path(),
        "touch.sh",
        &format!("touch {}", marker.display()),
    );

    let config: HooksConfig = toml::from_str(
        r#"
[on]
task_finished = "touch.sh"
"#,
    )
    .unwrap();
    let hooks = Arc::new(Hooks::new(config, temp.path().to_path_buf()));

    // An event nothing is configured for.
    hooks.fire(
        &EventRecord {
            id: 2,
            ts: Timestamp::now(),
            event: ForgeEvent::TaskDeleted { task_id: 1 },
        },
        None,
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!marker.exists(), "an unconfigured event ran something");

    hooks.fire(&finished(1), None);
    for _ in 0..200 {
        if marker.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the configured event did not run its hook");
}

#[tokio::test]
async fn a_storm_of_events_drops_rather_than_piling_up() {
    // A fleet of agents finishing at once must cost log lines, not a fork bomb.
    let temp = tempfile::tempdir().unwrap();
    let counter = temp.path().join("count");

    // Each run appends a line, so the file counts how many were let through.
    script(
        temp.path(),
        "slow.sh",
        &format!("echo x >> {}; sleep 0.4", counter.display()),
    );

    let config: HooksConfig = toml::from_str(
        r#"
concurrency = 1
queue = 1

[on]
task_finished = "slow.sh"
"#,
    )
    .unwrap();
    let hooks = Arc::new(Hooks::new(config, temp.path().to_path_buf()));

    for _ in 0..50 {
        hooks.fire(&finished(1), None);
    }

    // One running plus one queued. The other 48 were refused outright rather
    // than held, which is the whole point of a bounded queue.
    tokio::time::sleep(Duration::from_millis(1_500)).await;

    let ran = std::fs::read_to_string(&counter)
        .unwrap_or_default()
        .lines()
        .count();
    assert!(ran >= 1, "nothing ran at all");
    assert!(ran <= 2, "{ran} of 50 ran; the queue is not bounded");
}

#[tokio::test]
async fn a_hook_pointing_outside_its_directory_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("escaped");

    let config = HooksConfig {
        on: [(
            EventKind::TaskFinished.as_str().to_owned(),
            format!("../{}", marker.display()),
        )]
        .into_iter()
        .collect(),
        ..HooksConfig::default()
    };
    let hooks = Arc::new(Hooks::new(config, temp.path().to_path_buf()));

    hooks.fire(&finished(1), None);
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(!marker.exists(), "a path escaped the hooks directory");
}
