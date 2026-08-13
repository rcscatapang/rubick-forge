import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { NewTaskForm } from "@/components/new-task";
import type { Project } from "@/lib/api-types";
import { daemonReturning, renderApp } from "@/test/harness";

const project: Project = {
  id: 1,
  name: "forge",
  path: "/repos/forge",
  default_branch: "main",
  adapter_settings: {},
  created_at: "2026-08-13T00:00:00Z",
  updated_at: "2026-08-13T00:00:00Z",
};

function adapters(claudeInstalled: boolean) {
  return {
    adapters: [
      {
        id: "claude-code",
        name: "Claude Code",
        binary: claudeInstalled
          ? { name: "claude", path: "/bin/claude", version: "2", ok: true, detail: null }
          : {
              name: "claude",
              path: null,
              version: null,
              ok: false,
              detail: "`claude` was not found on PATH",
            },
        settings: [],
      },
    ],
  };
}

afterEach(() => vi.unstubAllGlobals());

describe("the new task form", () => {
  it("will not submit without a title", async () => {
    vi.stubGlobal("fetch", vi.fn(daemonReturning({ adapters: adapters(true) })));
    renderApp(<NewTaskForm projects={[project]} />);

    await waitFor(() => expect(screen.getByText("Give the task a title.")).toBeTruthy());
    expect(screen.getByRole("button", { name: /create task/i })).toHaveProperty("disabled", true);
  });

  it("says so before submitting when the agent is not installed", async () => {
    vi.stubGlobal("fetch", vi.fn(daemonReturning({ adapters: adapters(false) })));
    renderApp(<NewTaskForm projects={[project]} />);

    await userEvent.type(screen.getByLabelText("Title"), "Do the thing");

    await waitFor(() =>
      expect(screen.getByText("`claude` was not found on PATH")).toBeTruthy(),
    );
    expect(screen.getByRole("button", { name: /create task/i })).toHaveProperty("disabled", true);
  });

  it("asks for a worktree and an immediate start by default", async () => {
    vi.stubGlobal("fetch", vi.fn(daemonReturning({ adapters: adapters(true) })));
    renderApp(<NewTaskForm projects={[project]} />);

    expect(screen.getByLabelText(/its own worktree/i)).toHaveProperty("checked", true);
    expect(screen.getByLabelText(/start straight away/i)).toHaveProperty("checked", true);
  });

  it("tells the user to register a project when there are none", async () => {
    vi.stubGlobal("fetch", vi.fn(daemonReturning({ adapters: adapters(true) })));
    renderApp(<NewTaskForm projects={[]} />);

    expect(screen.getByText("Register a project first.")).toBeTruthy();
  });
});
