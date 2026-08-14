//! Agent adapters: how the daemon launches each CLI and reads what it is doing.
//!
//! There is one adapter engine and no special cases. The two Forge ships with
//! are TOML manifests compiled into the binary, loaded through exactly the code
//! path a file in the adapters directory takes — so a third CLI is a file, and
//! the built-ins prove the format is enough to describe a real one (SPEC D24).

pub mod manifest;
pub mod markers;
pub mod registry;
pub mod settings;
pub mod status;

use forge_core::{AdapterId, BinaryStatus, Task};
use serde_json::{Map, Value};

use manifest::{Manifest, SettingKind};
use markers::{MarkerSet, StatusPatterns};

pub use registry::{adapter, all, load_errors, reload, LoadError};

/// One setting an adapter understands, for the UI to render and the daemon to
/// check a project's blob against.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SettingDef {
    pub key: String,
    pub kind: SettingKind,
    pub description: String,
}

/// Everything the daemon needs to know about one agent CLI.
///
/// A struct rather than a trait: every adapter is now described by the same
/// data, so there is nothing left for an implementation to vary.
#[derive(Debug, Clone)]
pub struct Adapter {
    manifest: Manifest,
    patterns: StatusPatterns,
}

impl Adapter {
    pub fn new(manifest: Manifest) -> Self {
        let patterns = StatusPatterns {
            sets: manifest
                .status
                .iter()
                .map(|set| MarkerSet {
                    status: set.status,
                    markers: set.markers.clone(),
                })
                .collect(),
        };

        Self { manifest, patterns }
    }

    pub fn id(&self) -> &AdapterId {
        &self.manifest.id
    }

    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    pub fn binary_name(&self) -> &str {
        &self.manifest.binary
    }

    /// The ordered markers that turn pane text into a status.
    pub fn status_patterns(&self) -> &StatusPatterns {
        &self.patterns
    }

    /// The settings this adapter reads out of a project's blob.
    pub fn settings_schema(&self) -> Vec<SettingDef> {
        self.manifest
            .settings
            .iter()
            .map(|setting| SettingDef {
                key: setting.key.clone(),
                kind: setting.kind,
                description: setting.description.clone(),
            })
            .collect()
    }

    /// Whether a pane looks like it is asking permission.
    pub fn is_permission_prompt(&self, pane: &str) -> bool {
        let tail = markers::tail_of(pane, markers::TAIL_LINES);
        self.manifest
            .permission_markers()
            .any(|marker| tail.contains(marker))
    }

    /// The keys that answer a permission prompt yes, or no.
    ///
    /// tmux key names, sent as keys rather than pasted: these answer a dialog
    /// rather than being text for a prompt.
    pub fn answer_keys(&self, approve: bool) -> Vec<String> {
        if approve {
            self.manifest.answers.approve.clone()
        } else {
            self.manifest.answers.deny.clone()
        }
    }

    /// How an instruction reaches this agent.
    pub fn injection(&self) -> &manifest::Injection {
        &self.manifest.injection
    }

    /// Arguments derived from a project's settings.
    ///
    /// A value of the wrong type is skipped rather than stringified: turning
    /// `7` into `"7"` would hand the CLI something the user never wrote.
    pub fn launch_args(&self, settings: &Map<String, Value>, task: &Task) -> Vec<String> {
        let mut args = Vec::new();

        for setting in &self.manifest.settings {
            let Some(value) = settings.get(&setting.key) else {
                continue;
            };
            if setting.args.is_empty() && setting.kind != SettingKind::Args {
                continue;
            }

            // Split into argv elements rather than substituted, because that
            // is what "extra arguments" means. Still never a shell string.
            if setting.kind == SettingKind::Args {
                if let Value::String(extra) = value {
                    args.extend(extra.split_whitespace().map(str::to_owned));
                }
                continue;
            }

            let filled = match (setting.kind, value) {
                (SettingKind::Text, Value::String(text)) => text.clone(),
                (SettingKind::Number, Value::Number(number)) => number.to_string(),
                // A flag contributes its arguments or nothing; there is no
                // value to substitute.
                (SettingKind::Flag, Value::Bool(true)) => String::new(),
                _ => continue,
            };

            args.extend(
                setting
                    .args
                    .iter()
                    .map(|template| manifest::fill(template, &filled, task.id, &task.title)),
            );
        }

        args
    }

    /// The full argv for a task's session.
    ///
    /// Every element is built separately and nothing is ever joined into a
    /// shell string, so a title or a prompt containing `;` or `$(…)` is data.
    pub fn launch_command(&self, task: &Task, settings: &Map<String, Value>) -> Vec<String> {
        let mut command = vec![self.manifest.binary.clone()];

        command.extend(
            self.manifest
                .launch_args
                .iter()
                .map(|template| manifest::fill(template, "", task.id, &task.title)),
        );
        command.extend(self.launch_args(settings, task));

        // The prompt goes last and as its own argv element, so nothing in it
        // can be read as an option.
        if let Some(prompt) = task.initial_prompt.as_deref() {
            if !prompt.trim().is_empty() {
                command.push(prompt.to_owned());
            }
        }

        command
    }

    /// Whether the CLI is on `PATH` and answers.
    pub async fn binary_check(&self) -> BinaryStatus {
        crate::binaries::probe_with(&self.manifest.binary, &self.manifest.version_args).await
    }
}

/// Whether a pane looks like it is asking permission, whichever CLI drew it.
///
/// Adapter-agnostic because the caller may not know which one did: an event
/// arriving from another Mac carries the screen, not the adapter.
pub fn looks_like_permission_prompt(pane: &str) -> bool {
    all()
        .iter()
        .any(|adapter| adapter.is_permission_prompt(pane))
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
            adapter: AdapterId::default(),
            base_branch: "main".into(),
            branch: "forge/add-adapters-1".into(),
            worktree_path: None,
            initial_prompt: prompt.map(str::to_owned),
            status: AgentStatus::Stopped,
            created_at: Timestamp::now(),
            updated_at: Timestamp::now(),
        }
    }

    fn claude() -> std::sync::Arc<Adapter> {
        adapter(&forge_core::CLAUDE_CODE.parse().unwrap()).expect("a built-in")
    }

    fn settings(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    #[test]
    fn the_command_starts_with_the_binary_and_ends_with_the_prompt() {
        let command = claude().launch_command(&task(Some("fix the tests")), &Map::new());

        assert_eq!(command.first().unwrap(), "claude");
        assert_eq!(command.last().unwrap(), "fix the tests");
    }

    #[test]
    fn a_task_with_no_prompt_gets_no_empty_argument() {
        assert_eq!(
            claude().launch_command(&task(None), &Map::new()),
            ["claude"]
        );
        assert_eq!(
            claude().launch_command(&task(Some("  ")), &Map::new()),
            ["claude"]
        );
    }

    #[test]
    fn a_setting_becomes_the_arguments_its_manifest_declares() {
        let command = claude().launch_command(
            &task(None),
            &settings(&[("model", Value::String("opus".into()))]),
        );

        assert_eq!(command, ["claude", "--model", "opus"]);
    }

    #[test]
    fn a_setting_of_the_wrong_type_is_skipped_rather_than_stringified() {
        // Turning `7` into "7" would hand the CLI something nobody wrote.
        let command = claude().launch_command(
            &task(None),
            &settings(&[("model", Value::Number(7.into()))]),
        );

        assert_eq!(command, ["claude"]);
    }

    #[test]
    fn a_prompt_that_looks_like_an_option_is_still_one_argv_element() {
        let command = claude().launch_command(&task(Some("--help; rm -rf ~")), &Map::new());

        assert_eq!(command.last().unwrap(), "--help; rm -rf ~");
        assert_eq!(command.len(), 2, "one element, not several");
    }

    #[test]
    fn a_settings_value_cannot_become_a_second_command() {
        let hostile = "$(rm -rf ~)";
        let command = claude().launch_command(
            &task(None),
            &settings(&[("model", Value::String(hostile.into()))]),
        );

        assert_eq!(command, ["claude", "--model", hostile]);
    }

    #[test]
    fn both_built_ins_load_and_can_read_a_screen() {
        assert_eq!(all().len(), 2);

        for adapter in all() {
            assert!(!adapter.status_patterns().sets.is_empty());
            assert!(!adapter.binary_name().is_empty());
        }
    }

    #[test]
    fn a_permission_dialog_is_recognised_whichever_cli_drew_it() {
        assert!(looks_like_permission_prompt(
            "Do you want to edit src/main.rs?"
        ));
        assert!(looks_like_permission_prompt("Allow this command? (y/n)"));
    }

    #[test]
    fn a_screen_that_is_merely_idle_is_not_a_question_to_answer() {
        assert!(!looks_like_permission_prompt("? for shortcuts"));
        assert!(!looks_like_permission_prompt("esc to interrupt"));
        assert!(!looks_like_permission_prompt(""));
    }

    #[test]
    fn answering_uses_the_keys_the_manifest_declares() {
        assert_eq!(claude().answer_keys(true), ["1", "Enter"]);
        assert_eq!(claude().answer_keys(false), ["Escape"]);
    }
}
