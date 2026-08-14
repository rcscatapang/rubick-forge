import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

import { useEvents } from "@/hooks/use-events";
import { useNotifications, type PermissionState } from "@/hooks/use-notifications";

interface Watching {
  /** The task whose terminal is on screen, if any. */
  taskId: number | null;
  watch: (taskId: number | null) => void;
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
 * One event stream for the whole window, and the notifications it raises.
 *
 * Mounted once above the routes: a second subscription would mean a second
 * socket, and every notification twice.
 */
export function LiveDaemon({ children }: { children: ReactNode }) {
  const [taskId, setTaskId] = useState<number | null>(null);
  const focused = useWindowFocus();

  // Only suppress for a window the user is actually looking at. A backgrounded
  // window showing a terminal is exactly when a notification is wanted, so
  // focus has to be watched rather than sampled once.
  const watching = focused ? taskId : null;

  const { notify, permission } = useNotifications(watching);
  useEvents(notify);

  const value = useMemo(() => ({ taskId, watch: setTaskId }), [taskId]);

  return (
    <PermissionContext.Provider value={permission}>
      <WatchingContext.Provider value={value}>{children}</WatchingContext.Provider>
    </PermissionContext.Provider>
  );
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
