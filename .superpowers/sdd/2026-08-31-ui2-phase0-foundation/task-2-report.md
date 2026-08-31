# Task 2 报告：Tailwind 3 → 4 升级

## 状态

DONE。验收门槛全部通过，已提交 `a3753e5`。

## 执行说明

任务开始时（HEAD=b7f6284），工作树中已存在未提交的同类改动（依赖、vite.config、index.css 已改，两个配置文件已删）——疑似上一次中断运行残留。本次核对 diff 与 brief Step 1–3 完全一致后，直接进入验证、冒烟与提交环节。

## 依赖变更

### 变更前（ui/package.json devDependencies）

```json
"@vitejs/plugin-react": "^4",
"autoprefixer": "^10",
"postcss": "^8",
"tailwindcss": "^3",
```

### 变更后（ui/package.json devDependencies）

```json
"@tailwindcss/vite": "^4.3.3",
"@vitejs/plugin-react": "^4",
"tailwindcss": "^4.3.3",
```

- 移除了直接依赖：`autoprefixer`、`postcss`、`tailwindcss@^3`（`pnpm remove tailwindcss autoprefixer postcss` + `pnpm add -D tailwindcss@^4 @tailwindcss/vite`）
- lockfile 检查：`tailwindcss@4.3.3` 为解析后依赖；无 `autoprefixer`、无 `tailwindcss@3`；`postcss@8.5.26` 仅剩为 vite/lightningcss 的传递依赖（无害）

## 删除文件

- `ui/postcss.config.js`（已删除）
- `ui/tailwind.config.js`（已删除）

## vite.config.ts 最终内容

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

## index.css 最终内容

```css
@import "tailwindcss";
```

## v4 类名核对

grep `ui/src` 实际使用：
- `outline-none` ×2（`components/common/legacy.tsx:20`、`pages/GatewayPage.tsx:188`）——v4 兼容，无需改
- `shadow-lg` ×1（`App.tsx:70`）——v4 兼容，无需改
- 无 `shadow-sm`/`rounded-sm`/`rounded-xs` 等 v4 更名类

## 验收门槛结果

### Gate 1: `pnpm --dir ui exec tsc --noEmit`

```
TSC_EXIT=0
```

零错误。

### Gate 2: `pnpm --dir ui build`

```
> jai-ui@0.1.0 build /Users/jiangnan/Documents/workspace/JAI/ui
> tsc --noEmit && vite build

vite v6.4.3 building for production...
transforming...
✓ 46 modules transformed.
rendering chunks...
computing gzip size...
dist/index.html                   0.40 kB │ gzip:  0.27 kB
dist/assets/index-BiXosJhz.css   20.98 kB │ gzip:  4.88 kB
dist/assets/index-D3LgcqFX.js   251.71 kB │ gzip: 77.27 kB
✓ built in 518ms
```

退出码 0，零错误。

## 冒烟测试（Step 5 视觉抽查的替代验证）

subagent 无法加载浏览器技能，改用 dev server 冒烟验证：
- 先发现 5173 端口被上一次运行残留的旧 dev server（PID 11116）占用，已 kill 后重启
- `pnpm --dir ui dev` 启动成功：`VITE v6.4.3 ready in 154 ms`，`http://127.0.0.1:5173/`
- 请求 `/src/index.css` 返回 Tailwind v4 特征输出：`@layer theme, base, components, utilities;` + `@layer theme/base/utilities` 分层，CSS 27,997 字节——证明 `@tailwindcss/vite` 插件 + CSS-first 配置在 dev 链路生效
- 冒烟后已关闭 dev server，端口释放

## 提交

```
a3753e5 build(ui): Tailwind 3→4 升级（@tailwindcss/vite，CSS-first）
 6 files changed, 322 insertions(+), 516 deletions(-)
 delete mode 100644 ui/postcss.config.js
 delete mode 100644 ui/tailwind.config.js
```

暂存范围与 brief 完全一致（`ui/package.json ui/pnpm-lock.yaml ui/vite.config.ts ui/src/index.css ui/postcss.config.js ui/tailwind.config.js`，删除文件一并 add，未用 `git add -A`）。

## git status 确认

```
On branch main
Your branch is ahead of 'origin/main' by 2 commits.
Untracked files:
	.playwright-mcp/
```

工作树干净（除任务上下文已声明的无关目录 `?? .playwright-mcp/` 外，无任何改动残留）。

## 违规检查

未改动 `ui/src/api.ts`、`ui/src/types.ts`、`ui/src/pages/*`、`src-tauri/`、`crates/`。

## Concerns

无。
