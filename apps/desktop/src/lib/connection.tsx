import { createContext, useContext, useMemo, type ReactNode } from "react";

import { DEFAULT_DAEMON_URL, type DaemonConnection } from "@/lib/daemon";

/**
 * Where this window's daemon is.
 *
 * The app holds no state of its own beyond this: everything else it shows is
 * read from the daemon.
 */
const DaemonContext = createContext<DaemonConnection | null>(null);

const TOKEN_KEY = "forge.daemon.token";
const URL_KEY = "forge.daemon.url";

/** What the browser has been told about the daemon, if anything. */
export function storedConnection(): DaemonConnection {
  return {
    url: localStorage.getItem(URL_KEY) ?? DEFAULT_DAEMON_URL,
    token: localStorage.getItem(TOKEN_KEY) ?? "",
  };
}

export function rememberConnection(connection: DaemonConnection) {
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
  const value = useMemo(() => connection ?? storedConnection(), [connection]);

  return <DaemonContext.Provider value={value}>{children}</DaemonContext.Provider>;
}

export function useDaemon(): DaemonConnection {
  const connection = useContext(DaemonContext);
  if (!connection) {
    throw new Error("useDaemon must be used inside a DaemonProvider");
  }
  return connection;
}
