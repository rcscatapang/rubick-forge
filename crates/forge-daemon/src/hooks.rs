//! Event hooks: scripts the daemon runs when something happens.
//!
//! Fire and forget, by design (SPEC D24). A hook is told what happened and its
//! output is recorded; nothing it does changes what the daemon decides. That is
//! what keeps "run a script on this event" from becoming a plugin ABI.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use forge_core::{EventKind, EventRecord};
use serde::Deserialize;
use tokio::sync::Semaphore;

/// How long a hook may run before it is killed.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// How many hooks may run at once.
///
/// Small on purpose: hooks run on the same Mac as the agents, and the agents
/// are the point.
const DEFAULT_CONCURRENCY: usize = 4;

/// How many waiting hooks are held before new ones are dropped.
///
/// A storm — a fleet of agents all finishing at once — must cost log lines
/// rather than unbounded memory and a fork bomb.
const DEFAULT_QUEUE: usize = 64;

/// How much of a hook's output is kept.
const MAX_OUTPUT: usize = 2_000;

/// `hooks.toml`: which script runs for which event.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HooksConfig {
    pub timeout_secs: Option<u64>,
    pub concurrency: Option<usize>,
    pub queue: Option<usize>,
    /// Event kind to script name, e.g. `task_finished = "notify.sh"`.
    ///
    /// A name, not a path: scripts live in `hooks/` and nowhere else, so a
    /// config file cannot point the daemon at an arbitrary executable.
    pub on: BTreeMap<String, String>,
}

impl HooksConfig {
    fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS))
    }

    fn concurrency(&self) -> usize {
        self.concurrency.unwrap_or(DEFAULT_CONCURRENCY).max(1)
    }

    fn queue(&self) -> usize {
        self.queue.unwrap_or(DEFAULT_QUEUE).max(1)
    }

    /// The script for `kind`, if one is configured.
    ///
    /// An unknown event name in the config is ignored rather than fatal — the
    /// same file may be shared with a newer daemon that has more kinds.
    pub fn script_for(&self, kind: EventKind) -> Option<&str> {
        self.on.get(kind.as_str()).map(String::as_str)
    }

    /// Event names in the config that this daemon has no kind for.
    pub fn unknown_kinds(&self) -> Vec<&str> {
        self.on
            .keys()
            .filter(|name| name.parse::<EventKind>().is_err())
            .map(String::as_str)
            .collect()
    }
}

/// Whether `name` is a script name rather than a path to somewhere else.
///
/// Scripts live in the hooks directory. A name containing a separator, or `..`,
/// would let a config file run any executable on the machine.
pub fn is_script_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && name != "."
        && name != ".."
        && !name.starts_with('-')
}

/// Reads `hooks.toml` and runs what it names.
pub struct Hooks {
    config: HooksConfig,
    dir: PathBuf,
    /// Caps how many run at once, and — through `try_acquire` — how many wait.
    slots: Arc<Semaphore>,
    queue: usize,
}

impl Hooks {
    /// Load from a state directory. Missing or broken config means no hooks.
    pub fn load(state_dir: &crate::paths::StateDir) -> Self {
        let path = state_dir.hooks_config();

        let config = match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<HooksConfig>(&text) {
                Ok(config) => config,
                Err(error) => {
                    // Not fatal: a daemon that refused to run agents because a
                    // hook file had a typo would have the priorities backwards.
                    tracing::warn!(%error, path = %path.display(), "ignoring hooks.toml");
                    HooksConfig::default()
                }
            },
            Err(_) => HooksConfig::default(),
        };

        for name in config.unknown_kinds() {
            tracing::warn!(name, "hooks.toml names an event this daemon does not have");
        }

        Self::new(config, state_dir.hooks_dir())
    }

    pub fn new(config: HooksConfig, dir: PathBuf) -> Self {
        let permits = config.concurrency() + config.queue();

        Self {
            slots: Arc::new(Semaphore::new(permits)),
            queue: config.queue(),
            config,
            dir,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.config.on.is_empty()
    }

    /// Run whatever this event calls for, without waiting for it.
    ///
    /// Returns immediately: an event is published on the bus, and the bus must
    /// not be held up by somebody's shell script.
    pub fn fire(self: &Arc<Self>, record: &EventRecord, cwd: Option<PathBuf>) {
        let Some(script) = self.config.script_for(record.event.kind()) else {
            return;
        };

        if !is_script_name(script) {
            tracing::warn!(
                script,
                "a hook must be a script name in the hooks directory, not a path"
            );
            return;
        }

        // Acquired here rather than in the task, so a storm is refused now
        // instead of piling up tasks that all wait.
        let Ok(permit) = Arc::clone(&self.slots).try_acquire_owned() else {
            tracing::warn!(
                kind = %record.event.kind(),
                queue = self.queue,
                "dropping a hook: too many already waiting"
            );
            return;
        };

        let path = self.dir.join(script);
        let timeout = self.config.timeout();
        let payload = serde_json::to_string(record).unwrap_or_else(|_| "{}".to_owned());
        let kind = record.event.kind();

        tokio::spawn(async move {
            // Bound, not `let _ =`: that drops immediately and would release
            // the slot before the hook had run, so nothing would be bounded.
            let _permit = permit;
            let outcome = run(&path, &payload, cwd.as_deref(), timeout).await;

            match outcome {
                Ok(finished) => tracing::info!(
                    hook = %path.display(),
                    kind = %kind,
                    code = finished.code,
                    output = %finished.output,
                    "a hook ran"
                ),
                Err(error) => {
                    tracing::warn!(hook = %path.display(), kind = %kind, error, "a hook failed")
                }
            }
        });
    }
}

/// What a hook did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    /// `None` when it was killed.
    pub code: Option<i32>,
    /// Both streams, trimmed to something a log can hold.
    pub output: String,
}

/// Run one hook: event JSON on stdin, killed at the timeout.
pub async fn run(
    path: &Path,
    payload: &str,
    cwd: Option<&Path>,
    timeout: Duration,
) -> Result<Finished, String> {
    use tokio::io::AsyncWriteExt;

    let mut command = tokio::process::Command::new(path);
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    // The worktree when the event has one, so a hook can just run git.
    if let Some(cwd) = cwd.filter(|dir| dir.is_dir()) {
        command.current_dir(cwd);
    }

    // Told what happened rather than asked anything. A hook's exit code is
    // recorded and never acted on.
    command.env("FORGE_EVENT_KIND", "");

    let mut child = command
        .spawn()
        .map_err(|err| format!("cannot run it: {err}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.as_bytes()).await;
        // Dropped so the script sees end-of-input rather than hanging on read.
        drop(stdin);
    }

    // `kill_on_drop` means the timeout also reaps the process.
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => return Err(format!("it would not finish: {err}")),
        Err(_) => {
            return Err(format!(
                "it was still running after {}s and was killed",
                timeout.as_secs()
            ))
        }
    };

    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));

    Ok(Finished {
        code: output.status.code(),
        output: cap(text.trim()),
    })
}

fn cap(text: &str) -> String {
    if text.chars().count() <= MAX_OUTPUT {
        return text.to_owned();
    }

    let kept: String = text.chars().take(MAX_OUTPUT).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hook_is_a_script_name_and_never_a_path() {
        assert!(is_script_name("notify.sh"));
        assert!(is_script_name("do-the-thing"));

        // Each of these would run something outside the hooks directory.
        assert!(!is_script_name("../../usr/bin/curl"));
        assert!(!is_script_name("/bin/sh"));
        assert!(!is_script_name("sub/dir.sh"));
        assert!(!is_script_name(".."));
        assert!(!is_script_name(""));
        assert!(!is_script_name("-rf"));
    }

    #[test]
    fn an_event_name_this_daemon_does_not_have_is_reported_not_fatal() {
        let config: HooksConfig = toml::from_str(
            r#"
[on]
task_finished = "one.sh"
invented_kind = "two.sh"
"#,
        )
        .unwrap();

        assert_eq!(config.script_for(EventKind::TaskFinished), Some("one.sh"));
        assert_eq!(config.unknown_kinds(), ["invented_kind"]);
    }

    #[test]
    fn the_defaults_are_the_documented_ones() {
        let config = HooksConfig::default();

        assert_eq!(config.timeout(), Duration::from_secs(30));
        assert_eq!(config.concurrency(), 4);
        assert_eq!(config.queue(), 64);
    }

    #[test]
    fn a_concurrency_of_zero_would_run_nothing_so_it_is_one() {
        let config = HooksConfig {
            concurrency: Some(0),
            queue: Some(0),
            ..HooksConfig::default()
        };

        assert_eq!(config.concurrency(), 1);
        assert_eq!(config.queue(), 1);
    }

    #[test]
    fn output_is_capped_rather_than_filling_a_log() {
        let capped = cap(&"x".repeat(MAX_OUTPUT + 500));

        assert!(capped.chars().count() <= MAX_OUTPUT + 1);
        assert!(capped.ends_with('…'));
    }
}
