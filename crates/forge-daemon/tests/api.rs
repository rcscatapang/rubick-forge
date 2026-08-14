//! End-to-end checks of the HTTP/WS surface against an in-process daemon.

mod harness;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use forge_core::{EventRecord, ForgeEvent};
use futures_util::StreamExt;
use harness::{Harness, Socket};

fn project_event(id: i64) -> ForgeEvent {
    ForgeEvent::ProjectRegistered {
        project_id: id,
        name: format!("p{id}"),
        path: format!("/repos/p{id}"),
    }
}

/// Read one event frame, failing rather than hanging if none arrives.
async fn next_event(socket: &mut Socket) -> EventRecord {
    let message = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .expect("an event should arrive")
        .expect("the socket should stay open")
        .expect("the frame should be readable");

    serde_json::from_str(message.to_text().unwrap()).unwrap()
}

#[tokio::test]
async fn health_answers_without_a_token() {
    let harness = Harness::new();

    let (status, body) = harness.anonymous_get("/health").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["version"], "0.1.0-test");
    assert!(body["uptime_secs"].is_number());
    assert!(body["machine"].is_string());
    // The tools the daemon needs, then the agent CLIs it can drive.
    let names: Vec<&str> = body["binaries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["tmux", "git", "claude", "codex"]);
}

#[tokio::test]
async fn the_adapters_a_daemon_can_drive_are_listed_with_their_settings() {
    let harness = Harness::new();

    let (status, body) = harness.get("/adapters").await;

    assert_eq!(status, StatusCode::OK);
    let adapters = body["adapters"].as_array().unwrap();
    assert_eq!(adapters.len(), 2);

    let ids: Vec<&str> = adapters.iter().map(|a| a["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["claude-code", "codex"]);

    assert_eq!(adapters[0]["name"], "Claude Code");
    assert_eq!(adapters[0]["binary"]["name"], "claude");
    assert!(adapters[0]["binary"]["ok"].is_boolean());

    let settings = adapters[0]["settings"].as_array().unwrap();
    assert!(settings.iter().any(|s| s["key"] == "model"));
    assert!(settings.iter().all(|s| s["kind"].is_string()));
    assert!(settings
        .iter()
        .all(|s| !s["description"].as_str().unwrap().is_empty()));
}

#[tokio::test]
async fn the_adapter_list_needs_a_token() {
    let harness = Harness::new();

    assert_eq!(
        harness.unauthenticated("GET", "/adapters").await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn every_other_endpoint_needs_the_right_token() {
    let harness = Harness::new();

    let (status, body) = harness.anonymous_get("/events").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "unauthorized");

    let (status, _) = harness
        .send(
            Request::builder()
                .uri("/events")
                .header("authorization", "Bearer not-the-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = harness.get("/events").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_rest_endpoint_refuses_a_token_in_the_url() {
    let harness = Harness::new();

    let (status, _) = harness
        .anonymous_get(&format!("/events?token={}", harness.token))
        .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unknown_paths_and_methods_answer_in_the_error_envelope() {
    let harness = Harness::new();

    let (status, body) = harness.get("/nowhere").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");

    let (status, body) = harness
        .send(
            Request::builder()
                .method("POST")
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(body["error"]["code"], "method_not_allowed");
}

#[tokio::test]
async fn a_malformed_query_string_answers_in_the_error_envelope() {
    let harness = Harness::new();

    let (status, body) = harness.get("/events?after=soon").await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "bad_request");
}

#[tokio::test]
async fn the_feed_can_be_narrowed_to_one_task() {
    let harness = Harness::new();
    harness.bus().publish(project_event(1)).unwrap();
    harness
        .bus()
        .publish(ForgeEvent::TaskDeleted { task_id: 7 })
        .unwrap();

    let (_, body) = harness.get("/events?task=7").await;

    assert_eq!(body["events"].as_array().unwrap().len(), 1);
    assert_eq!(body["events"][0]["task_id"], 7);
}

#[tokio::test]
async fn events_page_by_id_cursor() {
    let harness = Harness::new();
    for id in 1..=3 {
        harness.bus().publish(project_event(id)).unwrap();
    }

    let (_, first) = harness.get("/events?limit=2").await;
    assert_eq!(first["events"].as_array().unwrap().len(), 2);
    assert_eq!(first["events"][0]["kind"], "project_registered");

    let cursor = first["next_after"].as_i64().unwrap();
    let (_, rest) = harness.get(&format!("/events?after={cursor}")).await;
    assert_eq!(rest["events"].as_array().unwrap().len(), 1);
    assert_eq!(rest["events"][0]["project_id"], 3);
}

#[tokio::test]
async fn a_websocket_receives_events_published_after_it_connects() {
    let harness = Harness::new();
    let addr = harness.serve().await;

    let mut socket = harness
        .connect(addr, &format!("token={}", harness.token))
        .await;
    // The handler subscribes during the upgrade; publishing immediately after a
    // successful connect is enough to be seen.
    let published = harness.bus().publish(project_event(1)).unwrap();

    let received = next_event(&mut socket).await;
    assert_eq!(received, published);
}

#[tokio::test]
async fn a_websocket_with_a_cursor_backfills_before_going_live() {
    let harness = Harness::new();
    let addr = harness.serve().await;

    let first = harness.bus().publish(project_event(1)).unwrap();
    harness.bus().publish(project_event(2)).unwrap();

    let mut socket = harness
        .connect(addr, &format!("token={}&after={}", harness.token, first.id))
        .await;

    // The event published before the connection but after the cursor.
    assert_eq!(next_event(&mut socket).await.event, project_event(2));

    let live = harness.bus().publish(project_event(3)).unwrap();
    let received = next_event(&mut socket).await;
    assert_eq!(received, live);
}

#[tokio::test]
async fn an_unauthenticated_websocket_upgrade_is_rejected() {
    let harness = Harness::new();
    let addr = harness.serve().await;

    let result = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events")).await;

    assert!(result.is_err(), "the upgrade should not be accepted");
}

#[tokio::test]
async fn a_wrong_token_cannot_open_a_websocket() {
    let harness = Harness::new();
    let addr = harness.serve().await;

    let result =
        tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events?token=nope")).await;

    assert!(result.is_err(), "the upgrade should not be accepted");
}

#[tokio::test]
async fn two_viewers_both_see_every_event() {
    let harness = Harness::new();
    let addr = harness.serve().await;

    let mut first = harness
        .connect(addr, &format!("token={}", harness.token))
        .await;
    let mut second = harness
        .connect(addr, &format!("token={}", harness.token))
        .await;

    let published = harness.bus().publish(project_event(1)).unwrap();

    assert_eq!(next_event(&mut first).await, published);
    assert_eq!(next_event(&mut second).await, published);
}

#[tokio::test]
async fn closing_a_socket_leaves_the_other_viewer_alone() {
    let harness = Harness::new();
    let addr = harness.serve().await;

    let mut leaving = harness
        .connect(addr, &format!("token={}", harness.token))
        .await;
    let mut staying = harness
        .connect(addr, &format!("token={}", harness.token))
        .await;

    leaving.close(None).await.unwrap();

    let published = harness.bus().publish(project_event(1)).unwrap();
    assert_eq!(next_event(&mut staying).await, published);
}

#[tokio::test]
async fn everything_on_the_socket_is_also_in_the_history() {
    let harness = Harness::new();
    let addr = harness.serve().await;

    let mut socket = harness
        .connect(addr, &format!("token={}", harness.token))
        .await;
    harness.bus().publish(project_event(1)).unwrap();
    let streamed = next_event(&mut socket).await;

    let (_, history) = harness.get("/events").await;
    let stored: EventRecord = serde_json::from_value(history["events"][0].clone()).unwrap();

    assert_eq!(streamed, stored);
}
