//! Provisioning and tearing down a task's git worktree.
//!
//! Worktrees live outside the repository — a sibling shadow directory by
//! default — so a task's checkout never appears inside the repository the user
//! works in.

use std::path::{Path, PathBuf};

use forge_core::{task_branch_name, Project};

use crate::git::{self, GitError};
use crate::slug::{path_component, task_slug};
use crate::store::{Store, StoreError};

/// The directory name used beside a repository when nothing overrides it.
pub const SHADOW_DIR: &str = ".forge-worktrees";

/// Overrides the sibling default for every project.
pub const WORKTREE_ROOT_SETTING: &str = "worktree_root";

/// Where a task's worktree goes, and what branch it checks out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub path: PathBuf,
    pub branch: String,
}

/// Decide a task's worktree location without touching the filesystem.
///
/// Both path components are sanitised. A project name is user-supplied and
/// editable, so an unsanitised one — `../..`, or anything absolute, which
/// `join` would let swallow the root entirely — would put a directory that
/// cleanup later deletes somewhere nobody asked for.
pub fn placement(root: &Path, project: &Project, title: &str, task_id: i64) -> Placement {
    let slug = task_slug(title, task_id);
    let directory = path_component(&project.name, &format!("project-{}", project.id));

    Placement {
        path: root.join(directory).join(&slug),
        branch: task_branch_name(&slug),
    }
}

/// The configured worktree root, or the repository's sibling shadow directory.
///
/// A repository at the filesystem root has no parent; that falls back to a
/// shadow directory inside it, which is odd but reachable rather than a crash.
pub fn root_for(store: &Store, project: &Project) -> Result<PathBuf, StoreError> {
    if let Some(configured) = store.setting(WORKTREE_ROOT_SETTING)? {
        if !configured.trim().is_empty() {
            return Ok(PathBuf::from(configured));
        }
    }

    let repo = Path::new(&project.path);
    Ok(repo.parent().unwrap_or(repo).join(SHADOW_DIR))
}

/// Create the worktree, returning where it landed.
pub async fn create(project: &Project, placement: &Placement, base: &str) -> Result<(), GitError> {
    git::worktree_add(
        Path::new(&project.path),
        &placement.path,
        &placement.branch,
        base,
    )
    .await
}

/// What a cleanup should do with the task's branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchDisposal {
    Keep,
    Delete,
}

/// Remove a worktree, and optionally its branch.
///
/// `force` covers both refusals git can raise here: a worktree with local
/// changes, and a branch that was never merged. Cleaning up is meant to be a
/// single decision by the human, not two.
///
/// A directory the user deleted by hand is not an error. git still has a record
/// of it, so pruning is exactly the repair needed, and refusing would leave the
/// task with a worktree it can never be rid of.
pub async fn remove(
    project: &Project,
    path: &Path,
    branch: &str,
    disposal: BranchDisposal,
    force: bool,
) -> Result<(), GitError> {
    let repo = Path::new(&project.path);

    if path.exists() {
        git::worktree_remove(repo, path, force).await?;
    } else {
        git::worktree_prune(repo).await?;
    }

    if disposal == BranchDisposal::Delete {
        git::branch_delete(repo, branch, force).await?;
    }
    Ok(())
}

/// Whether the worktree still has uncommitted or untracked changes.
///
/// A directory that is no longer there has nothing to lose.
pub async fn is_dirty(path: &Path) -> Result<bool, GitError> {
    if !path.exists() {
        return Ok(false);
    }
    Ok(git::status(path).await?.dirty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::{AdapterSettings, Timestamp};

    fn project(path: &str) -> Project {
        Project {
            id: 1,
            name: "forge".into(),
            path: path.into(),
            default_branch: "main".into(),
            adapter_settings: AdapterSettings::default(),
            created_at: Timestamp::now(),
            updated_at: Timestamp::now(),
        }
    }

    #[test]
    fn the_default_root_sits_beside_the_repository() {
        let store = Store::open_in_memory().unwrap();

        assert_eq!(
            root_for(&store, &project("/Users/me/code/forge")).unwrap(),
            Path::new("/Users/me/code/.forge-worktrees")
        );
    }

    #[test]
    fn a_configured_root_wins() {
        let store = Store::open_in_memory().unwrap();
        store
            .set_setting(WORKTREE_ROOT_SETTING, "/Volumes/work/trees")
            .unwrap();

        assert_eq!(
            root_for(&store, &project("/Users/me/code/forge")).unwrap(),
            Path::new("/Volumes/work/trees")
        );
    }

    #[test]
    fn a_blank_setting_is_treated_as_unset() {
        let store = Store::open_in_memory().unwrap();
        store.set_setting(WORKTREE_ROOT_SETTING, "   ").unwrap();

        assert_eq!(
            root_for(&store, &project("/Users/me/code/forge")).unwrap(),
            Path::new("/Users/me/code/.forge-worktrees")
        );
    }

    #[test]
    fn a_repository_without_a_parent_still_gets_a_root() {
        let store = Store::open_in_memory().unwrap();

        assert_eq!(
            root_for(&store, &project("/")).unwrap(),
            Path::new("/.forge-worktrees")
        );
    }

    #[test]
    fn placement_nests_the_slug_under_the_project() {
        let placed = placement(
            Path::new("/trees"),
            &project("/repos/forge"),
            "Add adapters",
            7,
        );

        assert_eq!(placed.path, Path::new("/trees/forge/add-adapters-7"));
        assert_eq!(placed.branch, "forge/add-adapters-7");
    }

    #[test]
    fn two_tasks_with_one_title_get_different_places() {
        let root = Path::new("/trees");
        let project = project("/repos/forge");

        let first = placement(root, &project, "Fix it", 1);
        let second = placement(root, &project, "Fix it", 2);

        assert_ne!(first.path, second.path);
        assert_ne!(first.branch, second.branch);
    }

    #[test]
    fn a_hostile_title_cannot_climb_out_of_the_root() {
        let placed = placement(
            Path::new("/trees"),
            &project("/repos/forge"),
            "../../../etc",
            9,
        );

        assert_eq!(placed.path, Path::new("/trees/forge/etc-9"));
        assert!(placed.path.starts_with("/trees/forge"));
    }

    #[test]
    fn a_hostile_project_name_cannot_climb_out_of_the_root() {
        for name in ["../../..", "/etc", "..", "a/b"] {
            let mut project = project("/repos/forge");
            project.name = name.to_owned();

            let placed = placement(Path::new("/trees"), &project, "Fix it", 9);

            assert!(
                placed.path.starts_with("/trees"),
                "{name} escaped to {}",
                placed.path.display()
            );
            assert_eq!(
                placed.path.components().count(),
                4,
                "{}",
                placed.path.display()
            );
        }
    }

    #[test]
    fn a_project_name_with_nothing_usable_falls_back_to_its_id() {
        let mut project = project("/repos/forge");
        project.name = "///".into();

        let placed = placement(Path::new("/trees"), &project, "Fix it", 9);

        assert_eq!(placed.path, Path::new("/trees/project-1/fix-it-9"));
    }
}
