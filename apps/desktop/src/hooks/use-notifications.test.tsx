import { renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const sendNotification = vi.fn();
const isPermissionGranted = vi.fn(async () => true);
const requestPermission = vi.fn(async () => "granted" as string);

vi.mock("@tauri-apps/plugin-notification", () => ({
  sendNotification: (...args: unknown[]) => sendNotification(...args),
  isPermissionGranted: () => isPermissionGranted(),
  requestPermission: () => requestPermission(),
}));

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

import { useAskNotificationPermission, useNotifications } from "@/hooks/use-notifications";
import type { EventRecord } from "@/lib/api-types";
import { DaemonProvider } from "@/lib/connection";
import { daemonReturning } from "@/test/harness";

function wrapper({ children }: { children: ReactNode }) {
  const queries = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return (
    <DaemonProvider connection={{ url: "http://daemon.test", token: "t" }}>
      <QueryClientProvider client={queries}>{children}</QueryClientProvider>
    </DaemonProvider>
  );
}

function waiting(id: number): EventRecord {
  return { id, ts: "2026-08-13T00:00:00Z", kind: "agent_waiting", task_id: 7, session_id: 3, tail: "Allow?" };
}

function working(id: number): EventRecord {
  return {
    id,
    ts: "2026-08-13T00:00:00Z",
    kind: "status_changed",
    task_id: 7,
    session_id: 3,
    from: "waiting",
    to: "working",
  };
}

function stubDaemon() {
  vi.stubGlobal(
    "fetch",
    vi.fn(
      daemonReturning({
        tasks: { tasks: [] },
        projects: { projects: [] },
        settings: { settings: {} },
      }),
    ),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
  sendNotification.mockClear();
  isPermissionGranted.mockResolvedValue(true);
  requestPermission.mockResolvedValue("granted");
});

describe("raising notifications", () => {
  it("does not restack while an agent keeps asking unanswered", async () => {
    stubDaemon();
    const { result } = renderHook(() => useNotifications(null), { wrapper });
    await waitFor(() => expect(result.current).toBeInstanceOf(Function));

    result.current(waiting(1));
    result.current(waiting(2));
    result.current(waiting(3));

    expect(sendNotification).toHaveBeenCalledTimes(1);
  });

  it("notifies again once something else has happened to that task", async () => {
    stubDaemon();
    const { result } = renderHook(() => useNotifications(null), { wrapper });
    await waitFor(() => expect(result.current).toBeInstanceOf(Function));

    result.current(waiting(1));
    // The user answered, the agent got on with it, and now it is asking again.
    result.current(working(2));
    result.current(waiting(3));

    expect(sendNotification).toHaveBeenCalledTimes(2);
  });

  it("asks for permission once, and treats a dismissed prompt as undecided", async () => {
    stubDaemon();
    isPermissionGranted.mockResolvedValue(false);
    requestPermission.mockResolvedValue("default");

    const { result } = renderHook(() => useAskNotificationPermission());

    await waitFor(() => expect(requestPermission).toHaveBeenCalledTimes(1));
    expect(result.current).toBe("unknown");
  });

  it("reports an explicit refusal, so the app can explain it", async () => {
    stubDaemon();
    isPermissionGranted.mockResolvedValue(false);
    requestPermission.mockResolvedValue("denied");

    const { result } = renderHook(() => useAskNotificationPermission());

    await waitFor(() => expect(result.current).toBe("denied"));
  });
});
