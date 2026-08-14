import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

import { useEvents } from "@/hooks/use-events";
import {
  useAskNotificationPermission,
  useNotifications,
  type PermissionState,
} from "@/hooks/use-notifications";
import { DaemonProvider } from "@/lib/connection";
import { useMachines } from "@/lib/machine-registry";

/** Which task's terminal is on screen, and on which machine. */
export interface Watched {
  machineId: string;
  taskId: number;
}

interface Watching {
  watched: Watched | null;
  watch: (watched: Watched | null) => void;
}

const WatchingContext = createContext<Watching | null>(null);

/**
 * Whether macOS is letting the app notify.
 *
 * Has a default rather than requiring a provider, so a component rendered on
 * its own shows no complaint instead of throwing.
 */
export const PermissionContext = createContext<PermissionState>("unknown");

/**
 * One event stream per machine, and the notifications they raise.
 *
 * Mounted once above the routes. Each machine gets its own socket with its own
 * reconnect, because a Mac that has gone to sleep must not stop the others'
 * events arriving.
 */
export function LiveDaemon({ children }: { children: ReactNode }) {
  const { machines } = useMachines();
  const [watched, setWatched] = useState<Watched | null>(null);
  const focused = useWindowFocus();
  const permission = useAskNotificationPermission();

  // Only suppress for a window the user is actually looking at. A backgrounded
  // window showing a terminal is exactly when a notification is wanted, so
  // focus has to be watched rather than sampled once.
  const visible = focused ? watched : null;

  const value = useMemo(() => ({ watched, watch: setWatched }), [watched]);

  return (
    <PermissionContext.Provider value={permission}>
      <WatchingContext.Provider value={value}>
        {machines.map(({ machine, connection }) => (
          <DaemonProvider key={machine.id} machineId={machine.id} connection={connection}>
            <MachineStream
              watching={visible?.machineId === machine.id ? visible.taskId : null}
            />
          </DaemonProvider>
        ))}
        {children}
      </WatchingContext.Provider>
    </PermissionContext.Provider>
  );
}

/** One machine's events, feeding its query cache and its notifications. */
function MachineStream({ watching }: { watching: number | null }) {
  const notify = useNotifications(watching);
  useEvents(notify);
  return null;
}

/** Whether this window has focus, kept current. */
function useWindowFocus(): boolean {
  const [focused, setFocused] = useState(() => document.hasFocus());

  useEffect(() => {
    const gained = () => setFocused(true);
    const lost = () => setFocused(false);

    window.addEventListener("focus", gained);
    window.addEventListener("blur", lost);

    return () => {
      window.removeEventListener("focus", gained);
      window.removeEventListener("blur", lost);
    };
  }, []);

  return focused;
}

/** Tell the app which task's terminal is on screen. */
export function useWatching(): Watching {
  const value = useContext(WatchingContext);
  if (!value) {
    throw new Error("useWatching must be used inside a LiveDaemon");
  }
  return value;
}

export function useNotificationPermission(): PermissionState {
  return useContext(PermissionContext);
}
