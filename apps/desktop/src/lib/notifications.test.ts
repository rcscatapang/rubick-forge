import { describe, expect, it } from "vitest";

import type { EventRecord, Project, Task } from "@/lib/api-types";
import {
  DEFAULT_PREFS,
  isRepeat,
  notificationFor,
  prefKey,
  sanitise,
  type NotifiedKind,
} from "@/lib/notifications";

const project: Project = {
  id: 1,
  name: "forge",
  path: "/repos/forge",
  default_branch: "main",
  adapter_settings: {},
  created_at: "2026-08-13T00:00:00Z",
  updated_at: "2026-08-13T00:00:00Z",
};

const task: Task = {
  id: 7,
  project_id: 1,
  title: "Add adapters",
  adapter: "claude-code",
  base_branch: "main",
  branch: "forge/add-adapters-7",
  worktree_path: null,
  initial_prompt: null,
  status: "waiting",
  created_at: "2026-08-13T00:00:00Z",
  updated_at: "2026-08-13T00:00:00Z",
};

const context = { tasks: [task], projects: [project], prefs: DEFAULT_PREFS };

function waiting(tail: string): EventRecord {
  return {
    id: 1,
    ts: "2026-08-13T00:00:00Z",
    kind: "agent_waiting",
    task_id: 7,
    session_id: 3,
    tail,
  };
}

describe("sanitising a pane tail", () => {
  it("strips the escape sequences a terminal is made of", () => {
    const raw = "\x1b[38;2;255;106;193mDo you want to edit src/main.rs?\x1b[39m";

    expect(sanitise(raw)).toBe("Do you want to edit src/main.rs?");
  });

  it("drops the box drawing and blank filler of a redrawing pane", () => {
    const raw = "\u2502 Do you want to proceed?  \u2502\n\u2500\u2500\u2500\u2500\n\n   \n\u2502 1. Yes \u2502";

    expect(sanitise(raw)).toBe("Do you want to proceed? \u00b7 1. Yes");
  });

  it("caps the length rather than handing the system a screenful", () => {
    const sanitised = sanitise("word ".repeat(200));

    expect(sanitised.length).toBeLessThanOrEqual(241);
    expect(sanitised.endsWith("\u2026")).toBe(true);
  });

  it("strips the escape sequences a colour code is not", () => {
    // Mode reports and private sequences, which a real pane sends plenty of.
    const raw = "\x1b[>4;2m\x1b[=7h\x1b[?2004hReady when you are\x1b[0m";

    expect(sanitise(raw)).toBe("Ready when you are");
  });

  it("survives a tail with nothing readable in it", () => {
    expect(sanitise("\x1b[2J\x1b[H")).toBe("");
    expect(sanitise("")).toBe("");
  });
});

describe("deciding what to notify about", () => {
  it("names the task and its project", () => {
    const notification = notificationFor(waiting("Allow edit?"), context);

    expect(notification?.title).toBe("Add adapters \u2014 forge");
    expect(notification?.body).toBe("Allow edit?");
    expect(notification?.taskId).toBe(7);
  });

  it("says something even when the tail is unreadable", () => {
    expect(notificationFor(waiting("\x1b[2J"), context)?.body).toBe("Waiting for you.");
  });

  it("ignores everything that is not one of the three signals", () => {
    const noise: EventRecord = {
      id: 2,
      ts: "2026-08-13T00:00:00Z",
      kind: "status_changed",
      task_id: 7,
      session_id: 3,
      from: "idle",
      to: "working",
    };

    expect(notificationFor(noise, context)).toBeNull();
  });

  it("respects a kind the user turned off, and only that kind", () => {
    const prefs = { ...DEFAULT_PREFS, agent_waiting: false };

    expect(notificationFor(waiting("Allow edit?"), { ...context, prefs })).toBeNull();
    expect(
      notificationFor(
        { id: 3, ts: "2026-08-13T00:00:00Z", kind: "task_finished", task_id: 7, session_id: 3 },
        { ...context, prefs },
      ),
    ).not.toBeNull();
  });

  it("stays quiet about the task already on screen", () => {
    expect(notificationFor(waiting("Allow edit?"), { ...context, watching: 7 })).toBeNull();
    expect(notificationFor(waiting("Allow edit?"), { ...context, watching: 8 })).not.toBeNull();
  });

  it("falls back to the task id when it knows nothing about it", () => {
    const notification = notificationFor(waiting("Allow edit?"), {
      ...context,
      tasks: [],
      projects: [],
    });

    expect(notification?.title).toBe("Task 7");
  });
});

describe("an errored agent", () => {
  it("says what went wrong", () => {
    const errored: EventRecord = {
      id: 4,
      ts: "2026-08-13T00:00:00Z",
      kind: "agent_error",
      task_id: 7,
      session_id: 3,
      detail: "\x1b[31mAPI Error: overloaded\x1b[0m",
    };

    const notification = notificationFor(errored, context);

    expect(notification?.kind).toBe("agent_error");
    expect(notification?.body).toBe("API Error: overloaded");
    expect(notification?.title).toBe("Add adapters \u2014 forge");
  });

  it("says something even when the detail is unreadable", () => {
    const errored: EventRecord = {
      id: 5,
      ts: "2026-08-13T00:00:00Z",
      kind: "agent_error",
      task_id: 7,
      session_id: 3,
      detail: "",
    };

    expect(notificationFor(errored, context)?.body).toBe("The agent errored.");
  });
});

describe("a finished task", () => {
  it("says so plainly", () => {
    const finished: EventRecord = {
      id: 6,
      ts: "2026-08-13T00:00:00Z",
      kind: "task_finished",
      task_id: 7,
      session_id: 3,
    };

    expect(notificationFor(finished, context)?.body).toBe("Finished.");
  });
});

describe("not restacking", () => {
  it("treats the same kind for the same task as the same interruption", () => {
    const seen = new Map<number, NotifiedKind>();
    const notification = notificationFor(waiting("Allow edit?"), context)!;

    expect(isRepeat(notification, seen)).toBe(false);
    seen.set(notification.taskId, notification.kind);
    expect(isRepeat(notification, seen)).toBe(true);
  });

  it("lets a different kind through, and another task's", () => {
    const seen = new Map<number, NotifiedKind>([[7, "agent_waiting"]]);

    expect(isRepeat({ kind: "agent_error", taskId: 7, title: "", body: "" }, seen)).toBe(false);
    expect(isRepeat({ kind: "agent_waiting", taskId: 8, title: "", body: "" }, seen)).toBe(false);
  });
});

describe("what GitHub is worth interrupting for", () => {
  it("announces a failed check with the pull request it failed on", () => {
    const failed: EventRecord = {
      id: 7,
      ts: "2026-08-14T10:00:00Z",
      kind: "checks_failed",
      task_id: 7,
      number: 41,
      url: "https://github.com/o/n/pull/41",
    };

    const notification = notificationFor(failed, context);

    expect(notification?.body).toBe("Checks failed on PR #41.");
    expect(notification?.title).toBe("Add adapters \u2014 forge");
  });

  it("announces a merge, which is the end of the job", () => {
    const merged: EventRecord = {
      id: 8,
      ts: "2026-08-14T10:00:00Z",
      kind: "pr_merged",
      task_id: 7,
      number: 41,
      url: "https://github.com/o/n/pull/41",
    };

    expect(notificationFor(merged, context)?.body).toBe("PR #41 was merged.");
  });

  it("says nothing about a pull request opening, which you just did", () => {
    const opened: EventRecord = {
      id: 9,
      ts: "2026-08-14T10:00:00Z",
      kind: "pr_opened",
      task_id: 7,
      number: 41,
      url: "https://github.com/o/n/pull/41",
    };
    const passed: EventRecord = { ...opened, id: 10, kind: "checks_passed" };

    expect(notificationFor(opened, context)).toBeNull();
    expect(notificationFor(passed, context)).toBeNull();
  });

  it("respects a GitHub kind turned off without touching the agent ones", () => {
    const prefs = { ...DEFAULT_PREFS, checks_failed: false };
    const failed: EventRecord = {
      id: 11,
      ts: "2026-08-14T10:00:00Z",
      kind: "checks_failed",
      task_id: 7,
      number: 41,
      url: "https://github.com/o/n/pull/41",
    };

    expect(notificationFor(failed, { ...context, prefs })).toBeNull();
    expect(notificationFor(waiting("Allow?"), { ...context, prefs })).not.toBeNull();
  });
});

describe("preferences", () => {
  it("are all on to begin with", () => {
    expect(Object.values(DEFAULT_PREFS).every(Boolean)).toBe(true);
  });

  it("are stored under a key the daemon owns, so other clients share them", () => {
    expect(prefKey("agent_waiting")).toBe("notify.agent_waiting");
  });
});
