//! `/tasks` end to end, against real repositories and real git worktrees.

mod harness;

use std::path::{Path, PathBuf};

use axum::http::StatusCode;
use harness::{git, repo, Harness};
use serde_json::{json, Value};

/// A registered project plus the repository behind it.
struct Fixture {
    harness: Harness,
    repo: tempfile::TempDir,
    project_id: i64,
    /// A worktree root of this fixture's own, so worktrees are cleaned up with
    /// it instead of accumulating in the shared temp directory. One test opts
    /// out to check the default placement.
    worktree_root: Option<tempfile::TempDir>,
}

impl Fixture {
    async fn new() -> Self {
        let mut fixture = Self::with_default_root().await;
        let root = tempfile::tempdir().unwrap();
        fixture.harness.set_setting(
            "worktree_root",
            &std::fs::canonicalize(root.path())
                .unwrap()
                .display()
                .to_string(),
        );
        fixture.worktree_root = Some(root);
        fixture
    }

    /// A fixture that uses the sibling shadow directory, as a fresh install
    /// would.
    async fn with_default_root() -> Self {
        let harness = Harness::new();
        let repo = repo();
        let project_id = harness.register(&repo).await;
        Self {
            harness,
            repo,
            project_id,
            worktree_root: None,
        }
    }

    fn root(&self) -> PathBuf {
        std::fs::canonicalize(self.repo.path()).unwrap()
    }

    /// The project's name, which registration takes from the repository
    /// directory — a temp name here, not something to hard-code.
    fn project_name(&self) -> String {
        self.root()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    /// The directory the daemon derives from that name: lowercase ASCII words
    /// joined by hyphens, since a project name may be anything a user types.
    fn project_dir(&self) -> String {
        let mut slug = String::new();
        for ch in self.project_name().chars() {
            if ch.is_ascii_alphanumeric() {
                slug.extend(ch.to_lowercase());
            } else if !slug.ends_with('-') {
                slug.push('-');
            }
        }
        slug.trim_matches('-').to_owned()
    }

    /// Where this project's worktrees land: the fixture's own root when it
    /// has one, else the sibling shadow directory beside the repository.
    fn worktree_dir(&self) -> PathBuf {
        let root = match &self.worktree_root {
            Some(root) => std::fs::canonicalize(root.path()).unwrap(),
            None => self.root().parent().unwrap().join(".forge-worktrees"),
        };
        root.join(self.project_dir())
    }

    async fn create(&self, body: Value) -> (StatusCode, Value) {
        let mut payload = json!({
            "project_id": self.project_id,
            "title": "Add adapters",
            "adapter": "claude-code",
        });
        for (key, value) in body.as_object().unwrap() {
            payload[key] = value.clone();
        }
        self.harness.post("/tasks", payload).await
    }

    /// The worktrees git itself reports, excluding the repository's own.
    fn worktrees(&self) -> Vec<String> {
        let root = self.root();
        git(&root, &["worktree", "list", "--porcelain"])
            .lines()
            .filter_map(|line| line.strip_prefix("worktree "))
            .map(str::to_owned)
            .filter(|path| Path::new(path) != root)
            .collect()
    }

    fn branches(&self) -> Vec<String> {
        git(&self.root(), &["branch", "--format=%(refname:short)"])
            .lines()
            .map(str::trim)
            .map(str::to_owned)
            .collect()
    }
}

#[tokio::test]
async fn creating_a_task_provisions_a_branch_and_a_worktree() {
    let fixture = Fixture::with_default_root().await;

    let (status, task) = fixture.create(json!({})).await;

    assert_eq!(status, StatusCode::CREATED, "{task}");
    let id = task["id"].as_i64().unwrap();
    assert_eq!(task["branch"], format!("forge/add-adapters-{id}"));
    assert_eq!(task["base_branch"], "main");
    assert_eq!(task["status"], "stopped");

    let path = PathBuf::from(task["worktree_path"].as_str().unwrap());
    assert_eq!(
        path,
        fixture.worktree_dir().join(format!("add-adapters-{id}"))
    );
    assert!(path.is_dir(), "the worktree should exist on disk");
    assert!(path.join("README.md").is_file(), "and be checked out");

    assert_eq!(fixture.worktrees(), [path.display().to_string()]);
    assert!(fixture
        .branches()
        .contains(&format!("forge/add-adapters-{id}")));
}

#[tokio::test]
async fn a_task_can_opt_out_of_a_worktree_and_run_in_the_repository() {
    let fixture = Fixture::new().await;

    let (status, task) = fixture.create(json!({ "use_worktree": false })).await;

    assert_eq!(status, StatusCode::CREATED);
    assert!(task["worktree_path"].is_null());
    assert_eq!(task["branch"], "main");
    assert!(fixture.worktrees().is_empty());

    // Its git facts are the repository's own.
    let id = task["id"].as_i64().unwrap();
    let (status, facts) = fixture.harness.get(&format!("/tasks/{id}/git")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(facts["branch"], "main");
}

#[tokio::test]
async fn cleanup_on_a_task_without_a_worktree_says_so() {
    let fixture = Fixture::new().await;
    let (_, task) = fixture.create(json!({ "use_worktree": false })).await;
    let id = task["id"].as_i64().unwrap();

    let (status, body) = fixture
        .harness
        .post(&format!("/tasks/{id}/worktree/cleanup"), json!({}))
        .await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("no worktree"));
}

#[tokio::test]
async fn a_failed_provision_leaves_no_task_row_and_no_directory() {
    let fixture = Fixture::new().await;

    // Occupy the branch the first task will want.
    let (_, first) = fixture.create(json!({})).await;
    let id = first["id"].as_i64().unwrap();
    let taken = format!("forge/add-adapters-{}", id + 1);
    git(&fixture.root(), &["branch", &taken]);

    let (status, body) = fixture.create(json!({})).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("already exists"),
        "{body}"
    );
    let (_, list) = fixture.harness.get("/tasks").await;
    assert_eq!(
        list["tasks"].as_array().unwrap().len(),
        1,
        "only the first task should survive"
    );
    assert!(!fixture
        .worktree_dir()
        .join(format!("add-adapters-{}", id + 1))
        .exists());
}

#[tokio::test]
async fn two_tasks_with_the_same_title_get_their_own_branch_and_directory() {
    let fixture = Fixture::new().await;

    let (_, first) = fixture.create(json!({})).await;
    let (status, second) = fixture.create(json!({})).await;

    assert_eq!(status, StatusCode::CREATED);
    assert_ne!(first["branch"], second["branch"]);
    assert_ne!(first["worktree_path"], second["worktree_path"]);
    assert_eq!(fixture.worktrees().len(), 2);
}

#[tokio::test]
async fn a_base_branch_that_does_not_exist_is_refused() {
    let fixture = Fixture::new().await;

    let (status, body) = fixture.create(json!({ "base_branch": "nope" })).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("not a branch"));
    assert!(fixture.harness.get("/tasks").await.1["tasks"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_task_forks_from_the_base_branch_it_names() {
    let fixture = Fixture::new().await;
    let root = fixture.root();
    git(&root, &["checkout", "--quiet", "-b", "develop"]);
    std::fs::write(root.join("only-on-develop.txt"), "x").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "--quiet", "-m", "develop only"]);
    git(&root, &["checkout", "--quiet", "main"]);

    let (status, task) = fixture.create(json!({ "base_branch": "develop" })).await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(task["base_branch"], "develop");
    let path = PathBuf::from(task["worktree_path"].as_str().unwrap());
    assert!(path.join("only-on-develop.txt").is_file());
}

#[tokio::test]
async fn an_empty_title_is_refused() {
    let fixture = Fixture::new().await;

    let (status, _) = fixture.create(json!({ "title": "   " })).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_task_for_an_unknown_project_is_refused() {
    let fixture = Fixture::new().await;

    let (status, _) = fixture.create(json!({ "project_id": 404 })).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn tasks_can_be_listed_filtered_and_patched() {
    let fixture = Fixture::new().await;
    let (_, task) = fixture.create(json!({})).await;
    let id = task["id"].as_i64().unwrap();

    let (status, list) = fixture.harness.get("/tasks").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["tasks"].as_array().unwrap().len(), 1);

    let (_, filtered) = fixture
        .harness
        .get(&format!("/tasks?project={}", fixture.project_id))
        .await;
    assert_eq!(filtered["tasks"].as_array().unwrap().len(), 1);

    let (_, none) = fixture.harness.get("/tasks?status=working").await;
    assert!(none["tasks"].as_array().unwrap().is_empty());

    let (status, patched) = fixture
        .harness
        .patch(
            &format!("/tasks/{id}"),
            json!({ "title": "Renamed", "initial_prompt": "go" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["title"], "Renamed");
    assert_eq!(patched["initial_prompt"], "go");
    // Renaming does not move the worktree.
    assert_eq!(patched["worktree_path"], task["worktree_path"]);
    assert_eq!(patched["branch"], task["branch"]);
}

#[tokio::test]
async fn cleanup_refuses_a_dirty_worktree_and_succeeds_with_force() {
    let fixture = Fixture::new().await;
    let (_, task) = fixture.create(json!({})).await;
    let id = task["id"].as_i64().unwrap();
    let path = PathBuf::from(task["worktree_path"].as_str().unwrap());
    std::fs::write(path.join("README.md"), "uncommitted").unwrap();

    let (status, body) = fixture
        .harness
        .post(&format!("/tasks/{id}/worktree/cleanup"), json!({}))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("uncommitted"));
    assert!(path.is_dir(), "a refused cleanup changes nothing");

    let (status, cleaned) = fixture
        .harness
        .post(
            &format!("/tasks/{id}/worktree/cleanup"),
            json!({ "force": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{cleaned}");
    assert!(!path.exists());
    assert!(cleaned["worktree_path"].is_null());
    // The branch was kept, so the task still points at where its work is.
    assert_eq!(cleaned["branch"], task["branch"]);
    assert!(fixture.worktrees().is_empty());
}

#[tokio::test]
async fn cleanup_keeps_the_branch_unless_asked_to_delete_it() {
    let fixture = Fixture::new().await;
    let (_, task) = fixture.create(json!({})).await;
    let id = task["id"].as_i64().unwrap();
    let branch = task["branch"].as_str().unwrap().to_owned();

    fixture
        .harness
        .post(&format!("/tasks/{id}/worktree/cleanup"), json!({}))
        .await;
    assert!(fixture.branches().contains(&branch));
}

#[tokio::test]
async fn cleanup_deletes_the_branch_when_asked() {
    let fixture = Fixture::new().await;
    let (_, task) = fixture.create(json!({})).await;
    let id = task["id"].as_i64().unwrap();
    let branch = task["branch"].as_str().unwrap().to_owned();

    let (status, cleaned) = fixture
        .harness
        .post(
            &format!("/tasks/{id}/worktree/cleanup"),
            json!({ "delete_branch": true, "force": true }),
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert!(!fixture.branches().contains(&branch));
    assert_eq!(
        cleaned["branch"], "main",
        "with no branch left, back to base"
    );
}

#[tokio::test]
async fn deleting_a_task_does_not_destroy_its_branch() {
    let fixture = Fixture::new().await;
    let (_, task) = fixture.create(json!({})).await;
    let id = task["id"].as_i64().unwrap();
    let branch = task["branch"].as_str().unwrap().to_owned();
    let path = PathBuf::from(task["worktree_path"].as_str().unwrap());
    std::fs::write(path.join("work.txt"), "unmerged").unwrap();

    let (status, _) = fixture
        .harness
        .delete(&format!("/tasks/{id}?force=true"))
        .await;

    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!path.exists());
    assert!(
        fixture.branches().contains(&branch),
        "unmerged work must survive a task delete"
    );
}

#[tokio::test]
async fn a_worktree_deleted_by_hand_can_still_be_cleaned_up() {
    let fixture = Fixture::new().await;
    let (_, task) = fixture.create(json!({})).await;
    let id = task["id"].as_i64().unwrap();
    let path = PathBuf::from(task["worktree_path"].as_str().unwrap());

    std::fs::remove_dir_all(&path).unwrap();

    let (status, cleaned) = fixture
        .harness
        .post(&format!("/tasks/{id}/worktree/cleanup"), json!({}))
        .await;

    assert_eq!(status, StatusCode::OK, "{cleaned}");
    assert!(cleaned["worktree_path"].is_null());
    assert!(fixture.worktrees().is_empty(), "git forgot it too");
}

#[tokio::test]
async fn a_hostile_project_name_cannot_place_a_worktree_outside_the_root() {
    let fixture = Fixture::new().await;
    let worktree_root = fixture.worktree_dir().parent().unwrap().to_path_buf();

    for hostile in ["../../../escaped", "/etc", ".."] {
        fixture
            .harness
            .patch(
                &format!("/projects/{}", fixture.project_id),
                json!({ "name": hostile }),
            )
            .await;

        let (status, task) = fixture.create(json!({})).await;
        assert_eq!(status, StatusCode::CREATED, "{hostile}: {task}");

        let path = PathBuf::from(task["worktree_path"].as_str().unwrap());
        assert!(
            path.starts_with(&worktree_root),
            "{hostile} escaped to {}",
            path.display()
        );
        // One directory for the project, one for the task, and nothing else.
        assert_eq!(
            path.strip_prefix(&worktree_root)
                .unwrap()
                .components()
                .count(),
            2,
            "{}",
            path.display()
        );
    }
}

#[tokio::test]
async fn deleting_a_task_with_a_worktree_needs_force() {
    let fixture = Fixture::new().await;
    let (_, task) = fixture.create(json!({})).await;
    let id = task["id"].as_i64().unwrap();
    let path = PathBuf::from(task["worktree_path"].as_str().unwrap());

    let (status, body) = fixture.harness.delete(&format!("/tasks/{id}")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("clean it up first"));
    assert!(path.is_dir());

    let (status, _) = fixture
        .harness
        .delete(&format!("/tasks/{id}?force=true"))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!path.exists());
    assert!(fixture.worktrees().is_empty());
    assert_eq!(
        fixture.harness.get(&format!("/tasks/{id}")).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_cleaned_up_task_deletes_without_force() {
    let fixture = Fixture::new().await;
    let (_, task) = fixture.create(json!({})).await;
    let id = task["id"].as_i64().unwrap();

    fixture
        .harness
        .post(&format!("/tasks/{id}/worktree/cleanup"), json!({}))
        .await;
    let (status, _) = fixture.harness.delete(&format!("/tasks/{id}")).await;

    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_task_with_a_live_session_can_be_neither_cleaned_nor_deleted() {
    let fixture = Fixture::new().await;
    let (_, task) = fixture.create(json!({})).await;
    let id = task["id"].as_i64().unwrap();
    fixture.harness.insert_session(id);

    let (status, _) = fixture
        .harness
        .post(&format!("/tasks/{id}/worktree/cleanup"), json!({}))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, _) = fixture
        .harness
        .delete(&format!("/tasks/{id}?force=true"))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_custom_worktree_root_is_respected() {
    let fixture = Fixture::with_default_root().await;
    let elsewhere = tempfile::tempdir().unwrap();
    fixture
        .harness
        .set_setting("worktree_root", &elsewhere.path().display().to_string());

    let (status, task) = fixture.create(json!({})).await;

    assert_eq!(status, StatusCode::CREATED, "{task}");
    let path = PathBuf::from(task["worktree_path"].as_str().unwrap());
    assert!(path.starts_with(elsewhere.path()), "{}", path.display());
    assert!(path.is_dir());
    assert!(
        !fixture.worktree_dir().exists(),
        "the default root is unused"
    );
}

#[tokio::test]
async fn a_worktrees_git_facts_are_its_own_not_the_repositorys() {
    let fixture = Fixture::new().await;
    let (_, task) = fixture.create(json!({})).await;
    let id = task["id"].as_i64().unwrap();
    let path = PathBuf::from(task["worktree_path"].as_str().unwrap());

    let (status, clean) = fixture.harness.get(&format!("/tasks/{id}/git")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(clean["branch"], format!("forge/add-adapters-{id}"));
    assert_eq!(clean["dirty"], false);

    std::fs::write(path.join("scratch.txt"), "wip").unwrap();
    let (_, dirty) = fixture.harness.get(&format!("/tasks/{id}/git")).await;
    assert_eq!(dirty["dirty"], true);
    // The repository itself is untouched.
    let (_, repo_facts) = fixture
        .harness
        .get(&format!("/projects/{}/git", fixture.project_id))
        .await;
    assert_eq!(repo_facts["dirty"], false);
}

#[tokio::test]
async fn the_lifecycle_is_announced_on_the_bus() {
    let fixture = Fixture::new().await;
    let (_, task) = fixture.create(json!({})).await;
    let id = task["id"].as_i64().unwrap();

    fixture
        .harness
        .post(&format!("/tasks/{id}/worktree/cleanup"), json!({}))
        .await;
    fixture.harness.delete(&format!("/tasks/{id}")).await;

    let (_, feed) = fixture.harness.get(&format!("/events?task={id}")).await;
    let kinds: Vec<&str> = feed["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["kind"].as_str().unwrap())
        .collect();

    assert_eq!(
        kinds,
        [
            "worktree_created",
            "task_created",
            "worktree_removed",
            "task_deleted"
        ]
    );
}

#[tokio::test]
async fn a_hostile_title_stays_inside_the_worktree_root() {
    let fixture = Fixture::new().await;

    let (status, task) = fixture
        .create(json!({ "title": "../../etc/passwd; rm -rf /" }))
        .await;

    assert_eq!(status, StatusCode::CREATED, "{task}");
    let path = PathBuf::from(task["worktree_path"].as_str().unwrap());
    assert!(
        path.starts_with(fixture.worktree_dir()),
        "{}",
        path.display()
    );
    assert!(path.is_dir());
    // The title itself is stored verbatim; only the slug is sanitised.
    assert_eq!(task["title"], "../../etc/passwd; rm -rf /");
}

#[tokio::test]
async fn the_whole_surface_needs_a_token() {
    let harness = Harness::new();

    for (method, uri) in [
        ("GET", "/tasks"),
        ("POST", "/tasks"),
        ("GET", "/tasks/1"),
        ("PATCH", "/tasks/1"),
        ("DELETE", "/tasks/1"),
        ("GET", "/tasks/1/git"),
        ("POST", "/tasks/1/worktree/cleanup"),
    ] {
        assert_eq!(
            harness.unauthenticated(method, uri).await,
            StatusCode::UNAUTHORIZED,
            "{method} {uri}"
        );
    }
}
