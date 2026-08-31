## Task 6: 左侧可折叠边栏

**Files:**
- Create: `ui/src/components/layout/SidebarNav.tsx`
- Modify: `ui/src/App.tsx`（外壳换侧边栏布局）、`ui/src/components/layout/ThemeToggle.tsx`（不动，Task 3 已建）

**Interfaces:**
- Consumes: Task 3 的 `ThemeToggle`、Task 5 的 `useNav`
- Produces: `SidebarNav`（9 项 lucide 图标 + 文字、可折叠为纯图标、折叠态存 `localStorage["jai-sidebar-collapsed"]`）；App 外壳变为 `flex h-screen`（侧边栏 + 主区）

- [ ] **Step 1: 创建 SidebarNav.tsx**

`ui/src/components/layout/SidebarNav.tsx`:

```tsx
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
```

- [ ] **Step 2: App.tsx 换侧边栏布局**

`ui/src/App.tsx` 全文替换为:

```tsx
import { SidebarNav } from "./components/layout/SidebarNav";
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
    <div className="flex h-screen bg-background text-foreground">
      <SidebarNav />
      <main className="flex-1 overflow-y-auto p-6">
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
  );
}
```

（删除 `useState`/`useEffect`/`tabs` 数组/ThemeToggle 临时挂载与旧 header。）

- [ ] **Step 3: 构建 + 布局冒烟**

```bash
pnpm --dir ui build
```

Expected: 零错误。dev 下检查：
- 展开态（w-44）：9 项图标+文字完整，当前页高亮（accent 底）
- 折叠态（w-14）：纯图标、hover 出 title，主题切换器仍在底部
- 折叠状态刷新后保持（localStorage）
- 980×640 与 760px 最小宽度下均不换行、无横向滚动

- [ ] **Step 4: Commit**

```bash
git add ui/src/components/layout ui/src/App.tsx
git commit -m "feat(ui): 左侧可折叠边栏导航（替换顶栏页签）"
```

---

