# JAI UI 2.0 — 阶段 0 基建 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 完成 UI 2.0 的基础设施：Tailwind 3→4、shadcn/ui 基座、蓝紫双主题（明暗）、sonner 通知、导航 Context、左侧可折叠边栏，并把 2433 行的 `App.tsx` 机械拆分为 `pages/ + lib/ + components/`。

**Architecture:** 数据层（`api.ts`/`types.ts`）与 Rust 后端在整个阶段 0 保持零改动，作为行为不回归的边界。先做纯机械拆分（视觉不变、每任务可独立验收），再叠加 Tailwind 4 + shadcn 主题体系；旧页面硬编码色值通过 `@theme inline` 兼容映射自动跟随明暗主题，随阶段 1–5 逐页迁移后删除。窗口外壳最后替换为侧边栏布局。

**Tech Stack:** Tauri 2 · React 19 · Vite 6 · TypeScript 5 · Tailwind CSS 4 · shadcn/ui（Radix）· next-themes · sonner · lucide-react · clsx/tailwind-merge/cva

**范围说明:** 本计划只覆盖 spec 的阶段 0（基建）。阶段 1–6 在阶段 0 验收通过后各自制定独立计划。阶段 0 结束时：9 个页面功能与当前完全一致，仅外观换成新主题体系。

## Global Constraints

- 所有 UI 命令在 `ui/` 下执行：`pnpm --dir ui <cmd>`（仓库根是 Rust workspace，勿用裸 `pnpm`）
- **只暂存本任务涉及的文件**。工作区存在无关脏文件（如 `crates/gateway-core/src/server/proxy.rs`、store/migrations 相关），严禁 `git add -A` 或 `git add .`
- 禁止改动：`ui/src/api.ts`、`ui/src/types.ts`、`src-tauri/`、`crates/`
- 每任务硬门槛：`pnpm --dir ui build`（含 `tsc --noEmit`）**零错误**，输出需贴出验证
- commit message 遵循仓库惯例：`feat:` / `fix:` / `refactor:` / `build:` / `docs:` / `chore:`
- 界面文案保持中文；交互文案沿用现状
- 阶段 0 不做：recharts 图表、表单校验库（RHF/zod）、任何页面的视觉重设计

---

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

## Task 2: Tailwind 3 → 4 升级

**Files:**
- Modify: `ui/package.json`（依赖）、`ui/vite.config.ts`、`ui/src/index.css`
- Delete: `ui/postcss.config.js`、`ui/tailwind.config.js`

**Interfaces:**
- Consumes: Task 1 拆分后的结构（与页面代码无关，仅构建链）
- Produces: Tailwind 4 构建链（`@tailwindcss/vite` 插件 + CSS-first 配置）；`index.css` 变为 `@import "tailwindcss";`

- [ ] **Step 1: 换依赖**

```bash
cd /Users/jiangnan/Documents/workspace/JAI/ui && pnpm remove tailwindcss autoprefixer postcss && pnpm add -D tailwindcss@^4 @tailwindcss/vite
```

- [ ] **Step 2: 删除旧配置，改 vite 插件**

```bash
rm ui/postcss.config.js ui/tailwind.config.js
```

`ui/vite.config.ts` 全文替换为:

```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Tauri 固定 devUrl=5173：strictPort 防止漂移
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    host: "127.0.0.1",
    port: 5173,
    strictPort: true,
  },
  build: {
    target: "es2021",
  },
});
```

- [ ] **Step 3: 重写 index.css**

`ui/src/index.css` 全文替换为:

```css
@import "tailwindcss";
```

- [ ] **Step 4: 构建验证**

```bash
pnpm --dir ui build
```

Expected: 零错误。若出现个别 v4 类名报错（如 `outline-none` 已改名 `outline-hidden`、`shadow-sm`→`shadow-xs`），逐一核对 `ui/src` 实际使用并修正（当前清单：`outline-none`×多、`shadow-lg`×1，v4 均兼容，无需改）。

- [ ] **Step 5: 视觉抽查**

`pnpm --dir ui dev`，打开 http://127.0.0.1:5173，确认暗色观感与升级前基本一致（中性灰 + 琥珀主色的布局、圆角、间距无异常）。

- [ ] **Step 6: Commit**

```bash
git add ui/package.json ui/pnpm-lock.yaml ui/vite.config.ts ui/src/index.css ui/postcss.config.js ui/tailwind.config.js
git commit -m "build(ui): Tailwind 3→4 升级（@tailwindcss/vite，CSS-first）"
```

---

## Task 3: shadcn 基座 + 蓝紫双主题 + 过渡期兼容映射

**Files:**
- Create: `ui/src/lib/utils.ts`、`ui/components.json`、`ui/src/components/ui/*`（shadcn CLI 生成）、`ui/src/components/layout/ThemeToggle.tsx`
- Modify: `ui/package.json`、`ui/tsconfig.json`、`ui/vite.config.ts`（alias）、`ui/src/index.css`（主题 token + 兼容映射）、`ui/src/main.tsx`（ThemeProvider）、`ui/src/components/common/legacy.tsx`（btnPrimary 语义化）、`ui/src/pages/*.tsx`（3 处 text-white → text-foreground）、`ui/src/pages/SkillsPage.tsx` + `ui/src/pages/SettingsPage.tsx`（amber banner 语义化）

**Interfaces:**
- Consumes: Task 2 的 Tailwind 4 构建链
- Produces:
  - `lib/utils.ts`: `cn(...inputs: ClassValue[]): string`（clsx + tailwind-merge）
  - `components/ui/*`: shadcn 基础件（button、badge、card、input、label、skeleton、tooltip、dialog、alert-dialog、switch、select、dropdown-menu、table、separator、sonner）
  - `components/layout/ThemeToggle.tsx`: `export function ThemeToggle()`（next-themes，亮/暗/跟随系统）
  - `main.tsx`: 包 `<ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>`
  - `index.css`: 完整明暗 token（蓝紫主色）+ `@theme inline` 兼容映射（neutral/amber → 语义变量）
  - 后续任务（Task 4/6）依赖 `@/` 别名、`next-themes`、shadcn `Button`/`DropdownMenu`

- [ ] **Step 1: 装依赖**

```bash
cd /Users/jiangnan/Documents/workspace/JAI/ui && pnpm add clsx tailwind-merge class-variance-authority lucide-react next-themes sonner && pnpm add -D tw-animate-css
```

- [ ] **Step 2: 配置 @ 别名**

`ui/tsconfig.json` 的 `compilerOptions` 增加:

```json
"baseUrl": ".",
"paths": { "@/*": ["./src/*"] }
```

`ui/vite.config.ts` 改为:

```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath, URL } from "node:url";

// Tauri 固定 devUrl=5173：strictPort 防止漂移
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: { host: "127.0.0.1", port: 5173, strictPort: true },
  build: { target: "es2021" },
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) } },
});
```

- [ ] **Step 3: 创建 components.json**

`ui/components.json`（必须放在 `ui/` 项目根，shadcn CLI 从 cwd 查找；css 路径相对 `ui/` 根）:

```json
{
  "$schema": "https://ui.shadcn.com/schema.json",
  "style": "new-york",
  "rsc": false,
  "tsx": true,
  "tailwind": {
    "config": "",
    "css": "src/index.css",
    "baseColor": "neutral",
    "cssVariables": true
  },
  "iconLibrary": "lucide",
  "aliases": {
    "components": "@/components",
    "utils": "@/lib/utils",
    "ui": "@/components/ui",
    "lib": "@/lib",
    "hooks": "@/hooks"
  }
}
```

- [ ] **Step 4: 创建 cn() 工具**

`ui/src/lib/utils.ts`:

```ts
import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
```

- [ ] **Step 5: shadcn CLI 拷入基础件**

```bash
cd /Users/jiangnan/Documents/workspace/JAI/ui && pnpm dlx shadcn@latest add button badge card input label skeleton tooltip dialog alert-dialog switch select dropdown-menu table separator sonner --yes
```

Expected: `src/components/ui/` 下生成对应组件，radix 依赖自动安装。若 CLI 因网络/交互失败，重试一次；仍失败则按 shadcn 文档手动创建 `button`、`dialog`、`sonner` 三个组件（其余阶段 1–4 需要时再补），并在此任务报告中说明。

- [ ] **Step 6: 重写 index.css（主题 token + 兼容映射）**

`ui/src/index.css` 全文替换为:

```css
@import "tailwindcss";
@import "tw-animate-css";

@custom-variant dark (&:where(.dark, .dark *));

:root {
  --radius: 0.5rem;
  --background: oklch(1 0 0);
  --foreground: oklch(0.141 0.005 285.823);
  --card: oklch(1 0 0);
  --card-foreground: oklch(0.141 0.005 285.823);
  --popover: oklch(1 0 0);
  --popover-foreground: oklch(0.141 0.005 285.823);
  --primary: oklch(0.546 0.245 262.881);        /* 蓝紫：indigo-600 风格 */
  --primary-foreground: oklch(0.985 0 0);
  --secondary: oklch(0.967 0.001 286.375);
  --secondary-foreground: oklch(0.21 0.006 285.885);
  --muted: oklch(0.967 0.001 286.375);
  --muted-foreground: oklch(0.552 0.016 285.938);
  --accent: oklch(0.967 0.001 286.375);
  --accent-foreground: oklch(0.21 0.006 285.885);
  --destructive: oklch(0.577 0.245 27.325);
  --border: oklch(0.92 0.004 286.32);
  --input: oklch(0.92 0.004 286.32);
  --ring: oklch(0.546 0.245 262.881);
}

.dark {
  --background: oklch(0.141 0.005 285.823);
  --foreground: oklch(0.985 0 0);
  --card: oklch(0.21 0.006 285.885);
  --card-foreground: oklch(0.985 0 0);
  --popover: oklch(0.21 0.006 285.885);
  --popover-foreground: oklch(0.985 0 0);
  --primary: oklch(0.685 0.169 237.323);        /* 暗色下更亮的蓝紫 */
  --primary-foreground: oklch(0.985 0 0);
  --secondary: oklch(0.274 0.006 286.033);
  --secondary-foreground: oklch(0.985 0 0);
  --muted: oklch(0.274 0.006 286.033);
  --muted-foreground: oklch(0.705 0.015 286.067);
  --accent: oklch(0.274 0.006 286.033);
  --accent-foreground: oklch(0.985 0 0);
  --destructive: oklch(0.704 0.191 22.216);
  --border: oklch(1 0 0 / 10%);
  --input: oklch(1 0 0 / 15%);
  --ring: oklch(0.685 0.169 237.323);
}

@theme inline {
  --color-background: var(--background);
  --color-foreground: var(--foreground);
  --color-card: var(--card);
  --color-card-foreground: var(--card-foreground);
  --color-popover: var(--popover);
  --color-popover-foreground: var(--popover-foreground);
  --color-primary: var(--primary);
  --color-primary-foreground: var(--primary-foreground);
  --color-secondary: var(--secondary);
  --color-secondary-foreground: var(--secondary-foreground);
  --color-muted: var(--muted);
  --color-muted-foreground: var(--muted-foreground);
  --color-accent: var(--accent);
  --color-accent-foreground: var(--accent-foreground);
  --color-destructive: var(--destructive);
  --color-border: var(--border);
  --color-input: var(--input);
  --color-ring: var(--ring);
  --radius-sm: calc(var(--radius) - 4px);
  --radius-md: calc(var(--radius) - 2px);
  --radius-lg: var(--radius);
  --radius-xl: calc(var(--radius) + 4px);

  /* ===== 过渡期兼容映射（阶段 2–5 逐页迁移后删除）===== */
  --color-neutral-100: var(--foreground);
  --color-neutral-200: var(--foreground);
  --color-neutral-300: var(--muted-foreground);
  --color-neutral-400: var(--muted-foreground);
  --color-neutral-500: var(--muted-foreground);
  --color-neutral-600: var(--muted-foreground);
  --color-neutral-700: var(--border);
  --color-neutral-800: var(--border);
  --color-neutral-900: var(--card);
  --color-neutral-950: var(--background);
  --color-amber-400: var(--primary);
  --color-amber-500: var(--primary);
  --color-amber-600: var(--primary);
}

@layer base {
  * {
    @apply border-border outline-ring/50;
  }
  body {
    @apply bg-background text-foreground;
  }
  /* Tailwind v4 preflight 不再给按钮 pointer，恢复（仅非禁用态） */
  button:not(:disabled) {
    cursor: pointer;
  }
}
```

- [ ] **Step 7: 兼容性手工修补（4+2 处，全部列出）**

1. `ui/src/components/common/legacy.tsx` — `btnPrimary` 语义化:

```tsx
export const btnPrimary = `${btnCls} bg-primary text-primary-foreground hover:bg-primary/90`;
```

2. 3 处 `text-white`（在浅色主题下灰底白字不可读）→ `text-foreground`。执行以下命令定位并逐一修改（均为「选中态按钮」模式 `bg-neutral-800 text-white`）：

```bash
grep -n "text-white" ui/src/pages ui/src/App.tsx
```

预期 3 处（Task 1 后分布在 `App.tsx` 外壳与 `StatsPage` 等），逐处将 `text-white` 改为 `text-foreground`。

3. `ui/src/pages/SkillsPage.tsx` 导入提示 banner（`border-amber-800 bg-amber-950/30 text-amber-200`，Task 1 前的 1131 行）→ 语义化:

```tsx
className="flex flex-wrap items-center gap-2 rounded border border-primary/40 bg-primary/10 px-3 py-2 text-sm text-primary"
```

4. `ui/src/pages/SettingsPage.tsx` CORS 警告 banner（`border-amber-800 bg-amber-950/40 text-amber-300`，Task 1 前的 2366 行）→ 语义化:

```tsx
className="mb-3 rounded border border-primary/40 bg-primary/10 px-3 py-2 text-xs text-primary"
```

- [ ] **Step 8: main.tsx 挂 ThemeProvider**

`ui/src/main.tsx` 全文替换为:

```tsx
import React from "react";
import ReactDOM from "react-dom/client";
import { ThemeProvider } from "next-themes";
import App from "./App";
import "./index.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ThemeProvider
      attribute="class"
      defaultTheme="system"
      enableSystem
      disableTransitionOnChange
    >
      <App />
    </ThemeProvider>
  </React.StrictMode>,
);
```

- [ ] **Step 9: 创建 ThemeToggle 并临时挂到 App 外壳顶栏**

`ui/src/components/layout/ThemeToggle.tsx`:

```tsx
import { Moon, Sun } from "lucide-react";
import { useTheme } from "next-themes";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

export function ThemeToggle() {
  const { setTheme } = useTheme();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon" aria-label="切换主题">
          <Sun className="size-4 scale-100 rotate-0 transition-all dark:scale-0 dark:-rotate-90" />
          <Moon className="absolute size-4 scale-0 rotate-90 transition-all dark:scale-100 dark:rotate-0" />
          <span className="sr-only">切换主题</span>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem onClick={() => setTheme("light")}>亮色</DropdownMenuItem>
        <DropdownMenuItem onClick={() => setTheme("dark")}>暗色</DropdownMenuItem>
        <DropdownMenuItem onClick={() => setTheme("system")}>跟随系统</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
```

在 `ui/src/App.tsx` 顶栏 header 内、JAI logo 之后插入 `<ThemeToggle />`，import 改为:

```tsx
import { ThemeToggle } from "./components/layout/ThemeToggle";
```

（Task 6 侧边栏落地后移入侧边栏。）

- [ ] **Step 10: 构建 + 双主题冒烟**

```bash
pnpm --dir ui build
```

Expected: 零错误。然后 `pnpm --dir ui dev`，用 ThemeToggle 在亮/暗/跟随系统间切换，9 个页面逐一走查：

- 亮色下：页面背景近白、卡片纯白、正文近黑、按钮为蓝紫主色白字；旧页面硬编码中性灰类自动跟随（兼容映射生效）
- 暗色下：背景深蓝紫黑、卡片稍亮、正文近白、按钮为亮蓝紫
- 已知过渡期瑕疵（本次接受，阶段 2–5 迁移时治理）：红/绿状态 banner（如 `bg-red-950/40 text-red-300`、`bg-emerald-950/40 text-emerald-300`）在亮色下对比度偏低；`bg-white` 开关滑块不受主题影响属正常
- 刷新页面主题保持（localStorage 持久化）

- [ ] **Step 11: Commit**

```bash
git add ui/package.json ui/pnpm-lock.yaml ui/tsconfig.json ui/vite.config.ts ui/components.json ui/src/lib ui/src/components/ui ui/src/components/layout ui/src/components/common ui/src/index.css ui/src/main.tsx ui/src/App.tsx ui/src/pages
git commit -m "feat(ui): shadcn 基座 + 蓝紫双主题 + 过渡期兼容映射"
```

---

## Task 4: sonner 替换 CustomEvent toast

**Files:**
- Modify: `ui/src/lib/toast.ts`、`ui/src/main.tsx`（挂 Toaster）、`ui/src/App.tsx`（删 toast 状态/监听/UI）

**Interfaces:**
- Consumes: Task 3 的 `components/ui/sonner.tsx` 与 `next-themes`
- Produces: `toast(msg, kind)` 签名不变（22 处调用点零改动），底层改为 sonner；`<Toaster position="bottom-center" richColors />` 挂载在 `main.tsx`

- [ ] **Step 1: 重写 lib/toast.ts**

`ui/src/lib/toast.ts` 全文替换为:

```ts
import { toast as sonnerToast } from "sonner";

export type ToastKind = "ok" | "err";

// 统一封装层：阶段 2–5 调整样式只改这一处
export function toast(msg: string, kind: ToastKind = "ok") {
  if (kind === "err") {
    sonnerToast.error(msg);
  } else {
    sonnerToast.success(msg);
  }
}
```

- [ ] **Step 2: main.tsx 挂 Toaster**

`ui/src/main.tsx` 中 `</ThemeProvider>` 前加一行:

```tsx
import { Toaster } from "@/components/ui/sonner";
```

```tsx
    </ThemeProvider>
    <Toaster position="bottom-center" richColors />
```

- [ ] **Step 3: 清掉 App.tsx 旧 toast 机制**

`ui/src/App.tsx`：
1. 删除 `toastMsg` state、`onToast` 监听及其 useEffect 逻辑、底部 `{toastMsg && ...}` JSX
2. 删除 `import type { ToastKind } from "./lib/toast"`（不再使用）
3. 保留 `jai-goto-tab` 监听（Task 5 处理）

- [ ] **Step 4: 构建 + 冒烟**

```bash
pnpm --dir ui build
```

Expected: 零错误。`pnpm --dir ui dev` 后触发任意 toast（如点「复制」）确认：右下/底部出现 sonner 成功态（绿色）与失败态（红色）通知，位置在底部居中。

- [ ] **Step 5: Commit**

```bash
git add ui/src/lib/toast.ts ui/src/main.tsx ui/src/App.tsx
git commit -m "feat(ui): sonner 替换 CustomEvent toast（签名不变，调用点零改动）"
```

---

## Task 5: 导航 CustomEvent 换 NavContext

**Files:**
- Modify: `ui/src/lib/nav.ts`（→ `nav.tsx`，加 Context）、`ui/src/main.tsx`（包 NavProvider）、`ui/src/App.tsx`（用 useNav 取代本地 state 与监听器）

**Interfaces:**
- Consumes: Task 1 的 `goTab(tab)` 调用点（页面内 2 处，保持不变）
- Produces: `NavProvider`、`useNav(): { tab: Tab; setTab: (t: Tab) => void }`；`goTab(tab)` 继续可用（内部改走 provider 注册的 setter）

- [ ] **Step 1: 重写 lib/nav.ts → lib/nav.tsx**

`ui/src/lib/nav.ts` 删除，新建 `ui/src/lib/nav.tsx`:

```tsx
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

export type Tab =
  | "gateway" | "sync" | "mcp" | "skills" | "providers"
  | "models" | "stats" | "logs" | "settings";

const NavContext = createContext<{ tab: Tab; setTab: (t: Tab) => void } | null>(null);

let goTabFn: ((t: Tab) => void) | null = null;

// 页面内既有调用点继续使用（Task 1 起未变）
export function goTab(tab: Tab) {
  goTabFn?.(tab);
}

export function NavProvider({ children }: { children: ReactNode }) {
  const [tab, setTabState] = useState<Tab>("gateway");
  const setTab = useCallback((t: Tab) => setTabState(t), []);
  const value = useMemo(() => ({ tab, setTab }), [tab, setTab]);

  useEffect(() => {
    goTabFn = setTab;
    return () => {
      goTabFn = null;
    };
  }, [setTab]);

  return <NavContext.Provider value={value}>{children}</NavContext.Provider>;
}

export function useNav() {
  const ctx = useContext(NavContext);
  if (!ctx) throw new Error("useNav 必须在 NavProvider 内使用");
  return ctx;
}
```

- [ ] **Step 2: main.tsx 包 NavProvider**

`ui/src/main.tsx`：import `NavProvider` 并包住 `<App />`:

```tsx
import { NavProvider } from "./lib/nav";
```

```tsx
    <ThemeProvider ...>
      <NavProvider>
        <App />
      </NavProvider>
    </ThemeProvider>
    <Toaster position="bottom-center" richColors />
```

- [ ] **Step 3: App.tsx 改用 useNav**

`ui/src/App.tsx`：
1. 删除本地 `const [tab, setTab] = useState<Tab>("gateway")` 与 `jai-goto-tab` 监听及其 useEffect
2. 改为:

```tsx
import { useNav } from "./lib/nav";

export default function App() {
  const { tab, setTab } = useNav();
  ...
}
```

（`tab === k && <XxxPage />` 渲染与 `tabs` 数组不变；`setTab` 来自 useNav。）

- [ ] **Step 4: 构建 + 冒烟**

```bash
pnpm --dir ui build
```

Expected: 零错误。dev 下验证跨页跳转：SyncPage 内「去网关导出」类链接点击后能切到网关页（`goTab` 调用点仍工作）。

- [ ] **Step 5: Commit**

```bash
git add ui/src/lib ui/src/main.tsx ui/src/App.tsx
git commit -m "refactor(ui): 导航 CustomEvent 换 NavContext（goTab 调用点不变）"
```

---

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

## Task 7: 阶段 0 终验与推送

**Files:**
- 无代码改动（只验证 + 推送）

**Interfaces:**
- Consumes: Task 1–6 全部产出

- [ ] **Step 1: 构建硬门槛**

```bash
pnpm --dir ui build
```

Expected: `tsc --noEmit` + `vite build` 零错误。贴出完整输出末尾的构建成功信息。

- [ ] **Step 2: 手工验收清单**

`pnpm --dir ui dev` 后逐项过：

| # | 检查项 | 预期 |
| --- | --- | --- |
| 1 | 9 个页签切换 | 页面正常渲染，无空白/报错 |
| 2 | 亮/暗/跟随系统切换 | 全页颜色跟随，持久化 |
| 3 | 侧边栏折叠/展开 | 动画顺畅，状态持久 |
| 4 | toast（成功/失败） | sonner 样式，底部居中 |
| 5 | 跨页跳转（同步→网关） | goTab 仍工作 |
| 6 | 复制按钮 | 成功 toast |
| 7 | 危险操作按钮（轮换/删除） | 与原行为一致（未迁移，仍原样） |
| 8 | 网关状态/供应商列表等数据加载 | 与 Task 1 拆分前一致（接口行为未动） |

- [ ] **Step 3: 检查 git 干净度**

```bash
git status --short
```

Expected: 除任务提交外，仅剩无关的既有脏文件（如 `crates/gateway-core/src/server/proxy.rs` 等），不得有 `ui/` 下的意外改动。

- [ ] **Step 4: 推送**

```bash
git push origin main
```

- [ ] **Step 5: 汇报阶段 0 完成**

按 AGENTS.md 格式输出 `=== 完成 ===`，验证指标 = `pnpm --dir ui build` 零错误 + 验收清单逐项通过。

---

## Self-Review（已执行）

1. **Spec 覆盖**：spec 阶段 0 各项（Tailwind 3→4 ✓ Task 2；shadcn 初始化+基础件 ✓ Task 3；双主题变量与切换器 ✓ Task 3；sonner 替换 toast 签名不变 ✓ Task 4；App.tsx 机械拆分 ✓ Task 1；侧边栏导航+折叠持久化 ✓ Task 6；goTab CustomEvent 换 Context ✓ Task 5；每阶段构建零错误 ✓ 每 Task 验证步 + Task 7）。
2. **占位符扫描**：无 TBD/TODO/「适当处理」类表述；所有代码步骤给出完整代码或精确命令。
3. **类型一致性**：`toast(msg, kind)` 签名 Task 1 定义 → Task 4 保持；`goTab(tab)` Task 1 定义 → Task 5 保持（22 处 toast、2 处 goTab 调用点零改动）；`Tab` union Task 1/5 一致；`cn()` 仅 Task 3 定义、Task 6 使用；`useNav` 仅 Task 5 定义、Task 5/6 使用；`ThemeToggle` Task 3 定义、Task 3 临时挂载 + Task 6 移入侧边栏。
4. **已知过渡期瑕疵**（spec 风险表第 1 条落点）：红/绿状态 banner 亮色对比度偏低，已显式登记在 Task 3 Step 10 冒烟清单，由阶段 2–5 逐页迁移治理；兼容映射块在 Task 3 Step 6 CSS 中以注释标注「阶段 2–5 删除」。
