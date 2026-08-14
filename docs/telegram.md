# The Telegram bot

Fleet control from a phone. One Mac's daemon — the always-on one — hosts a bot
that answers for itself and for every machine in its config (SPEC D21, the
"lite hub").

## The security posture

- **Outbound only.** The daemon long-polls Telegram; Telegram never calls in.
  There is no webhook and no inbound port. Nothing about the bot is reachable
  from the internet.
- **Allowlisted user ids.** Anyone can find a bot and message it. Only the
  Telegram user ids you list get a reply — everyone else is dropped with a log
  line and *no* response, because answering would confirm the bot exists.
- **Enabling it without an allowlist is a config error.** The daemon refuses to
  start rather than run a bot anyone can drive.
- **Secrets in the keychain.** The bot token and each remote machine's bearer
  token live in the login keychain. `daemon.toml` holds only the account names
  to look them up by.

## Setting it up

### 1. Make a bot

Message [@BotFather](https://t.me/botfather), send `/newbot`, and keep the
token it gives you. Put it in the keychain:

```sh
security add-generic-password -U \
  -s tech.cloverly.rubick-forge -a telegram-bot -w '123456:AA…'
```

### 2. Find your user id

Message [@userinfobot](https://t.me/userinfobot). It replies with your numeric
id. That number, not your @handle, is what the allowlist holds — handles change
and ids do not.

### 3. Configure the daemon

In `daemon.toml` on the Mac that should host the bot:

```toml
[telegram]
enabled = true
allowed_user_ids = [123456789]
token_ref = "telegram-bot"     # the keychain account from step 1
```

Restart it, and message the bot `/help`.

## Speaking for other Macs

The hosting daemon reaches other machines over exactly the API the desktop app
uses. Add each one and put its bearer token in the keychain:

```sh
# On the other Mac
cat ~/Library/Application\ Support/rubick-forge/token

# On the hosting Mac
security add-generic-password -U \
  -s tech.cloverly.rubick-forge -a mac-mini-token -w '<that token>'
```

```toml
[[machines]]
name = "Mac mini"
url = "http://100.101.102.103:8787"
token_ref = "mac-mini-token"
```

Those machines need `tailscale_bind` on — see [remote.md](remote.md). They do
not need any Telegram configuration, and they are not told they are in anyone's
fleet: still no daemon-to-daemon protocol, just one daemon using another's
public API.

The hosting daemon also follows each machine's `/ws/events`, so a remote agent
asking a question raises a push with working buttons, not just a line in the
next `/status`. Each stream reconnects on its own and resumes from the last
event it saw, so a Mac that drops off the tailnet replays what was missed
rather than losing it.

A machine that is asleep costs its own line of a reply and nothing else.

## Commands

| | |
|---|---|
| `/status` | One line per machine: how many tasks, how many need you |
| `/agents` | Every running agent, with its state, project and machine |
| `/start <project> <what to do>` | Create a task and launch its agent |
| `/stop <task>` | Stop a task's agent |
| `/ask <task> <text>` | Type into a running agent, and show what it did |

A project or task can be named by id, by its exact name, or by any prefix that
matches only one thing. **Ambiguity is never guessed** — the bot lists the
candidates and waits. Task ids are per-daemon, so the same number on two
machines is ambiguous like any other duplicate; say the name instead.

`/start` takes the first few words of your prompt as the task title, uses the
project's default branch, makes a worktree, and runs Claude Code. A task whose
agent will not start is deleted again rather than left sitting there.

## Notifications and the buttons on them

The three signal kinds are pushed as they happen: an agent is **waiting**, a
task **finished**, an agent **errored**. Each carries the end of the pane, with
escape sequences and box drawing stripped and the length capped.

A `waiting` push carries **Approve** and **Deny** buttons that send the
adapter's own keys for that dialog.

A button lives in the chat forever; the question does not. Each button
therefore names the exact prompt it belongs to — machine, task, session, and
the id of the event that raised it, all inside Telegram's 64 bytes, so the
binding survives a daemon restart with no state to keep. A tap only lands if
that session is still on that question. Otherwise it says so and **sends
nothing**: if you answered at the keyboard and the agent has moved on, a stray
"yes" would answer a question nobody asked.

Answering also takes the buttons off the message, so scrollback cannot be
tapped twice.

## Known limitations

- **`/start` always uses Claude Code.** There is no way to name an adapter in a
  chat command, so it takes the default.
- **The bot messages allowlisted users directly.** It has no group behaviour
  beyond answering commands addressed to it.
