import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render } from "@testing-library/react";
import type { ReactElement } from "react";
import { MemoryRouter } from "react-router";

import { DaemonProvider } from "@/lib/connection";
import { MachineRegistry } from "@/lib/machine-registry";

/**
 * Render a component the way the app does, against a daemon that is whatever
 * the test says it is.
 */
export function renderApp(ui: ReactElement) {
  // As if this Mac had connected before: the machines list resolves the local
  // token synchronously, so a test can assert without awaiting a keychain read.
  localStorage.setItem("forge.daemon.url", "http://daemon.test");
  localStorage.setItem("forge.daemon.token", "test-token");

  const queries = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });

  return render(
    <MachineRegistry>
      <DaemonProvider connection={{ url: "http://daemon.test", token: "test-token" }}>
        <QueryClientProvider client={queries}>
          <MemoryRouter>{ui}</MemoryRouter>
        </QueryClientProvider>
      </DaemonProvider>
    </MachineRegistry>,
  );
}

/** Answer specific paths, and fail loudly for anything unexpected. */
export function daemonReturning(routes: Record<string, unknown>) {
  return async (input: string) => {
    const path = new URL(input).pathname.slice(1);
    const body = routes[path];

    if (body === undefined) {
      return new Response(
        JSON.stringify({ error: { code: "not_found", message: `no stub for ${path}` } }),
        { status: 404, headers: { "content-type": "application/json" } },
      );
    }

    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  };
}
