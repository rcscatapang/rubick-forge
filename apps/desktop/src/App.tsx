import { HashRouter, Route, Routes } from "react-router";
import { Toaster } from "sonner";

import { DaemonProvider } from "@/lib/connection";
import { DashboardPage } from "@/pages/dashboard";
import { SessionPage } from "@/pages/session";

export default function App() {
  return (
    <DaemonProvider>
      <HashRouter>
        <Routes>
          <Route path="/" element={<DashboardPage />} />
          <Route path="/sessions/:id" element={<SessionPage />} />
        </Routes>
        <Toaster position="bottom-right" />
      </HashRouter>
    </DaemonProvider>
  );
}
