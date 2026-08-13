use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{AdapterId, AgentStatus, Timestamp};

/// Per-adapter configuration held on a project.
///
/// The daemon does not interpret the inner objects; each adapter validates its
/// own slice against `settings_schema`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AdapterSettings(BTreeMap<AdapterId, Map<String, Value>>);

impl AdapterSettings {
    pub fn get(&self, adapter: AdapterId) -> Option<&Map<String, Value>> {
        self.0.get(&adapter)
    }

    pub fn set(&mut self, adapter: AdapterId, settings: Map<String, Value>) {
        self.0.insert(adapter, settings);
    }
}

/// A registered git repository — the anchor every task hangs off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: i64,
    pub name: String,
    /// Absolute path to the repository root.
    pub path: String,
    pub default_branch: String,
    pub adapter_settings: AdapterSettings,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// A unit of work: a prompt, a branch, and the sessions that have run it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: i64,
    pub project_id: i64,
    pub title: String,
    pub adapter: AdapterId,
    pub base_branch: String,
    /// `forge/<slug>` when the task has a worktree, else the base branch.
    pub branch: String,
    /// `None` means "run in the repo root".
    pub worktree_path: Option<String>,
    pub initial_prompt: Option<String>,
    /// Mirrors the live session, and sticks at its last value once ended.
    pub status: AgentStatus,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Task {
    /// Where an agent for this task runs: its worktree, or the repo root.
    pub fn working_dir<'a>(&'a self, project: &'a Project) -> &'a str {
        self.worktree_path.as_deref().unwrap_or(&project.path)
    }
}

/// One run of a task inside tmux. A task accumulates sessions across restarts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: i64,
    pub task_id: i64,
    /// `forge-<task-id>`, unique across live sessions.
    pub tmux_name: String,
    /// The pane's process id, when tmux reports one.
    pub pid: Option<i64>,
    pub status: AgentStatus,
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
}

impl Session {
    pub fn is_live(&self) -> bool {
        self.ended_at.is_none()
    }
}

/// The tmux session name the daemon uses for a task.
pub fn tmux_session_name(task_id: i64) -> String {
    format!("forge-{task_id}")
}

/// The branch a worktree-backed task checks out.
pub fn task_branch_name(slug: &str) -> String {
    format!("forge/{slug}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Project {
        Project {
            id: 1,
            name: "forge".into(),
            path: "/repos/forge".into(),
            default_branch: "main".into(),
            adapter_settings: AdapterSettings::default(),
            created_at: Timestamp::now(),
            updated_at: Timestamp::now(),
        }
    }

    fn task() -> Task {
        Task {
            id: 2,
            project_id: 1,
            title: "Add adapters".into(),
            adapter: AdapterId::ClaudeCode,
            base_branch: "main".into(),
            branch: "forge/add-adapters".into(),
            worktree_path: Some("/repos/.forge-worktrees/forge/add-adapters".into()),
            initial_prompt: None,
            status: AgentStatus::Stopped,
            created_at: Timestamp::now(),
            updated_at: Timestamp::now(),
        }
    }

    #[test]
    fn adapter_settings_are_keyed_by_the_adapter_wire_name() {
        let mut settings = AdapterSettings::default();
        let mut claude = Map::new();
        claude.insert("model".into(), Value::String("opus".into()));
        settings.set(AdapterId::ClaudeCode, claude);

        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(json, r#"{"claude-code":{"model":"opus"}}"#);

        let parsed: AdapterSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.get(AdapterId::ClaudeCode).unwrap()["model"], "opus");
        assert!(parsed.get(AdapterId::Codex).is_none());
    }

    #[test]
    fn empty_settings_are_an_empty_object() {
        assert_eq!(
            serde_json::to_string(&AdapterSettings::default()).unwrap(),
            "{}"
        );
    }

    #[test]
    fn a_worktree_task_runs_in_its_worktree() {
        let project = project();
        let task = task();
        assert_eq!(
            task.working_dir(&project),
            "/repos/.forge-worktrees/forge/add-adapters"
        );
    }

    #[test]
    fn a_rootless_task_runs_in_the_repo_root() {
        let project = project();
        let mut task = task();
        task.worktree_path = None;
        assert_eq!(task.working_dir(&project), "/repos/forge");
    }

    #[test]
    fn names_follow_the_spec_conventions() {
        assert_eq!(tmux_session_name(12), "forge-12");
        assert_eq!(task_branch_name("add-adapters"), "forge/add-adapters");
    }

    #[test]
    fn a_session_is_live_until_it_ends() {
        let mut session = Session {
            id: 3,
            task_id: 2,
            tmux_name: tmux_session_name(2),
            pid: Some(4242),
            status: AgentStatus::Working,
            started_at: Timestamp::now(),
            ended_at: None,
        };
        assert!(session.is_live());
        session.ended_at = Some(Timestamp::now());
        assert!(!session.is_live());
    }
}
