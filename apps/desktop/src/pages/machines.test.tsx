import { screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openPath: vi.fn() }));

import { localMachine, rememberMachines } from "@/lib/machines";
import { DashboardPage } from "@/pages/dashboard";
import { renderApp } from "@/test/harness";

const REMOTE_URL = "http://100.101.102.103:8787";

const remote = {
  id: "machine-1",
  name: "Mac mini",
  url: REMOTE_URL,
  sshHost: "mac-mini",
};

/** A daemon on `daemon.test` that works, and one on the tailnet that is asleep. */
function oneMachineDown() {
  return vi.fn(async (input: string) => {
    const url = new URL(input);

    if (url.origin === REMOTE_URL) {
      throw new Error("the Mac mini is asleep");
    }

    const bodies: Record<string, unknown> = {
      "/health": { version: "0.1.0", uptime_secs: 1, machine: "This Mac", binaries: [] },
      "/projects": { projects: [] },
      "/tasks": { tasks: [] },
      "/adapters": { adapters: [] },
      "/events": { events: [], next_after: null },
      "/settings": { settings: {} },
    };

    const body = bodies[url.pathname];
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

describe("a dashboard with more than one machine", () => {
  it("names each machine once there is more than one to tell apart", async () => {
    rememberMachines([localMachine(), remote]);
    vi.stubGlobal("fetch", oneMachineDown());

    renderApp(<DashboardPage />);

    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "Mac mini" })).toBeInTheDocument(),
    );
    expect(screen.getByRole("heading", { name: "This Mac" })).toBeInTheDocument();
  });

  it("keeps the working machine live while the other is unreachable", async () => {
    rememberMachines([localMachine(), remote]);
    vi.stubGlobal("fetch", oneMachineDown());

    renderApp(<DashboardPage />);

    // The one that is down says so...
    await waitFor(() => expect(screen.getByText("unreachable")).toBeInTheDocument());
    // ...and the one that is up still rendered everything it has.
    expect(screen.getByText(/daemon 0\.1\.0/)).toBeInTheDocument();
    expect(screen.getAllByText(/No repositories yet/)).toHaveLength(2);
  });

  it("says a machine refused the token, on that machine only", async () => {
    rememberMachines([localMachine(), remote]);
    // `/health` needs no token, so this machine looks reachable and then
    // refuses everything else — the case the chip alone cannot show.
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string) => {
        const url = new URL(input);
        const unauthorised = url.origin === REMOTE_URL && url.pathname !== "/health";

        const bodies: Record<string, unknown> = {
          "/health": { version: "0.1.0", uptime_secs: 1, machine: "Mac mini", binaries: [] },
          "/projects": { projects: [] },
          "/tasks": { tasks: [] },
          "/adapters": { adapters: [] },
          "/events": { events: [], next_after: null },
          "/settings": { settings: {} },
        };

        if (unauthorised) {
          return new Response(
            JSON.stringify({ error: { code: "unauthorized", message: "no" } }),
            { status: 401, headers: { "content-type": "application/json" } },
          );
        }

        return new Response(JSON.stringify(bodies[url.pathname] ?? {}), {
          status: 200,
          headers: { "content-type": "application/json" },
        });
      }),
    );

    renderApp(<DashboardPage />);

    await waitFor(() =>
      expect(screen.getByText(/rejected the app.s token/)).toBeInTheDocument(),
    );
    // Exactly one machine complained.
    expect(screen.getAllByText(/rejected the app.s token/)).toHaveLength(1);
  });

  it("labels nothing when this Mac is the only machine", async () => {
    vi.stubGlobal("fetch", oneMachineDown());

    renderApp(<DashboardPage />);

    await waitFor(() => expect(screen.getByText(/daemon 0\.1\.0/)).toBeInTheDocument());
    expect(screen.queryByRole("heading", { name: "This Mac" })).not.toBeInTheDocument();
  });
});
