# GitHub

Close the loop: an issue becomes a task, the agent works, and the result is a
commit, a push and a pull request — then CI status comes back to the dashboard.

## Polling, never webhooks

A webhook is an inbound endpoint, and this project does not have one (SPEC
D23). Everything here is the daemon asking GitHub a question:

- Open pull requests on Forge branches are checked **every 60 seconds**.
- A task whose pull request is merged or closed stops being polled. It cannot
  change again, and polling it would spend budget on a settled answer.
- With no open pull requests, nothing is polled at all.

That costs no port, no public DNS, and no shared secret with GitHub beyond the
token you gave it.

### Staying inside the rate limit

Every poll is a **conditional request**: the daemon sends the ETag it saw last
time, and GitHub answers `304 Not Modified` when nothing has changed. A 304
costs no rate-limit budget, so a quiet repository is nearly free to watch.

The daemon also keeps **50 requests in reserve**. When the remaining budget
drops below that, polling pauses — so opening a pull request, which is
something you are waiting on, is never blocked by background polling having
spent everything. Polling resumes when the window resets.

While polling is paused the chips keep showing what they last knew. Hover one
to see when it was last checked: a stale green is only honest if it says how
old it is.

## Setting it up

Create a personal access token with the **`repo`** scope
([github.com/settings/tokens](https://github.com/settings/tokens)), then paste
it into **GitHub** on the dashboard.

The token is checked with GitHub before it is stored. A classic token missing
`repo` is refused at that point, with the scopes it does have named — better
than discovering it at the moment you try to open a pull request. A
fine-grained token reports no scopes at all, so it is accepted and proved by
its first real call.

It is kept in the **daemon's** keychain, not the app's: git runs daemon-side,
so that is where the credential is needed. The app never holds a copy.

If you already use `gh`, `gh auth token` prints a usable token.

## What a project needs

A project's repository is worked out from its `origin` remote — the
`git@github.com:owner/name.git`, `https://github.com/owner/name` and
`ssh://git@github.com/owner/name` spellings all work.

**A project with no GitHub remote shows no GitHub affordances and no errors.**
That includes repositories on other forges and purely local ones. Nothing is
broken; there is simply nothing to show.

GitHub Enterprise hosts are not supported: a different host is a different API.

## Issue to task

**Issues on owner/name…** on a project lists its open issues and filters them.
*Start task* on one creates a task and launches its agent:

| | |
|---|---|
| Task title | `#41 Flaky test on Tuesdays` |
| Branch | `forge/issue-41-flaky-test-on-tuesdays` |
| Prompt | The issue title and body |

The issue number is recorded, so the pull request opened later says `Closes
#41` and GitHub closes the issue on merge.

Issues are fetched only when you open the list. They cost a request each time,
and most sessions never look.

## Task to pull request

**Commit…** on a task with a worktree shows what would be committed — the file
list and the line counts — before committing it. This stages everything in the
worktree, and "everything" is only a safe default if you can see it.

- An empty commit is refused. It is not what pressing "commit" meant.
- Committing while the agent is still **working** warns first: it captures
  whatever half of a change is on disk.
- The default message is the task's title, and is editable.

**Push and open PR** pushes `forge/<slug>` and opens a pull request against the
task's base branch, with a body quoting what the agent was asked to do.

Nothing here ever rewrites history. There is no amend, no rebase, and no force
push; a push that would need one fails with git's own complaint for you to
resolve.

Committing and pushing are disabled for a task on another Mac — git runs where
the worktree is, so use that Mac's app, or its terminal over ssh.

## What comes back

Each polled pull request updates a chip on its task row: the pull request
number and state, and the rolled-up check state next to it.

**One failed check decides the commit.** A green run alongside a red one is not
passing, and a chip that said so would be the most misleading thing on the
dashboard. Cancelled and skipped runs are neither, and do not hold the answer
open.

Five events are recorded, and two of them notify:

| Event | Notifies |
|---|---|
| `pr_opened` | no — you just did it |
| `checks_passed` | no |
| `checks_failed` | **yes** |
| `pr_merged` | **yes** |
| `pr_closed` | no |

Both notifying kinds are the end of something you were waiting on, and both
happen while you are looking elsewhere. Each has its own toggle alongside the
agent ones.

Only *transitions* are announced. Checks that were already red stay quiet;
otherwise a minute-by-minute poll would report the same failure sixty times an
hour.

## What is deliberately not here

- **Webhooks.** Not now, not later.
- **Review comments in the app.** Read them on GitHub.
- **Auto-merge**, and any other action that lands code without you.
- **Non-GitHub forges.**
- **Merge-back into the base branch** (D7): the pull request is the handoff.
