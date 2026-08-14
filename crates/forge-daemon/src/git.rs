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

/// Create a worktree at `path`, on a new `branch` forked from `base`.
///
/// Fails rather than adopts when the branch or the directory already exists,
/// which is what makes the caller's rollback meaningful.
pub async fn worktree_add(
    repo: &Path,
    path: &Path,
    branch: &str,
    base: &str,
) -> Result<(), GitError> {
    let output = run(
        repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            branch,
            "--end-of-options",
            &path.display().to_string(),
            base,
        ],
    )
    .await?;

    refusable(&output)
}

/// Remove a worktree. Without `force`, git refuses one with local changes.
pub async fn worktree_remove(repo: &Path, path: &Path, force: bool) -> Result<(), GitError> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push("--end-of-options");
    let path = path.display().to_string();
    args.push(&path);

    let output = run(repo, &args).await?;
    refusable(&output)
}

/// Drop a branch. Without `force`, git refuses one that is not merged.
pub async fn branch_delete(repo: &Path, branch: &str, force: bool) -> Result<(), GitError> {
    let flag = if force { "-D" } else { "-d" };
    let output = run(repo, &["branch", flag, "--end-of-options", branch]).await?;
    refusable(&output)
}

/// Forget worktrees whose directories have been deleted behind git's back.
pub async fn worktree_prune(repo: &Path) -> Result<(), GitError> {
    let output = run(repo, &["worktree", "prune"]).await?;
    require(&output, repo)
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

/// Worktree and branch commands fail for reasons the *user* caused and can
/// fix — the branch exists, the directory exists, the tree has changes, the
/// branch is unmerged — so git's own sentence is the most useful thing to pass
/// on, unlike the read paths where it is noise.
fn refusable(output: &Output) -> Result<(), GitError> {
    if output.success() {
        return Ok(());
    }
    Err(GitError::Refused {
        detail: output
            .first_error_line()
            .trim_start_matches("fatal: ")
            .to_owned(),
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

/// What committing everything in a worktree would commit.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct DiffStat {
    pub files: u32,
    pub insertions: u32,
    pub deletions: u32,
    /// The paths, so a dialog can show what is about to be committed.
    pub paths: Vec<String>,
}

impl DiffStat {
    pub fn is_empty(&self) -> bool {
        self.files == 0
    }
}

/// What is uncommitted in a worktree, staged or not.
///
/// `--porcelain` is the stable format; the human one is not promised to stay
/// the same between git versions.
pub async fn diff_stat(tree: &Path) -> Result<DiffStat, GitError> {
    let status = run(tree, &["status", "--porcelain=v1", "--untracked-files=all"]).await?;
    if !status.success() {
        return Err(failed("status", &status));
    }

    let paths: Vec<String> = status
        .stdout
        .lines()
        .filter_map(|line| line.get(3..).map(str::trim).filter(|path| !path.is_empty()))
        .map(str::to_owned)
        .collect();

    if paths.is_empty() {
        return Ok(DiffStat::default());
    }

    // `--intent-to-add` puts untracked files into the diff as additions without
    // staging their contents. Without it a worktree of brand-new files reports
    // "5 files, +0 −0" — precisely the number the caller asked for.
    let marked = run(tree, &["add", "--intent-to-add", "--all"]).await?;
    if !marked.success() {
        return Err(failed("add --intent-to-add", &marked));
    }

    let numstat = run(tree, &["diff", "HEAD", "--numstat"]).await?;
    let (mut insertions, mut deletions) = (0, 0);

    if numstat.success() {
        for line in numstat.stdout.lines() {
            let mut columns = line.split_whitespace();
            insertions += columns
                .next()
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(0);
            deletions += columns
                .next()
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(0);
        }
    }

    Ok(DiffStat {
        files: paths.len() as u32,
        insertions,
        deletions,
        paths,
    })
}

/// Stage everything and commit it.
///
/// Never amends and never rebases: this only ever adds a commit. A worktree
/// with nothing in it is a refusal, because an empty commit is not what anyone
/// pressing "commit" meant.
pub async fn commit_all(tree: &Path, message: &str) -> Result<String, GitError> {
    if message.trim().is_empty() {
        return Err(GitError::Failed {
            command: "commit".to_owned(),
            detail: "a commit needs a message".to_owned(),
        });
    }

    let staged = run(tree, &["add", "--all"]).await?;
    if !staged.success() {
        return Err(failed("add", &staged));
    }

    // `--message` takes the next argv entry whole, so a message beginning with
    // a dash is a message rather than an option.
    let committed = run(tree, &["commit", "--message", message]).await?;
    if !committed.success() {
        return Err(failed("commit", &committed));
    }

    let head = run(tree, &["rev-parse", "HEAD"]).await?;
    Ok(head.stdout.trim().to_owned())
}

fn failed(command: &str, output: &Output) -> GitError {
    GitError::Failed {
        command: command.to_owned(),
        detail: output.first_error_line().to_owned(),
    }
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
    #[error("git refused: {detail}")]
    Refused { detail: String },
    #[error("`git {command}` failed: {detail}")]
    Failed { command: String, detail: String },
}
