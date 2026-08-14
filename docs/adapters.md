# Agent adapters

An adapter is everything the daemon knows about one agent CLI: how to launch
it, what its settings mean, and how to read what it is doing from its terminal
output.

There are two, and the set is closed for now: `claude-code` and `codex`.

## What an adapter provides

| | |
|---|---|
| `binary_check` | Is the CLI on `PATH`, and what version? Reported on `/health`. |
| `launch_command` | The argv for a task's session. |
| `status_patterns` | Ordered markers that turn pane text into a status. |
| `settings_schema` | The settings the adapter reads from a project's blob. |

Instruction injection is not per-adapter: every CLI reads its terminal, so
sending text is a paste followed by Enter, which the session runtime does.

## Launching

```
<cli> [settings-derived flags...] [initial prompt]
```

Everything is a separate argv entry and nothing reaches a shell, so a prompt
containing `$(…)`, quotes, semicolons or newlines is text and stays text.

The initial prompt goes last, as one argument. A task with no prompt, or a
blank one, just starts the CLI.

Settings come from the project's `adapter_settings` under that adapter's key:

| Adapter | Setting | Becomes |
|---------|---------|---------|
| `claude-code` | `model` | `--model <value>` |
| `claude-code` | `permission_mode` | `--permission-mode <value>` |
| `claude-code` | `extra_args` | split on spaces, appended |
| `codex` | `model` | `--model <value>` |
| `codex` | `extra_args` | split on spaces, appended |

A setting of the wrong type is ignored rather than stringified. Keys an adapter
does not know are stored and handed back untouched, so a setting added ahead of
daemon support is not lost.

## Status detection is a heuristic

The daemon reads the bottom of the pane and looks for literal fragments of each
CLI's interface. That is all it is. There is no protocol, no structured output,
and no agreement from the CLI that these strings are stable.

Markers are plain substrings rather than regular expressions, kept as one list
per adapter in `src/adapters/*.rs`, so that correcting them is reading a list
and editing a string.

They are tried in a fixed order, and the order is the design:

1. **Process exited** — a dead pane decides the outcome whatever is on screen.
   Exit zero is `stopped`; anything else is `error`.
2. **Waiting** — permission prompts, confirmations, trust dialogs.
3. **Error** — authentication failures, API errors.
4. **Working** — the interrupt hint a CLI shows while it is busy.
5. **Idle** — an empty input prompt.
6. **Nothing recognised** — keep believing the last status.

`waiting` comes first because a CLI usually keeps its status line on screen
while it asks a question, and because being wrong about `waiting` is the
expensive mistake: it is the state a human is notified about.

### Guards

- **Debounce.** Entering `waiting` must survive one extra poll before it is
  announced. A CLI redrawing its screen can flash a dialog's remains for a
  single frame, and that must not become a notification.
- **No restacking.** `agent_waiting` fires on *entering* the state, not while
  it holds.
- **Staleness cap.** A belief nobody confirms decays to `idle` after 15 polls.
  Without it, an agent that finished into a screen the markers do not know
  would claim to be working forever.
- **Bounded reads.** Only the last 200 lines are captured, and only the last 30
  are matched. A question that has scrolled away is not a question being asked.

### Known-unreliable states

Stated plainly, because the alternative is a user trusting it too much:

- **`working` vs `idle`** is the weakest distinction. Both CLIs show similar
  furniture in both states, so a working agent that prints nothing for a while
  may read as idle.
- **`error`** only catches errors the CLI prints in a form we recognise. An
  agent that fails quietly reads as idle. A crash — a non-zero exit — is
  reliable, because that comes from the process, not the screen.
- **Anything after a CLI update.** Markers are fragments of someone else's
  interface. When it changes, they stop matching, and the honest failure is a
  status that stops moving rather than one that is wrong.
- **A human typing in the pane.** `tmux attach` is supported, and a person
  driving the session changes what is on screen. The status follows the screen,
  not who caused it.

Everything here is "good enough to glance at". Nothing should be automated off
these statuses except notifications, which is the one thing they were tuned
for.

## Fixtures

`crates/forge-daemon/tests/fixtures/panes/` holds verbatim `capture-pane`
output from the real CLIs, named `<adapter>.<status>-<detail>.txt`. The marker
tests classify each one and check it comes out as its name says.

To capture a new one:

```sh
tmux -L forge-capture new-session -d -s c -c /some/repo -x 100 -y 30
tmux -L forge-capture respawn-pane -k -t c -- claude
# drive it into the state you want, then:
tmux -L forge-capture capture-pane -p -t c > claude-code.waiting-permission.txt
tmux -L forge-capture kill-server
```

Scrub anything personal — paths, account names, organisation names — before
checking it in. The markers do not depend on them.
