//! A viewer's own `tmux attach`, running in a pty the daemon owns.
//!
//! One of these per viewer, not per session: tmux is built for several clients
//! on one session, so two windows watching the same agent each get their own
//! attach and neither disturbs the other. Detaching is all that happens when a
//! viewer leaves — the session, and the agent in it, carry on.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex, PoisonError};

use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use tokio::sync::mpsc;

/// What a client's terminal is, before it has told us its real size.
pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;

/// Bytes read from the pty in one go. A screenful of a redrawing TUI is well
/// under this, so a busy agent still arrives in few messages.
const READ_CHUNK: usize = 8 * 1024;

/// How many chunks may be waiting for a slow viewer before it is disconnected.
///
/// Dropping bytes is not an option — a terminal stream with holes renders as
/// garbage — so a viewer that cannot keep up is cut off instead, and
/// reconnecting redraws from tmux.
const BACKLOG: usize = 256;

/// A running `tmux attach`, and the two ends of its pty.
pub struct Attachment {
    master: Box<dyn MasterPty + Send>,
    /// Taken in `Drop`, so the wait can happen off this thread.
    child: Option<Box<dyn Child + Send + Sync>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    output: mpsc::Receiver<Vec<u8>>,
}

impl Attachment {
    /// Attach to `session` through the given tmux binary and socket.
    pub fn open(
        tmux_binary: &str,
        socket: Option<&str>,
        session: &str,
        size: (u16, u16),
    ) -> Result<Self, TerminalError> {
        let pty = NativePtySystem::default()
            .openpty(pty_size(size))
            .map_err(|source| TerminalError::Open(source.to_string()))?;

        let mut command = CommandBuilder::new(tmux_binary);
        if let Some(socket) = socket {
            command.arg("-L");
            command.arg(socket);
        }
        command.arg("attach");
        command.arg("-t");
        command.arg(session);

        // Without this the attached tmux has no idea what the client can
        // render, and a TUI comes out as boxes.
        command.env("TERM", "xterm-256color");

        let mut child = pty
            .slave
            .spawn_command(command)
            .map_err(|source| TerminalError::Spawn(source.to_string()))?;

        // The slave has been handed to the child; holding our copy would keep
        // the pty open after it exits, and the reader would never see EOF.
        drop(pty.slave);

        // From here the child is running, so anything that fails has to take
        // it down — an attach nothing holds is an orphan nothing can reap.
        let open = || -> Result<_, TerminalError> {
            let writer = pty
                .master
                .take_writer()
                .map_err(|source| TerminalError::Open(source.to_string()))?;
            let reader = pty
                .master
                .try_clone_reader()
                .map_err(|source| TerminalError::Open(source.to_string()))?;
            Ok((writer, reader))
        };

        let (writer, reader) = match open() {
            Ok(pair) => pair,
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err);
            }
        };

        let (sender, output) = mpsc::channel(BACKLOG);
        spawn_reader(reader, sender);

        Ok(Self {
            master: pty.master,
            child: Some(child),
            writer: Arc::new(Mutex::new(writer)),
            output,
        })
    }

    /// The next chunk of terminal output, or `None` once the attach has ended.
    pub async fn read(&mut self) -> Option<Vec<u8>> {
        self.output.recv().await
    }

    /// A handle for sending keystrokes.
    ///
    /// Separate from the attachment because the pty master is not `Sync`:
    /// holding a reference to the whole thing across an await would make the
    /// whole connection un-sendable between threads.
    pub fn writer(&self) -> TerminalWriter {
        TerminalWriter(self.writer.clone())
    }

    /// Tell the pty how big the viewer's terminal is.
    ///
    /// tmux sizes a window to its *smallest* attached client, so a viewer with
    /// a small window shrinks what everyone else sees, including a human
    /// attached in their own terminal.
    pub fn resize(&self, size: (u16, u16)) -> Result<(), TerminalError> {
        self.master
            .resize(pty_size(size))
            .map_err(|source| TerminalError::Resize(source.to_string()))
    }
}

impl Drop for Attachment {
    /// Detaching is the point: killing our own `tmux attach` leaves the session
    /// and the agent inside it running. Without this, every viewer that ever
    /// connected would leave a process behind.
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };

        let _ = child.kill();

        // Reaping blocks, and this runs on whichever thread dropped the
        // connection — usually one of the runtime's.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Sends keystrokes to a terminal, independently of reading from it.
#[derive(Clone)]
pub struct TerminalWriter(Arc<Mutex<Box<dyn Write + Send>>>);

impl TerminalWriter {
    pub async fn write(&self, bytes: Vec<u8>) -> Result<(), TerminalError> {
        let writer = self.0.clone();

        // The pty is blocking, so writing to it does not belong on the async
        // runtime's threads.
        tokio::task::spawn_blocking(move || {
            let mut writer = writer.lock().unwrap_or_else(PoisonError::into_inner);
            writer.write_all(&bytes).and_then(|()| writer.flush())
        })
        .await
        .map_err(|_| TerminalError::Write("the write task was cancelled".to_owned()))?
        .map_err(|source| TerminalError::Write(source.to_string()))
    }
}

/// The pty is blocking, so it gets a thread rather than a task.
fn spawn_reader(mut reader: Box<dyn Read + Send>, sender: mpsc::Sender<Vec<u8>>) {
    std::thread::spawn(move || {
        let mut buffer = vec![0u8; READ_CHUNK];

        loop {
            match reader.read(&mut buffer) {
                // EOF: the attach has ended.
                Ok(0) => break,
                Ok(read) => match sender.try_send(buffer[..read].to_vec()) {
                    Ok(()) => {}
                    // The viewer is too far behind. Blocking here would stop
                    // draining the pty, which backs up into the tmux server
                    // and hurts every other client; dropping bytes would
                    // render as garbage. Ending the connection is the only
                    // honest option, and reconnecting redraws from tmux.
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        tracing::warn!("a terminal viewer fell too far behind; disconnecting it");
                        break;
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => break,
                },
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });
}

/// A zero dimension makes tmux refuse; a client that reports one gets the
/// default instead of an error.
fn pty_size((cols, rows): (u16, u16)) -> PtySize {
    PtySize {
        rows: if rows == 0 { DEFAULT_ROWS } else { rows },
        cols: if cols == 0 { DEFAULT_COLS } else { cols },
        pixel_width: 0,
        pixel_height: 0,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("cannot open a terminal: {0}")]
    Open(String),
    #[error("cannot attach to the session: {0}")]
    Spawn(String),
    #[error("cannot send input to the terminal: {0}")]
    Write(String),
    #[error("cannot resize the terminal: {0}")]
    Resize(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_size_falls_back_to_something_usable() {
        let size = pty_size((0, 0));

        assert_eq!(size.cols, DEFAULT_COLS);
        assert_eq!(size.rows, DEFAULT_ROWS);
    }

    #[test]
    fn a_real_size_is_passed_through() {
        let size = pty_size((120, 40));

        assert_eq!(size.cols, 120);
        assert_eq!(size.rows, 40);
    }
}
