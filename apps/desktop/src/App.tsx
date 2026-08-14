import { HashRouter, Route, Routes } from "react-router";
import { Toaster } from "sonner";

import { ConnectionGate } from "@/components/connection-gate";
import { LiveDaemon } from "@/components/live-daemon";
import { DaemonProvider } from "@/lib/connection";
import { MachineRegistry } from "@/lib/machine-registry";
import { DashboardPage } from "@/pages/dashboard";
import { SessionPage } from "@/pages/session";

export default function App() {
  return (
    <MachineRegistry>
      {/* The local daemon is the one the gate is about: without it there is no
          install to repair and no token to read. Remote machines are added
          from inside, once there is an inside. */}
      <DaemonProvider>
        <HashRouter>
          <ConnectionGate>
            <LiveDaemon>
              <Routes>
                <Route path="/" element={<DashboardPage />} />
                <Route path="/machines/:machineId/sessions/:id" element={<SessionPage />} />
              </Routes>
            </LiveDaemon>
          </ConnectionGate>
          <Toaster position="bottom-right" />
        </HashRouter>
      </DaemonProvider>
    </MachineRegistry>
  );
}
