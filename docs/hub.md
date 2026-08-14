# Hub mode

A queue on your always-on Mac that hands work to whichever machine can do it.
"Run this" stops caring which Mac runs it.

Hub mode is a **flag on an ordinary daemon**, not a separate program. The Mac
that is the hub is also still a worker.

## What the workers know

Nothing. There is no registration, no heartbeat, no agent to install.

The hub reaches other machines over exactly the API the desktop app uses, and
always outbound — nothing ever connects *to* the hub. A worker cannot tell it is
in a fleet, and does not need to.

That also means a hub going away costs you the queue and nothing else. The app
still talks to every machine directly.

## Turning it on

The hub needs the machines list from [telegram.md](telegram.md) — the same
`[[machines]]` entries, with each machine's token in the keychain. Then:

```toml
hub = true

[[machines]]
name = "Mac mini"
url = "http://100.101.102.103:8787"
token_ref = "mac-mini-token"
```

`hub = true` with no `[[machines]]` is refused at load: a queue with nowhere to
send anything is a mistake, not a configuration.

## Queueing work

From the dashboard's **Queue** section, or from Telegram.

On the hub, `/start <project> <what to do>` **queues** rather than running
locally — a phone cannot say which Mac, and choosing one is what the queue is
for. `/queue` lists what is on it and `/cancel <n>` takes a row off. On any
other machine `/start` runs the task there, because there is no queue to put it
on.

A queued row is **not a task**. It has no branch, no worktree and no session —
it becomes a task on whichever machine takes it, and the queue then remembers
only where it went.

| | |
|---|---|
| **Project** | Matched by **name** across machines, never by path |
| **Title** | What the task will be called |
| **Prompt** | What the agent should do |
| **Machine** | One you name, or *Any machine* |
| **Adapter** | Which agent CLI to run; defaults to Claude Code |

Naming a machine is the dashboard's job. There is no chat syntax for it,
because `/start on mini forge …` cannot be told apart from a project called
`on`.

## How a machine is chosen

Every 15 seconds the hub asks each machine what projects it has and how many
sessions it is running, then for each waiting row:

1. **A named machine is honoured or refused, never redirected.** Someone who
   said "on the Mac mini" did not mean "anywhere". If that Mac does not have the
   project, the row waits and says so.
2. Otherwise, every reachable machine with a project of that **name** is a
   candidate.
3. **Fewest active sessions wins.** Ties go to the earlier machine, so a fleet
   doing nothing places predictably rather than arbitrarily.

That is the whole heuristic. There are no priorities, no dependencies between
rows, and no per-machine limits. It is not a scheduler and is not meant to
become one.

### Matching by name is your model to manage

Two machines with a project called `forge` are two machines that can run `forge`
work — even if they are different repositories. The hub cannot tell, and will
not guess.

That is why every placement records **which machines were considered**, shown on
the row and in the `task_dispatched` event. If work lands somewhere surprising,
the row says what the alternatives were.

### Nothing is ever cloned

A project has to exist on a machine before work can go there. The hub will not
clone a repository onto a Mac to make a row placeable, now or later — that is
the difference between a queue and a build farm.

A row nobody can take **stays queued with a reason**:

```
#3 Fix the flaky test · waiting — no machine has a project called forge.
                                 Register it on one.
```

Register it on a machine and the next pass picks it up. Nothing needs
restarting.

A machine that is merely asleep says so instead, because that leads somewhere
different:

```
#3 Fix the flaky test · waiting — no machine that answered has a project
                                 called forge (Mac mini not answering)
```

## Dispatching exactly once

Creating the task and recording where it went are two steps, and a crash — or
just a lost response — can land between them. The row is then still `queued`,
and the next pass tries it again.

Every dispatch therefore names its attempt. The hub sends
`idempotency_key: "forge-queue-<row id>"` with `POST /tasks`, and **any** daemon
receiving a create under a key it has already used returns the task it made the
first time rather than making a second one. The retry gets the original task
back and records that.

That key is deliberately generic. A worker honouring it learns nothing about
hubs or queues — it is the ordinary "this request is safe to repeat" contract,
and any client with a flaky connection can use it.

The queue row is also claimed with a conditional update, so it only moves to
`dispatched` while it is still `queued`. That covers a second dispatcher racing
the first; the key covers the crash.

## Events

| Event | When |
|---|---|
| `task_queued` | Something was added to the queue |
| `task_dispatched` | It became a task on a machine, with the machines considered |
| `dispatch_failed` | A machine refused it; the row stays queued |

All three reach the dashboard's activity feed and the Telegram bot. None of them
notify: a queue that pinged you for every placement would be a queue you stopped
reading.

A row that cannot be placed records its reason but does **not** publish an
event, or an unplaceable row would emit one every 15 seconds forever.

## One merged view

`GET /fleet` on the hub returns every machine with its projects and how busy it
is. It exists for a client that cannot hold several connections — a phone.

The desktop app does not use it. It keeps its own per-machine connections, which
is what lets it carry on when the hub is down.

## Cancelling

A **waiting** row can be cancelled. A **dispatched** one cannot: the task is
real and running on another Mac, so stopping it is that Mac's business — use
`/stop` or the task's own controls.

Cancelling publishes no event, so a dashboard open on another machine sees it on
its next read rather than immediately.

## What is deliberately not here

- **Repo syncing.** See "nothing is ever cloned".
- **Priorities and dependencies** between queued rows. The queue is
  first-in-first-out over what is placeable.
- **Per-machine parallelism limits** beyond "fewest sessions wins".
- **Anything worker-initiated.** Workers do not ask for work, report in, or
  know the hub exists.
