import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { PermissionContext } from "@/components/live-daemon";
import { NotificationSettings } from "@/components/notification-settings";
import { renderApp } from "@/test/harness";

/** A daemon whose settings are whatever the test says, and that records writes. */
function daemonWithSettings(initial: Record<string, string>) {
  const written: Record<string, string | null>[] = [];
  let settings = { ...initial };

  const fetchMock = vi.fn(async (input: string, init?: RequestInit) => {
    const path = new URL(input).pathname.slice(1);

    if (path === "settings" && init?.method === "PATCH") {
      const changes = JSON.parse(init.body as string) as Record<string, string | null>;
      written.push(changes);
      for (const [key, value] of Object.entries(changes)) {
        if (value === null) delete settings[key];
        else settings[key] = value;
      }
    }

    if (path === "settings") {
      return new Response(JSON.stringify({ settings }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    }

    return new Response(JSON.stringify({ tasks: [], projects: [] }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  });

  return { fetchMock, written };
}

afterEach(() => vi.unstubAllGlobals());

describe("notification settings", () => {
  it("is on for all three kinds until the daemon says otherwise", async () => {
    const { fetchMock } = daemonWithSettings({});
    vi.stubGlobal("fetch", fetchMock);

    renderApp(<NotificationSettings />);

    await waitFor(() =>
      expect(screen.getByLabelText("An agent needs you")).toHaveProperty("checked", true),
    );
    expect(screen.getByLabelText("A task finishes")).toHaveProperty("checked", true);
    expect(screen.getByLabelText("An agent errors")).toHaveProperty("checked", true);
  });

  it("reflects what the daemon has stored", async () => {
    const { fetchMock } = daemonWithSettings({ "notify.agent_waiting": "false" });
    vi.stubGlobal("fetch", fetchMock);

    renderApp(<NotificationSettings />);

    await waitFor(() =>
      expect(screen.getByLabelText("An agent needs you")).toHaveProperty("checked", false),
    );
    expect(screen.getByLabelText("A task finishes")).toHaveProperty("checked", true);
  });

  it("writes a change back to the daemon, so other clients see it", async () => {
    const { fetchMock, written } = daemonWithSettings({});
    vi.stubGlobal("fetch", fetchMock);

    renderApp(<NotificationSettings />);
    await waitFor(() => expect(screen.getByLabelText("A task finishes")).toBeTruthy());

    await userEvent.click(screen.getByLabelText("A task finishes"));

    await waitFor(() => expect(written).toEqual([{ "notify.task_finished": "false" }]));
    await waitFor(() =>
      expect(screen.getByLabelText("A task finishes")).toHaveProperty("checked", false),
    );
    // And only that kind changed.
    expect(screen.getByLabelText("An agent needs you")).toHaveProperty("checked", true);
  });

  it("complains about nothing while the permission is undecided", async () => {
    const { fetchMock } = daemonWithSettings({});
    vi.stubGlobal("fetch", fetchMock);

    renderApp(<NotificationSettings />);

    await waitFor(() => expect(screen.getByLabelText("An agent needs you")).toBeTruthy());
    expect(screen.queryByText(/System Settings/)).toBeNull();
  });

  it("says what to do about a refusal, and keeps working", async () => {
    const { fetchMock } = daemonWithSettings({});
    vi.stubGlobal("fetch", fetchMock);

    renderApp(
      <PermissionContext value="denied">
        <NotificationSettings />
      </PermissionContext>,
    );

    await waitFor(() => expect(screen.getByText(/System Settings/)).toBeTruthy());
    // The toggles are still there: the dashboard shows the same states either way.
    expect(screen.getByLabelText("An agent needs you")).toBeTruthy();
  });
});
