# The desktop app

The app is a window onto the daemon. It holds no state of its own beyond where
that daemon is, and it never touches an agent, tmux, git or the database.

Closing it changes nothing: the daemon keeps running under launchd, tmux keeps
the sessions, and the agents keep working. Reopening it reattaches to whatever
is there.

## What lives where

| | |
|---|---|
| **Daemon** | Projects, tasks, sessions, worktrees, history, settings, and every decision about them |
| **App** | The daemon's URL and token — and nothing else |

Everything on screen is read from the daemon over HTTP, and kept current by its
event stream. Nothing polls except the health check, which is how the app
notices the daemon went away.

## Connecting

On start the app asks `GET /health`. Until that answers, it shows why rather
than an empty dashboard — an app that renders nothing looks the same as one
with nothing to show.

If the daemon is not there, the app offers to install it: pick the
`forge-daemon` binary and it runs the binary's own `--install-launchd`, then
reads the token the daemon wrote. Doing it by hand works just as well:

```sh
forge-daemon --install-launchd
```

Then choose "Try again".

Reading the token file and running that one subcommand are the only two native
things the app shell does, and both are about *reaching* the daemon rather than
doing its work. A webview can do neither.

## Staying current

The app subscribes to `WS /ws/events` and invalidates exactly what each event
touched. A dropped socket reconnects with the last event id it saw, so nothing
is missed while it was away.

That is why the dashboard updates without a refresh button: a status change, a
worktree appearing, an agent asking a question — each arrives as an event and
the affected part of the screen refetches.

## The dashboard

- **Needs you** comes first, and `waiting` is the loudest thing on the screen.
  It is the one state that cannot resolve itself.
- **Tasks** are grouped by project, each showing its branch, whether the
  worktree is dirty, and how far ahead of its upstream it is.
- **Actions** are disabled by state rather than hidden, so a row does not
  reshuffle every time an agent changes what it is doing. A running task cannot
  be deleted or have its worktree removed; stop it first.
- **Activity** is the daemon's own event history, newest first.

No test or build status. That arrives with the GitHub integration.

## Notifications

Three things get a native notification: an agent is **waiting** for you, a task
**finished**, an agent **errored**. Nothing else — a dashboard you have to
watch is one you stop watching, and a stream of notifications is one you turn
off.

Each kind has a toggle, and the toggles live in the daemon's settings rather
than in the app, so a second Mac — or the Telegram bot, later — shares the same
answer.

- The pane tail in a `waiting` notification is stripped of everything that
  draws a terminal and capped in length, so what you read is the question.
- The task whose terminal is on screen in a focused window is never announced.
  Telling someone what they are looking at is noise.
- An agent that keeps asking while nobody answers does not restack. The daemon
  already announces `waiting` only on entering it, and the app will not repeat
  the same kind for the same task.
- If macOS refuses permission, the app says so in the settings section and
  carries on: the dashboard shows the same states either way.

Notifications need this app to be open. Away from your Mac, the answer is the
Telegram bot, which lives in the daemon and does not depend on any app being
running.

## Terminals

Opening a task's terminal attaches to its tmux session over a WebSocket. It is
the same session `tmux attach -t forge-<task-id>` reaches from any terminal,
and closing the view detaches without disturbing the agent.

Read-only is a toggle rather than a separate mode: it is sent over the open
socket, because retaking a terminal would lose what is on screen. The daemon
enforces it too, so it does not depend on the app behaving.
