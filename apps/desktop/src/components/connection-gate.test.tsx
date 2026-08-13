import { screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ConnectionGate } from "@/components/connection-gate";
import { renderApp } from "@/test/harness";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

afterEach(() => vi.unstubAllGlobals());

describe("the connection gate", () => {
  it("shows the dashboard once the daemon answers", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(
            JSON.stringify({ version: "0.1.0", uptime_secs: 1, machine: "m", binaries: [] }),
            { status: 200, headers: { "content-type": "application/json" } },
          ),
      ),
    );

    renderApp(
      <ConnectionGate>
        <p>the dashboard</p>
      </ConnectionGate>,
    );

    await waitFor(() => expect(screen.getByText("the dashboard")).toBeTruthy());
  });

  it("offers a way out when the daemon is not there, rather than an empty dashboard", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("Failed to fetch");
      }),
    );

    renderApp(
      <ConnectionGate>
        <p>the dashboard</p>
      </ConnectionGate>,
    );

    await waitFor(() => expect(screen.getByText(/not answering/i)).toBeTruthy());
    expect(screen.queryByText("the dashboard")).toBeNull();
    expect(screen.getByRole("button", { name: /install the daemon/i })).toBeTruthy();
    // And the command to do it by hand, for anyone who would rather.
    expect(screen.getByText(/--install-launchd/)).toBeTruthy();
  });
});
