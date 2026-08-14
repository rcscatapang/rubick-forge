//! One interface over this Mac's daemon and the others it speaks for.
//!
//! Only the daemon hosting the Telegram bot has a fleet (SPEC D21, the "lite
//! hub"). A remote machine is reached over exactly the API the desktop app
//! uses — there is still no daemon-to-daemon protocol, and no machine knows it
//! is in anyone's fleet.

use std::time::Duration;

use forge_core::{AdapterId, EventRecord, Project, Session, Task};
use futures_util::future::BoxFuture;

use crate::config::MachineEntry;
use crate::http::AppState;
use crate::telegram::commands::Candidate;

/// Long enough for a cold daemon over a tailnet, short enough that one asleep
/// Mac does not hold up a reply about the others.
const REMOTE_TIMEOUT: Duration = Duration::from_secs(10);

/// A task with everything a reply needs in order to name it.
#[derive(Debug, Clone)]
pub struct FleetTask {
    pub machine: String,
    pub task: Task,
    pub project: Option<String>,
    pub session: Option<Session>,
}

/// How long to wait before reopening a remote machine's event stream.
const RETRY_DELAYS: [u64; 5] = [1, 2, 5, 15, 30];

/// What one machine can be asked, whether it is this one or another.
///
/// Boxed futures rather than `async fn`: a fleet is a list holding a mix of
/// local and remote machines, which is the whole point of it.
pub trait Machine: Send + Sync {
    fn name(&self) -> &str;

    fn projects(&self) -> BoxFuture<'_, Result<Vec<Project>, FleetError>>;

    /// Every task, with its project name and live session filled in.
    fn tasks(&self) -> BoxFuture<'_, Result<Vec<FleetTask>, FleetError>>;

    /// Create a task and launch its agent.
    fn start(&self, new: NewRemoteTask) -> BoxFuture<'_, Result<Task, FleetError>>;

    fn stop(&self, task_id: i64) -> BoxFuture<'_, Result<(), FleetError>>;

    fn instruct(&self, task_id: i64, text: String) -> BoxFuture<'_, Result<(), FleetError>>;

    /// Answer a dialog, as keystrokes rather than typed text.
    fn answer(&self, task_id: i64, approve: bool) -> BoxFuture<'_, Result<(), FleetError>>;

    /// The last of what is on screen, for a reply that shows what happened.
    fn pane_tail(&self, task_id: i64) -> BoxFuture<'_, Result<String, FleetError>>;

    /// Follow this machine's events, calling `on_event` for each.
    ///
    /// Never returns: it reconnects for as long as the daemon runs. The local
    /// machine reads its own bus; a remote one opens a WebSocket.
    fn watch(&self, on_event: EventSink) -> BoxFuture<'_, ()>;
}

/// What to create on a machine.
///
/// One struct rather than five arguments: they travel together everywhere, and
/// the last two are easy to transpose when they are both `Option<String>`.
#[derive(Debug, Clone)]
pub struct NewRemoteTask {
    /// The project's id *on that machine*, which is not its id anywhere else.
    pub project_id: i64,
    pub title: String,
    pub prompt: String,
    pub adapter: AdapterId,
    /// Names this attempt so a retry cannot create a second task.
    pub idempotency_key: Option<String>,
}

/// What to do with each event a machine reports.
///
/// Boxed rather than generic so `Machine` stays object-safe, which is the
/// whole point of the trait.
pub type EventSink = Box<dyn Fn(EventRecord) -> BoxFuture<'static, ()> + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum FleetError {
    #[error("{machine} is not answering")]
    Unreachable { machine: String },
    #[error("{0}")]
    Refused(String),
}

impl FleetError {
    /// Which machine could not be asked, for a reply that degrades per machine.
    pub fn machine(&self) -> Option<&str> {
        match self {
            FleetError::Unreachable { machine } => Some(machine),
            FleetError::Refused(_) => None,
        }
    }
}

/// Every machine the bot speaks for, this one first.
pub struct Fleet {
    machines: Vec<Box<dyn Machine>>,
}

impl Fleet {
    /// Build the fleet. `tokens` lines up with `entries`.
    ///
    /// A machine whose token could not be read is still listed, with an empty
    /// one: it then reports whatever its daemon says about an unauthenticated
    /// request, which is a clearer answer than the machine vanishing.
    pub fn new(state: AppState, entries: &[MachineEntry], tokens: &[String]) -> Self {
        let mut machines: Vec<Box<dyn Machine>> = vec![Box::new(LocalMachine::new(state))];

        for (entry, token) in entries.iter().zip(tokens) {
            machines.push(Box::new(RemoteMachine::new(entry, token)));
        }

        Self { machines }
    }

    /// A machine by the position a callback payload recorded. Positions are
    /// the config's order, which is stable for the daemon's lifetime.
    pub fn at(&self, index: usize) -> Option<&dyn Machine> {
        self.machines.get(index).map(AsRef::as_ref)
    }

    /// How many machines this daemon speaks for, this one included.
    pub fn len(&self) -> usize {
        self.machines.len()
    }

    /// Never true: the fleet always contains this Mac.
    pub fn is_empty(&self) -> bool {
        self.machines.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.machines.iter().map(|machine| machine.name())
    }

    /// Every machine, with its position, so a caller can watch each in turn.
    pub fn each(&self) -> impl Iterator<Item = (usize, &dyn Machine)> {
        self.machines
            .iter()
            .enumerate()
            .map(|(index, machine)| (index, machine.as_ref()))
    }

    /// Every machine's tasks, and the machines that could not be asked.
    ///
    /// One Mac being asleep costs that Mac's part of a reply and nothing else,
    /// so failures come back alongside the answers rather than instead of them.
    pub async fn tasks(&self) -> (Vec<FleetTask>, Vec<FleetError>) {
        let mut found = Vec::new();
        let mut failed = Vec::new();

        for machine in &self.machines {
            match machine.tasks().await {
                Ok(tasks) => found.extend(tasks),
                Err(error) => failed.push(error),
            }
        }

        (found, failed)
    }

    /// Everything a `/stop` or `/ask` reference could have meant, with the
    /// index of the machine each is on.
    pub async fn task_candidates(&self) -> (Vec<(usize, Candidate, FleetTask)>, Vec<FleetError>) {
        let mut candidates = Vec::new();
        let mut failed = Vec::new();

        for (index, machine) in self.machines.iter().enumerate() {
            match machine.tasks().await {
                Ok(tasks) => candidates.extend(tasks.into_iter().map(|found| {
                    let candidate = Candidate {
                        id: found.task.id,
                        name: found.task.title.clone(),
                        machine: found.machine.clone(),
                    };
                    (index, candidate, found)
                })),
                Err(error) => failed.push(error),
            }
        }

        (candidates, failed)
    }

    /// Everything a `/start` reference could have meant.
    pub async fn project_candidates(&self) -> (Vec<(usize, Candidate)>, Vec<FleetError>) {
        let mut candidates = Vec::new();
        let mut failed = Vec::new();

        for (index, machine) in self.machines.iter().enumerate() {
            match machine.projects().await {
                Ok(projects) => candidates.extend(projects.into_iter().map(|project| {
                    (
                        index,
                        Candidate {
                            id: project.id,
                            name: project.name,
                            machine: machine.name().to_owned(),
                        },
                    )
                })),
                Err(error) => failed.push(error),
            }
        }

        (candidates, failed)
    }
}

/// This Mac, reached through the daemon's own internals rather than its API.
struct LocalMachine {
    name: String,
    state: AppState,
}

impl LocalMachine {
    fn new(state: AppState) -> Self {
        Self {
            name: state.machine.to_string(),
            state,
        }
    }
}

impl Machine for LocalMachine {
    fn name(&self) -> &str {
        &self.name
    }

    fn projects(&self) -> BoxFuture<'_, Result<Vec<Project>, FleetError>> {
        Box::pin(async move { self.state.store.projects().map_err(refused) })
    }

    fn tasks(&self) -> BoxFuture<'_, Result<Vec<FleetTask>, FleetError>> {
        Box::pin(async move {
            let projects = self.state.store.projects().map_err(refused)?;
            let tasks = self
                .state
                .store
                .tasks(Default::default())
                .map_err(refused)?;

            tasks
                .into_iter()
                .map(|task| {
                    Ok(FleetTask {
                        machine: self.name.clone(),
                        project: projects
                            .iter()
                            .find(|project| project.id == task.project_id)
                            .map(|project| project.name.clone()),
                        session: self.state.store.live_session(task.id).map_err(refused)?,
                        task,
                    })
                })
                .collect()
        })
    }

    fn start(&self, new: NewRemoteTask) -> BoxFuture<'_, Result<Task, FleetError>> {
        Box::pin(async move {
            crate::http::tasks::create_and_start(
                &self.state,
                new.project_id,
                new.title,
                new.prompt,
                new.adapter,
                new.idempotency_key,
            )
            .await
            .map_err(|err| FleetError::Refused(err.message().to_owned()))
        })
    }

    fn stop(&self, task_id: i64) -> BoxFuture<'_, Result<(), FleetError>> {
        Box::pin(async move {
            self.state
                .sessions
                .stop(task_id)
                .await
                .map(|_| ())
                .map_err(refused)
        })
    }

    fn instruct(&self, task_id: i64, text: String) -> BoxFuture<'_, Result<(), FleetError>> {
        Box::pin(async move {
            self.state
                .sessions
                .send_instruction(task_id, &text)
                .await
                .map_err(refused)
        })
    }

    fn answer(&self, task_id: i64, approve: bool) -> BoxFuture<'_, Result<(), FleetError>> {
        Box::pin(async move {
            let task = self
                .state
                .store
                .task(task_id)
                .map_err(refused)?
                .ok_or_else(|| FleetError::Refused(format!("there is no task {task_id}")))?;

            let keys = crate::adapters::adapter(task.adapter).answer_keys(approve);

            self.state
                .sessions
                .send_answer(task_id, keys)
                .await
                .map_err(refused)
        })
    }

    fn pane_tail(&self, task_id: i64) -> BoxFuture<'_, Result<String, FleetError>> {
        Box::pin(async move {
            self.state
                .sessions
                .pane_tail(task_id)
                .await
                .map_err(refused)
        })
    }

    fn watch(&self, on_event: EventSink) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let mut events = self.state.bus.subscribe();

            // A lagging receiver misses events rather than blocking the bus,
            // which is the bus's documented behaviour and not this loop's
            // problem to solve.
            while let Ok(record) = events.recv().await {
                on_event(record).await;
            }
        })
    }
}

fn refused(err: impl std::fmt::Display) -> FleetError {
    FleetError::Refused(err.to_string())
}

/// Another Mac, over the same HTTP API the desktop app uses.
struct RemoteMachine {
    name: String,
    base: String,
    token: String,
    http: reqwest::Client,
}

impl RemoteMachine {
    fn new(entry: &MachineEntry, token: &str) -> Self {
        Self {
            name: entry.name.clone(),
            base: entry.url.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
            http: reqwest::Client::builder()
                .timeout(REMOTE_TIMEOUT)
                .build()
                .unwrap_or_default(),
        }
    }

    async fn get<T: for<'de> serde::Deserialize<'de>>(&self, path: &str) -> Result<T, FleetError> {
        self.send(self.http.get(format!("{}/{path}", self.base)))
            .await
    }

    async fn post<T: for<'de> serde::Deserialize<'de>>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<T, FleetError> {
        self.send(self.http.post(format!("{}/{path}", self.base)).json(&body))
            .await
    }

    async fn send<T: for<'de> serde::Deserialize<'de>>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, FleetError> {
        let response =
            request
                .bearer_auth(&self.token)
                .send()
                .await
                .map_err(|_| FleetError::Unreachable {
                    machine: self.name.clone(),
                })?;

        if !response.status().is_success() {
            let status = response.status();
            // The daemon's error envelope reads as a sentence; use it when it
            // is there and the status code when it is not.
            let described = response
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|body| body["error"]["message"].as_str().map(str::to_owned));

            return Err(FleetError::Refused(
                described.unwrap_or_else(|| format!("{} answered {status}", self.name)),
            ));
        }

        response.json().await.map_err(|_| FleetError::Unreachable {
            machine: self.name.clone(),
        })
    }
}

/// The daemon's list bodies are `{"projects": […]}` and friends.
#[derive(serde::Deserialize)]
struct Projects {
    projects: Vec<Project>,
}

#[derive(serde::Deserialize)]
struct Tasks {
    tasks: Vec<Task>,
}

#[derive(serde::Deserialize)]
struct Sessions {
    sessions: Vec<Session>,
}

#[derive(serde::Deserialize)]
struct Pane {
    tail: String,
}

impl Machine for RemoteMachine {
    fn name(&self) -> &str {
        &self.name
    }

    fn projects(&self) -> BoxFuture<'_, Result<Vec<Project>, FleetError>> {
        Box::pin(async move { Ok(self.get::<Projects>("projects").await?.projects) })
    }

    fn tasks(&self) -> BoxFuture<'_, Result<Vec<FleetTask>, FleetError>> {
        Box::pin(async move {
            let projects = self.get::<Projects>("projects").await?.projects;
            let tasks = self.get::<Tasks>("tasks").await?.tasks;

            let mut found = Vec::with_capacity(tasks.len());
            for task in tasks {
                // Only a live task has a session worth asking about, and each
                // ask is another round trip over the tailnet.
                let session = if task.status.is_live() {
                    self.get::<Sessions>(&format!("tasks/{}/sessions", task.id))
                        .await
                        .ok()
                        .and_then(|body| body.sessions.into_iter().find(|s| s.ended_at.is_none()))
                } else {
                    None
                };

                found.push(FleetTask {
                    machine: self.name.clone(),
                    project: projects
                        .iter()
                        .find(|project| project.id == task.project_id)
                        .map(|project| project.name.clone()),
                    session,
                    task,
                });
            }

            Ok(found)
        })
    }

    fn start(&self, new: NewRemoteTask) -> BoxFuture<'_, Result<Task, FleetError>> {
        Box::pin(async move {
            // A daemon that already made this task returns it rather than a
            // second one, which is what makes a retry after a lost answer safe.
            let task: Task = self
                .post(
                    "tasks",
                    serde_json::json!({
                        "idempotency_key": new.idempotency_key,
                        "project_id": new.project_id,
                        "title": new.title,
                        "adapter": new.adapter,
                        "initial_prompt": new.prompt,
                    }),
                )
                .await?;

            // Starting one that is already running is not a failure worth
            // reporting: the caller asked for a running agent and has one.
            match self
                .post::<Session>(&format!("tasks/{}/start", task.id), serde_json::json!({}))
                .await
            {
                Ok(_) => Ok(task),
                Err(FleetError::Refused(detail)) if detail.contains("already running") => Ok(task),
                Err(error) => Err(error),
            }
        })
    }

    fn stop(&self, task_id: i64) -> BoxFuture<'_, Result<(), FleetError>> {
        Box::pin(async move {
            let _: Session = self
                .post(&format!("tasks/{task_id}/stop"), serde_json::json!({}))
                .await?;
            Ok(())
        })
    }

    fn instruct(&self, task_id: i64, text: String) -> BoxFuture<'_, Result<(), FleetError>> {
        Box::pin(async move {
            self.accepted(
                &format!("tasks/{task_id}/instruction"),
                serde_json::json!({ "text": text }),
            )
            .await
        })
    }

    fn answer(&self, task_id: i64, approve: bool) -> BoxFuture<'_, Result<(), FleetError>> {
        Box::pin(async move {
            self.accepted(
                &format!("tasks/{task_id}/answer"),
                serde_json::json!({ "approve": approve }),
            )
            .await
        })
    }

    fn pane_tail(&self, task_id: i64) -> BoxFuture<'_, Result<String, FleetError>> {
        Box::pin(async move {
            Ok(self
                .get::<Pane>(&format!("tasks/{task_id}/pane"))
                .await?
                .tail)
        })
    }

    fn watch(&self, on_event: EventSink) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let mut attempt = 0usize;
            let mut after: Option<i64> = None;

            loop {
                if self.stream_once(&mut after, &on_event).await.is_ok() {
                    attempt = 0;
                } else {
                    tracing::debug!(machine = self.name, "a remote event stream ended");
                }

                let wait = RETRY_DELAYS[attempt.min(RETRY_DELAYS.len() - 1)];
                attempt = attempt.saturating_add(1);
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
        })
    }
}

impl RemoteMachine {
    /// A POST whose success is `202 Accepted` with no body.
    async fn accepted(&self, path: &str, body: serde_json::Value) -> Result<(), FleetError> {
        let response = self
            .http
            .post(format!("{}/{path}", self.base))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(|_| FleetError::Unreachable {
                machine: self.name.clone(),
            })?;

        if response.status().is_success() {
            return Ok(());
        }

        let status = response.status();
        let described = response
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|body| body["error"]["message"].as_str().map(str::to_owned));

        Err(FleetError::Refused(described.unwrap_or_else(|| {
            format!("{} answered {status}", self.name)
        })))
    }
}

impl RemoteMachine {
    /// One connection's worth of this machine's events.
    ///
    /// `after` is carried across reconnects, so a machine that drops off the
    /// tailnet and comes back replays what was missed instead of losing it.
    async fn stream_once(&self, after: &mut Option<i64>, on_event: &EventSink) -> Result<(), ()> {
        use futures_util::StreamExt;

        // Browsers cannot set headers on a handshake, so the daemon accepts the
        // token in the query string on `/ws` routes; this client does the same.
        let mut url = format!(
            "{}/ws/events?token={}",
            websocket_base(&self.base),
            self.token
        );
        if let Some(seen) = after {
            url.push_str(&format!("&after={seen}"));
        }

        // The URL has a bearer token in it, and tungstenite puts the URL it
        // failed on into its errors. Nothing derived from it reaches a log.
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .map_err(|_| ())?;

        while let Some(frame) = socket.next().await {
            let Ok(frame) = frame else { return Err(()) };
            let Ok(text) = frame.into_text() else {
                continue;
            };
            let Ok(record) = serde_json::from_str::<EventRecord>(&text) else {
                continue;
            };

            *after = Some(record.id);
            on_event(record).await;
        }

        Ok(())
    }
}

/// An `http(s)` base as its WebSocket equivalent.
///
/// Only the scheme is rewritten: a blanket replace turns a host that happens to
/// contain "http" into nonsense.
fn websocket_base(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');

    match trimmed.split_once("://") {
        Some(("http", rest)) => format!("ws://{rest}"),
        Some(("https", rest)) => format!("wss://{rest}"),
        _ => trimmed.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_scheme_becomes_a_websocket_one() {
        assert_eq!(
            websocket_base("http://100.64.0.1:8787"),
            "ws://100.64.0.1:8787"
        );
        assert_eq!(websocket_base("https://mini:8787/"), "wss://mini:8787");
        // A blanket replace would have made this "ws://ws-mini:8787".
        assert_eq!(
            websocket_base("http://http-mini:8787"),
            "ws://http-mini:8787"
        );
    }

    #[test]
    fn an_unreachable_machine_says_which_one_so_a_reply_can_degrade_per_machine() {
        let error = FleetError::Unreachable {
            machine: "Mac mini".into(),
        };

        assert_eq!(error.machine(), Some("Mac mini"));
        assert!(error.to_string().contains("Mac mini"));
    }

    #[test]
    fn a_refusal_belongs_to_no_machine_in_particular() {
        assert_eq!(FleetError::Refused("nope".into()).machine(), None);
    }
}
