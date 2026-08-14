import { screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Project, Task } from "@/lib/api-types";
import { DashboardPage } from "@/pages/dashboard";
import { daemonReturning, renderApp } from "@/test/harness";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openPath: vi.fn() }));

function project(id: number, name: string): Project {
  return {
    id,
    name,
    path: `/repos/${name}`,
    default_branch: "main",
    adapter_settings: {},
    created_at: "2026-08-13T00:00:00Z",
    updated_at: "2026-08-13T00:00:00Z",
  };
}

function task(id: number, projectId: number, overrides: Partial<Task> = {}): Task {
  return {
    id,
    project_id: projectId,
    title: `Task ${id}`,
    adapter: "claude-code",
    base_branch: "main",
    branch: `forge/task-${id}`,
    worktree_path: null,
    initial_prompt: null,
    status: "idle",
    created_at: "2026-08-13T00:00:00Z",
    updated_at: "2026-08-13T00:00:00Z",
    ...overrides,
  };
}

function daemon(tasks: Task[]) {
  const routes: Record<string, unknown> = {
    health: { version: "0.1.0", uptime_secs: 1, machine: "mac-mini", binaries: [] },
    projects: { projects: [project(1, "forge"), project(2, "clockwerk")] },
    tasks: { tasks },
    adapters: { adapters: [] },
    events: { events: [], next_after: null },
  };

  for (const entry of tasks) {
    routes[`tasks/${entry.id}/sessions`] = { sessions: [] };
    routes[`tasks/${entry.id}/git`] = {
      branch: entry.branch,
      head: "abc1234",
      dirty: false,
      upstream: null,
      ahead: null,
      behind: null,
    };
  }

  return routes;
}

afterEach(() => vi.unstubAllGlobals());

describe("the dashboard", () => {
  it("groups tasks under the project they belong to", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(daemonReturning(daemon([task(1, 1), task(2, 2, { status: "working" })]))),
    );

    renderApp(<DashboardPage />);

    // By role, because a project's name is also an option in the new-task form.
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "forge" })).toBeTruthy(),
    );
    expect(screen.getByRole("heading", { name: "clockwerk" })).toBeTruthy();

    // Each task appears once, under its own project, with its own badge.
    expect(screen.getByText("Task 1")).toBeTruthy();
    expect(screen.getByText("Task 2")).toBeTruthy();
    await waitFor(() => expect(screen.getByText("Working")).toBeTruthy());
    expect(screen.getByText("Idle")).toBeTruthy();
  });

  it("puts what needs a human first, and only what needs one", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        daemonReturning(
          daemon([task(1, 1, { status: "working" }), task(2, 1, { status: "waiting" })]),
        ),
      ),
    );

    renderApp(<DashboardPage />);

    await waitFor(() => expect(screen.getByText(/needs you \(1\)/i)).toBeTruthy());
    // Once in its own section and once under its project.
    expect(screen.getAllByText("Task 2")).toHaveLength(2);
    expect(screen.getAllByText("Task 1")).toHaveLength(1);
  });

  it("says nothing needs a human when nothing does", async () => {
    vi.stubGlobal("fetch", vi.fn(daemonReturning(daemon([task(1, 1)]))));

    renderApp(<DashboardPage />);

    await waitFor(() => expect(screen.getByText("Task 1")).toBeTruthy());
    expect(screen.queryByText(/needs you/i)).toBeNull();
  });

  it("asks for a repository when there are none", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        daemonReturning({
          ...daemon([]),
          projects: { projects: [] },
        }),
      ),
    );

    renderApp(<DashboardPage />);

    await waitFor(() => expect(screen.getByText(/No repositories yet/)).toBeTruthy());
  });

  it("names a dependency the daemon says is missing", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        daemonReturning({
          ...daemon([]),
          health: {
            version: "0.1.0",
            uptime_secs: 1,
            machine: "mac-mini",
            binaries: [
              { name: "tmux", path: null, version: null, ok: false, detail: "not found" },
            ],
          },
        }),
      ),
    );

    renderApp(<DashboardPage />);

    await waitFor(() => expect(screen.getByText(/tmux unavailable/)).toBeTruthy());
  });
});
