## Task 1: 机械拆分 App.tsx（纯搬移，视觉不变）

**Files:**
- Create: `ui/src/lib/toast.ts`、`ui/src/lib/clipboard.ts`、`ui/src/lib/format.ts`、`ui/src/lib/nav.ts`
- Create: `ui/src/components/common/legacy.tsx`
- Create: `ui/src/pages/{GatewayPage,SyncPage,McpPage,SkillsPage,ProvidersPage,ModelsPage,StatsPage,LogsPage,SettingsPage}.tsx`
- Modify: `ui/src/App.tsx` → 瘦身为外壳（保留顶栏页签 UI 不动）

**Interfaces:**
- Consumes: 现有 `ui/src/App.tsx`（2433 行，18 个顶层函数）
- Produces:
  - `lib/toast.ts`: `type ToastKind = "ok" | "err"`；`toast(msg: string, kind?: ToastKind): void`（内部仍用 CustomEvent，Task 4 换实现）
  - `lib/nav.ts`: `type Tab`（9 个页签 union）；`goTab(tab: Tab): void`（内部仍用 CustomEvent，Task 5 换实现）
  - `lib/format.ts`: `fmtClock(ms: number | null | undefined): string`
  - `lib/clipboard.ts`: `copyText(text: string): void`
  - `components/common/legacy.tsx`: `Card`、`inputCls`、`btnCls`、`btnPrimary`、`btnGhost`、`btnDanger`（顶部注释「过渡期共享样式，阶段 2–5 逐页迁移后删除」）
  - `pages/*.tsx`: 9 个页组件，`export function XxxPage()`
  - `App.tsx` 外壳：保留现有顶栏/主区/监听器，页函数改为从 `pages/` import

- [ ] **Step 1: 建立函数 → 文件映射（以当前文件为准重新 grep）**

```bash
grep -n "^function \|^export default function " ui/src/App.tsx
```

映射表（函数名 → 目标文件）：

| 函数 | 行位（近似） | 目标 |
| --- | --- | --- |
| `toast`/`ToastKind` | 11-17 | `lib/toast.ts` |
| `copyText` | 19-24 | `lib/clipboard.ts` |
| `goTab` | 26-28 | `lib/nav.ts` |
| `Card` | 32-47 | `components/common/legacy.tsx` |
| `inputCls`/`btnCls`/`btnPrimary`/`btnGhost`/`btnDanger` | 49-55 | `components/common/legacy.tsx` |
| `fmtClock` | 57-66 | `lib/format.ts` |
| `App` | 72-152 | 留在 `App.tsx`（改造为外壳） |
| `GatewayTab` | 156-372 | `pages/GatewayPage.tsx`（`export function GatewayPage()`） |
| `SyncTab` | 374-629 | `pages/SyncPage.tsx` |
| `McpImportForm`/`McpTab`/`McpForm` | 631-977 | `pages/McpPage.tsx` |
| `SkillsTab`/`SkillForm` | 979-1281 | `pages/SkillsPage.tsx` |
| `ProvidersTab`/`ProviderCard`/`NewProviderForm`/`EditProviderForm` | 1283-1761 | `pages/ProvidersPage.tsx` |
| `ModelsTab`/`ModelRowEditor` | 1763-1983 | `pages/ModelsPage.tsx` |
| `StatsTab` | 1985-2058 | `pages/StatsPage.tsx` |
| `LogsTab` | 2062-2225 | `pages/LogsPage.tsx` |
| `SettingsTab` | 2227-2433 | `pages/SettingsPage.tsx` |

- [ ] **Step 2: 创建 lib 与 legacy 文件（代码原样搬移，仅加导出/注释）**

`ui/src/lib/toast.ts`:

```ts
// 全局 toast（阶段 0 临时用 CustomEvent，Task 4 换 sonner 实现，签名不变）
export type ToastKind = "ok" | "err";

export function toast(msg: string, kind: ToastKind = "ok") {
  window.dispatchEvent(new CustomEvent("jai-toast", { detail: { msg, kind } }));
}
```

`ui/src/lib/clipboard.ts`:

```ts
import { toast } from "./toast";

export function copyText(text: string) {
  navigator.clipboard?.writeText(text).then(
    () => toast("已复制"),
    () => toast("复制失败", "err")
  );
}
```

`ui/src/lib/format.ts`:

```ts
export function fmtClock(ms: number | null | undefined): string {
  if (!ms) return "—";
  return new Date(ms).toLocaleString("zh-CN", {
    hour12: false,
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}
```

`ui/src/lib/nav.ts`:

```ts
// 跨页跳转（阶段 0 临时用 CustomEvent，Task 5 换 NavContext，调用点不变）
export type Tab =
  | "gateway" | "sync" | "mcp" | "skills" | "providers"
  | "models" | "stats" | "logs" | "settings";

export function goTab(tab: Tab) {
  window.dispatchEvent(new CustomEvent("jai-goto-tab", { detail: { tab } }));
}
```

`ui/src/components/common/legacy.tsx`（过渡期共享样式，阶段 2–5 逐页迁移后删除）:

```tsx
export function Card({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div className="rounded-lg border border-neutral-800 bg-neutral-900/60 p-4">
      <h2 className="mb-3 text-sm font-semibold tracking-wide text-neutral-300">
        {title}
      </h2>
      {children}
    </div>
  );
}

export const inputCls =
  "w-full rounded border border-neutral-700 bg-neutral-950 px-2 py-1.5 text-sm text-neutral-100 outline-none focus:border-amber-500";
export const btnCls =
  "rounded px-3 py-1.5 text-sm font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-40";
export const btnPrimary = `${btnCls} bg-amber-600 text-black hover:bg-amber-500`;
export const btnGhost = `${btnCls} border border-neutral-700 text-neutral-300 hover:border-neutral-500`;
export const btnDanger = `${btnCls} border border-red-900/60 text-red-400 hover:bg-red-950`;
```

- [ ] **Step 3: 拆出 9 个页面文件**

每个页面文件遵循统一结构（函数体内代码原样搬移，仅改 import 路径）：

```tsx
import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { /* 该页实际用到的 types.ts 类型 */ } from "../types";
import { toast } from "../lib/toast";
import { copyText } from "../lib/clipboard";
import { goTab } from "../lib/nav";
import { fmtClock } from "../lib/format";
import {
  Card, inputCls, btnCls, btnPrimary, btnGhost, btnDanger,
} from "../components/common/legacy";

export function XxxPage() {
  /* 原函数体原样搬移，不改任何 JSX 与逻辑 */
}
```

- 类型 import 按各页实际使用精简；`GatewayPage` 内有一处行内 `import("./types")` 需改为 `"../types"`
- 本任务不改任何 JSX/逻辑/样式

- [ ] **Step 4: 重写 App.tsx 外壳**

```tsx
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
        {tabs.map(([k, label]) => (
          <button
            key={k}
            onClick={() => setTab(k)}
            className={`rounded px-3 py-1.5 text-sm ${
              tab === k ? "bg-neutral-800 font-medium text-white" : "text-neutral-400 hover:text-neutral-200"
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
```

（外壳代码与拆分前逐字一致，仅函数体搬走。）

- [ ] **Step 5: tsc 引导修复 import**

```bash
pnpm --dir ui exec tsc --noEmit
```

预期错误仅两类，逐条修复：
1. `Cannot find module './types'`（`GatewayPage` 行内 import）→ 改 `"../types"`
2. `'X' is declared but its value is never read`（`noUnusedLocals` 生效）→ 删除该页未使用的 import

循环执行直到 `tsc --noEmit` 零错误。不得改动任何 JSX/逻辑。

- [ ] **Step 6: 构建验证**

```bash
pnpm --dir ui build
```

Expected: `tsc --noEmit` 通过 + `vite build` 成功，输出无 error。

- [ ] **Step 7: 冒烟**

```bash
pnpm --dir ui dev
```

打开 http://127.0.0.1:5173（无 Tauri 后端时接口报错属正常，页面应能渲染、页签可切换、toast 可弹出）。逐页签点击一遍确认 9 页都在。

- [ ] **Step 8: Commit**

```bash
git add ui/src/lib ui/src/components/common ui/src/pages ui/src/App.tsx
git commit -m "refactor(ui): 拆分 App.tsx 到 pages/ 与共享模块（纯搬移）"
```

---

