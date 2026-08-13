import { screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { TaskRow } from "@/components/task-row";
import type { Task } from "@/lib/api-types";
import { daemonReturning, renderApp } from "@/test/harness";

function task(overrides: Partial<Task> = {}): Task {
  return {
    id: 7,
    project_id: 1,
    title: "Add adapters",
    adapter: "claude-code",
    base_branch: "main",
    branch: "forge/add-adapters-7",
    worktree_path: "/repos/.forge-worktrees/forge/add-adapters-7",
    initial_prompt: null,
    status: "stopped",
    created_at: "2026-08-13T00:00:00Z",
    updated_at: "2026-08-13T00:00:00Z",
    ...overrides,
  };
}

const noSessions = {
  "tasks/7/sessions": { sessions: [] },
  "tasks/7/git": {
    branch: "forge/add-adapters-7",
    head: "abc1234",
    dirty: false,
    upstream: null,
    ahead: null,
    behind: null,
  },
};

afterEach(() => vi.unstubAllGlobals());

describe("a task row", () => {
  it("offers to start a stopped task, not to stop it", async () => {
    vi.stubGlobal("fetch", vi.fn(daemonReturning(noSessions)));
    renderApp(<TaskRow task={task()} />);

    expect(screen.getByRole("button", { name: "Start" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
  });

  it("offers to stop and restart a running one", async () => {
    vi.stubGlobal("fetch", vi.fn(daemonReturning(noSessions)));
    renderApp(<TaskRow task={task({ status: "working" })} />);

    expect(screen.getByRole("button", { name: "Stop" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Restart" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Start" })).toBeNull();
  });

  it("will not clean up or delete a task that is still running", async () => {
    vi.stubGlobal("fetch", vi.fn(daemonReturning(noSessions)));
    renderApp(<TaskRow task={task({ status: "waiting" })} />);

    expect(screen.getByRole("button", { name: /clean up/i })).toHaveProperty("disabled", true);
    expect(screen.getByRole("button", { name: "Delete" })).toHaveProperty("disabled", true);
  });

  it("links to the terminal only once there is a session to watch", async () => {
    vi.stubGlobal("fetch", vi.fn(daemonReturning(noSessions)));
    const { unmount } = renderApp(<TaskRow task={task({ status: "working" })} />);
    await waitFor(() => expect(screen.queryByRole("link", { name: "Terminal" })).toBeNull());
    unmount();

    vi.stubGlobal(
      "fetch",
      vi.fn(
        daemonReturning({
          ...noSessions,
          "tasks/7/sessions": {
            sessions: [
              {
                id: 3,
                task_id: 7,
                tmux_name: "forge-7",
                pid: 42,
                status: "working",
                started_at: "2026-08-13T00:00:00Z",
                ended_at: null,
              },
            ],
          },
        }),
      ),
    );
    renderApp(<TaskRow task={task({ status: "working" })} />);

    await waitFor(() =>
      expect(screen.getByRole("link", { name: "Terminal" }).getAttribute("href")).toBe(
        "/sessions/3",
      ),
    );
  });

  it("shows uncommitted work without having to be asked", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        daemonReturning({
          ...noSessions,
          "tasks/7/git": {
            branch: "forge/add-adapters-7",
            head: "abc1234",
            dirty: true,
            upstream: "origin/main",
            ahead: 2,
            behind: 0,
          },
        }),
      ),
    );
    renderApp(<TaskRow task={task()} />);

    await waitFor(() => expect(screen.getByText(/uncommitted changes/)).toBeTruthy());
    expect(screen.getByText(/2 to push/)).toBeTruthy();
  });
});
