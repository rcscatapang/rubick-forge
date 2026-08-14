import { screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

import { HubQueue } from "@/components/hub-queue";
import type { Project, QueuedTask } from "@/lib/api-types";
import { renderApp } from "@/test/harness";

function project(name: string): Project {
  return {
    id: 1,
    name,
    path: `/repos/${name}`,
    default_branch: "main",
    adapter_settings: {},
    created_at: "2026-08-14T10:00:00Z",
    updated_at: "2026-08-14T10:00:00Z",
  };
}

function row(overrides: Partial<QueuedTask> = {}): QueuedTask {
  return {
    id: 3,
    project_name: "forge",
    adapter: "claude-code",
    title: "Fix the flaky test",
    prompt: null,
    target: null,
    state: "queued",
    reason: null,
    machine: null,
    remote_task: null,
    considered: [],
    created_at: "2026-08-14T10:00:00Z",
    updated_at: "2026-08-14T10:00:00Z",
    ...overrides,
  };
}

/** A daemon that is or is not a hub, with a queue of `rows`. */
function daemon({ hub, rows = [] }: { hub: boolean; rows?: QueuedTask[] }) {
  return vi.fn(async (input: string) => {
    const path = new URL(input).pathname;

    const bodies: Record<string, unknown> = {
      "/hub": { hub, machines: hub ? ["This Mac", "Mac mini"] : [] },
      "/hub/queue": { queue: rows },
    };

    const body = bodies[path];
    if (body === undefined) {
      return new Response(JSON.stringify({ error: { code: "not_found", message: "no" } }), {
        status: 404,
        headers: { "content-type": "application/json" },
      });
    }

    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  });
}

beforeEach(() => {
  localStorage.clear();
  invoke.mockReset();
  invoke.mockResolvedValue("token");
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the hub's queue", () => {
  it("shows nothing at all on a machine that is not the hub", async () => {
    // Every worker's queue is empty by construction; a section saying so on
    // each of them would be noise.
    const fetch = daemon({ hub: false });
    vi.stubGlobal("fetch", fetch);

    const { container } = renderApp(<HubQueue projects={[project("forge")]} />);

    await waitFor(() => expect(fetch).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });

  it("offers the queue on the hub, and says what it is for", async () => {
    vi.stubGlobal("fetch", daemon({ hub: true }));

    renderApp(<HubQueue projects={[project("forge")]} />);

    await waitFor(() => expect(screen.getByText("Queue")).toBeInTheDocument());
    expect(screen.getByText(/Nothing is cloned anywhere/)).toBeInTheDocument();
    expect(screen.getByText("Nothing on the queue.")).toBeInTheDocument();
  });

  it("lets a row be aimed at one machine, or at none", async () => {
    vi.stubGlobal("fetch", daemon({ hub: true }));

    renderApp(<HubQueue projects={[project("forge")]} />);

    await waitFor(() => expect(screen.getByText("Queue")).toBeInTheDocument());
    expect(screen.getByRole("option", { name: "Any machine" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Mac mini" })).toBeInTheDocument();
  });

  it("says why a waiting row is still waiting", async () => {
    vi.stubGlobal(
      "fetch",
      daemon({
        hub: true,
        rows: [row({ reason: "no machine has a project called forge" })],
      }),
    );

    renderApp(<HubQueue projects={[]} />);

    await waitFor(() =>
      expect(screen.getByText("no machine has a project called forge")).toBeInTheDocument(),
    );
    expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument();
  });

  it("shows where a dispatched row went, and does not offer to cancel it", async () => {
    // The task is real and lives on another Mac; stopping it is that Mac's job.
    vi.stubGlobal(
      "fetch",
      daemon({
        hub: true,
        rows: [row({ state: "dispatched", machine: "Mac mini", remote_task: 12 })],
      }),
    );

    renderApp(<HubQueue projects={[]} />);

    const listed = await waitFor(() => screen.getByRole("listitem"));

    expect(listed).toHaveTextContent("Mac mini");
    expect(listed).toHaveTextContent("task 12");
    expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();
  });

  it("records which machines could have taken it", async () => {
    vi.stubGlobal(
      "fetch",
      daemon({
        hub: true,
        rows: [
          row({
            state: "dispatched",
            machine: "Mac mini",
            remote_task: 12,
            considered: ["Mac mini", "Laptop"],
          }),
        ],
      }),
    );

    renderApp(<HubQueue projects={[]} />);

    await waitFor(() =>
      expect(screen.getByText(/considered Mac mini, Laptop/)).toBeInTheDocument(),
    );
  });

  it("says so when the queue cannot be read", async () => {
    // The hub answered that it is one, then refused the queue — a token that
    // lost its permissions, or a daemon part-way through a restart.
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string) => {
        if (new URL(input).pathname === "/hub") {
          return new Response(JSON.stringify({ hub: true, machines: ["This Mac"] }), {
            status: 200,
            headers: { "content-type": "application/json" },
          });
        }
        return new Response(
          JSON.stringify({ error: { code: "unauthorized", message: "no" } }),
          { status: 401, headers: { "content-type": "application/json" } },
        );
      }),
    );

    renderApp(<HubQueue projects={[]} />);

    await waitFor(() =>
      expect(screen.getByText(/hub is not answering/)).toBeInTheDocument(),
    );
    // And it says the rest of the app still works.
    expect(screen.getByText(/driven directly/)).toBeInTheDocument();
  });

  it("shows nothing when the hub daemon cannot be reached at all", async () => {
    // A hub that is down answers nothing, so the app cannot know it was one.
    // The rest of the dashboard carries on against each machine directly.
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new Error("the hub is asleep");
      }),
    );

    const { container } = renderApp(<HubQueue projects={[]} />);

    await waitFor(() => expect(container).toBeEmptyDOMElement());
  });
});
