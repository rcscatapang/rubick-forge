//! The activity feed: `GET /events` for history, `WS /ws/events` for live.
//!
//! Both take the same `after` cursor, so a client that drops its socket
//! reconnects with the last id it saw and misses nothing.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use axum::Json;
use forge_core::EventRecord;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use super::error::ApiResult;
use super::extract::Query;
use super::AppState;
use crate::store::EventPage;

#[derive(Debug, Default, Deserialize)]
pub struct Cursor {
    /// Return events with an id greater than this.
    pub after: Option<i64>,
    pub limit: Option<usize>,
    /// Restrict the feed to one task.
    pub task: Option<i64>,
    /// Return the end of history rather than its beginning.
    #[serde(default)]
    pub newest: bool,
    /// Read by the auth middleware, and accepted here so it is not a
    /// "query string is not valid" rejection.
    #[serde(default, rename = "token")]
    _token: Option<String>,
}

impl Cursor {
    fn to_query(&self) -> crate::store::Query {
        crate::store::Query {
            after: self.after,
            limit: self.limit,
            task_id: self.task,
            newest: self.newest,
        }
    }
}

pub async fn list(
    State(state): State<AppState>,
    Query(cursor): Query<Cursor>,
) -> ApiResult<Json<EventPage>> {
    Ok(Json(state.store.events(cursor.to_query())?))
}

pub async fn stream(
    State(state): State<AppState>,
    Query(cursor): Query<Cursor>,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade.on_upgrade(move |socket| relay(socket, state, cursor))
}

/// Subscribe first, then backfill: an event published while the backfill runs
/// arrives on the subscription and is skipped by id, so there is no gap and no
/// duplicate.
async fn relay(socket: WebSocket, state: AppState, cursor: Cursor) {
    let mut live = state.bus.subscribe();
    let (mut sink, mut stream) = socket.split();
    let mut last_sent = cursor.after;

    if cursor.after.is_some() {
        let mut query = cursor.to_query();
        // Backfill pages at the store's own maximum, not the client's `limit`,
        // which only bounds a single REST page — and always forwards, whatever
        // end the client asked a REST page to come from.
        query.limit = None;
        query.newest = false;

        loop {
            let page = match state.store.events(query) {
                Ok(page) => page,
                Err(error) => {
                    tracing::error!(%error, "cannot backfill the event stream");
                    return;
                }
            };
            if page.events.is_empty() {
                break;
            }
            for record in &page.events {
                if send(&mut sink, record).await.is_err() {
                    return;
                }
            }
            last_sent = page.next_after;
            query.after = page.next_after;
        }
    }

    loop {
        tokio::select! {
            // The only thing a client sends is a close frame; reading is how we
            // notice it went away.
            incoming = stream.next() => match incoming {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => continue,
            },
            received = live.recv() => match received {
                Ok(record) => {
                    if last_sent.is_some_and(|seen| record.id <= seen) {
                        continue;
                    }
                    if cursor.task.is_some_and(|task| record.event.task_id() != Some(task)) {
                        continue;
                    }
                    last_sent = Some(record.id);
                    if send(&mut sink, &record).await.is_err() {
                        return;
                    }
                }
                // The client fell far enough behind that events were dropped.
                // Closing is honest: it reconnects with its cursor and
                // backfills the gap from history.
                Err(RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "event subscriber lagged; closing so it can backfill");
                    let _ = sink.send(Message::Close(None)).await;
                    return;
                }
                Err(RecvError::Closed) => return,
            },
        }
    }
}

async fn send(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    record: &EventRecord,
) -> Result<(), ()> {
    let text = serde_json::to_string(record).map_err(|error| {
        tracing::error!(%error, "cannot serialise an event");
    })?;
    sink.send(Message::text(text)).await.map_err(|_| ())
}
