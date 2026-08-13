//! `WS /ws/sessions/:id/terminal` — a live terminal for one viewer.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use forge_core::Session;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;

use super::error::{ApiError, ApiResult};
use super::extract::Query;
use super::AppState;
use crate::terminal::{Attachment, DEFAULT_COLS, DEFAULT_ROWS};

#[derive(Debug, Deserialize)]
pub struct Connect {
    /// The client's terminal size, so the first redraw is already right.
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    /// Watch without being able to type.
    #[serde(default)]
    pub read_only: bool,
    /// Read by the auth middleware; accepted here so it is not a rejection.
    #[serde(default, rename = "token")]
    _token: Option<String>,
}

/// How often the daemon proves the socket is still there.
///
/// A viewer whose machine sleeps or loses Wi-Fi leaves a half-open TCP
/// connection: reads never return and writes succeed into a buffer. A periodic
/// ping is what eventually fails, and failing is what reaps the pty.
const KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(20);

/// What a client may send that is not keystrokes.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Control {
    Resize {
        cols: u16,
        rows: u16,
    },
    /// Stop or resume accepting this viewer's keystrokes, without dropping
    /// the attach — retaking the terminal would lose what is on screen.
    ReadOnly {
        value: bool,
    },
}

pub async fn stream(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(connect): Query<Connect>,
    upgrade: WebSocketUpgrade,
) -> ApiResult<Response> {
    // Checked before the upgrade, so a client learns why rather than seeing a
    // socket open and close.
    let session = state
        .store
        .session(id)?
        .ok_or_else(|| ApiError::not_found(format!("there is no session with id {id}")))?;

    if !session.is_live() {
        return Err(ApiError::conflict(
            "that session has ended; start the task again to get a new one",
        ));
    }

    Ok(upgrade.on_upgrade(move |socket| attach(socket, state, session, connect)))
}

async fn attach(socket: WebSocket, state: AppState, session: Session, connect: Connect) {
    let size = (
        connect.cols.unwrap_or(DEFAULT_COLS),
        connect.rows.unwrap_or(DEFAULT_ROWS),
    );

    let runtime = state.sessions.runtime();
    let mut attachment =
        match Attachment::open(runtime.binary(), runtime.socket(), &session.tmux_name, size) {
            Ok(attachment) => attachment,
            Err(error) => {
                tracing::warn!(session = session.id, %error, "cannot attach a terminal");
                let mut socket = socket;
                let _ = socket.close().await;
                return;
            }
        };

    let writer = attachment.writer();
    let (mut sink, mut stream) = socket.split();
    let mut read_only = connect.read_only;
    let mut keepalive = tokio::time::interval(KEEPALIVE);
    keepalive.tick().await;

    loop {
        tokio::select! {
            _ = keepalive.tick() => {
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            },
            // The pty has something to show.
            output = attachment.read() => match output {
                Some(bytes) => {
                    if sink.send(Message::binary(bytes)).await.is_err() {
                        break;
                    }
                }
                // `tmux attach` ended: the session was killed, or the user
                // detached from inside it.
                None => break,
            },

            incoming = stream.next() => match incoming {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,

                // Keystrokes.
                Some(Ok(Message::Binary(bytes))) => {
                    if read_only {
                        continue;
                    }
                    if writer.write(bytes.into()).await.is_err() {
                        break;
                    }
                }

                // Control messages.
                Some(Ok(Message::Text(text))) => {
                    match serde_json::from_str::<Control>(&text) {
                        Ok(Control::Resize { cols, rows }) => {
                            if let Err(error) = attachment.resize((cols, rows)) {
                                tracing::debug!(%error, "cannot resize a terminal");
                            }
                        }
                        Ok(Control::ReadOnly { value }) => read_only = value,
                        Err(_) => {
                            tracing::debug!("ignoring an unreadable control message");
                        }
                    }
                }

                Some(Ok(_)) => continue,
            },
        }
    }

    // Dropping the attachment detaches. The session, and the agent in it, are
    // untouched.
    drop(attachment);
    let _ = sink.close().await;
}
