//! Validation of a project's per-adapter settings.
//!
//! Only the shape is checked: which keys an adapter understands is the
//! adapter's own business. Keys the daemon does not recognise are stored and
//! handed back untouched rather than silently dropped.

use forge_core::{AdapterId, AdapterSettings};
use serde_json::Value;

use super::{SettingDef, SettingKind};

/// A settings blob a project cannot be saved with.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("`{0}` is not an adapter this daemon knows; expected one of: claude-code, codex")]
    UnknownAdapter(String),
    #[error("settings for `{adapter}` must be an object, not {found}")]
    NotAnObject { adapter: AdapterId, found: String },
    #[error("`{adapter}.{key}` must be a string, number or boolean, not {found}")]
    UnusableValue {
        adapter: AdapterId,
        key: String,
        found: String,
    },
    #[error("`{adapter}.{key}` must be {expected}, not {found}")]
    WrongKind {
        adapter: AdapterId,
        key: String,
        expected: &'static str,
        found: String,
    },
}

/// Check a blob that has already parsed into [`AdapterSettings`].
///
/// The outer shape is enforced by parsing; what is left is the values, which
/// become command-line flags and so have to be scalars.
pub fn validate(settings: &AdapterSettings) -> Result<(), SettingsError> {
    for adapter in AdapterId::ALL {
        let Some(values) = settings.get(adapter) else {
            continue;
        };
        let schema = super::adapter(adapter).settings_schema();

        for (key, value) in values {
            if !is_scalar(value) {
                return Err(SettingsError::UnusableValue {
                    adapter,
                    key: key.clone(),
                    found: describe(value),
                });
            }

            // A key the adapter documents has to be the kind it documents.
            // One it does not is kept as given, so a setting added ahead of
            // daemon support is not lost.
            if let Some(def) = schema.iter().find(|def| def.key == key) {
                if !matches_kind(def, value) {
                    return Err(SettingsError::WrongKind {
                        adapter,
                        key: key.clone(),
                        expected: expected(def.kind),
                        found: describe(value),
                    });
                }
            }
        }
    }
    Ok(())
}

fn matches_kind(def: &SettingDef, value: &Value) -> bool {
    match def.kind {
        SettingKind::Text => value.is_string(),
        SettingKind::Number => value.is_number(),
        SettingKind::Flag => value.is_boolean(),
    }
}

fn expected(kind: SettingKind) -> &'static str {
    match kind {
        SettingKind::Text => "a string",
        SettingKind::Number => "a number",
        SettingKind::Flag => "a boolean",
    }
}

/// Settings become argv entries, so anything that is not a scalar has no
/// meaning on a command line.
fn is_scalar(value: &Value) -> bool {
    matches!(value, Value::String(_) | Value::Number(_) | Value::Bool(_))
}

fn describe(value: &Value) -> String {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
    .to_owned()
}

/// Turn a raw JSON blob into settings, naming what is wrong when it is not.
pub fn parse(raw: Value) -> Result<AdapterSettings, SettingsError> {
    let Value::Object(adapters) = &raw else {
        return Err(SettingsError::UnknownAdapter(describe(&raw)));
    };

    for (name, value) in adapters {
        let adapter: AdapterId = name
            .parse()
            .map_err(|_| SettingsError::UnknownAdapter(name.clone()))?;

        if !value.is_object() {
            return Err(SettingsError::NotAnObject {
                adapter,
                found: describe(value),
            });
        }
    }

    let settings: AdapterSettings =
        serde_json::from_value(raw).expect("the shape was just checked");
    validate(&settings)?;
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_well_formed_blob_is_accepted() {
        let settings = parse(json!({
            "claude-code": { "model": "opus", "verbose": true, "max_turns": 40 },
            "codex": {}
        }))
        .unwrap();

        assert_eq!(
            settings.get(AdapterId::ClaudeCode).unwrap()["model"],
            "opus"
        );
        assert!(settings.get(AdapterId::Codex).unwrap().is_empty());
    }

    #[test]
    fn an_empty_blob_is_fine() {
        assert!(parse(json!({})).unwrap().get(AdapterId::Codex).is_none());
    }

    #[test]
    fn a_documented_setting_must_be_the_kind_it_is_documented_as() {
        let err = parse(json!({ "claude-code": { "model": 7 } })).unwrap_err();

        assert!(
            matches!(err, SettingsError::WrongKind { ref key, .. } if key == "model"),
            "{err}"
        );
        assert!(parse(json!({ "claude-code": { "model": "opus" } })).is_ok());
    }

    #[test]
    fn keys_the_daemon_does_not_recognise_are_kept_not_dropped() {
        let settings = parse(json!({ "claude-code": { "future-flag": "on" } })).unwrap();

        assert_eq!(
            settings.get(AdapterId::ClaudeCode).unwrap()["future-flag"],
            "on"
        );
    }

    #[test]
    fn an_unknown_adapter_is_named_in_the_error() {
        let err = parse(json!({ "aider": {} })).unwrap_err();

        assert!(matches!(err, SettingsError::UnknownAdapter(name) if name == "aider"));
    }

    #[test]
    fn an_adapters_settings_must_be_an_object() {
        let err = parse(json!({ "codex": "opus" })).unwrap_err();

        assert!(matches!(
            err,
            SettingsError::NotAnObject {
                adapter: AdapterId::Codex,
                ..
            }
        ));
    }

    #[test]
    fn the_blob_itself_must_be_an_object() {
        assert!(parse(json!([])).is_err());
        assert!(parse(json!("claude-code")).is_err());
    }

    #[test]
    fn values_that_cannot_become_flags_are_refused() {
        for value in [json!(["a"]), json!({ "nested": 1 }), json!(null)] {
            let err = parse(json!({ "claude-code": { "model": value } })).unwrap_err();
            assert!(
                matches!(err, SettingsError::UnusableValue { ref key, .. } if key == "model"),
                "{err}"
            );
        }
    }
}
