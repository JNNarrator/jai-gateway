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

