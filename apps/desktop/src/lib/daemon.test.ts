import { describe, expect, it } from "vitest";

import { apiUrl, DEFAULT_DAEMON_URL, socketUrl } from "@/lib/daemon";

const connection = { url: DEFAULT_DAEMON_URL, token: "abc123" };

describe("daemon urls", () => {
  it("builds an api url with query parameters", () => {
    expect(apiUrl(connection, "tasks", { project: 1, status: "working" })).toBe(
      "http://127.0.0.1:8787/tasks?project=1&status=working",
    );
  });

  it("leaves out parameters that were not given", () => {
    expect(apiUrl(connection, "tasks", { project: undefined })).toBe(
      "http://127.0.0.1:8787/tasks",
    );
  });

  it("carries the token on a websocket url, since a handshake has no headers", () => {
    const url = new URL(socketUrl(connection, "ws/sessions/7/terminal", { cols: 100 }));

    expect(url.protocol).toBe("ws:");
    expect(url.pathname).toBe("/ws/sessions/7/terminal");
    expect(url.searchParams.get("token")).toBe("abc123");
    expect(url.searchParams.get("cols")).toBe("100");
  });

  it("keeps a secure daemon secure", () => {
    expect(
      new URL(socketUrl({ url: "https://mac-mini:8787", token: "t" }, "ws/events")).protocol,
    ).toBe("wss:");
  });
});
