# Adapter manifests and event hooks

Two ways to extend Forge, neither of which is code you load into it.

## Why there is no plugin API

Forge will never load a dylib, a WASM module, or a script into its own process.
A plugin that runs inside the daemon can crash it, wedge it, or read its
secrets — and the daemon is the thing that keeps your agents alive.

So extension is exactly two things:

- **Adapter manifests** — a TOML file that *describes* an agent CLI. Data, not
  behaviour.
- **Event hooks** — an executable Forge runs in its own process, told what
  happened and unable to change it.

Both are inert. Neither can make the daemon do something it would not otherwise
do.

---

# Adapter manifests

A manifest tells Forge how to launch an agent CLI and how to read its screen.
Claude Code and Codex are manifests too — compiled into the binary, loaded
through the same code path as yours. There is one adapter engine and no special
cases, so if the format can describe them it can describe yours.

Drop a `.toml` file in:

```
~/Library/Application Support/rubick-forge/adapters/
```

Then `POST /adapters/reload`, or restart the daemon.

## A complete manifest

```toml
manifest_version = 1
id = "aider"                 # lowercase, digits, dashes. Reaches a URL and a
                             # database column, so it is kept plain.
name = "Aider"               # what the UI calls it
binary = "aider"             # resolved on PATH. Never a path, never a shell.
version_args = ["--version"] # what makes it print a version, for /health

# Extra argv elements at launch, before the prompt.
launch_args = ["--no-auto-commits"]

[injection]
paste = true                 # paste through a tmux buffer rather than sending
                             # keys. What any TUI needs: pasted text cannot be
                             # read as key bindings.
submit_keys = ["Enter"]

[answers]                    # the keys that answer a permission dialog
approve = ["1", "Enter"]
deny = ["Escape"]

# Marker sets, tried in order. Order is the design — see below.
[[status]]
status = "waiting"
markers = ["Do you want to", "(y/n)"]
permission_markers = ["Do you want to", "(y/n)"]

[[status]]
status = "error"
markers = ["API error", "Not logged in"]

[[status]]
status = "working"
markers = ["Thinking", "esc to interrupt"]

[[status]]
status = "idle"
markers = ["> ", "? for help"]

[[settings]]
key = "model"
kind = "text"                # text | number | flag | args
description = "Model to run."
args = ["--model", "{value}"]

[[settings]]
key = "extra_args"
kind = "args"                # the value is split on whitespace into argv
description = "Extra command-line arguments."
```

The initial prompt is appended as the last argv element. There is no way to
have it typed in after launch instead — that was in an early draft of this
format and removed rather than shipped as a documented option that did nothing.

The built-ins live in `crates/forge-daemon/src/adapters/builtin/` and are worth
reading as worked examples.

## Markers are literal, not patterns

A marker is a **plain substring** of the CLI's own interface. There are no
regular expressions, and there will not be:

- They are fragments of somebody else's UI. A list of strings is something you
  can read and correct without reasoning about escaping.
- A pattern language on untrusted screen output is a catastrophic-backtracking
  bug waiting to happen. Substring matching cannot do that.

Only the **last 30 lines** of the pane are examined. The states worth detecting
are all at the bottom — a prompt, a spinner, a question — and looking further up
matches things the agent merely *printed* about a state rather than being in.

### Order is the whole design

Sets are tried in order and the first match wins. Put `waiting` first: a CLI
usually keeps its status line on screen while it asks a question, and being
wrong about `waiting` is the expensive mistake — that is the state a human has
to be told about.

`error` markers must match **the CLI's own failures only**. If `error:` is in
your list, ordinary compiler output from a working agent will read as a broken
session.

### Key lists are checked, not escaped

`answers.approve`, `answers.deny` and `injection.submit_keys` reach
`tmux send-keys`. tmux splits its *own* argument list on a bare `;` before `--`
can protect anything after it, so a manifest saying
`approve = [";", "kill-server"]` would be a command into the tmux server rather
than a keystroke. Those lists are therefore restricted to plain key names and a
manifest using anything else is refused.

### `permission_markers`

A narrower subset meaning "answering yes or no makes sense here". `markers` are
tuned for recall so anything blocking reads as `waiting`; these decide whether
Forge offers Approve and Deny buttons at all. Offering them on a screen that is
not a dialog would type `1` and Enter into a prompt.

## Templates cannot become commands

`{value}`, `{prompt}`, `{task_id}` and `{task_title}` are substituted into
**one argv element each**. Nothing is ever joined into a shell string, and no
shell is involved anywhere on this path. A model name of `$(rm -rf ~)` is passed
to the CLI as those literal characters.

A placeholder Forge does not fill is a validation error rather than a literal
`{moddel}` reaching the CLI and being blamed on it.

## When a manifest is wrong

**A broken manifest costs its own adapter and nothing else.** The daemon starts,
every other adapter works, and the error appears in two places:

```sh
curl -s localhost:8787/health | jq .adapter_errors
# [{"source": "aider.toml", "detail": "manifest_version is 2, but this daemon …"}]
```

`GET /adapters` reports the same list next to the adapters that did load.

A file cannot take over a built-in's id: shadowing `claude-code` would give you
a daemon that behaves differently for reasons nothing on screen explains.

## Reloading

```sh
curl -XPOST -H "Authorization: Bearer $TOKEN" localhost:8787/adapters/reload
```

Running sessions keep the adapter they started with — their argv is already
fixed. A reload changes what the *next* task gets.

Settings for an adapter that is not loaded are **kept**, not dropped. A project
configured for an agent whose manifest is temporarily broken does not lose its
configuration.

---

# Event hooks

A script Forge runs when something happens.

```
~/Library/Application Support/rubick-forge/hooks/     # the scripts
~/Library/Application Support/rubick-forge/hooks.toml # what runs when
```

```toml
timeout_secs = 30   # a hook is killed at this
concurrency = 4     # how many run at once
queue = 64          # how many may wait before new ones are dropped

[on]
task_finished = "notify.sh"
agent_waiting = "notify.sh"
checks_failed = "page-me.sh"
```

Any event kind works — the same names `GET /events` uses. A name this daemon
does not have is logged and ignored, so one file can be shared with a newer
version.

## The contract

| | |
|---|---|
| **stdin** | The event as JSON, then end-of-input |
| **cwd** | The task's worktree when the event has one, else the daemon's |
| **env** | `FORGE_EVENT_KIND`, `FORGE_EVENT_ID`, and `FORGE_TASK_ID` when the event has a task. Everything else is inherited from the daemon. |
| **stdout/stderr** | Captured, capped, and written to the daemon log — not the activity feed |
| **exit code** | Recorded and **never acted on** |

```sh
#!/bin/sh
# hooks/notify.sh
kind=$(jq -r .kind)
osascript -e "display notification \"$kind\""
```

Scripts must be executable (`chmod +x`), and are named by **filename only** —
`notify.sh`, never `../../usr/bin/curl`. A path is refused.

## Limits, and why

- **Killed at the timeout** (30 s default). A hook that hangs must not
  accumulate.
- **Two separate limits.** `concurrency` is how many run at once; hooks
  admitted beyond that **wait**. `queue` is how many may wait. Past
  `concurrency + queue`, new hooks are **dropped with a log line** rather than
  held — a fleet of agents all finishing at once should cost log lines, not a
  fork bomb on the Mac your agents are running on.
- **Fire and forget.** A hook cannot veto, delay, or alter anything. That is the
  line between "run a script on this event" and a plugin API, and it is what
  makes a broken hook harmless.

## What is deliberately not here

- **Dynamic code loading** — no dylibs, no WASM, not ever (SPEC D24).
- **Hooks that influence the daemon.** The exit code is recorded, not obeyed.
- **A marketplace or registry.** A manifest is a file you can read.
