import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ReactNode } from "react";

import { useEvents } from "@/hooks/use-events";
import { DaemonProvider } from "@/lib/connection";
import { keysFor } from "@/lib/queries";

/** A WebSocket a test can open, close and inspect. */
class FakeSocket {
  static opened: FakeSocket[] = [];

  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onmessage: ((message: { data: string }) => void) | null = null;
  closed = false;

  constructor(readonly url: string) {
    FakeSocket.opened.push(this);
  }

  close() {
    this.closed = true;
  }

  /** What the daemon end would do. */
  open() {
    this.onopen?.();
  }

  drop() {
    this.onclose?.();
  }
}

let queries: QueryClient;

function wrapper({ children }: { children: ReactNode }) {
  return (
    <DaemonProvider machineId="local" connection={{ url: "http://daemon.test", token: "t" }}>
      <QueryClientProvider client={queries}>{children}</QueryClientProvider>
    </DaemonProvider>
  );
}

beforeEach(() => {
  FakeSocket.opened = [];
  queries = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  vi.stubGlobal("WebSocket", FakeSocket);
  vi.useFakeTimers({ shouldAdvanceTime: true });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("staying subscribed to a machine", () => {
  it("carries the token, which a browser cannot put in a header", () => {
    renderHook(() => useEvents(), { wrapper });

    expect(FakeSocket.opened).toHaveLength(1);
    expect(FakeSocket.opened[0].url).toContain("token=t");
    expect(FakeSocket.opened[0].url.startsWith("ws://")).toBe(true);
  });

  it("refills the machine's data whenever the socket opens", async () => {
    const invalidate = vi.spyOn(queries, "invalidateQueries");
    renderHook(() => useEvents(), { wrapper });

    FakeSocket.opened[0].open();

    // Everything read while it was away is suspect, so the whole machine is
    // asked again rather than waiting for an event that may never come.
    expect(invalidate).toHaveBeenCalledWith({ queryKey: keysFor("local").all });
  });

  it("reconnects after a drop, backing off as it goes", async () => {
    renderHook(() => useEvents(), { wrapper });

    FakeSocket.opened[0].open();
    FakeSocket.opened[0].drop();

    // First retry is quick.
    await vi.advanceTimersByTimeAsync(500);
    expect(FakeSocket.opened).toHaveLength(2);

    // The second is not: a daemon that is down should not be hammered.
    FakeSocket.opened[1].drop();
    await vi.advanceTimersByTimeAsync(500);
    expect(FakeSocket.opened).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(500);
    expect(FakeSocket.opened).toHaveLength(3);
  });

  it("starts backing off from the beginning again once it gets through", async () => {
    renderHook(() => useEvents(), { wrapper });

    FakeSocket.opened[0].drop();
    await vi.advanceTimersByTimeAsync(500);
    FakeSocket.opened[1].drop();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(FakeSocket.opened).toHaveLength(3);

    // It connected, so the next drop is a fresh problem, not a continuing one.
    FakeSocket.opened[2].open();
    FakeSocket.opened[2].drop();
    await vi.advanceTimersByTimeAsync(500);
    expect(FakeSocket.opened).toHaveLength(4);
  });

  it("resumes from the last event it saw, so a gap is filled not skipped", async () => {
    renderHook(() => useEvents(), { wrapper });

    FakeSocket.opened[0].open();
    FakeSocket.opened[0].onmessage?.({
      data: JSON.stringify({
        id: 41,
        ts: "2026-08-14T10:00:00Z",
        kind: "task_created",
        task_id: 1,
        project_id: 1,
        title: "one",
      }),
    });
    FakeSocket.opened[0].drop();

    await vi.advanceTimersByTimeAsync(500);

    expect(FakeSocket.opened[1].url).toContain("after=41");
  });

  it("hands each event on, and stops when it goes away", async () => {
    const seen = vi.fn();
    const { unmount } = renderHook(() => useEvents(seen), { wrapper });

    FakeSocket.opened[0].open();
    FakeSocket.opened[0].onmessage?.({
      data: JSON.stringify({
        id: 1,
        ts: "2026-08-14T10:00:00Z",
        kind: "agent_waiting",
        task_id: 7,
        session_id: 3,
        tail: "Allow?",
      }),
    });

    await waitFor(() => expect(seen).toHaveBeenCalledTimes(1));

    unmount();
    expect(FakeSocket.opened[0].closed).toBe(true);
    // A closed socket must not schedule another attempt.
    FakeSocket.opened[0].drop();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(FakeSocket.opened).toHaveLength(1);
  });
});
