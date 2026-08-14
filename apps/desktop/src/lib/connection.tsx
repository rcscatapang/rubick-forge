import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from "react";

import {
  rememberConnection,
  storedConnection,
  type DaemonConnection,
} from "@/lib/daemon";
import { LOCAL_MACHINE_ID } from "@/lib/machines";

interface Connected {
  /** Which machine's daemon this subtree is talking to. */
  machineId: string;
  connection: DaemonConnection;
  /** Adopt a new daemon or token, taking effect immediately. */
  connect: (connection: DaemonConnection) => void;
}

/**
 * Which daemon this part of the tree is a view of.
 *
 * Scoped rather than global: with several machines, the dashboard renders one
 * subtree per machine and each reads its own daemon from here. Everything a
 * component shows still comes from a daemon — the app holds no state of its
 * own beyond where they are.
 */
const DaemonContext = createContext<Connected | null>(null);

export { storedConnection };

export function DaemonProvider({
  machineId = LOCAL_MACHINE_ID,
  connection,
  children,
}: {
  machineId?: string;
  /** Supplied by the machines list. Omitted, the provider owns the local one. */
  connection?: DaemonConnection;
  children: ReactNode;
}) {
  const [own, setOwn] = useState(storedConnection);
  const given = connection !== undefined;
  const local = machineId === LOCAL_MACHINE_ID;

  // State, not just storage: a token written to disk that the running app does
  // not pick up leaves it authenticating with the old one until it restarts.
  // A provider that was handed a connection is showing the machines list's
  // idea of that machine, so it writes the token down and lets the list
  // notice; keeping state of its own here would be a write nothing reads.
  const connect = useCallback(
    (next: DaemonConnection) => {
      if (local) rememberConnection(next);
      if (!given) setOwn(next);
    },
    [local, given],
  );

  const current = connection ?? own;
  const value = useMemo(
    () => ({ machineId, connection: current, connect }),
    [machineId, current, connect],
  );

  return <DaemonContext.Provider value={value}>{children}</DaemonContext.Provider>;
}

function connected(): Connected {
  const value = useContext(DaemonContext);
  if (!value) {
    throw new Error("useDaemon must be used inside a DaemonProvider");
  }
  return value;
}

export function useDaemon(): DaemonConnection {
  return connected().connection;
}

/** Which machine the surrounding subtree belongs to. */
export function useMachineId(): string {
  return connected().machineId;
}

export function useConnect(): (connection: DaemonConnection) => void {
  return connected().connect;
}
