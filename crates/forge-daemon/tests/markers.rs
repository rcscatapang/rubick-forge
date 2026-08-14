//! Adapter markers against panes captured from the real CLIs.
//!
//! The fixtures in `tests/fixtures/panes/` are verbatim `capture-pane` output.
//! When a CLI's interface changes, these are what fail — and updating them is
//! the deliberate act of re-checking the markers against reality.

use forge_core::{AdapterId, AgentStatus};
use forge_daemon::adapters;

/// `<adapter>.<what-it-is>.txt`
fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/panes")
        .join(format!("{name}.txt"));

    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()))
}

/// Every fixture is read through the manifest engine, so these test the
/// shipped manifests rather than any Rust the built-ins used to have.
fn classify(id: AdapterId, name: &str) -> Option<AgentStatus> {
    adapters::adapter(&id)
        .expect("a built-in adapter")
        .status_patterns()
        .classify(&fixture(name))
}

#[test]
fn claude_codes_trust_prompt_is_waiting() {
    assert_eq!(
        classify(AdapterId::default(), "claude-code.waiting-trust"),
        Some(AgentStatus::Waiting)
    );
}

#[test]
fn claude_codes_empty_prompt_is_idle() {
    assert_eq!(
        classify(AdapterId::default(), "claude-code.idle"),
        Some(AgentStatus::Idle)
    );
}

#[test]
fn codexs_trust_prompt_is_waiting() {
    assert_eq!(
        classify("codex".parse::<AdapterId>().unwrap(), "codex.waiting-trust"),
        Some(AgentStatus::Waiting)
    );
}

#[test]
fn codexs_empty_prompt_is_idle() {
    assert_eq!(
        classify("codex".parse::<AdapterId>().unwrap(), "codex.idle"),
        Some(AgentStatus::Idle)
    );
}

#[test]
fn every_fixture_is_recognised_by_the_adapter_it_names() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/panes");
    let mut checked = 0;

    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let (adapter, state) = name.split_once('.').expect("named <adapter>.<state>");

        let id: AdapterId = adapter.parse().expect("a known adapter id");
        let expected: AgentStatus = state
            .split('-')
            .next()
            .unwrap()
            .parse()
            .expect("named after the status it shows");

        assert_eq!(
            classify(id, &name),
            Some(expected),
            "{name} should read as {expected}"
        );
        checked += 1;
    }

    assert!(checked >= 4, "only {checked} fixtures found");
}

#[test]
fn a_screen_from_one_cli_is_not_read_by_the_other() {
    // Markers this loose would make every status meaningless.
    assert_ne!(
        classify("codex".parse::<AdapterId>().unwrap(), "claude-code.idle"),
        Some(AgentStatus::Waiting),
        "Codex should not think Claude Code is asking it something"
    );
    assert_ne!(
        classify(AdapterId::default(), "codex.idle"),
        Some(AgentStatus::Waiting)
    );
}

#[test]
fn an_empty_pane_says_nothing() {
    for adapter in adapters::all() {
        assert_eq!(adapter.status_patterns().classify(""), None);
        assert_eq!(adapter.status_patterns().classify("\n\n\n"), None);
    }
}

#[test]
fn markers_do_not_match_the_output_an_agent_routinely_prints() {
    // Compiler and test output an agent produces while working. None of it
    // may read as an error, or a healthy session would look broken — and a
    // screen-read error is loud.
    let ordinary = "\
error[E0308]: mismatched types
  --> src/main.rs:4:9
error: could not compile `thing` (lib) due to 1 previous error
test result: FAILED. 1 passed; 2 failed
warning: unused variable: `x`
";

    for adapter in adapters::all() {
        let read = adapter.status_patterns().classify(ordinary);
        assert_ne!(
            read,
            Some(AgentStatus::Error),
            "{} reads ordinary build output as its own failure",
            adapter.id()
        );
    }
}

#[test]
fn the_daemon_exposes_a_schema_for_every_setting_it_reads() {
    for adapter in adapters::all() {
        let schema = adapter.settings_schema();

        assert!(!schema.is_empty(), "{} documents nothing", adapter.id());
        for def in schema {
            assert!(!def.description.is_empty(), "{} is undescribed", def.key);
        }
    }
}
