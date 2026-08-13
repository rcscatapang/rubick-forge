import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from "react";

import { DEFAULT_DAEMON_URL, type DaemonConnection } from "@/lib/daemon";

interface Connected {
  connection: DaemonConnection;
  /** Adopt a new daemon or token, taking effect immediately. */
  connect: (connection: DaemonConnection) => void;
}

/**
 * Where this window's daemon is.
 *
 * The app holds no state of its own beyond this: everything else it shows is
 * read from the daemon.
 */
const DaemonContext = createContext<Connected | null>(null);

const TOKEN_KEY = "forge.daemon.token";
const URL_KEY = "forge.daemon.url";

/** What the browser has been told about the daemon, if anything. */
export function storedConnection(): DaemonConnection {
  return {
    url: localStorage.getItem(URL_KEY) ?? DEFAULT_DAEMON_URL,
    token: localStorage.getItem(TOKEN_KEY) ?? "",
  };
}

function remember(connection: DaemonConnection) {
  localStorage.setItem(URL_KEY, connection.url);
  localStorage.setItem(TOKEN_KEY, connection.token);
}

export function DaemonProvider({
  connection,
  children,
}: {
  connection?: DaemonConnection;
  children: ReactNode;
}) {
  const [current, setCurrent] = useState(() => connection ?? storedConnection());

  // State, not just storage: a token written to disk that the running app does
  // not pick up leaves it authenticating with the old one until it restarts.
  const connect = useCallback((next: DaemonConnection) => {
    remember(next);
    setCurrent(next);
  }, []);

  const value = useMemo(() => ({ connection: current, connect }), [current, connect]);

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

export function useConnect(): (connection: DaemonConnection) => void {
  return connected().connect;
}
