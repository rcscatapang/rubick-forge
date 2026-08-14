# Rubick Forge

An open-source **AI coding-agent control plane** for macOS. Run Claude Code and Codex in durable tmux sessions managed by an always-on daemon, watch and steer them from a desktop dashboard — and eventually from your phone.

## Why

Coding agents are long-running, interactive processes — but the tools that spawn them die with your terminal or your app. Rubick Forge's core rule: **the UI is never responsible for keeping agents alive.** A launchd-supervised daemon owns the agents; tmux makes their sessions survive even daemon restarts; the desktop app is just a window onto them. Close the app, reboot the daemon, `tmux attach` from any terminal — the agents keep working.

## Where this is going

The roadmap, roughly in build order.

**The local control plane** — the core of it, and the part everything else hangs off:

- **Agents**: launch Claude Code or Codex per task, interactively, in tmux. Start/stop/restart, live terminal (xterm.js), send instructions, heuristic status (idle / working / waiting / error / stopped).
- **Projects & git**: register local repos; each task gets its own branch + git worktree — in `.forge-worktrees/` beside the repo, never inside it — or runs in the repo root when you choose; worktree status and cleanup from the UI.
- **Durability**: launchd keeps the daemon alive; the daemon re-adopts tmux sessions on restart. `tmux attach -t forge-<task>` is always an escape hatch.
- **Notifications**: native macOS pings when an agent is waiting for input, finishes, or errors — per-kind toggles, and silence for whatever is already on your screen.

**Then, in order:**

- **Remote machines**: every Mac runs the same daemon; the app merges them over Tailscale. No public exposure, ever.
- **Telegram**: `/status`, `/agents`, `/start`, `/stop`, `/ask` — plus notifications with terminal tails and inline approve/deny for permission prompts.
- **GitHub**: commit / push / open PR from a task's worktree, create tasks from tracker items, PR + CI status on the dashboard. Polling only, no webhooks.
- **Hub + plugins**: a task queue on your always-on Mac that dispatches to machines; new agent CLIs added via declarative TOML manifests and event-hook scripts — no plugin ABI.
- **Release**: packaging, docs, polish — the point where it becomes a daily driver.

## Architecture

```
Desktop app (Tauri, React)          Phone (Telegram, later)
        │  HTTP/WS + token                  │
        ▼                                   ▼
   forge-daemon  (Rust, launchd-supervised, SQLite)
        │
   tmux sessions ──► claude / codex CLIs
        │
   git worktrees per task
```

- `crates/forge-core` — shared domain types and events
- `crates/forge-daemon` — the daemon: HTTP/WS API, tmux, git, SQLite, adapters
- `apps/desktop` — Tauri 2 + React 19; strictly an API client

## Requirements

- macOS (Apple Silicon or Intel). Linux and Windows are out of scope.
- `tmux` 3.0 or newer, and `git`, on PATH
- The agent CLIs you want to drive: [Claude Code](https://docs.anthropic.com/en/docs/claude-code) and/or Codex, already authenticated

## Development

```sh
# daemon (dev mode, foreground)
cargo run -p forge-daemon -- --foreground

# desktop app
cd apps/desktop && npm install && npm run tauri dev
```

Checks, same as CI:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python3 scripts/check-d3-boundary.py

cd apps/desktop && npm run typecheck && npm test && npm run build
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the one architectural rule.
[docs/daemon.md](docs/daemon.md) covers the state directory, config, token and
launchd commands; [docs/runtime.md](docs/runtime.md) the tmux session model and
recovery; [docs/adapters.md](docs/adapters.md) how each agent CLI is launched
and read; [docs/app.md](docs/app.md) what the desktop app does and does not
hold; [docs/api.md](docs/api.md) the HTTP/WS surface;
[docs/remote.md](docs/remote.md) the tailnet bind and adding a second Mac;
[docs/telegram.md](docs/telegram.md) the bot; [docs/github.md](docs/github.md)
pull requests and CI status; [docs/hub.md](docs/hub.md) the task queue;
[docs/plugins.md](docs/plugins.md) adapter manifests and event hooks.

## Security posture

Local-first. The daemon binds loopback (and optionally your Tailscale interface) — never `0.0.0.0`. Bearer-token auth from day one; secrets in the macOS keychain; no inbound endpoints (GitHub is polled, Telegram long-polls outbound).

## License

[MIT](LICENSE)
