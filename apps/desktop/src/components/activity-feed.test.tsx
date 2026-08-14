import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

import { ActivityFeed } from "@/components/activity-feed";
import type { EventRecord } from "@/lib/api-types";
import { localMachine, rememberMachines } from "@/lib/machines";
import { renderApp } from "@/test/harness";

const REMOTE_URL = "http://100.101.102.103:8787";

const remote = { id: "machine-1", name: "Mac mini", url: REMOTE_URL, sshHost: null };

function created(id: number, ts: string, title: string): EventRecord {
  return { id, ts, kind: "task_created", task_id: id, project_id: 1, title };
}

/** Each machine's own history, oldest-first the way the daemon pages it. */
function twoMachines(here: EventRecord[], there: EventRecord[]) {
  return vi.fn(async (input: string) => {
    const url = new URL(input);
    const events = url.origin === REMOTE_URL ? there : here;

    const bodies: Record<string, unknown> = {
      "/events": { events, next_after: null },
      "/health": { version: "0.1.0", uptime_secs: 1, machine: "who", binaries: [] },
      "/settings": { settings: {} },
    };

    return new Response(JSON.stringify(bodies[url.pathname] ?? {}), {
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

describe("the activity feed", () => {
  it("interleaves both machines by timestamp, newest first", async () => {
    rememberMachines([localMachine(), remote]);
    vi.stubGlobal(
      "fetch",
      twoMachines(
        [created(1, "2026-08-14T10:00:00Z", "oldest"), created(2, "2026-08-14T10:02:00Z", "third")],
        [created(1, "2026-08-14T10:01:00Z", "second"), created(2, "2026-08-14T10:03:00Z", "newest")],
      ),
    );

    renderApp(<ActivityFeed />);

    await waitFor(() => expect(screen.getByText(/newest/)).toBeInTheDocument());

    const rows = screen.getAllByRole("listitem").map((row) => row.textContent);
    expect(rows).toHaveLength(4);
    // Ordered across machines, not concatenated per machine.
    expect(rows[0]).toContain("newest");
    expect(rows[1]).toContain("third");
    expect(rows[2]).toContain("second");
    expect(rows[3]).toContain("oldest");
  });

  it("badges each entry with the machine it came from", async () => {
    rememberMachines([localMachine(), remote]);
    vi.stubGlobal(
      "fetch",
      twoMachines(
        [created(1, "2026-08-14T10:00:00Z", "here")],
        [created(1, "2026-08-14T10:01:00Z", "there")],
      ),
    );

    renderApp(<ActivityFeed />);

    await waitFor(() => expect(screen.getByText(/there/)).toBeInTheDocument());

    const [first, second] = screen.getAllByRole("listitem");
    expect(within(first).getByText("Mac mini")).toBeInTheDocument();
    expect(within(second).getByText("This Mac")).toBeInTheDocument();
  });

  it("badges nothing when there is only one machine to be from", async () => {
    vi.stubGlobal("fetch", twoMachines([created(1, "2026-08-14T10:00:00Z", "here")], []));

    renderApp(<ActivityFeed />);

    await waitFor(() => expect(screen.getByText(/here/)).toBeInTheDocument());
    expect(screen.queryByText("This Mac")).not.toBeInTheDocument();
  });

  it("shows the machine that answered while the other is unreachable", async () => {
    rememberMachines([localMachine(), remote]);
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string) => {
        const url = new URL(input);
        if (url.origin === REMOTE_URL) throw new Error("asleep");

        return new Response(
          JSON.stringify({
            events: [created(1, "2026-08-14T10:00:00Z", "still here")],
            next_after: null,
          }),
          { status: 200, headers: { "content-type": "application/json" } },
        );
      }),
    );

    renderApp(<ActivityFeed />);

    await waitFor(() => expect(screen.getByText(/still here/)).toBeInTheDocument());
  });
});
