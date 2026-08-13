# The session runtime

Agents run in tmux. That is a hard dependency, and it is what makes an agent's
work survive the daemon crashing, being upgraded, or being killed outright —
tmux keeps running, and the daemon finds its sessions again when it comes back.

## Naming, and the escape hatch

One tmux session per running task, named after the task:

```sh
tmux attach -t forge-<task-id>
```

That is a supported thing to do, not a workaround. Forge tolerates a human
attached to a session it is driving: capturing output and sending input both
keep working, and detaching changes nothing.

Killing a session by hand works too — the daemon notices and marks the task
stopped rather than insisting its records are right.

## Requirements

tmux **3.0 or newer**. Older versions lack the pane formats the daemon reads to
tell a running agent from one that exited, and how it exited.

`GET /health` reports the version it found and says plainly when it is too old.
A missing or too-old tmux is never a crash: the daemon still serves history,
projects, tasks and settings; it just refuses to start sessions, with a message
naming the reason.

## What a session is

- One window, one pane.
- Working directory is the task's worktree, or the repository root for a task
  created with `use_worktree: false`.
- The command is given to tmux as an argv list. Nothing a user types — a task
  title, a prompt — is ever assembled into a shell string.
- `remain-on-exit` is on, so the pane survives its process. That is how the
  daemon reads the exit status that separates "finished" from "crashed"; the
  daemon kills the session itself once it is done with it.

Sessions are created empty and then respawned with the real command, because
`remain-on-exit` has to be set before the command runs — otherwise a command
that exits immediately takes the pane, the session, and its own exit status
with it. `create` returns only once the pane is running the command, so a
caller can start a session and immediately send it input.

## Starting and stopping

```
POST /tasks/:id/start      # 201 with the new session
POST /tasks/:id/stop       # 200 with the closed session
POST /tasks/:id/restart    # 201 with a second session
GET  /tasks/:id/sessions   # every run of the task, oldest first
```

Stopping is graceful, then not: the daemon sends an interrupt, waits for the
process to leave on its own, then kills the session. An agent gets the chance
to shut down cleanly before being taken away.

Both halves are configurable in `daemon.toml`:

```toml
stop_keys = ["C-c"]     # tmux key names, sent in order
stop_grace_secs = 3     # how long to wait before killing
poll_secs = 2           # how often live sessions are checked
```

Restarting is a stop followed by a start, and produces a *new* session row. A
task accumulates sessions over its life; none of them are reused, so its
history stays readable.

Starting a task that is already running is a `409`, and so is stopping one that
is not.

## Watching

While the daemon runs, it checks live sessions every `poll_secs`:

- A session that has gone is closed and its task marked `stopped`.
- A pane whose process has exited is closed too, and the exit code decides
  which: zero is `stopped`, anything else is `error`. The session is then
  killed, since `remain-on-exit` left it standing.

So an agent that finishes or crashes is noticed within a poll interval, not
only at the next boot.

## Recovery

On boot the daemon reconciles the `sessions` table against what tmux actually
has:

- A row whose session is still there is **adopted** — no restart, no duplicate
  row, no interruption to the agent.
- A row whose session has gone is closed and marked `stopped`, and a
  `session_stopped` event with reason `vanished` goes on the bus.
- A session named `forge-…` that the daemon has no row for is **left alone** and
  logged once. It is not adopted and it is not killed: it is not ours.

Reconciling is idempotent — running it twice changes nothing the second time —
and it is a handful of tmux calls, so tens of sessions cost well under a
second.

The same pass is what "sessions survive a daemon restart" means in practice.
There is nothing else to it: launchd restarts the daemon, the daemon asks tmux
what is still running, and carries on.

## Terminals

Each viewer gets its own `tmux attach` in a pty the daemon owns, and raw bytes
are proxied over a WebSocket. That is why a full TUI renders: nothing
interprets the stream, it is the same bytes a terminal would receive.

Closing the socket kills that attach and nothing else. A session outlives every
viewer that ever watched it.

See [api.md](api.md) for the frame protocol.

## Isolation while testing

`TmuxRuntime::with_socket(label)` talks to a private tmux server under
`tmux -L <label>`. The test suite uses this so it can create, kill and inspect
sessions freely without touching the tmux the developer is sitting in.
