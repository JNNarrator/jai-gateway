import { useEffect, useState } from "react";
import type { ToastKind } from "./lib/toast";
import type { Tab } from "./lib/nav";
import { GatewayPage } from "./pages/GatewayPage";
import { SyncPage } from "./pages/SyncPage";
import { McpPage } from "./pages/McpPage";
import { SkillsPage } from "./pages/SkillsPage";
import { ProvidersPage } from "./pages/ProvidersPage";
import { ModelsPage } from "./pages/ModelsPage";
import { StatsPage } from "./pages/StatsPage";
import { LogsPage } from "./pages/LogsPage";
import { SettingsPage } from "./pages/SettingsPage";
import { ThemeToggle } from "./components/layout/ThemeToggle";

export default function App() {
  const [tab, setTab] = useState<Tab>("gateway");
  const [toastMsg, setToastMsg] = useState<{ msg: string; kind: ToastKind } | null>(null);
  const tabs: [Tab, string][] = [
    ["gateway", "网关"], ["sync", "同步"], ["mcp", "MCP"], ["skills", "技能"],
    ["providers", "供应商"], ["models", "模型"], ["stats", "统计"],
    ["logs", "日志"], ["settings", "设置"],
  ];

  useEffect(() => {
    const onToast = (e: Event) => {
      const detail = (e as CustomEvent).detail as { msg: string; kind: ToastKind };
      setToastMsg({ msg: detail.msg, kind: detail.kind ?? "ok" });
      window.setTimeout(() => setToastMsg(null), 2500);
    };
    const onGoto = (e: Event) => {
      const detail = (e as CustomEvent).detail as { tab: string };
      setTab(detail.tab as Tab);
    };
    window.addEventListener("jai-toast", onToast);
    window.addEventListener("jai-goto-tab", onGoto);
    return () => {
      window.removeEventListener("jai-toast", onToast);
      window.removeEventListener("jai-goto-tab", onGoto);
    };
  }, []);

  return (
    <div className="flex h-screen flex-col bg-neutral-950 text-neutral-200">
      <header className="flex items-center gap-1 border-b border-neutral-800 px-4 py-2">
        <span className="mr-4 font-mono text-sm font-bold text-amber-500">JAI</span>
        <ThemeToggle />
        {tabs.map(([k, label]) => (
          <button
            key={k}
            onClick={() => setTab(k)}
            className={`rounded px-3 py-1.5 text-sm ${
              tab === k ? "bg-neutral-800 font-medium text-foreground" : "text-neutral-400 hover:text-neutral-200"
            }`}
          >
            {label}
          </button>
        ))}
      </header>
      <main className="flex-1 space-y-4 overflow-y-auto p-6">
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
      {toastMsg && (
        <div
          className={`fixed bottom-6 left-1/2 z-50 -translate-x-1/2 rounded-lg border px-4 py-2 text-sm shadow-lg ${
            toastMsg.kind === "err"
              ? "border-red-800 bg-red-950 text-red-200"
              : "border-emerald-800 bg-emerald-950 text-emerald-200"
          }`}
        >
          {toastMsg.msg}
        </div>
      )}
    </div>
  );
}
