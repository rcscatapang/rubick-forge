//! `/projects` end to end, against real git repositories on disk.

mod harness;

use axum::http::StatusCode;
use harness::{repo, Harness};
use serde_json::json;

#[tokio::test]
async fn a_repository_can_be_registered_listed_patched_and_removed() {
    let harness = Harness::new();
    let temp = repo();
    let root = std::fs::canonicalize(temp.path()).unwrap();

    let (status, project) = harness
        .post("/projects", json!({ "path": root.display().to_string() }))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(project["path"], root.display().to_string());
    assert_eq!(project["default_branch"], "main");
    assert_eq!(project["adapter_settings"], json!({}));
    let id = project["id"].as_i64().unwrap();

    let (status, list) = harness.get("/projects").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["projects"].as_array().unwrap().len(), 1);

    let (status, fetched) = harness.get(&format!("/projects/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched, project);

    harness::git(&root, &["branch", "develop"]);
    let (status, patched) = harness
        .patch(
            &format!("/projects/{id}"),
            json!({ "default_branch": "develop", "name": "renamed" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["default_branch"], "develop");
    assert_eq!(patched["name"], "renamed");

    let (status, _) = harness.delete(&format!("/projects/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = harness.get(&format!("/projects/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_name_defaults_to_the_repository_directory() {
    let harness = Harness::new();
    let temp = repo();
    let root = std::fs::canonicalize(temp.path()).unwrap();

    let (_, project) = harness
        .post("/projects", json!({ "path": root.display().to_string() }))
        .await;

    assert_eq!(project["name"], root.file_name().unwrap().to_str().unwrap());
}

#[tokio::test]
async fn a_subdirectory_registers_the_repository_root() {
    let harness = Harness::new();
    let temp = repo();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let nested = root.join("src");
    std::fs::create_dir_all(&nested).unwrap();

    let (status, project) = harness
        .post("/projects", json!({ "path": nested.display().to_string() }))
        .await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(project["path"], root.display().to_string());
}

#[tokio::test]
async fn registering_a_non_repository_writes_nothing() {
    let harness = Harness::new();
    let temp = tempfile::tempdir().unwrap();

    let (status, body) = harness
        .post(
            "/projects",
            json!({ "path": temp.path().display().to_string() }),
        )
        .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("not inside a git repository"));

    let (_, list) = harness.get("/projects").await;
    assert!(list["projects"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn registering_a_missing_path_writes_nothing() {
    let harness = Harness::new();

    let (status, _) = harness
        .post("/projects", json!({ "path": "/definitely/not/here" }))
        .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, list) = harness.get("/projects").await;
    assert!(list["projects"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn the_same_repository_cannot_be_registered_twice() {
    let harness = Harness::new();
    let temp = repo();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let path = json!({ "path": root.display().to_string() });

    harness.post("/projects", path.clone()).await;
    let (status, body) = harness.post("/projects", path).await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("already registered"));

    let (_, list) = harness.get("/projects").await;
    assert_eq!(list["projects"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_subdirectory_counts_as_the_same_repository() {
    let harness = Harness::new();
    let temp = repo();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let nested = root.join("src");
    std::fs::create_dir_all(&nested).unwrap();

    harness
        .post("/projects", json!({ "path": root.display().to_string() }))
        .await;
    let (status, _) = harness
        .post("/projects", json!({ "path": nested.display().to_string() }))
        .await;

    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn patching_to_a_branch_that_does_not_exist_is_refused() {
    let harness = Harness::new();
    let temp = repo();
    let id = harness.register(&temp).await;

    let (status, _) = harness
        .patch(
            &format!("/projects/{id}"),
            json!({ "default_branch": "nope" }),
        )
        .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, project) = harness.get(&format!("/projects/{id}")).await;
    assert_eq!(project["default_branch"], "main");
}

#[tokio::test]
async fn adapter_settings_round_trip_and_are_shape_checked() {
    let harness = Harness::new();
    let temp = repo();
    let id = harness.register(&temp).await;

    let (status, patched) = harness
        .patch(
            &format!("/projects/{id}"),
            json!({ "adapter_settings": { "claude-code": { "model": "opus" } } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["adapter_settings"]["claude-code"]["model"], "opus");

    // Adapters are manifests now, so an id this daemon has no manifest for is
    // settings written ahead of one arriving, not a mistake.
    let (status, _) = harness
        .patch(
            &format!("/projects/{id}"),
            json!({ "adapter_settings": { "aider": { "model": "sonnet" } } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // An id that could never name a manifest is still refused.
    let (status, body) = harness
        .patch(
            &format!("/projects/{id}"),
            json!({ "adapter_settings": { "../evil": {} } }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"].as_str().unwrap().contains("evil"));

    // The rejected patch changed nothing.
    let (_, project) = harness.get(&format!("/projects/{id}")).await;
    assert_eq!(project["adapter_settings"]["aider"]["model"], "sonnet");
}

#[tokio::test]
async fn git_facts_are_reported_for_the_repository_root() {
    let harness = Harness::new();
    let temp = repo();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let id = harness.register(&temp).await;

    let (status, clean) = harness.get(&format!("/projects/{id}/git")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(clean["branch"], "main");
    assert_eq!(clean["dirty"], false);
    assert!(clean["head"].is_string());
    assert!(clean["upstream"].is_null());

    std::fs::write(root.join("scratch.txt"), "wip").unwrap();
    let (_, dirty) = harness.get(&format!("/projects/{id}/git")).await;
    assert_eq!(dirty["dirty"], true);
}

#[tokio::test]
async fn git_facts_degrade_on_a_detached_head() {
    let harness = Harness::new();
    let temp = repo();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let id = harness.register(&temp).await;

    let head = harness::git(&root, &["rev-parse", "HEAD"]);
    harness::git(&root, &["checkout", "--quiet", head.trim()]);

    let (status, facts) = harness.get(&format!("/projects/{id}/git")).await;

    assert_eq!(status, StatusCode::OK);
    assert!(facts["branch"].is_null());
    assert!(facts["head"].is_string());
}

#[tokio::test]
async fn a_project_with_a_live_task_cannot_be_removed() {
    let harness = Harness::new();
    let temp = repo();
    let id = harness.register(&temp).await;
    harness.insert_task(id, "working");

    let (status, body) = harness.delete(&format!("/projects/{id}")).await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("not stopped"));

    let (status, _) = harness.get(&format!("/projects/{id}")).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_project_whose_tasks_are_all_stopped_can_be_removed() {
    let harness = Harness::new();
    let temp = repo();
    let id = harness.register(&temp).await;
    harness.insert_task(id, "stopped");

    let (status, _) = harness.delete(&format!("/projects/{id}")).await;

    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn registering_and_removing_are_announced_on_the_bus() {
    let harness = Harness::new();
    let temp = repo();
    let id = harness.register(&temp).await;
    harness.delete(&format!("/projects/{id}")).await;

    let (_, feed) = harness.get("/events").await;
    let kinds: Vec<&str> = feed["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["kind"].as_str().unwrap())
        .collect();

    assert_eq!(kinds, ["project_registered", "project_removed"]);
    assert_eq!(feed["events"][0]["project_id"], id);
    assert_eq!(feed["events"][1]["project_id"], id);
}

#[tokio::test]
async fn the_whole_surface_needs_a_token() {
    let harness = Harness::new();

    for (method, uri) in [
        ("GET", "/projects"),
        ("POST", "/projects"),
        ("GET", "/projects/1"),
        ("PATCH", "/projects/1"),
        ("DELETE", "/projects/1"),
        ("GET", "/projects/1/git"),
    ] {
        let status = harness.unauthenticated(method, uri).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}
