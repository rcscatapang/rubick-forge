//! Everything the daemon asks git, as typed calls over the `git` CLI.
//!
//! git is shelled out to rather than linked, so this is the only place that
//! knows its output formats. Errors carry git's own first complaint, which is
//! usually the most useful sentence available; deciding what a client is
//! allowed to see is the HTTP layer's job.

use std::path::{Path, PathBuf};

use forge_core::GitStatus;

use crate::exec::{self, ExecError, Output};

const BINARY: &str = "git";

/// The absolute repository root containing `path`.
///
/// This is also the registration check: a path that is not in a repository
/// fails here, and one that is gets normalised, so `/repo/src` and `/repo`
/// cannot both be registered as different projects.
pub async fn repo_root(path: &Path) -> Result<PathBuf, GitError> {
    if !path.is_dir() {
        return Err(GitError::NotADirectory(path.to_path_buf()));
    }

    let output = run(path, &["rev-parse", "--show-toplevel"]).await?;
    if !output.success() {
        return Err(GitError::NotARepository(path.to_path_buf()));
    }

    let root = output.stdout.trim();
    if root.is_empty() {
        return Err(GitError::NotARepository(path.to_path_buf()));
    }

    // `--show-toplevel` is already absolute; canonicalising also resolves the
    // symlinks macOS puts in front of /tmp and /var.
    std::fs::canonicalize(root).map_err(|source| GitError::Unreadable {
        path: PathBuf::from(root),
        detail: source.to_string(),
    })
}

/// The branch a new task should fork from unless told otherwise.
///
/// Prefers what the remote calls its default, because that is what the human
/// means by "main" even when their local HEAD is elsewhere.
pub async fn default_branch(repo: &Path) -> Result<String, GitError> {
    let remote_head = run(
        repo,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .await?;
    if remote_head.success() {
        if let Some(branch) = remote_head.stdout.trim().strip_prefix("origin/") {
            if !branch.is_empty() {
                return Ok(branch.to_owned());
            }
        }
    }

    let head = run(repo, &["symbolic-ref", "--short", "HEAD"]).await?;
    if head.success() && !head.stdout.trim().is_empty() {
        return Ok(head.stdout.trim().to_owned());
    }

    // Detached HEAD, or a repository with no commits: fall back to what a
    // fresh `git init` here would have used.
    let configured = run(repo, &["config", "--get", "init.defaultBranch"]).await?;
    let configured = configured.stdout.trim();
    Ok(if configured.is_empty() {
        "main".to_owned()
    } else {
        configured.to_owned()
    })
}

/// Whether `branch` resolves to a commit in `repo`.
pub async fn branch_exists(repo: &Path, branch: &str) -> Result<bool, GitError> {
    let output = run(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{branch}^{{commit}}"),
        ],
    )
    .await?;
    Ok(output.success())
}

/// Branch, dirtiness and divergence for a working tree.
pub async fn status(tree: &Path) -> Result<GitStatus, GitError> {
    // First, and checked: every later command answers with a non-zero exit for
    // states that are perfectly normal (detached HEAD, no commits yet), so
    // they cannot tell a broken repository from a legitimate one. This one can.
    let dirty = {
        let output = run(tree, &["status", "--porcelain"]).await?;
        require(&output, tree)?;
        !output.stdout.trim().is_empty()
    };

    let branch = {
        let output = run(tree, &["symbolic-ref", "--short", "HEAD"]).await?;
        output
            .success()
            .then(|| output.stdout.trim().to_owned())
            .filter(|branch| !branch.is_empty())
    };

    let head = {
        let output = run(tree, &["rev-parse", "--short", "HEAD"]).await?;
        output
            .success()
            .then(|| output.stdout.trim().to_owned())
            .filter(|head| !head.is_empty())
    };

    let upstream = {
        let output = run(
            tree,
            &[
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ],
        )
        .await?;
        output
            .success()
            .then(|| output.stdout.trim().to_owned())
            .filter(|name| !name.is_empty())
    };

    let (ahead, behind) = match upstream {
        Some(_) => divergence(tree).await?,
        None => (None, None),
    };

    Ok(GitStatus {
        branch,
        head,
        dirty,
        upstream,
        ahead,
        behind,
    })
}

/// `behind<TAB>ahead` from git's own left-right count.
async fn divergence(tree: &Path) -> Result<(Option<u32>, Option<u32>), GitError> {
    let output = run(
        tree,
        &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"],
    )
    .await?;

    if !output.success() {
        return Ok((None, None));
    }

    let mut counts = output
        .stdout
        .split_whitespace()
        .map(|count| count.parse::<u32>().ok());

    let behind = counts.next().flatten();
    let ahead = counts.next().flatten();
    Ok((ahead, behind))
}

async fn run(cwd: &Path, args: &[&str]) -> Result<Output, GitError> {
    exec::run(BINARY, args, Some(cwd), exec::DEFAULT_TIMEOUT)
        .await
        .map_err(|source| match source {
            ExecError::NotFound(_) => GitError::Missing,
            ExecError::MissingWorkingDir(dir) => GitError::NotADirectory(dir),
            other => GitError::Failed {
                command: args.join(" "),
                detail: other.to_string(),
            },
        })
}

/// Turn a non-zero exit into an error, keeping only git's first complaint.
fn require(output: &Output, tree: &Path) -> Result<(), GitError> {
    if output.success() {
        return Ok(());
    }
    Err(GitError::Unreadable {
        path: tree.to_path_buf(),
        detail: output.first_error_line().to_owned(),
    })
}

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("`git` was not found on PATH; install it or fix the daemon's PATH")]
    Missing,
    #[error("{0} is not a directory")]
    NotADirectory(PathBuf),
    #[error("{0} is not inside a git repository")]
    NotARepository(PathBuf),
    #[error("cannot read the repository at {path}: {detail}")]
    Unreadable { path: PathBuf, detail: String },
    #[error("`git {command}` failed: {detail}")]
    Failed { command: String, detail: String },
}
