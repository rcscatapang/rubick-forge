# Contributing

**Status: work in progress.** The repo is public from day one, but it is not
ready for daily use and the design is still moving. Questions and discussion are
welcome; large unsolicited PRs will probably collide with work in flight, so
start a conversation first.

## The one architectural rule

**The desktop app never touches agents, tmux, git, or SQLite.**

`forge-daemon` owns all of that and exposes it over HTTP/WS. `apps/desktop/src-tauri`
is a window shell: it opens a window, and later raises notifications and reads
the keychain. Concretely, a change is rejected if it adds a storage crate
(rusqlite, sled, …), a process-spawning crate, or a git crate to
`apps/desktop/src-tauri/Cargo.toml`. CI enforces this with
`scripts/check-d3-boundary.py`, which allowlists the shell's dependencies — so
adding one is a deliberate edit to that list, with a comment saying why, not a
quiet import.

Why: closing the app must change nothing about running agents. The daemon
(supervised by launchd) and tmux are the durability layers; the UI is a viewer.

## Development

```sh
# Rust workspace
cargo build --workspace
cargo test --workspace
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
python3 scripts/check-d3-boundary.py

# daemon (dev mode, foreground)
cargo run -p forge-daemon -- --foreground

# desktop app
cd apps/desktop && npm install
npm run tauri dev      # app window
npm run typecheck      # tsc -b (covers src and vite.config.ts)
npm test               # vitest
npm run build          # tsc -b && vite build
```

CI (GitHub Actions, macOS runner) runs exactly these checks. Clippy warnings are
denied.

## Conventions

- macOS only. No Linux/Windows branches.
- Domain types live in `crates/forge-core` and are mirrored by hand into
  `apps/desktop/src/lib/api-types.ts`. Change both in the same commit.
- Format Rust with `cargo fmt`; keep clippy clean.
