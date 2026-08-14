//! Agent adapters: how the daemon launches each CLI and reads what it is doing.

mod claude_code;
mod codex;
pub mod markers;
pub mod settings;
pub mod status;

use forge_core::{AdapterId, BinaryStatus, Task};
use futures_util::future::BoxFuture;
use serde_json::{Map, Value};

use markers::StatusPatterns;

/// What kind of value a setting takes. Only scalars, because settings become
/// command-line arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingKind {
    Text,
    Number,
    Flag,
}

/// One setting an adapter understands, for the UI to render and the daemon to
/// check a project's blob against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct SettingDef {
    pub key: &'static str,
    pub kind: SettingKind,
    pub description: &'static str,
}

/// Everything the daemon needs to know about one agent CLI.
pub trait AgentAdapter: Send + Sync {
    fn id(&self) -> AdapterId;

    /// The ordered markers that turn pane text into a status.
    fn status_patterns(&self) -> &'static StatusPatterns;

    /// The settings this adapter reads out of a project's blob.
    fn settings_schema(&self) -> &'static [SettingDef];

    /// Arguments derived from a project's settings for this adapter.
    fn launch_args(&self, settings: &Map<String, Value>) -> Vec<String>;

    /// The full argv for a task's session.
    ///
    /// The initial prompt goes last and as its own entry, so nothing in it can
    /// be read as an option or reach a shell.
    fn launch_command(&self, task: &Task, settings: &Map<String, Value>) -> Vec<String> {
        let mut command = vec![self.id().binary_name().to_owned()];
        command.extend(self.launch_args(settings));

        if let Some(prompt) = task.initial_prompt.as_deref() {
            if !prompt.trim().is_empty() {
                command.push(prompt.to_owned());
            }
        }

        command
    }

    /// The keys that answer a permission prompt yes, or no.
    ///
    /// tmux key names, sent as keys rather than pasted: these are answers to a
    /// dialog, not text for a prompt, and both CLIs read them as keystrokes.
    ///
    /// The default suits a numbered list with the safe option first, which is
    /// what both built-ins draw. An adapter whose dialog works differently
    /// overrides it.
    fn answer_keys(&self, approve: bool) -> &'static [&'static str] {
        if approve {
            &["1", "Enter"]
        } else {
            &["Escape"]
        }
    }

    /// Whether the CLI is on `PATH` and answers.
    ///
    /// Boxed so the trait stays usable behind `dyn`: the registry hands out
    /// one adapter chosen at runtime, which is the whole point of it.
    fn binary_check(&self) -> BoxFuture<'static, BinaryStatus> {
        let name = self.id().binary_name();
        Box::pin(crate::binaries::probe(name))
    }
}

/// `--model <value>` and free-form extra arguments, which both CLIs take.
///
/// A value of the wrong type is skipped rather than stringified: turning `7`
/// into `"7"` would hand the CLI something the user never wrote.
pub(crate) fn common_args(settings: &Map<String, Value>) -> Vec<String> {
    let mut args = Vec::new();

    if let Some(model) = settings.get("model").and_then(Value::as_str) {
        args.push("--model".to_owned());
        args.push(model.to_owned());
    }
    args
}

/// Whatever the user put in `extra_args`, split into argv entries.
pub(crate) fn extra_args(settings: &Map<String, Value>) -> Vec<String> {
    settings
        .get("extra_args")
        .and_then(Value::as_str)
        .map(|extra| extra.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// The adapter for `id`. The set is closed, so this cannot fail.
pub fn adapter(id: AdapterId) -> &'static dyn AgentAdapter {
    match id {
        AdapterId::ClaudeCode => &claude_code::ClaudeCode,
        AdapterId::Codex => &codex::Codex,
    }
}

/// Every adapter, in the order the UI lists them.
pub fn all() -> impl Iterator<Item = &'static dyn AgentAdapter> {
    AdapterId::ALL.into_iter().map(adapter)
}

/// Presence checks for every agent CLI, for `/health`.
pub async fn probe_all() -> Vec<BinaryStatus> {
    let mut statuses = Vec::new();
    for adapter in all() {
        statuses.push(adapter.binary_check().await);
    }
    statuses
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::{AgentStatus, Timestamp};

    fn task(prompt: Option<&str>) -> Task {
        Task {
            id: 1,
            project_id: 1,
            title: "Add adapters".into(),
            adapter: AdapterId::ClaudeCode,
            base_branch: "main".into(),
            branch: "forge/add-adapters-1".into(),
            worktree_path: None,
            initial_prompt: prompt.map(str::to_owned),
            status: AgentStatus::Stopped,
            created_at: Timestamp::now(),
            updated_at: Timestamp::now(),
        }
    }

    #[test]
    fn every_adapter_id_has_an_adapter_that_agrees_about_its_id() {
        for id in AdapterId::ALL {
            assert_eq!(adapter(id).id(), id);
        }
        assert_eq!(all().count(), AdapterId::ALL.len());
    }

    #[test]
    fn a_command_starts_with_the_cli_and_ends_with_the_prompt() {
        let command = adapter(AdapterId::ClaudeCode).launch_command(&task(Some("go")), &Map::new());

        assert_eq!(command, ["claude", "go"]);
    }

    #[test]
    fn a_task_without_a_prompt_just_starts_the_cli() {
        assert_eq!(
            adapter(AdapterId::Codex).launch_command(&task(None), &Map::new()),
            ["codex"]
        );
        assert_eq!(
            adapter(AdapterId::Codex).launch_command(&task(Some("   ")), &Map::new()),
            ["codex"]
        );
    }

    #[test]
    fn a_hostile_prompt_stays_one_argument() {
        let hostile = "$(rm -rf /); --dangerously-skip-permissions\nand more";
        let command =
            adapter(AdapterId::ClaudeCode).launch_command(&task(Some(hostile)), &Map::new());

        assert_eq!(command.len(), 2);
        assert_eq!(
            command[1], hostile,
            "the prompt is one argv entry, verbatim"
        );
    }

    #[test]
    fn settings_come_before_the_prompt() {
        let settings = serde_json::json!({ "model": "opus" })
            .as_object()
            .unwrap()
            .clone();

        let command = adapter(AdapterId::ClaudeCode).launch_command(&task(Some("go")), &settings);

        assert_eq!(command, ["claude", "--model", "opus", "go"]);
    }

    #[test]
    fn every_adapter_can_recognise_all_four_live_states() {
        for adapter in all() {
            let recognised: Vec<AgentStatus> = adapter.status_patterns().statuses().collect();

            for expected in [
                AgentStatus::Waiting,
                AgentStatus::Working,
                AgentStatus::Idle,
                AgentStatus::Error,
            ] {
                assert!(
                    recognised.contains(&expected),
                    "{} cannot recognise {expected}",
                    adapter.id()
                );
            }
        }
    }

    #[test]
    fn waiting_is_checked_before_anything_else() {
        // A CLI keeps its status line on screen while it asks a question, so
        // the question has to win.
        for adapter in all() {
            let first = adapter.status_patterns().statuses().next();
            assert_eq!(first, Some(AgentStatus::Waiting), "{}", adapter.id());
        }
    }
}
