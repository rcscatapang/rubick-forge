//! The hub against stub worker daemons.
//!
//! Each "machine" is a small axum server answering the handful of routes the
//! fleet asks for, so placement and dispatch can be driven without two Macs.

mod harness;

use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use forge_core::{AdapterId, QueueState};
use forge_daemon::config::MachineEntry;
use forge_daemon::fleet::{Fleet, NewRemoteTask};
use forge_daemon::hub::dispatch;
use forge_daemon::hub::placement::{place, Placement};
use forge_daemon::store::NewQueuedTask;
use harness::Harness;
use serde_json::{json, Value};

/// One stub worker: what it has, and what it was asked to do.
#[derive(Default)]
struct Worker {
    /// Project names it has registered, with the ids they have *there*.
    projects: Vec<(i64, String)>,
    /// Task rows it will report, as (id, status).
    tasks: Vec<(i64, String)>,
    /// Every `POST /tasks` it received.
    created: Vec<Value>,
    /// The id it hands out for the next created task.
    next_id: i64,
    /// Ids it has handed out, in order.
    handed_out: Vec<i64>,
    /// Tasks already made, by idempotency key — the real daemon's behaviour.
    by_key: std::collections::HashMap<String, i64>,
    /// When set, every request fails — a Mac that is asleep.
    down: bool,
}

type Shared = Arc<Mutex<Worker>>;

async fn projects(State(shared): State<Shared>) -> Result<Json<Value>, axum::http::StatusCode> {
    let worker = shared.lock().unwrap();
    if worker.down {
        return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }

    Ok(Json(json!({
        "projects": worker.projects.iter().map(|(id, name)| json!({
            "id": id, "name": name, "path": format!("/repos/{name}"),
            "default_branch": "main", "adapter_settings": {},
            "created_at": "2026-08-14T10:00:00Z", "updated_at": "2026-08-14T10:00:00Z",
        })).collect::<Vec<_>>()
    })))
}

async fn tasks(State(shared): State<Shared>) -> Result<Json<Value>, axum::http::StatusCode> {
    let worker = shared.lock().unwrap();
    if worker.down {
        return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }

    Ok(Json(json!({
        "tasks": worker.tasks.iter().map(|(id, status)| task_json(*id, status)).collect::<Vec<_>>()
    })))
}

fn task_json(id: i64, status: &str) -> Value {
    json!({
        "id": id, "project_id": 1, "title": format!("task {id}"),
        "adapter": "claude-code", "base_branch": "main",
        "branch": format!("forge/task-{id}"), "worktree_path": null,
        "initial_prompt": null, "status": status,
        "created_at": "2026-08-14T10:00:00Z", "updated_at": "2026-08-14T10:00:00Z",
    })
}

async fn create(
    State(shared): State<Shared>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, axum::http::StatusCode> {
    let mut worker = shared.lock().unwrap();
    if worker.down {
        return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }

    worker.created.push(body.clone());

    // A create naming an attempt that already happened returns the first task,
    // which is what makes a hub's retry safe.
    if let Some(key) = body["idempotency_key"].as_str() {
        if let Some(existing) = worker.by_key.get(key).copied() {
            return Ok(Json(task_json(existing, "idle")));
        }
    }

    worker.next_id += 1;
    let id = worker.next_id;
    worker.handed_out.push(id);

    if let Some(key) = body["idempotency_key"].as_str() {
        worker.by_key.insert(key.to_owned(), id);
    }

    Ok(Json(task_json(id, "idle")))
}

async fn start(Path(id): Path<i64>) -> Json<Value> {
    Json(json!({
        "id": 1, "task_id": id, "tmux_name": format!("forge-{id}"),
        "pid": 999, "status": "working",
        "started_at": "2026-08-14T10:00:00Z", "ended_at": null,
    }))
}

/// A stub worker on a loopback port.
async fn worker(has: &[(i64, &str)], running: &[(i64, &str)]) -> (String, Shared) {
    let shared: Shared = Arc::new(Mutex::new(Worker {
        projects: has.iter().map(|(id, n)| (*id, (*n).to_owned())).collect(),
        tasks: running
            .iter()
            .map(|(id, s)| (*id, (*s).to_owned()))
            .collect(),
        next_id: 100,
        ..Worker::default()
    }));

    let app = Router::new()
        .route("/projects", get(projects))
        .route("/tasks", get(tasks).post(create))
        .route("/tasks/{id}/start", post(start))
        .with_state(Arc::clone(&shared));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (format!("http://{addr}"), shared)
}

fn entry(name: &str, url: &str) -> MachineEntry {
    MachineEntry {
        name: name.to_owned(),
        url: url.to_owned(),
        token_ref: "unused".to_owned(),
    }
}

fn queued(project: &str, target: Option<&str>) -> NewQueuedTask {
    NewQueuedTask {
        project_name: project.to_owned(),
        adapter: AdapterId::ClaudeCode,
        title: "Fix the flaky test".into(),
        prompt: Some("it fails one run in ten".into()),
        target: target.map(str::to_owned),
    }
}

#[tokio::test]
async fn an_inventory_reports_what_each_machine_has_and_is_doing() {
    let harness = Harness::new();
    let (mini, _) = worker(&[(7, "forge")], &[(1, "working"), (2, "stopped")]).await;

    let fleet = Fleet::new(
        harness.state.clone(),
        &[entry("Mac mini", &mini)],
        &["token".to_owned()],
    );

    let inventory = dispatch::take_inventory(&fleet).await;

    // This Mac first, then the configured one.
    assert_eq!(inventory.len(), 2);
    assert_eq!(inventory[1].name, "Mac mini");
    assert!(inventory[1].reachable);
    assert_eq!(inventory[1].projects, [(7, "forge".to_owned())]);
    // Only the live one counts towards placement.
    assert_eq!(inventory[1].active, 1);
}

#[tokio::test]
async fn a_machine_that_is_asleep_is_reported_unreachable_rather_than_absent() {
    let harness = Harness::new();
    let (mini, shared) = worker(&[(7, "forge")], &[]).await;
    shared.lock().unwrap().down = true;

    let fleet = Fleet::new(
        harness.state.clone(),
        &[entry("Mac mini", &mini)],
        &["token".to_owned()],
    );

    let inventory = dispatch::take_inventory(&fleet).await;

    assert_eq!(inventory.len(), 2, "it is still on the list");
    assert!(!inventory[1].reachable);
    assert!(inventory[1].projects.is_empty());
}

#[tokio::test]
async fn a_row_is_placed_on_the_machine_that_has_its_project() {
    let harness = Harness::new();
    let (mini, _) = worker(&[(7, "forge")], &[]).await;
    let (laptop, _) = worker(&[(3, "something-else")], &[]).await;

    let fleet = Fleet::new(
        harness.state.clone(),
        &[entry("Mac mini", &mini), entry("Laptop", &laptop)],
        &["token".to_owned(), "token".to_owned()],
    );
    let inventory = dispatch::take_inventory(&fleet).await;
    let row = harness.state.store.enqueue(&queued("forge", None)).unwrap();

    let Placement::Send { machine, .. } = place(&row, &inventory) else {
        panic!("the Mac mini has forge");
    };

    assert_eq!(machine, "Mac mini");
}

#[tokio::test]
async fn a_row_for_a_project_nobody_has_stays_queued_with_a_reason() {
    let harness = Harness::new();
    let (mini, _) = worker(&[(7, "something-else")], &[]).await;

    let fleet = Fleet::new(
        harness.state.clone(),
        &[entry("Mac mini", &mini)],
        &["token".to_owned()],
    );
    let inventory = dispatch::take_inventory(&fleet).await;
    let row = harness.state.store.enqueue(&queued("forge", None)).unwrap();

    let Placement::Wait { reason, .. } = place(&row, &inventory) else {
        panic!("nobody has forge");
    };

    harness
        .state
        .store
        .set_queue_reason(row.id, &reason, &[])
        .unwrap();

    let after = harness.state.store.queued_task(row.id).unwrap().unwrap();
    assert_eq!(after.state, QueueState::Queued);
    assert!(
        after.reason.unwrap().contains("no machine has"),
        "unexplained"
    );
}

#[tokio::test]
async fn registering_the_project_makes_a_waiting_row_placeable() {
    let harness = Harness::new();
    let (mini, shared) = worker(&[], &[]).await;

    let fleet = Fleet::new(
        harness.state.clone(),
        &[entry("Mac mini", &mini)],
        &["token".to_owned()],
    );
    let row = harness.state.store.enqueue(&queued("forge", None)).unwrap();

    // Nothing has it yet.
    let before = dispatch::take_inventory(&fleet).await;
    assert!(matches!(place(&row, &before), Placement::Wait { .. }));

    // Someone registers it on that Mac.
    shared.lock().unwrap().projects.push((7, "forge".into()));

    let after = dispatch::take_inventory(&fleet).await;
    assert!(matches!(place(&row, &after), Placement::Send { .. }));
}

#[tokio::test]
async fn the_machine_doing_least_is_chosen_between_two_that_qualify() {
    let harness = Harness::new();
    let (busy, _) = worker(&[(7, "forge")], &[(1, "working"), (2, "working")]).await;
    let (idle, _) = worker(&[(9, "forge")], &[(1, "stopped")]).await;

    let fleet = Fleet::new(
        harness.state.clone(),
        &[entry("Busy", &busy), entry("Idle", &idle)],
        &["token".to_owned(), "token".to_owned()],
    );
    let inventory = dispatch::take_inventory(&fleet).await;
    let row = harness.state.store.enqueue(&queued("forge", None)).unwrap();

    let Placement::Send {
        machine,
        considered,
        ..
    } = place(&row, &inventory)
    else {
        panic!("both have forge");
    };

    assert_eq!(machine, "Idle");
    // Both are recorded, which is the audit trail requirement 3 asks for.
    assert!(considered.contains(&"Busy".to_owned()), "{considered:?}");
    assert!(considered.contains(&"Idle".to_owned()), "{considered:?}");
}

#[tokio::test]
async fn a_row_aimed_at_a_machine_goes_there_even_if_it_is_busier() {
    let harness = Harness::new();
    let (busy, _) = worker(&[(7, "forge")], &[(1, "working"), (2, "working")]).await;
    let (idle, _) = worker(&[(9, "forge")], &[]).await;

    let fleet = Fleet::new(
        harness.state.clone(),
        &[entry("Busy", &busy), entry("Idle", &idle)],
        &["token".to_owned(), "token".to_owned()],
    );
    let inventory = dispatch::take_inventory(&fleet).await;
    let row = harness
        .state
        .store
        .enqueue(&queued("forge", Some("Busy")))
        .unwrap();

    let Placement::Send { machine, .. } = place(&row, &inventory) else {
        panic!("Busy has forge");
    };

    assert_eq!(machine, "Busy", "a named target is honoured, not optimised");
}

#[tokio::test]
async fn a_dispatched_row_records_where_it_went_and_cannot_be_dispatched_twice() {
    // This is the idempotency guarantee: a retry after a crash between creating
    // the task and recording it must not be able to claim the row again.
    let harness = Harness::new();
    let row = harness.state.store.enqueue(&queued("forge", None)).unwrap();

    assert!(harness
        .state
        .store
        .mark_dispatched(row.id, "Mac mini", 101, &["Mac mini".to_owned()])
        .unwrap());

    // The retry loses, and the first placement stands.
    assert!(!harness
        .state
        .store
        .mark_dispatched(row.id, "Laptop", 202, &[])
        .unwrap());

    let after = harness.state.store.queued_task(row.id).unwrap().unwrap();
    assert_eq!(after.state, QueueState::Dispatched);
    assert_eq!(after.machine.as_deref(), Some("Mac mini"));
    assert_eq!(after.remote_task, Some(101));
    assert_eq!(after.considered, ["Mac mini"]);
}

#[tokio::test]
async fn a_created_task_carries_the_queued_title_and_prompt() {
    let harness = Harness::new();
    let (mini, shared) = worker(&[(7, "forge")], &[]).await;

    let fleet = Fleet::new(
        harness.state.clone(),
        &[entry("Mac mini", &mini)],
        &["token".to_owned()],
    );

    let machine = fleet.at(1).expect("the configured machine");
    machine
        .start(NewRemoteTask {
            project_id: 7,
            title: "Fix the flaky test".into(),
            prompt: "it fails one run in ten".into(),
            adapter: AdapterId::Codex,
            idempotency_key: Some("forge-queue-3".into()),
        })
        .await
        .unwrap();

    let created = shared.lock().unwrap().created.clone();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0]["project_id"], 7);
    assert_eq!(created[0]["title"], "Fix the flaky test");
    assert_eq!(created[0]["initial_prompt"], "it fails one run in ten");
    // The adapter the row was queued for, not the default.
    assert_eq!(created[0]["adapter"], "codex");
    // And the key that makes a retry safe.
    assert_eq!(created[0]["idempotency_key"], "forge-queue-3");
}

#[tokio::test]
async fn a_pass_places_a_row_and_records_where_it_went() {
    let harness = Harness::new();
    let (mini, worker_state) = worker(&[(7, "forge")], &[]).await;

    let fleet = Fleet::new(
        harness.state.clone(),
        &[entry("Mac mini", &mini)],
        &["token".to_owned()],
    );
    let row = harness.state.store.enqueue(&queued("forge", None)).unwrap();

    dispatch::pass(&harness.state, &fleet).await.unwrap();

    let after = harness.state.store.queued_task(row.id).unwrap().unwrap();
    assert_eq!(after.state, QueueState::Dispatched);
    assert_eq!(after.machine.as_deref(), Some("Mac mini"));
    assert_eq!(after.considered, ["Mac mini"]);
    assert_eq!(worker_state.lock().unwrap().created.len(), 1);
}

#[tokio::test]
async fn a_pass_leaves_an_unplaceable_row_queued_with_a_reason() {
    let harness = Harness::new();
    let (mini, worker_state) = worker(&[(7, "something-else")], &[]).await;

    let fleet = Fleet::new(
        harness.state.clone(),
        &[entry("Mac mini", &mini)],
        &["token".to_owned()],
    );
    let row = harness.state.store.enqueue(&queued("forge", None)).unwrap();

    dispatch::pass(&harness.state, &fleet).await.unwrap();

    let after = harness.state.store.queued_task(row.id).unwrap().unwrap();
    assert_eq!(after.state, QueueState::Queued);
    assert!(after.reason.unwrap().contains("no machine has"));
    assert!(worker_state.lock().unwrap().created.is_empty());
}

#[tokio::test]
async fn registering_the_project_gets_a_waiting_row_dispatched_next_pass() {
    let harness = Harness::new();
    let (mini, worker_state) = worker(&[], &[]).await;

    let fleet = Fleet::new(
        harness.state.clone(),
        &[entry("Mac mini", &mini)],
        &["token".to_owned()],
    );
    let row = harness.state.store.enqueue(&queued("forge", None)).unwrap();

    dispatch::pass(&harness.state, &fleet).await.unwrap();
    assert_eq!(
        harness
            .state
            .store
            .queued_task(row.id)
            .unwrap()
            .unwrap()
            .state,
        QueueState::Queued
    );

    // Someone registers it on that Mac.
    worker_state
        .lock()
        .unwrap()
        .projects
        .push((9, "forge".into()));

    dispatch::pass(&harness.state, &fleet).await.unwrap();

    let after = harness.state.store.queued_task(row.id).unwrap().unwrap();
    assert_eq!(after.state, QueueState::Dispatched);
    // Created against the id the project has *there*.
    let created = worker_state.lock().unwrap().created.clone();
    assert_eq!(created[0]["project_id"], 9);
}

#[tokio::test]
async fn a_retry_under_the_same_key_gets_the_first_task_rather_than_a_second() {
    // The case requirement 2 names: the task was created, the hub died before
    // recording it, and the next pass tries the same row again. Both attempts
    // carry the row's key, so the machine answers the second with the first
    // task and no duplicate exists.
    let harness = Harness::new();
    let (mini, worker_state) = worker(&[(7, "forge")], &[]).await;

    let fleet = Fleet::new(
        harness.state.clone(),
        &[entry("Mac mini", &mini)],
        &["token".to_owned()],
    );
    let machine = fleet.at(1).expect("the configured machine");

    let attempt = NewRemoteTask {
        project_id: 7,
        title: "Fix the flaky test".into(),
        prompt: "it fails one run in ten".into(),
        adapter: AdapterId::ClaudeCode,
        idempotency_key: Some("forge-queue-3".into()),
    };

    let first = machine.start(attempt.clone()).await.unwrap();
    let second = machine.start(attempt).await.unwrap();

    assert_eq!(first.id, second.id, "the same task, not a second one");

    let state = worker_state.lock().unwrap();
    assert_eq!(state.created.len(), 2, "it did ask twice");
    assert_eq!(state.handed_out.len(), 1, "and only one task was made");
}

#[tokio::test]
async fn a_dispatch_carries_the_rows_own_key_so_its_retry_is_the_same_attempt() {
    let harness = Harness::new();
    let (mini, worker_state) = worker(&[(7, "forge")], &[]).await;

    let fleet = Fleet::new(
        harness.state.clone(),
        &[entry("Mac mini", &mini)],
        &["token".to_owned()],
    );
    let row = harness.state.store.enqueue(&queued("forge", None)).unwrap();

    dispatch::pass(&harness.state, &fleet).await.unwrap();

    let created = worker_state.lock().unwrap().created.clone();
    assert_eq!(
        created[0]["idempotency_key"],
        format!("forge-queue-{}", row.id)
    );
}
