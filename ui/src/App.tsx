import { SidebarNav } from "./components/layout/SidebarNav";
import { TitleBar } from "./components/layout/TitleBar";
import { useNav } from "./lib/nav";
import { GatewayPage } from "./pages/GatewayPage";
import { SyncPage } from "./pages/SyncPage";
import { McpPage } from "./pages/McpPage";
import { SkillsPage } from "./pages/SkillsPage";
import { ProvidersPage } from "./pages/ProvidersPage";
import { ModelsPage } from "./pages/ModelsPage";
import { StatsPage } from "./pages/StatsPage";
import { LogsPage } from "./pages/LogsPage";
import { SettingsPage } from "./pages/SettingsPage";

export default function App() {
  const { tab } = useNav();
  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <TitleBar />
      <div className="flex min-h-0 flex-1">
        <SidebarNav />
        <main className="min-w-0 flex-1 overflow-y-auto p-6">
          {tab === "gateway" && <GatewayPage />}
          {tab === "sync" && <SyncPage />}
          {tab === "mcp" && <McpPage />}
          {tab === "skills" && <SkillsPage />}
          {tab === "providers" && <ProvidersPage />}
          {tab === "models" && <ModelsPage />}
          {tab === "stats" && <StatsPage />}
          {tab === "logs" && <LogsPage />}
          {tab === "settings" && <SettingsPage />}
        </main>
      </div>
    </div>
  );
}
