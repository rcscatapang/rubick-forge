# The daemon API

Everything the desktop app can do, it does over this. See
[daemon.md](daemon.md) for the token, the port and the state directory.

Every endpoint except `GET /health` needs `Authorization: Bearer <token>`.

## Errors

Every failure — including unknown paths, wrong methods and malformed query
strings — comes back in one envelope. `message` is written to be shown to a
person as-is; raw stderr and SQL never appear in it.

```json
{ "error": { "code": "not_found", "message": "there is no project with id 7" } }
```

| `code` | Status | Means |
|--------|--------|-------|
| `unauthorized` | 401 | Missing or wrong bearer token |
| `bad_request` | 400 | The request is malformed, or names something git does not have |
| `not_found` | 404 | No such entity, or no such endpoint |
| `method_not_allowed` | 405 | The endpoint exists but not for this method |
| `conflict` | 409 | The request contradicts current state (duplicate, or a guard) |
| `git_missing` | 503 | `git` is not on the daemon's `PATH` |
| `runtime_unavailable` | 503 | tmux is missing, too old, or refusing to work |
| `adapter_unavailable` | 503 | the task's agent CLI is not on the daemon's `PATH` |
| `internal` | 500 | The daemon's own fault; the detail is in its log |

## Projects

A project is a registered git repository — the anchor every task hangs off.
Registering one reads the repository; it never writes to it, and removing a
project leaves the repository untouched.

### `GET /projects`

```json
{ "projects": [ { "id": 1, "name": "forge", "…": "…" } ] }
```

In registration order.

### `POST /projects`

```json
{
  "path": "/Users/you/code/forge",
  "name": "forge",
  "adapter_settings": { "claude-code": { "model": "opus" } }
}
```

Only `path` is required.

- `path` may be any directory inside the repository; it is resolved to the
  repository root and stored canonical. Registering `/repo` and `/repo/src`
  therefore collides, which is the intent — they are one repository.
- `name` defaults to the repository directory's own name.
- `adapter_settings` is per-adapter configuration the daemon stores but does
  not interpret. Keys must be known adapter ids, each value must be an object,
  and each setting must be a string, number or boolean — they become
  command-line flags. Unrecognised keys inside an adapter are kept as given.

The default branch is detected: the remote's `HEAD` when there is one, else the
current branch, else `init.defaultBranch`. Change it with `PATCH`.

Returns `201` with the project. Emits `project_registered`.

Refused with:

- `400` — the path does not exist, is not a directory, or is not inside a git
  repository; or `adapter_settings` is malformed. Nothing is written.
- `409` — that repository is already registered. The message names the
  existing project's id.

### `GET /projects/:id`

The project. `404` when there is none.

### `PATCH /projects/:id`

```json
{ "name": "forge", "default_branch": "develop", "adapter_settings": {} }
```

Every field is optional; omitted fields are left alone. `adapter_settings` is
replaced wholesale, not merged — send the whole blob. A `default_branch` that
does not exist in the repository is refused with `400`. A rejected patch
changes nothing.

### `DELETE /projects/:id`

`204` on success, and emits `project_removed`. The repository and any worktrees
on disk are left where they are.

Refused with `409` while the project still has a task expecting a live session
— idle, working or waiting. Errored and stopped tasks do not block, since
nothing is left to release them.

### `GET /projects/:id/git`

Git facts for the repository root, for the dashboard.

```json
{
  "branch": "main",
  "head": "abc1234",
  "dirty": false,
  "upstream": "origin/main",
  "ahead": 0,
  "behind": 0
}
```

Its own endpoint because it shells out to git: listing projects has to stay
cheap however many are registered.

Every field degrades rather than failing. A detached HEAD has a `null` branch;
a repository with no commits has a `null` head; a branch with no upstream has
`null` for `upstream`, `ahead` and `behind`. `dirty` counts untracked files.

## Adapters

### `GET /adapters`

What this daemon can drive, and how each one can be configured.

```json
{ "adapters": [ {
    "id": "claude-code",
    "name": "Claude Code",
    "binary": { "name": "claude", "path": "…", "version": "…", "ok": true, "detail": null },
    "settings": [ { "key": "model", "kind": "text", "description": "Model to run, passed as --model." } ]
} ] }
```

`settings` is what a project's `adapter_settings` for that adapter may
contain — enough to render a form without hard-coding it. A documented key
must be the kind it says; undocumented keys are stored and returned untouched.

See [adapters.md](adapters.md).

## Tasks

A task is the unit of work: a title, a prompt, a branch, and — unless you opt
out — a git worktree of its own. Agents run inside it.

### `GET /tasks?project=<id>&status=<status>`

```json
{ "tasks": [ { "id": 1, "title": "Add adapters", "…": "…" } ] }
```

Both filters are optional.

### `POST /tasks`

```json
{
  "project_id": 1,
  "title": "Add adapters",
  "adapter": "claude-code",
  "base_branch": "main",
  "initial_prompt": "Implement the adapter trait",
  "use_worktree": true
}
```

`project_id`, `title` and `adapter` are required.

- `base_branch` defaults to the project's. It must exist.
- `use_worktree` defaults to `true`. A task with a worktree gets a directory of
  its own and a `forge/<slug>` branch forked from the base. A task without one
  has `worktree_path: null` and runs in the repository root, on the base branch
  itself — so agents on such tasks share a checkout.
- The slug comes from the title plus the task's id, reduced to lowercase ASCII
  words. Two tasks may share a title; they never share a branch or a directory.

Worktrees are created at `<root>/<project>/<slug>`, where `<root>` is the
`worktree_root` setting when set, and `<parent-of-repo>/.forge-worktrees`
otherwise — beside the repository, never inside it. Both the project and slug
components are reduced to lowercase ASCII words, so nothing a user types can
place a directory outside the root.

Returns `201`. Emits `worktree_created` then `task_created`.

Provisioning is transactional in the way that matters: if the worktree cannot
be created, the task row is deleted again, so a failed create leaves neither a
row nor a directory.

Refused with:

- `400` — no title, or a base branch that does not exist.
- `404` — no such project.
- `409` — git refused, and says why (usually the branch or directory exists).

### `GET /tasks/:id`

The task. `404` when there is none.

### `PATCH /tasks/:id`

```json
{ "title": "Renamed", "initial_prompt": "…" }
```

Renaming does not move the worktree: the slug is fixed when the task is
created, and moving a checked-out directory under a running agent is not worth
the surprise.

### `DELETE /tasks/:id?force=true`

`204` on success. Emits `task_deleted`.

Refused with `409` while the task has a running session — stop it first — or
while it still has a worktree. `force=true` cleans the worktree up first,
discarding uncommitted changes. The `forge/<slug>` branch is kept: deleting a
task should not be able to destroy commits that were never merged anywhere.

### `POST /tasks/:id/worktree/cleanup`

```json
{ "force": false, "delete_branch": false }
```

Removes the worktree. Returns the task. Emits `worktree_removed`.

- `force` removes a worktree with uncommitted changes, and deletes an unmerged
  branch. Without it, either is a `409`.
- `delete_branch` drops `forge/<slug>` as well. The default keeps it, so work
  is recoverable after cleanup.

The task keeps pointing at `forge/<slug>` when the branch survives, since that
is where its work is; only deleting the branch sends the task back to its base.

A worktree whose directory you deleted by hand cleans up without complaint —
git's own record of it is pruned instead.

`409` when the task has no worktree, when its session is still running, or when
the worktree is dirty and `force` was not set.

### `GET /tasks/:id/git`

The same shape as `GET /projects/:id/git`, for the task's own working tree —
its worktree, or the repository root when it has none.

## Sessions

A session is one run of a task inside tmux. See [runtime.md](runtime.md) for
naming, recovery and the `tmux attach` escape hatch.

### `GET /tasks/:id/sessions`

```json
{ "sessions": [ { "id": 1, "task_id": 1, "tmux_name": "forge-1",
                  "pid": 4242, "status": "idle",
                  "started_at": "…", "ended_at": null } ] }
```

Oldest first. A task accumulates one row per run; `ended_at: null` marks the
live one.

### `POST /tasks/:id/start`

`201` with the new session. Emits `session_started`.

The session runs the task's adapter — see [adapters.md](adapters.md) — with the
project's settings for it and the task's initial prompt.

`409` when the task is already running. `503` when tmux or the agent CLI is
missing or unusable; the message says which, and `/health` has the detail.

### `POST /tasks/:id/stop`

`200` with the closed session. Emits `session_stopped`.

The daemon sends `C-c`, waits up to three seconds, then kills the session.
`409` when the task is not running.

### `POST /tasks/:id/restart`

`201` with a second session. The first is stopped and kept in the task's
history rather than reused.

### `POST /tasks/:id/instruction`

```json
{ "text": "Also update the tests" }
```

`202` when it has been typed into the running agent. The text is sent through
a paste buffer and then Enter, so newlines stay newlines and nothing in it is
read as a key or reaches a shell.

`409` when the task is not running, `400` when the text is empty.

## Events

See [daemon.md](daemon.md) for the cursor semantics.

### `GET /events?after=<id>&limit=<n>&task=<id>&newest=<bool>`

```json
{ "events": [ { "id": 1, "ts": "…", "kind": "project_registered", "…": "…" } ],
  "next_after": 1 }
```

Oldest first, whichever end the page came from. `limit` defaults to 100 and is
capped at 500. `task` narrows the feed to one task.

`newest=true` takes the *end* of history rather than its beginning — what a
feed wants. Paging forward from the first event ever recorded freezes a feed
once there is more than one page of history.

### `WS /ws/events?after=<id>&task=<id>`

One JSON frame per event, same shape as an `/events` row. Takes `?token=`
because a browser cannot set headers on the handshake.

## Terminals

### `WS /ws/sessions/:id/terminal?cols=&rows=&read_only=`

A live terminal for one session. The daemon runs `tmux attach` in a pty of its
own and proxies raw bytes both ways.

| Frame | Direction | Meaning |
|-------|-----------|---------|
| Binary | daemon → client | Terminal output, verbatim |
| Binary | client → daemon | Keystrokes, verbatim |
| Text | client → daemon | A control message |

Two control messages:

```json
{ "type": "resize", "cols": 132, "rows": 43 }
{ "type": "read_only", "value": true }
```

Toggling read-only over the socket rather than reconnecting keeps what is on
screen: retaking a terminal would lose it. An unreadable control message is
ignored rather than fatal.

The daemon pings every 20 seconds. A viewer whose machine sleeps or loses its
network leaves a connection that never reads and never errors on write; the
ping is what eventually fails, and failing is what reaps the pty.

A viewer that falls far enough behind is disconnected rather than thinned.
Dropping bytes would render as garbage, and blocking would back up into the
tmux server and hurt everyone else on that session. Reconnecting redraws.

`cols` and `rows` set the initial size so the first redraw is already right;
they default to 80×24. `read_only=true` discards the client's keystrokes at
the daemon, so it does not depend on the client behaving.

Takes `?token=` — a browser cannot set headers on a handshake. The upgrade is
refused outright for a session that does not exist or has already ended.

**One attach per viewer.** Two windows on one session each get their own, and
neither disturbs the other. Closing a socket detaches and reaps that pty; the
session, and the agent inside it, are untouched.

**Sizing.** tmux sizes a window to its *smallest* attached client — including
a human attached in their own terminal. A small app window will crop what
everyone sees, and there is nothing Forge can do about that from its side.

## Settings

Daemon-wide preferences that every client shares — notification toggles, the
worktree root. Deliberately untyped: what a setting means is the business of
whoever reads it.

### `GET /settings`

```json
{ "settings": { "notify.agent_waiting": "false", "worktree_root": "/trees" } }
```

### `PATCH /settings`

```json
{ "notify.agent_waiting": "false", "worktree_root": null }
```

Sets the keys named and leaves the rest alone; `null` clears one. Returns the
settings as they now stand. A value over 4096 characters is a `400` — this is a
preferences table, not a file store.

## Health

### `GET /health`

Unauthenticated, so a client can tell "daemon down" from "wrong token".

```json
{
  "version": "0.1.0",
  "uptime_secs": 1234,
  "machine": "Ryan's MacBook Pro",
  "binaries": [
    { "name": "tmux", "path": null, "version": null, "ok": false,
      "detail": "`tmux` was not found on PATH" }
  ]
}
```

Lists the tools the daemon needs — `tmux`, `git` — then every agent CLI it can
drive. A missing binary is a warning, never a crash: the daemon still serves
history and settings, it just cannot start the sessions that need it. Results
are cached for 30 seconds.
