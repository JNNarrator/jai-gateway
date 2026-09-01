# 终审修复波报告（final-fix）

- 基线：`e595582`（main）
- 修复 commit：`7dcf077` — `fix(ui): 终审修复 — 代码块亮色可读性、MCP env 校验与渲染容错、供应商编辑补传 extraHeaders、清理过渡期兼容映射`
- 验证：`pnpm --dir ui exec tsc --noEmit` 零错误；`pnpm --dir ui build` 零错误（vite 构建成功）
- 范围：纯前端（`ui/src/` + `ui/src/index.css`），未触碰 `ui/src/api.ts`、`ui/src/types.ts`、`crates/`、`src-tauri/`

---

## Important 修复

### I-1 GatewayPage 亮色主题代码块不可读

- 文件：`ui/src/pages/GatewayPage.tsx`
- 原因：`bg-neutral-950` 被 index.css 过渡期兼容映射解析为 `var(--background)`（亮色=白），文字恒定 `text-zinc-300` → 白底浅字 ≈1.2:1 不可读。
- 修复：
  - `<pre>`：`bg-neutral-950 dark:bg-black/60` → 恒定字面深色 `bg-zinc-950`，移除 dark 覆盖（「代码块刻意深底」为设计意图，双主题一致）。
  - 复制按钮：`border-neutral-700` → `border-zinc-700`（与深底协调）；文字 `text-zinc-400 hover:text-zinc-100` 保持不变。

### I-2 MCP env 非法 JSON → 渲染崩溃白屏

- 文件：`ui/src/pages/McpPage.tsx`（纵深防御，两层都修）
- 1) 保存前校验：`McpDialog.submit()` 新增 `envErr` 状态；env 非空时必须 `JSON.parse` 成功且为普通对象（非 null / 数组），失败就地显示 `环境变量需为合法 JSON 对象，例如 {"API_KEY":"xxx"}` 并阻止提交，风格与同函数内 args 的校验一致（输入框 `border-destructive` + `role="alert"` 错误行，输入时清错）。校验不限定 kind（env 字段对所有 kind 都会入库）。
- 2) 渲染容错：列表渲染处原来的裸 `Object.keys(JSON.parse(m.env)…)` 改为新工具函数 `envKeySummary(env)`——try-catch + 对象类型检查，合法对象返回键名列表，非法/非对象返回占位文案 `(非法 JSON)`，throw 不再冒泡（无 ErrorBoundary 也能自救）。

### I-3 供应商编辑对话框静默丢输入

- 文件：`ui/src/pages/ProvidersPage.tsx`
- 1) edit 提交补传 `extraHeaders`：submit 内已有的行序列化逻辑（trim、过滤空 key 行、有内容 `JSON.stringify`、全空 `null`）结果 `eh` 现在同时传给 `providerUpdate`。语义确认（`src-tauri/src/main.rs:235` `extra_headers: Option<Option<String>>` + `crates/gateway-core/src/store/mod.rs:223` 注释）：JS `null` → `Some(None)` 显式清空；JSON 串 → `Some(Some(s))` 覆盖；字段缺省 → 不动。与新建表单提交语义完全一致（新建全空行也是传 `null`）。api.ts 类型 `extraHeaders?: string | null` 匹配。
- 2) 协议族 Select 编辑模式 `disabled`（SelectTrigger 自带 `disabled:opacity-50 disabled:cursor-not-allowed` 样式），FormField 增加编辑模式 hint「协议族创建后不可修改」，防止误以为可改（IPC 层 providerUpdate 不支持改 family）。

---

## Minor 修复

### M-1 删除过渡期兼容映射（阶段 5 本应完成）

前置条件（消费方清零确认），删除映射**前**（I-1 已修完 GatewayPage 后）grep：

```
$ grep -rnE '(bg|text|border|divide)-(neutral|amber)-[0-9]+' ui/src --include='*.tsx' | grep -v components/ui
ui/src/pages/SettingsPage.tsx:213:                  ? "text-amber-600 dark:text-amber-400"
ui/src/pages/ProvidersPage.tsx:284:                <Badge variant="outline" className="border-amber-500/40 text-amber-600 dark:text-amber-400">
```

neutral 消费方为 0（GatewayPage 两处已在 I-1 改为 zinc 字面值）；另做了更宽扫描（含 `components/ui`、`.ts` 文件、`ring/from/to/via/outline/fill/stroke` 前缀）确认除上述两处 amber 外无其他消费方。随后删除 `ui/src/index.css` 中 `/* ===== 过渡期兼容映射（阶段 2–5 逐页迁移后删除）===== */` 整块（neutral-100..950、amber-400/500/600 共 13 行覆盖 + 注释）。

**amber 两处渲染判断依据**：Tailwind v4 默认主题自带字面 amber 色板（`ui/node_modules/.pnpm/tailwindcss@4.3.3/node_modules/tailwindcss/theme.css:38-40`：`--color-amber-400: oklch(82.8% 0.189 84.429)`、`--color-amber-500: oklch(76.9% 0.188 70.08)`、`--color-amber-600: oklch(66.6% 0.179 58.318)`）；项目 index.css 的 `@theme inline` 只做逐键覆盖，删除覆盖后同名类自动回落默认字面值。构建产物抽查确认：`dist/assets/index-*.css` 中 `--color-amber-600: oklch(66.6% .179 58.318)`（字面 amber，正是 Tailwind 默认值），映射残留（`--color-neutral-*:var(…)` / `--color-amber-*:var(--primary)`）为 0。两处语义均为警示（SettingsPage:213「文件降级（0600）」存储降级提示、ProvidersPage:284「缺少凭据」徽标），恢复真 amber 警示色语义正确，无需改这两处。

### M-3 死监听清理

- 文件：`ui/src/pages/ProvidersPage.tsx`（原 :136-138）
- 全库 grep `jai-refresh-providers` / `jai-refresh`：仅该 addEventListener/removeEventListener 两行，无任何 dispatch（基线重写时丢失）。已删除监听及关联清理代码，useEffect 保留初始 refresh。

### M-4 TitleBar onResized unlisten 失效

- 文件：`ui/src/components/layout/TitleBar.tsx`
- 类型确认（`@tauri-apps/api` 2.11.1 `window.d.ts:1214`）：`onResized(handler): Promise<UnlistenFn>`。原代码未接 Promise，`unlisten` 恒 undefined，cleanup 无效，StrictMode 下双监听泄漏。
- 修复：标准 async 模式 + `disposed` 标志——`unlistenPromise.then(fn => { if (disposed) fn(); else unlisten = fn; })`，cleanup 中 `disposed = true; unlisten?.()`。两条路径恰好释放一次：先 resolve 后 cleanup 走 `unlisten?.()`；cleanup 先执行（StrictMode 双挂载）则 resolve 时就地释放。effect 内改用局部 `getCurrentWindow()`，不再引用组件体变量，依赖问题消失，`eslint-disable-next-line react-hooks/exhaustive-deps` 已移除（本项目 devDependencies 亦未配置 eslint，该注释本就是死注释）。

### M-5 void 调用静默失败

- `ui/src/pages/SkillsPage.tsx` `createExample()`、`ui/src/pages/ModelsPage.tsx` `setAll()`：补 try-catch，失败 `toast(String(e), "err")`（统一封装 `ui/src/lib/toast.ts` 的错误分支 → sonner error），成功路径行为不变（SkillsPage 走 setErr 的 act() 模式与本处 toast err 均为页内既有错误通道，按任务指示采用 toast err）。

---

## 不修（按派发指示保留，仅记录）

- M-6 `separator.tsx` 零使用——保留组件。
- M-7 特效回退缺 Linux 实色兜底——低风险边角。
- M-8 chunk 体积 999.43 kB——后续 manualChunks（本次 build 仍输出该警告，属预期）。
- M-9 TitleBar userAgent 嗅探——当前可工作，保留。

## 验证输出

`pnpm --dir ui exec tsc --noEmit` → 无输出（零错误）。

`pnpm --dir ui build` 输出末尾：

```
✓ 2667 modules transformed.
rendering chunks...
dist/index.html                   0.47 kB │ gzip:   0.30 kB
dist/assets/index-CBS14erG.css   53.05 kB │ gzip:   9.61 kB
dist/assets/index-B9rnEd57.js   999.43 kB │ gzip: 301.54 kB

(!) Some chunks are larger than 500 kB after minification. Consider:
- Using dynamic import() to code-split the application
- Use build.rollupOptions.output.manualChunks to improve chunking: https://rollupjs.org/configuration-options/#manualchunks
- Adjust chunk size limit for this warning via build.chunkSizeWarningLimit.
✓ built in 2.36s
```

（仅 M-8 已记录的 chunk 体积警告与 zod 源码注释提示，无错误。）

## 提交与状态

- commit hash 列表：
  - `7dcf077` fix(ui): 终审修复（本波全部 7 个文件，85 insertions / 42 deletions）
- 涉及文件（7 个，全部在 commit `7dcf077` 中）：
  - `ui/src/pages/GatewayPage.tsx`、`ui/src/pages/McpPage.tsx`、`ui/src/pages/ProvidersPage.tsx`
  - `ui/src/index.css`、`ui/src/components/layout/TitleBar.tsx`
  - `ui/src/pages/SkillsPage.tsx`、`ui/src/pages/ModelsPage.tsx`
- 修复 commit 后 git status（`.superpowers/sdd/…` 目录对新增文件被 .gitignore 忽略——目录内旧文件为忽略规则生效前已被跟踪的历史遗留；本报告按该策略仅落盘、不入库。`progress.md` 的终审派发行是协调者写入、非本波改动，保持未暂存）：

```
 M .superpowers/sdd/2026-08-31-ui2-phase0-foundation/progress.md
?? .playwright-mcp/
```
