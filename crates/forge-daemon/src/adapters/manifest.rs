//! What a TOML adapter manifest says, and what makes one valid.
//!
//! This is the whole extension surface (SPEC D24). There is no ABI, no dynamic
//! loading and no code in a manifest — a new agent CLI is a description of how
//! to launch it and what its screen looks like.
//!
//! Parsing and validation are pure and live here; loading files is the
//! registry's job.

use forge_core::{AdapterId, AgentStatus};
use serde::{Deserialize, Serialize};

/// The format this daemon understands.
///
/// Versioned from the start so a third-party file written against a later
/// format is refused with a sentence rather than half-understood.
pub const MANIFEST_VERSION: u32 = 1;

/// How much text a marker may be.
///
/// Markers are literal fragments of somebody's interface, not patterns. A long
/// one is a mistake — usually a whole pasted line, which will stop matching the
/// moment the CLI rewraps it.
const MAX_MARKER: usize = 200;

/// One agent CLI, entirely described.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub manifest_version: u32,
    pub id: AdapterId,
    /// Human-facing name for the UI.
    pub name: String,
    /// The executable expected on `PATH`.
    pub binary: String,
    /// Arguments that make the binary print its version, for the health check.
    #[serde(default = "version_args")]
    pub version_args: Vec<String>,

    /// Extra arguments at launch, before the prompt.
    ///
    /// Each entry is one argv element and may contain `{placeholders}`. Never
    /// a shell string: a template is substituted into argv, so nothing in a
    /// value can become a second command.
    #[serde(default)]
    pub launch_args: Vec<String>,

    /// How an instruction is typed into a running agent.
    #[serde(default)]
    pub injection: Injection,

    /// The keys that answer a permission dialog yes and no.
    #[serde(default = "default_answers")]
    pub answers: Answers,

    /// Ordered status marker sets. Order is the design; see `markers.rs`.
    #[serde(default)]
    pub status: Vec<MarkerSet>,

    /// The settings this adapter reads out of a project's blob.
    #[serde(default)]
    pub settings: Vec<Setting>,
}

fn version_args() -> Vec<String> {
    vec!["--version".to_owned()]
}

/// How text is typed into a running agent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Injection {
    /// Paste through a tmux buffer rather than sending keys.
    ///
    /// The default, and what any TUI needs: pasted text cannot be read as key
    /// bindings however many newlines or control characters are in it.
    #[serde(default = "yes")]
    pub paste: bool,
    /// Keys sent after the text, to submit it.
    #[serde(default = "enter")]
    pub submit_keys: Vec<String>,
}

impl Default for Injection {
    fn default() -> Self {
        Self {
            paste: true,
            submit_keys: enter(),
        }
    }
}

fn yes() -> bool {
    true
}

fn enter() -> Vec<String> {
    vec!["Enter".to_owned()]
}

/// The keys that answer a dialog.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Answers {
    pub approve: Vec<String>,
    pub deny: Vec<String>,
}

fn default_answers() -> Answers {
    Answers {
        // Suits a numbered list with the safe option first, which is what both
        // built-ins draw.
        approve: vec!["1".to_owned(), "Enter".to_owned()],
        deny: vec!["Escape".to_owned()],
    }
}

/// The literal fragments that identify one state.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MarkerSet {
    pub status: AgentStatus,
    pub markers: Vec<String>,
    /// The subset meaning "this screen is asking permission", so buttons are
    /// only offered where answering yes or no makes sense.
    #[serde(default)]
    pub permission_markers: Vec<String>,
}

/// One setting an adapter reads, and how it becomes an argument.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    pub key: String,
    pub kind: SettingKind,
    pub description: String,
    /// argv template for this setting, with `{value}` where it goes.
    ///
    /// Omitted for a setting the adapter reads some other way. A `flag` uses
    /// its arguments only when the value is true.
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingKind {
    Text,
    Number,
    Flag,
    /// A string split on whitespace into one argv element each.
    ///
    /// For a CLI's "and anything else you want to pass" setting. Declared like
    /// any other, so no key is special to the engine.
    Args,
}

/// Why a manifest was refused.
///
/// One per file and never fatal: a broken manifest costs its own adapter, and
/// `/health` says which file and why.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    #[error("this is not valid TOML: {0}")]
    Parse(String),
    #[error(
        "manifest_version is {found}, but this daemon understands {MANIFEST_VERSION}. \
         Upgrade Forge, or use a manifest written for it."
    )]
    Version { found: u32 },
    #[error("{field} cannot be empty")]
    Empty { field: &'static str },
    #[error(
        "the binary `{0}` is not a bare command name. Forge resolves it on PATH \
         and never through a shell."
    )]
    BinaryPath(String),
    #[error("`{0}` has no status markers, so nothing it does could ever be read")]
    NoMarkers(String),
    #[error("a {status} marker is longer than {MAX_MARKER} characters: `{marker}`")]
    MarkerTooLong { status: AgentStatus, marker: String },
    #[error("a {0} marker is empty, which would match every screen")]
    EmptyMarker(AgentStatus),
    #[error("`{placeholder}` in `{template}` is not a placeholder this daemon fills")]
    UnknownPlaceholder {
        template: String,
        placeholder: String,
    },
    #[error("the setting `{0}` is declared twice")]
    DuplicateSetting(String),
    #[error(
        "`{key}` in {field} is not a usable tmux key name. tmux splits its own \
         arguments on `;`, so a key list is checked rather than escaped."
    )]
    UnusableKey { field: &'static str, key: String },
}

/// The placeholders a template may use.
///
/// A closed set on purpose. An unknown one is a typo that would otherwise reach
/// the CLI as a literal `{moddel}` and be blamed on the CLI.
pub const PLACEHOLDERS: [&str; 4] = ["value", "prompt", "task_id", "task_title"];

/// Parse and validate one manifest.
pub fn parse(text: &str) -> Result<Manifest, ManifestError> {
    let manifest: Manifest =
        toml::from_str(text).map_err(|err| ManifestError::Parse(err.message().to_owned()))?;

    manifest.validate()?;
    Ok(manifest)
}

impl Manifest {
    fn validate(&self) -> Result<(), ManifestError> {
        if self.manifest_version != MANIFEST_VERSION {
            return Err(ManifestError::Version {
                found: self.manifest_version,
            });
        }
        if self.name.trim().is_empty() {
            return Err(ManifestError::Empty { field: "name" });
        }
        if self.binary.trim().is_empty() {
            return Err(ManifestError::Empty { field: "binary" });
        }
        // A path here would let a manifest name any executable on the machine.
        if self.binary.contains('/') || self.binary.contains("..") {
            return Err(ManifestError::BinaryPath(self.binary.clone()));
        }
        if self.status.is_empty() {
            return Err(ManifestError::NoMarkers(self.id.to_string()));
        }

        for set in &self.status {
            for marker in set.markers.iter().chain(&set.permission_markers) {
                if marker.trim().is_empty() {
                    return Err(ManifestError::EmptyMarker(set.status));
                }
                if marker.chars().count() > MAX_MARKER {
                    return Err(ManifestError::MarkerTooLong {
                        status: set.status,
                        marker: marker.clone(),
                    });
                }
            }
        }

        let mut seen = Vec::new();
        for setting in &self.settings {
            if seen.contains(&setting.key) {
                return Err(ManifestError::DuplicateSetting(setting.key.clone()));
            }
            seen.push(setting.key.clone());

            for template in &setting.args {
                check_placeholders(template)?;
            }
        }

        for template in &self.launch_args {
            check_placeholders(template)?;
        }

        // Key lists reach `tmux send-keys`, and tmux splits its *own* argument
        // list on a bare `;` before `--` can protect anything after it. A
        // manifest saying `approve = [";", "kill-server"]` would otherwise be a
        // command injection into the tmux server.
        //
        // `version_args` are not checked: they are argv to a process, which is
        // safe however they are spelled, and `--version` starts with a dash.
        check_keys("answers.approve", &self.answers.approve)?;
        check_keys("answers.deny", &self.answers.deny)?;
        check_keys("injection.submit_keys", &self.injection.submit_keys)?;

        Ok(())
    }

    /// The permission markers across every set, for the "is this a dialog"
    /// check that decides whether to offer approve and deny.
    pub fn permission_markers(&self) -> impl Iterator<Item = &str> {
        self.status
            .iter()
            .flat_map(|set| set.permission_markers.iter())
            .map(String::as_str)
    }
}

/// Whether every entry is a plain word tmux and a CLI will take as one token.
fn check_keys(field: &'static str, keys: &[String]) -> Result<(), ManifestError> {
    for key in keys {
        let usable = !key.is_empty()
            && !key.starts_with('-')
            && key.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '/' | '.' | '=')
            });

        if !usable {
            return Err(ManifestError::UnusableKey {
                field,
                key: key.clone(),
            });
        }
    }

    Ok(())
}

/// Every `{name}` in `template` must be one this daemon fills.
fn check_placeholders(template: &str) -> Result<(), ManifestError> {
    let mut rest = template;

    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            // A lone `{` is a literal brace, which some CLIs do take. The rest
            // of the template is still checked: stopping here let
            // `"{ {moddel}"` through.
            rest = after;
            continue;
        };

        let name = &after[..close];
        if !PLACEHOLDERS.contains(&name) {
            return Err(ManifestError::UnknownPlaceholder {
                template: template.to_owned(),
                placeholder: name.to_owned(),
            });
        }

        rest = &after[close + 1..];
    }

    Ok(())
}

/// Fill `{placeholders}` in one argv element.
///
/// The result is a single argv element whatever is in `value`. There is no
/// shell anywhere on this path, so a value containing `;`, `$(…)` or a newline
/// is data and stays data.
pub fn fill(template: &str, value: &str, task_id: i64, task_title: &str) -> String {
    let task_id = task_id.to_string();
    let mut out = String::with_capacity(template.len());
    let mut rest = template;

    // One pass, not four `replace`s: a *value* containing `{task_title}` must
    // stay those characters rather than being substituted in turn.
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];

        let Some(close) = after.find('}') else {
            out.push('{');
            rest = after;
            continue;
        };

        out.push_str(match &after[..close] {
            "value" | "prompt" => value,
            "task_id" => &task_id,
            "task_title" => task_title,
            // Validation refuses these, so this is a manifest loaded by an
            // older daemon; leaving it alone is better than dropping it.
            other => {
                out.push('{');
                out.push_str(other);
                out.push('}');
                rest = &after[close + 1..];
                continue;
            }
        });

        rest = &after[close + 1..];
    }

    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
manifest_version = 1
id = "aider"
name = "Aider"
binary = "aider"

[[status]]
status = "waiting"
markers = ["Do you want to"]
"#;

    #[test]
    fn a_minimal_manifest_is_enough_to_describe_an_agent() {
        let manifest = parse(MINIMAL).unwrap();

        assert_eq!(manifest.id.as_str(), "aider");
        assert_eq!(manifest.binary, "aider");
        // Everything else has a default that suits an ordinary TUI.
        assert_eq!(manifest.version_args, ["--version"]);
        assert!(manifest.injection.paste);
        assert_eq!(manifest.injection.submit_keys, ["Enter"]);
        assert_eq!(manifest.answers.approve, ["1", "Enter"]);
    }

    #[test]
    fn a_manifest_from_a_later_format_is_refused_rather_than_half_read() {
        let text = MINIMAL.replace("manifest_version = 1", "manifest_version = 2");

        assert_eq!(parse(&text), Err(ManifestError::Version { found: 2 }));
    }

    /// `MINIMAL` with extra top-level keys, which in TOML must come *before*
    /// the array-of-tables or they belong to it.
    fn with_top_level(extra: &str) -> String {
        let (head, status) = MINIMAL.split_once("[[status]]").expect("a status table");
        format!("{head}{extra}\n[[status]]{status}")
    }

    #[test]
    fn a_key_this_daemon_does_not_know_is_a_typo_worth_reporting() {
        let text = with_top_level(r#"launch_agrs = ["--x"]"#);

        assert!(matches!(parse(&text), Err(ManifestError::Parse(_))));
    }

    #[test]
    fn an_adapter_with_no_markers_could_never_be_read() {
        let text = MINIMAL.split("[[status]]").next().unwrap().to_owned();

        assert!(matches!(parse(&text), Err(ManifestError::NoMarkers(_))));
    }

    #[test]
    fn an_empty_marker_would_match_every_screen() {
        let text = MINIMAL.replace(r#"markers = ["Do you want to"]"#, r#"markers = ["  "]"#);

        assert_eq!(
            parse(&text),
            Err(ManifestError::EmptyMarker(AgentStatus::Waiting))
        );
    }

    #[test]
    fn a_marker_the_length_of_a_pasted_line_is_refused() {
        // It would stop matching the moment the CLI rewrapped it.
        let long = "x".repeat(MAX_MARKER + 1);
        let text = MINIMAL.replace("Do you want to", &long);

        assert!(matches!(
            parse(&text),
            Err(ManifestError::MarkerTooLong { .. })
        ));
    }

    #[test]
    fn a_binary_cannot_be_a_path_to_anything_on_the_machine() {
        for binary in ["/usr/bin/curl", "../../bin/sh", "./run"] {
            let text = MINIMAL.replace(r#"binary = "aider""#, &format!(r#"binary = "{binary}""#));

            assert!(
                matches!(parse(&text), Err(ManifestError::BinaryPath(_))),
                "{binary} should be refused"
            );
        }
    }

    #[test]
    fn an_id_that_could_be_a_path_is_refused_by_the_id_itself() {
        let text = MINIMAL.replace(r#"id = "aider""#, r#"id = "../evil""#);

        assert!(matches!(parse(&text), Err(ManifestError::Parse(_))));
    }

    #[test]
    fn a_placeholder_this_daemon_does_not_fill_is_a_typo() {
        let text = with_top_level(r#"launch_args = ["--model", "{moddel}"]"#);

        let Err(ManifestError::UnknownPlaceholder { placeholder, .. }) = parse(&text) else {
            panic!("a misspelt placeholder should be caught");
        };
        assert_eq!(placeholder, "moddel");
    }

    #[test]
    fn every_placeholder_the_docs_promise_is_accepted() {
        for name in PLACEHOLDERS {
            let text = with_top_level(&format!(r#"launch_args = ["--x", "{{{name}}}"]"#));
            assert!(parse(&text).is_ok(), "{name} should be accepted");
        }
    }

    #[test]
    fn a_setting_declared_twice_is_a_mistake() {
        let text = format!(
            "{MINIMAL}
[[settings]]
key = \"model\"
kind = \"text\"
description = \"one\"

[[settings]]
key = \"model\"
kind = \"text\"
description = \"again\"
"
        );

        assert_eq!(
            parse(&text),
            Err(ManifestError::DuplicateSetting("model".to_owned()))
        );
    }

    #[test]
    fn filling_a_template_produces_one_argv_element_whatever_is_in_it() {
        // No shell is involved anywhere on this path.
        let hostile = "$(rm -rf ~); echo pwned";

        assert_eq!(fill("{value}", hostile, 1, "t"), hostile);
        assert_eq!(
            fill("--model={value}", hostile, 1, "t"),
            format!("--model={hostile}")
        );
    }

    #[test]
    fn a_newline_in_a_value_stays_inside_one_argv_element() {
        let hostile = "one\nrm -rf ~\n";

        assert_eq!(fill("{value}", hostile, 1, "t"), hostile);
    }

    #[test]
    fn a_value_that_looks_like_a_template_is_not_expanded_again() {
        // Data must not become template: a model name containing
        // `{task_title}` is those characters.
        assert_eq!(
            fill("--model={value}", "{task_title}", 1, "Fix it"),
            "--model={task_title}"
        );
    }

    #[test]
    fn a_key_list_cannot_smuggle_a_tmux_command() {
        // tmux splits its own argument list on a bare `;`, so these are checked
        // rather than escaped.
        for bad in [r#"approve = [";", "kill-server"]"#, r#"deny = ["-X"]"#] {
            let (head, status) = MINIMAL.split_once("[[status]]").expect("a status table");
            let approve = if bad.starts_with("approve") {
                bad
            } else {
                r#"approve = ["1"]"#
            };
            let deny = if bad.starts_with("deny") {
                bad
            } else {
                r#"deny = ["Escape"]"#
            };
            let text = format!("{head}[answers]\n{approve}\n{deny}\n\n[[status]]{status}");

            assert!(
                matches!(parse(&text), Err(ManifestError::UnusableKey { .. })),
                "{bad} should be refused"
            );
        }
    }

    #[test]
    fn an_ordinary_key_list_is_accepted() {
        let (head, status) = MINIMAL.split_once("[[status]]").expect("a status table");
        let text = format!(
            "{head}[answers]\napprove = [\"1\", \"Enter\"]\ndeny = [\"Escape\", \"C-c\"]\n\n[[status]]{status}"
        );

        assert!(parse(&text).is_ok(), "{:?}", parse(&text));
    }

    #[test]
    fn an_unclosed_brace_does_not_hide_a_typo_after_it() {
        let text = with_top_level(r#"launch_args = ["{ {moddel}"]"#);

        assert!(matches!(
            parse(&text),
            Err(ManifestError::UnknownPlaceholder { .. })
        ));
    }

    #[test]
    fn filling_knows_the_task_it_is_for() {
        assert_eq!(fill("{task_id}", "", 12, "Fix it"), "12");
        assert_eq!(fill("{task_title}", "", 12, "Fix it"), "Fix it");
    }

    #[test]
    fn permission_markers_are_gathered_from_every_set() {
        let text = format!(
            "{MINIMAL}
[[status]]
status = \"error\"
markers = [\"API Error\"]
permission_markers = [\"Allow this\"]
"
        );
        let manifest = parse(&text).unwrap();

        assert_eq!(
            manifest.permission_markers().collect::<Vec<_>>(),
            ["Allow this"]
        );
    }
}
