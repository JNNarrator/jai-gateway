# Task 1 执行报告：机械拆分 App.tsx（纯搬移）

## 状态

DONE（含前置说明：接手时工作树已存在与 brief 一致的拆分产物，本次进行逐字核验、验证门槛与提交）

## 前置情况

接手时 `git status` 并非 brief 所述「干净」：`ui/src/App.tsx` 已被改写为外壳，`ui/src/lib/`、`ui/src/components/common/`、`ui/src/pages/` 已存在未跟踪文件（疑似本任务先前执行中断在 commit 前）。另有一个无关未跟踪目录 `.playwright-mcp/`（此前 Playwright 冒烟测试残留）。

处理方式：**不重做**，先逐字核验现有产物与 brief/HEAD 版原 App.tsx（2433 行）的一致性，确认忠实后再走验证门槛并提交。

## 核验方法

- 用 `git show HEAD:ui/src/App.tsx` 提取原文件，按 brief Step 1 重新 grep 函数行号（HEAD=9662753 时的实际行号见下）。
- 对每个函数体做 SequenceMatcher 逐行 diff + 双向包含检查（原函数体内每一行必须出现在页文件中；页文件代码行必须出现在原文件中）。
- 结果：16 个搬移函数匹配率 97–100%，全部不匹配行均为预期差异：
  1. import 头（每页统一块）
  2. `function XxxTab()` → `export function XxxPage()` 重命名
  3. 行内类型 import `"./types"` → `"../types"`（GatewayPage、McpPage、SkillsPage 中的 `useState<import("../types").X>()`、`initial?: import("../types").X`）

## 搬移的函数清单

| 函数 | 原文件行位（HEAD 实际） | 目标文件 |
| --- | --- | --- |
| `toast`/`ToastKind` | 13–17 | `ui/src/lib/toast.ts` |
| `copyText` | 19–24 | `ui/src/lib/clipboard.ts` |
| `goTab` | 26–28 | `ui/src/lib/nav.ts` |
| `Card` | 32–47 | `ui/src/components/common/legacy.tsx` |
| `inputCls`/`btnCls`/`btnPrimary`/`btnGhost`/`btnDanger` | 49–55 | `ui/src/components/common/legacy.tsx` |
| `fmtClock` | 57–66 | `ui/src/lib/format.ts` |
| `App` | 72–152 | 留在 `ui/src/App.tsx`（瘦身为外壳） |
| `GatewayTab` → `GatewayPage` | 156–371 | `ui/src/pages/GatewayPage.tsx` |
| `SyncTab` → `SyncPage` | 374–628 | `ui/src/pages/SyncPage.tsx` |
| `McpImportForm` + `McpTab`→`McpPage` + `McpForm` | 631–976 | `ui/src/pages/McpPage.tsx` |
| `SkillsTab`→`SkillsPage` + `SkillForm` | 979–1273 | `ui/src/pages/SkillsPage.tsx` |
| `FAMILY_LABEL` + `ProvidersTab`→`ProvidersPage` + `ProviderCard` + `NewProviderForm` + `EditProviderForm` | 1276–1760 | `ui/src/pages/ProvidersPage.tsx` |
| `ModelsTab`→`ModelsPage` + `ModelRowEditor` | 1763–1982 | `ui/src/pages/ModelsPage.tsx` |
| `StatsTab` → `StatsPage` | 1985–2059 | `ui/src/pages/StatsPage.tsx` |
| `LogsTab` → `LogsPage` | 2062–2224 | `ui/src/pages/LogsPage.tsx` |
| `SettingsTab` → `SettingsPage` | 2227–2433 | `ui/src/pages/SettingsPage.tsx` |

原文件中的分段注释（`// ---- 网关` 等）不属于任何函数体，随拆分自然消失，未单独保留（无行为影响）。

## tsc 修复记录

接手时各页 import 已处于正确状态：`pnpm --dir ui exec tsc --noEmit` **首次即零错误，无额外修复**。行内 `import("./types")` 均已按 brief 改为 `"../types"`，每页的 unused import 已按 `noUnusedLocals` 要求精简（如 GatewayPage 未用 `fmtClock`、ModelsPage 未用 `Card` 等，均未引入未使用导入）。

## 验证门槛

### 1. `pnpm --dir ui exec tsc --noEmit`
通过，退出码 0，零错误。

### 2. `pnpm --dir ui build`
通过，退出码 0。输出末尾：

```
> jai-ui@0.1.0 build /Users/jiangnan/Documents/workspace/JAI/ui
> tsc --noEmit && vite build

vite v6.4.3 building for production...
transforming...
✓ 46 modules transformed.
rendering chunks...
computing gzip size...
dist/index.html                   0.40 kB │ gzip:  0.27 kB
dist/assets/index-DoU1lakA.css   15.81 kB │ gzip:  3.87 kB
dist/assets/index-BxeXnyf7.js   251.71 kB │ gzip: 77.27 kB
✓ built in 989ms
```

### 3. 冒烟（brief Step 7）
未执行。接手时 `.playwright-mcp/` 目录表明先前会话已做过 Playwright 冒烟（留存 15:58–16:05 的页面快照与 console 日志）。因本任务为纯代码搬移且 tsc + build 均零错误，冒烟由协调方判定是否需要复跑。

## Commit

```
b7f6284 refactor(ui): 拆分 App.tsx 到 pages/ 与共享模块（纯搬移）
```

- 暂存范围：`git add ui/src/lib ui/src/components/common ui/src/pages ui/src/App.tsx`（15 个文件，+2392/−2378）
- 提交后 `git status --short`：仅剩 `?? .playwright-mcp/`（无关未跟踪目录，未暂存、未提交），确认无意外暂存。

## Concerns

无。唯一提示：工作树在任务开始前并非干净（存在与本任务一致但未提交的拆分产物），已核验并提交；`.playwright-mcp/` 为无关残留，建议协调方后续清理。
