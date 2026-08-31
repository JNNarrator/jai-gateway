import { useState } from "react";
import {
  BarChart3,
  Boxes,
  Building2,
  ChevronsLeft,
  ChevronsRight,
  Puzzle,
  Radio,
  RefreshCw,
  ScrollText,
  Settings,
  Sparkles,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { useNav, type Tab } from "@/lib/nav";
import { ThemeToggle } from "./ThemeToggle";

const NAV: [Tab, string, typeof Radio][] = [
  ["gateway", "网关", Radio],
  ["sync", "同步", RefreshCw],
  ["mcp", "MCP", Puzzle],
  ["skills", "技能", Sparkles],
  ["providers", "供应商", Building2],
  ["models", "模型", Boxes],
  ["stats", "统计", BarChart3],
  ["logs", "日志", ScrollText],
  ["settings", "设置", Settings],
];

export function SidebarNav() {
  const { tab, setTab } = useNav();
  const [collapsed, setCollapsed] = useState(
    () => localStorage.getItem("jai-sidebar-collapsed") === "1",
  );

  const toggleCollapsed = () => {
    setCollapsed((c) => {
      localStorage.setItem("jai-sidebar-collapsed", c ? "0" : "1");
      return !c;
    });
  };

  return (
    <aside
      className={cn(
        "flex shrink-0 flex-col border-r border-border bg-card transition-[width] duration-200",
        collapsed ? "w-14" : "w-44",
      )}
    >
      <div
        className={cn(
          "flex h-12 items-center border-b border-border",
          collapsed ? "justify-center" : "px-4",
        )}
      >
        <span className="font-mono text-sm font-bold text-primary">
          {collapsed ? "J" : "JAI"}
        </span>
      </div>

      <nav className="flex-1 space-y-1 overflow-y-auto p-2">
        {NAV.map(([k, label, Icon]) => (
          <button
            key={k}
            onClick={() => setTab(k)}
            title={collapsed ? label : undefined}
            aria-label={label}
            className={cn(
              "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-sm transition-colors",
              collapsed && "justify-center",
              tab === k
                ? "bg-accent font-medium text-accent-foreground"
                : "text-muted-foreground hover:bg-muted hover:text-foreground",
            )}
          >
            <Icon className="size-4 shrink-0" />
            {!collapsed && <span>{label}</span>}
          </button>
        ))}
      </nav>

      <div
        className={cn(
          "flex items-center border-t border-border p-2",
          collapsed ? "flex-col gap-2" : "justify-between",
        )}
      >
        <ThemeToggle />
        <button
          onClick={toggleCollapsed}
          aria-label={collapsed ? "展开侧边栏" : "折叠侧边栏"}
          className="rounded-md p-1.5 text-muted-foreground hover:bg-muted hover:text-foreground"
        >
          {collapsed ? (
            <ChevronsRight className="size-4" />
          ) : (
            <ChevronsLeft className="size-4" />
          )}
        </button>
      </div>
    </aside>
  );
}
