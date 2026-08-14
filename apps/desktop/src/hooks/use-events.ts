import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";

import type { EventRecord } from "@/lib/api-types";
import { useDaemon, useMachineId } from "@/lib/connection";
import { socketUrl } from "@/lib/daemon";
import { invalidateFor, keysFor } from "@/lib/queries";

/** How long to wait before reconnecting, per attempt. */
const RETRY_DELAYS_MS = [500, 1_000, 2_000, 5_000, 10_000];

/**
 * Keep the app's view of the daemon current, and hand each event on.
 *
 * Nothing in the UI polls: the daemon says what changed, and that invalidates
 * exactly what the change touched. Reconnecting resumes from the last id seen,
 * so a dropped socket loses nothing.
 */
export function useEvents(onEvent?: (event: EventRecord) => void) {
  const connection = useDaemon();
  const machine = useMachineId();
  const queries = useQueryClient();

  const notify = useRef(onEvent);
  useEffect(() => {
    notify.current = onEvent;
  }, [onEvent]);

  useEffect(() => {
    if (!connection.token) return;

    let disposed = false;
    let attempt = 0;
    let lastSeen: number | undefined;
    let socket: WebSocket | null = null;
    let retry: ReturnType<typeof setTimeout> | undefined;

    const connect = () => {
      if (disposed) return;

      socket = new WebSocket(socketUrl(connection, "ws/events", { after: lastSeen }));

      socket.onopen = () => {
        // Everything read while this machine was unreachable is suspect, and
        // a query that errored will not retry on its own. An open socket is
        // the moment its answers are worth asking for again — which is what
        // makes coming back automatic rather than a restart.
        attempt = 0;
        void queries.invalidateQueries({ queryKey: keysFor(machine).all });
      };

      socket.onmessage = (message) => {
        const event = JSON.parse(message.data as string) as EventRecord;
        lastSeen = event.id;

        invalidateFor(queries, machine, event.kind, "task_id" in event ? event.task_id : undefined);
        notify.current?.(event);
      };

      socket.onclose = () => {
        if (disposed) return;
        const delay = RETRY_DELAYS_MS[Math.min(attempt, RETRY_DELAYS_MS.length - 1)];
        attempt += 1;
        retry = setTimeout(connect, delay);
      };
    };

    connect();

    return () => {
      disposed = true;
      clearTimeout(retry);
      socket?.close();
    };
  }, [connection, machine, queries]);
}
