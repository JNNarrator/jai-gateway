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

