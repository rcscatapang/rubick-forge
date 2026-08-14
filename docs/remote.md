# Remote machines

Every Mac runs the same daemon. The desktop app holds a connection to each and
merges them into one dashboard.

No daemon knows about any other. There is no daemon-to-daemon traffic, no
cluster, and no shared database — the list of machines belongs to the app, and
each daemon only ever answers for its own Mac.

## The security model

- A daemon binds **loopback always**, and the tailnet **only when asked**.
  `0.0.0.0` is not offered and is refused at config load.
- Remote reachability is tailnet membership. Tailscale (WireGuard) is the
  transport encryption; Forge ships no TLS of its own and has no accounts.
- Every daemon still has its own bearer token. Being on the tailnet gets you to
  the door; the token opens it.
- The app keeps remote tokens in the **macOS keychain**, one entry per machine
  under the service `tech.cloverly.rubick-forge`. The machines list itself is
  plain: name, address, optional ssh host. Nothing secret is in it, so a copy
  of it in a bug report costs nothing.

## Turning on the tailnet bind

On the Mac you want to reach, in `daemon.toml`:

```toml
tailscale_bind = true
```

`true` looks up this Mac's Tailscale address — an address in `100.64.0.0/10`,
or Tailscale's `fd7a:115c:a1e0::/48` if there is no IPv4 one — on its own
interfaces. Nothing is dialled to find it, and no `tailscale` binary is needed.

If that guess is wrong, name the address instead:

```toml
tailscale_bind = "100.101.102.103"
```

Either way loopback stays. The app on that Mac keeps reaching its own daemon at
`127.0.0.1:8787` whether Tailscale is up, down, or not installed.

Restart the daemon and check what it bound:

```sh
launchctl kickstart -k gui/$(id -u)/tech.cloverly.rubick-forge.daemon
lsof -nP -iTCP -sTCP:LISTEN | grep forge-daemon
```

You should see exactly two rows — `127.0.0.1:8787` and the tailnet address —
and never `*:8787`.

Tailscale not being up yet is not a config error: the address is resolved when
the daemon starts serving, not when the file is read, because launchd starts
the daemon at login and Tailscale may still be connecting.

## Adding a machine to the app

Get the token from the Mac you want to add:

```sh
cat ~/Library/Application\ Support/rubick-forge/token
```

Then, in **Machines** on the dashboard, choose *Add a machine…* and give it:

| | |
|---|---|
| **Name** | Whatever you call that Mac |
| **Address** | `http://<tailscale-address>:8787` |
| **Token** | The token above — it goes straight to the keychain |
| **ssh host** | Optional; enables “Terminal over ssh” |

*Test* asks that daemon `/health`, which is the one unauthenticated route, then
tries an authenticated one. That separates "nothing there" from "wrong token",
which are different problems with different fixes.

Editing a machine leaves its stored token alone unless you type a new one, so
renaming does not mean retyping a secret. Removing a machine forgets its
keychain entry too.

## What the dashboard does with them

- Each machine gets its own section, its own event socket, and its own
  reconnect. A Mac that is asleep shows **unreachable** on its own chip and
  costs nothing else on the screen; it recovers on its own when it comes back.
- Task ids are per-daemon, so everything is scoped by machine: two Macs both
  having a task 1 is normal and they never collide.
- The activity feed is the one merged view, ordered by the daemons' own UTC
  timestamps and badged with the machine. Two Macs whose clocks disagree will
  interleave by however much they disagree.
- A machine whose daemon is a different minor version than the app shows a
  **version mismatch** chip rather than failing in unexplained ways.
- Registering a repository and opening a worktree in Finder are disabled for a
  remote machine: a folder picker on this Mac cannot choose a path on another
  one. Register repositories on the Mac they live on.

## The ssh escape hatch

ssh is not a transport here — the app talks to every daemon over HTTP. It is
one convenience (SPEC D18). With an ssh host set on a machine, a running task
offers **Terminal over ssh**, which opens Terminal on:

```sh
ssh <host> -t tmux attach -t forge-<task-id>
```

That is the same session the in-app terminal attaches to, and the same one you
would reach by hand. Both the host and the session name are checked against
what they are allowed to be before they go anywhere near a command line: a
destination that is not a hostname is refused rather than quoted.

## What is deliberately not here

- **Any daemon-to-daemon traffic.** Machines do not discover, sync, or dispatch
  to each other. A task queue that hands work between them is the hub, in
  v0.5.
- **Machine rows in the daemon's database.** The list stays app-side until the
  Telegram bot needs a copy of it in v0.3.
- **Public exposure.** There is no mode that binds a public interface, and
  there will not be one.
