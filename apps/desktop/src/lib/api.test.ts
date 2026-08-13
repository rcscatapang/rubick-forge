import { afterEach, describe, expect, it, vi } from "vitest";

import { ApiError, client } from "@/lib/api";

const connection = { url: "http://127.0.0.1:8787", token: "abc123" };

function respondWith(body: unknown, status = 200) {
  return vi.fn(async (_url: string, _init?: RequestInit) =>
    status === 204
      ? new Response(null, { status })
      : new Response(JSON.stringify(body), {
          status,
          headers: { "content-type": "application/json" },
        }),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the daemon client", () => {
  it("sends the token on every request", async () => {
    const fetchMock = respondWith({ projects: [] });
    vi.stubGlobal("fetch", fetchMock);

    await client(connection).projects();

    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe("http://127.0.0.1:8787/projects");
    expect((init.headers as Record<string, string>).authorization).toBe("Bearer abc123");
  });

  it("turns the daemon's error envelope into a readable failure", async () => {
    vi.stubGlobal(
      "fetch",
      respondWith({ error: { code: "conflict", message: "that repository is already registered" } }, 409),
    );

    await expect(client(connection).projects()).rejects.toMatchObject({
      code: "conflict",
      status: 409,
      message: "that repository is already registered",
    });
  });

  it("says the daemon is not answering rather than showing a network error", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("Failed to fetch");
      }),
    );

    const error = (await client(connection)
      .health()
      .catch((err: unknown) => err)) as ApiError;

    expect(error.unreachable).toBe(true);
    expect(error.message).toContain("not answering");
    expect(error.message).not.toContain("fetch");
  });

  it("handles a no-content response", async () => {
    vi.stubGlobal("fetch", respondWith(null, 204));

    await expect(client(connection).deleteTask(1)).resolves.toBeUndefined();
  });

  it("passes filters through as query parameters", async () => {
    const fetchMock = respondWith({ tasks: [] });
    vi.stubGlobal("fetch", fetchMock);

    await client(connection).tasks({ project: 2, status: "waiting" });

    const [url] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe("http://127.0.0.1:8787/tasks?project=2&status=waiting");
  });

  it("sends a json body when there is one", async () => {
    const fetchMock = respondWith({ id: 1 });
    vi.stubGlobal("fetch", fetchMock);

    await client(connection).createTask({
      project_id: 1,
      title: "Do the thing",
      adapter: "claude-code",
    });

    const [, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(init.method).toBe("POST");
    expect(JSON.parse(init.body as string).title).toBe("Do the thing");
  });
});
