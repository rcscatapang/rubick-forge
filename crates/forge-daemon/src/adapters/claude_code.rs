//! The Claude Code CLI.

use forge_core::{AdapterId, AgentStatus};

use super::markers::{MarkerSet, StatusPatterns};
use super::{AgentAdapter, SettingDef, SettingKind};

pub struct ClaudeCode;

/// Fragments of Claude Code's own interface, newest-checked-first.
///
/// These are literal strings from the running CLI, captured in
/// `tests/fixtures/panes/`. When the interface changes they stop matching, and
/// the honest failure is a status that stays where it was — never a wrong one.
const PATTERNS: StatusPatterns = StatusPatterns {
    sets: &[
        MarkerSet {
            status: AgentStatus::Waiting,
            markers: &[
                // Permission and confirmation dialogs.
                "Do you want to",
                "Enter to confirm",
                "esc to cancel",
                "Esc to cancel",
                "1. Yes",
                "❯ 1.",
                "(y/n)",
                "Allow this",
                "Would you like",
            ],
        },
        MarkerSet {
            status: AgentStatus::Error,
            markers: &[
                "Execution error",
                "API Error",
                "Credit balance is too low",
                "Invalid API key",
                "Please run /login",
            ],
        },
        MarkerSet {
            status: AgentStatus::Working,
            markers: &[
                // The working footer shows an elapsed timer and a token count.
                "esc to interrupt",
                "tokens · esc",
                "· esc to",
            ],
        },
        MarkerSet {
            status: AgentStatus::Idle,
            markers: &[
                "auto mode on",
                "shift+tab to cycle",
                "Try \"how does",
                "? for shortcuts",
            ],
        },
    ],
};

const SETTINGS: &[SettingDef] = &[
    SettingDef {
        key: "model",
        kind: SettingKind::Text,
        description: "Model to run, passed as --model.",
    },
    SettingDef {
        key: "permission_mode",
        kind: SettingKind::Text,
        description: "Passed as --permission-mode, e.g. acceptEdits or plan.",
    },
    SettingDef {
        key: "extra_args",
        kind: SettingKind::Text,
        description: "Extra command-line arguments, split on spaces.",
    },
];

impl AgentAdapter for ClaudeCode {
    fn id(&self) -> AdapterId {
        AdapterId::ClaudeCode
    }

    fn status_patterns(&self) -> &'static StatusPatterns {
        &PATTERNS
    }

    fn settings_schema(&self) -> &'static [SettingDef] {
        SETTINGS
    }

    fn launch_args(&self, settings: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
        let mut args = super::common_args(settings);

        if let Some(mode) = settings.get("permission_mode").and_then(|v| v.as_str()) {
            args.push("--permission-mode".to_owned());
            args.push(mode.to_owned());
        }
        args.extend(super::extra_args(settings));

        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map};

    fn settings(value: serde_json::Value) -> Map<String, serde_json::Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn no_settings_means_no_arguments() {
        assert!(ClaudeCode.launch_args(&Map::new()).is_empty());
    }

    #[test]
    fn a_model_becomes_a_flag_and_a_value() {
        let args = ClaudeCode.launch_args(&settings(json!({ "model": "opus" })));

        assert_eq!(args, ["--model", "opus"]);
    }

    #[test]
    fn extra_arguments_are_split_into_separate_argv_entries() {
        let args = ClaudeCode.launch_args(&settings(json!({ "extra_args": "--verbose  --debug" })));

        assert_eq!(args, ["--verbose", "--debug"]);
    }

    #[test]
    fn settings_are_applied_in_a_stable_order() {
        let args = ClaudeCode.launch_args(&settings(json!({
            "extra_args": "--verbose",
            "model": "opus",
            "permission_mode": "plan",
        })));

        assert_eq!(
            args,
            ["--model", "opus", "--permission-mode", "plan", "--verbose"]
        );
    }

    #[test]
    fn a_setting_of_the_wrong_type_is_skipped_rather_than_stringified() {
        let args = ClaudeCode.launch_args(&settings(json!({ "model": 7 })));

        assert!(args.is_empty());
    }

    #[test]
    fn every_documented_setting_is_one_the_adapter_reads() {
        for def in ClaudeCode.settings_schema() {
            let args = ClaudeCode.launch_args(&settings(json!({ def.key: "value" })));
            assert!(!args.is_empty(), "{} is documented but ignored", def.key);
        }
    }
}
