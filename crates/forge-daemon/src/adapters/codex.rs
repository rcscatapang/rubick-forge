//! The Codex CLI.

use forge_core::{AdapterId, AgentStatus};

use super::markers::{MarkerSet, StatusPatterns};
use super::{AgentAdapter, SettingDef, SettingKind};

pub struct Codex;

/// Fragments of Codex's own interface, captured in `tests/fixtures/panes/`.
const PATTERNS: StatusPatterns = StatusPatterns {
    sets: &[
        MarkerSet {
            status: AgentStatus::Waiting,
            markers: &[
                "Do you trust",
                "Press enter to continue",
                "1. Yes",
                "› 1.",
                "Allow command",
                "(y/n)",
                "y to approve",
            ],
        },
        MarkerSet {
            status: AgentStatus::Error,
            markers: &[
                // Codex's own failures only. Nothing here may match the
                // compiler and test output an agent routinely prints, or a
                // healthy session would read as broken.
                "stream error",
                "We're currently experiencing high demand",
                "Not signed in",
                "run `codex login`",
            ],
        },
        MarkerSet {
            status: AgentStatus::Working,
            markers: &["Esc to interrupt", "esc to interrupt", "Working ("],
        },
        MarkerSet {
            status: AgentStatus::Idle,
            markers: &["Use /skills", "send   Ctrl", "› "],
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
        key: "extra_args",
        kind: SettingKind::Text,
        description: "Extra command-line arguments, split on spaces.",
    },
];

impl AgentAdapter for Codex {
    fn id(&self) -> AdapterId {
        AdapterId::Codex
    }

    fn status_patterns(&self) -> &'static StatusPatterns {
        &PATTERNS
    }

    fn settings_schema(&self) -> &'static [SettingDef] {
        SETTINGS
    }

    fn launch_args(&self, settings: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
        let mut args = super::common_args(settings);
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
        assert!(Codex.launch_args(&Map::new()).is_empty());
    }

    #[test]
    fn a_model_becomes_a_flag_and_a_value() {
        assert_eq!(
            Codex.launch_args(&settings(json!({ "model": "gpt-5.6" }))),
            ["--model", "gpt-5.6"]
        );
    }

    #[test]
    fn every_documented_setting_is_one_the_adapter_reads() {
        for def in Codex.settings_schema() {
            let args = Codex.launch_args(&settings(json!({ def.key: "value" })));
            assert!(!args.is_empty(), "{} is documented but ignored", def.key);
        }
    }
}
