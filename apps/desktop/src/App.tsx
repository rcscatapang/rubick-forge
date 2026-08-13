import { HashRouter, Route, Routes } from "react-router";
import { Toaster } from "sonner";

import { DashboardPage } from "@/pages/dashboard";

export default function App() {
  return (
    <HashRouter>
      <Routes>
        <Route path="/" element={<DashboardPage />} />
      </Routes>
      <Toaster position="bottom-right" />
    </HashRouter>
  );
}
