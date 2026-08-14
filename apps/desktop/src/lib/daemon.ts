/**
 * Where the daemon is, and how to address it.
 *
 * The app is a client and nothing else: every URL it builds points at the
 * daemon's HTTP/WS API.
 */

/** Matches the daemon's own default; see docs/daemon.md. */
export const DEFAULT_DAEMON_URL = "http://127.0.0.1:8787";

export interface DaemonConnection {
  /** Base URL, without a trailing slash. */
  url: string;
  token: string;
}

/** An absolute API URL, with any query parameters appended. */
export function apiUrl(
  connection: Pick<DaemonConnection, "url">,
  path: string,
  params: Record<string, string | number | boolean | undefined> = {},
): string {
  const url = new URL(path, `${connection.url}/`);

  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined) {
      url.searchParams.set(key, String(value));
    }
  }

  return url.toString();
}

/**
 * A WebSocket URL carrying the token.
 *
 * Browsers cannot set headers on a handshake, which is the one place the
 * daemon accepts a token in the query string.
 */
export function socketUrl(
  connection: DaemonConnection,
  path: string,
  params: Record<string, string | number | boolean | undefined> = {},
): string {
  const url = new URL(
    apiUrl(connection, path, { ...params, token: connection.token }),
  );
  url.protocol = url.protocol.replace(/^http/, "ws");
  return url.toString();
}
