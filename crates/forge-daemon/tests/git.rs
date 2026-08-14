//! The git wrapper, against real repositories and a real `git`.

mod harness;

use forge_daemon::git::{self, GitError};
use harness::{git as run_git, repo};

/// Write a file and commit it.
fn commit(repo: &std::path::Path, name: &str, contents: &str) {
    std::fs::write(repo.join(name), contents).unwrap();
    run_git(repo, &["add", name]);
    run_git(repo, &["commit", "--quiet", "-m", &format!("add {name}")]);
}

#[tokio::test]
async fn a_repository_resolves_to_its_own_root() {
    let temp = repo();
    let expected = std::fs::canonicalize(temp.path()).unwrap();

    assert_eq!(git::repo_root(temp.path()).await.unwrap(), expected);
}

#[tokio::test]
async fn a_subdirectory_resolves_to_the_root_above_it() {
    let temp = repo();
    let nested = temp.path().join("src").join("deep");
    std::fs::create_dir_all(&nested).unwrap();

    assert_eq!(
        git::repo_root(&nested).await.unwrap(),
        std::fs::canonicalize(temp.path()).unwrap()
    );
}

#[tokio::test]
async fn a_plain_directory_is_not_a_repository() {
    let temp = tempfile::tempdir().unwrap();

    assert!(matches!(
        git::repo_root(temp.path()).await,
        Err(GitError::NotARepository(_))
    ));
}

#[tokio::test]
async fn a_missing_path_is_named_as_such() {
    let temp = tempfile::tempdir().unwrap();

    assert!(matches!(
        git::repo_root(&temp.path().join("nope")).await,
        Err(GitError::NotADirectory(_))
    ));
}

#[tokio::test]
async fn the_default_branch_is_the_current_one_without_a_remote() {
    let temp = repo();

    assert_eq!(git::default_branch(temp.path()).await.unwrap(), "main");
}

#[tokio::test]
async fn the_remotes_head_wins_over_the_local_branch() {
    let temp = repo();
    run_git(temp.path(), &["checkout", "--quiet", "-b", "scratch"]);
    run_git(
        temp.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/repo.git",
        ],
    );
    // Stand in for what `git remote set-head` would have written.
    run_git(
        temp.path(),
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );

    assert_eq!(git::default_branch(temp.path()).await.unwrap(), "main");
}

#[tokio::test]
async fn a_detached_head_still_yields_a_default_branch() {
    let temp = repo();
    let head = run_git(temp.path(), &["rev-parse", "HEAD"]);
    run_git(temp.path(), &["checkout", "--quiet", head.trim()]);

    assert_eq!(git::default_branch(temp.path()).await.unwrap(), "main");
}

#[tokio::test]
async fn branch_existence_is_reported_without_erroring() {
    let temp = repo();

    assert!(git::branch_exists(temp.path(), "main").await.unwrap());
    assert!(!git::branch_exists(temp.path(), "nope").await.unwrap());
}

#[tokio::test]
async fn a_branch_name_that_looks_like_a_flag_is_still_data() {
    let temp = repo();

    // Read as an option, `--all` would make rev-parse succeed and the branch
    // appear to exist.
    assert!(!git::branch_exists(temp.path(), "--all").await.unwrap());
    assert!(!git::branch_exists(temp.path(), "-f").await.unwrap());
}

#[tokio::test]
async fn a_clean_repository_reports_a_branch_and_no_changes() {
    let temp = repo();

    let status = git::status(temp.path()).await.unwrap();

    assert_eq!(status.branch.as_deref(), Some("main"));
    assert!(status.head.is_some());
    assert!(!status.dirty);
    assert_eq!(status.upstream, None);
    assert_eq!(status.ahead, None);
    assert_eq!(status.behind, None);
}

#[tokio::test]
async fn an_untracked_file_makes_the_tree_dirty() {
    let temp = repo();
    std::fs::write(temp.path().join("scratch.txt"), "wip").unwrap();

    assert!(git::status(temp.path()).await.unwrap().dirty);
}

#[tokio::test]
async fn a_modified_tracked_file_makes_the_tree_dirty() {
    let temp = repo();
    std::fs::write(temp.path().join("README.md"), "changed").unwrap();

    assert!(git::status(temp.path()).await.unwrap().dirty);
}

#[tokio::test]
async fn a_detached_head_degrades_to_no_branch() {
    let temp = repo();
    let head = run_git(temp.path(), &["rev-parse", "HEAD"]);
    run_git(temp.path(), &["checkout", "--quiet", head.trim()]);

    let status = git::status(temp.path()).await.unwrap();

    assert_eq!(status.branch, None);
    assert!(status.head.is_some());
}

#[tokio::test]
async fn a_directory_that_is_not_a_repository_is_an_error_not_a_detached_head() {
    let temp = tempfile::tempdir().unwrap();

    assert!(matches!(
        git::status(temp.path()).await,
        Err(GitError::Unreadable { .. })
    ));
}

#[tokio::test]
async fn divergence_is_counted_against_a_tracking_branch() {
    let temp = repo();
    let clone_dir = tempfile::tempdir().unwrap();
    let clone = clone_dir.path().join("clone");

    // A local clone gives a real upstream without touching the network.
    let output = std::process::Command::new("git")
        .args(["clone", "--quiet"])
        .arg(temp.path())
        .arg(&clone)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    run_git(&clone, &["config", "user.email", "test@example.com"]);
    run_git(&clone, &["config", "user.name", "Test"]);

    let fresh = git::status(&clone).await.unwrap();
    assert_eq!(fresh.ahead, Some(0));
    assert_eq!(fresh.behind, Some(0));
    assert!(fresh.upstream.is_some());

    commit(&clone, "local.txt", "only here");
    let diverged = git::status(&clone).await.unwrap();
    assert_eq!(diverged.ahead, Some(1));
    assert_eq!(diverged.behind, Some(0));
}

#[tokio::test]
async fn a_clean_worktree_has_nothing_to_commit() {
    let temp = repo();
    commit(temp.path(), "a.txt", "one\n");

    assert!(git::diff_stat(temp.path()).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_diffstat_counts_what_would_be_committed() {
    let temp = repo();
    commit(temp.path(), "a.txt", "one\n");
    std::fs::write(temp.path().join("a.txt"), "one\ntwo\n").unwrap();

    let stat = git::diff_stat(temp.path()).await.unwrap();

    assert_eq!(stat.files, 1);
    assert_eq!(stat.insertions, 1);
    assert_eq!(stat.paths, ["a.txt"]);
    assert!(!stat.is_empty());
}

#[tokio::test]
async fn an_untracked_file_counts_even_though_it_is_not_in_the_diff() {
    let temp = repo();
    commit(temp.path(), "a.txt", "one\n");
    std::fs::write(temp.path().join("new.txt"), "fresh\n").unwrap();

    let stat = git::diff_stat(temp.path()).await.unwrap();

    assert_eq!(stat.files, 1);
    assert_eq!(stat.paths, ["new.txt"]);
}

#[tokio::test]
async fn committing_stages_everything_and_returns_the_new_head() {
    let temp = repo();
    commit(temp.path(), "a.txt", "one\n");
    std::fs::write(temp.path().join("a.txt"), "changed\n").unwrap();
    std::fs::write(temp.path().join("new.txt"), "fresh\n").unwrap();

    let sha = git::commit_all(temp.path(), "Fix the flaky test")
        .await
        .unwrap();

    assert_eq!(sha.len(), 40, "{sha}");
    assert!(git::diff_stat(temp.path()).await.unwrap().is_empty());
}

#[tokio::test]
async fn committing_nothing_is_refused_rather_than_recorded() {
    let temp = repo();
    commit(temp.path(), "a.txt", "one\n");

    // An empty commit is not what anyone pressing "commit" meant.
    assert!(matches!(
        git::commit_all(temp.path(), "nothing to see").await,
        Err(GitError::Failed { .. })
    ));
}

#[tokio::test]
async fn a_commit_needs_a_message() {
    let temp = repo();
    std::fs::write(temp.path().join("a.txt"), "one\n").unwrap();

    assert!(matches!(
        git::commit_all(temp.path(), "   ").await,
        Err(GitError::Failed { .. })
    ));
}

#[tokio::test]
async fn a_message_that_looks_like_an_option_is_still_a_message() {
    let temp = repo();
    commit(temp.path(), "a.txt", "one\n");
    std::fs::write(temp.path().join("a.txt"), "changed\n").unwrap();

    let before = harness::git(temp.path(), &["rev-list", "--count", "HEAD"])
        .trim()
        .parse::<u32>()
        .unwrap();

    git::commit_all(temp.path(), "--amend all the things")
        .await
        .unwrap();

    let log = harness::git(temp.path(), &["log", "-1", "--pretty=%s"]);
    assert_eq!(log.trim(), "--amend all the things");

    // And it added a commit rather than rewriting the one before it.
    let after = harness::git(temp.path(), &["rev-list", "--count", "HEAD"])
        .trim()
        .parse::<u32>()
        .unwrap();
    assert_eq!(after, before + 1);
}
