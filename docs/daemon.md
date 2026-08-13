# The daemon

`forge-daemon` owns everything: the agents, tmux, git, and the database. The
desktop app is a client of its HTTP/WS API and nothing more. Closing the app
changes nothing about running agents.

## State directory

Everything the daemon remembers lives in
`~/Library/Application Support/rubick-forge/`:

| File | What it is |
|------|------------|
| `forge.db` | SQLite database (WAL mode) |
| `daemon.toml` | Bind address, port, machine name, worktree root override |
| `token` | The bearer token, `0600` |
| `logs/` | Rotated daemon logs |

Set `FORGE_STATE_DIR` to relocate all of it. The LaunchAgent records whatever
directory it was installed from, so an install done with the variable set keeps
using that directory.

The whole tree is created on first start. Starting again is idempotent: the
existing config, token and database are reused.

## Configuration

`daemon.toml`, written with defaults on first start:

```toml
bind = "127.0.0.1"
port = 8787
machine = "Ryan's MacBook Pro"
stop_keys = ["C-c"]
stop_grace_secs = 3
poll_secs = 2
# worktree_root = "/Users/you/worktrees"
```

- `bind` — loopback by default. A Tailscale address is a valid explicit
  opt-in; `0.0.0.0` is refused at load, because Forge never exposes itself to
  a network it did not choose.
- `port` — fixed rather than discovered, so the app knows where to look.
- `machine` — defaults to the Mac's own name; labels this daemon in a
  multi-machine UI.
- `worktree_root` — overrides the default sibling `.forge-worktrees` directory.
- `stop_keys`, `stop_grace_secs` — how a stop asks an agent to leave before
  killing its session. See [runtime.md](runtime.md).
- `poll_secs` — how often live sessions are checked against reality.

Unknown keys are an error, not a silent default: a typo should be loud.

## Authentication

A 32-byte random token, hex-encoded, is generated on first start and written
`0600`. It is never logged, and a loose mode on the file is tightened on every
load.

Every endpoint except `GET /health` requires it:

```sh
curl -H "Authorization: Bearer $(cat ~/Library/Application\ Support/rubick-forge/token)" \
  http://127.0.0.1:8787/events
```

WebSocket clients cannot set headers, so `?token=<token>` is accepted on
`/ws/` routes — and only there. A secret in a URL is a secret in access logs,
and REST clients have no reason to need it. Both paths compare in constant
time.

`/health` is deliberately open so the app can tell "daemon down" from
"wrong token" without a credential.

## Endpoints in this milestone

```
GET  /health                                 # version, uptime, machine, tmux/git checks
GET  /events?after=<id>&limit=<n>&task=<id>  # history, oldest first, paged by id cursor
WS   /ws/events?after=<id>&task=<id>         # live stream, optionally backfilled
```

`task` narrows the feed to one task, on both the history and the live stream.

Every error — including an unknown path, a wrong method, and a malformed query
string — answers in one envelope, and `message` is meant to be shown to a
person as-is:

```json
{ "error": { "code": "unauthorized", "message": "…" } }
```

### The event cursor

`GET /events` returns `{ events, next_after }`. Pass `next_after` back as
`after` for the next page; an empty page means you are current.

`/ws/events` takes the same `after`. The handler subscribes to the live bus
*before* it backfills from history, so an event published mid-backfill arrives
on the subscription and is skipped by id — no gap, no duplicate.

If a client falls far enough behind that the daemon drops events for it, the
socket is closed rather than silently thinned. Reconnect with the last id you
saw and the backfill covers the gap.

## Database

SQLite in WAL mode with `synchronous = NORMAL`: a `kill -9` can cost the last
transaction, never the file. Foreign keys are on.

Migrations are forward-only and versioned in `PRAGMA user_version`. A database
newer than the running daemon is refused rather than downgraded.

`events` deliberately has no foreign keys — history outlives the tasks and
sessions it describes. Its `task_id` column is a denormalised index for the
`task` filter; the authoritative copy is inside `payload`.

Binary probes on `/health` are cached for 30 seconds. The endpoint is
unauthenticated and each probe spawns processes, so an uncached one would let
any caller make the daemon fork as fast as it can ask.

## launchd

```sh
forge-daemon --install-launchd     # write the plist and start it
forge-daemon --uninstall-launchd   # stop it and remove the plist
```

The agent is `tech.cloverly.rubick-forge.daemon` in
`~/Library/LaunchAgents/`, with `KeepAlive` and `RunAtLoad` set — which is what
makes the daemon always-on and lets the app be a mere viewer. Installing again
is a repair: it boots out the old agent and bootstraps the new plist.

Useful once installed:

```sh
launchctl print gui/$(id -u)/tech.cloverly.rubick-forge.daemon
launchctl kickstart -k gui/$(id -u)/tech.cloverly.rubick-forge.daemon
```

The state directory is left alone by `--uninstall-launchd`.

## Development mode

```sh
cargo run -p forge-daemon -- --foreground
```

Foreground mode logs to stderr instead of `logs/`. `RUST_LOG` is respected in
both modes:

```sh
RUST_LOG=forge_daemon=debug cargo run -p forge-daemon -- --foreground
```

Point a dev daemon at a scratch state directory to keep it away from your real
one:

```sh
FORGE_STATE_DIR=/tmp/forge-dev cargo run -p forge-daemon -- --foreground
```
