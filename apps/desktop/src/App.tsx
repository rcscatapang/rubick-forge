import { HashRouter, Route, Routes } from "react-router";
import { Toaster } from "sonner";

import { ConnectionGate } from "@/components/connection-gate";
import { LiveDaemon } from "@/components/live-daemon";
import { DaemonProvider } from "@/lib/connection";
import { DashboardPage } from "@/pages/dashboard";
import { SessionPage } from "@/pages/session";

export default function App() {
  return (
    <DaemonProvider>
      <HashRouter>
        <ConnectionGate>
          <LiveDaemon>
            <Routes>
              <Route path="/" element={<DashboardPage />} />
              <Route path="/sessions/:id" element={<SessionPage />} />
            </Routes>
          </LiveDaemon>
        </ConnectionGate>
        <Toaster position="bottom-right" />
      </HashRouter>
    </DaemonProvider>
  );
}
