# JAI UI 2.0 — 阶段 1（网关 + 统计）Implementation Plan

> 前置：阶段 0 已验收（shadcn 基座 + 双主题 + sonner + 侧边栏）。
> 范围：spec §7 阶段 1 —— 网关门面页视觉重做 + 统计页 recharts 图表。
> 边界：`api.ts`/`types.ts`/Rust 零改动；`lib/toast` 签名不变；禁止新增硬编码色值（一律语义变量）。

**验收要点（spec）**：截图前后对比；图表数据与原手绘柱状图一致；`pnpm --dir ui build` 零错误。

---

## Task 1: 共享组件（components/common/）

阶段 2–5 会复用，一次建好：

1. `PageHeader.tsx` — 页头：`title` + `description?` + `actions?`（右侧操作区），统一各页排版节奏
2. `CopyField.tsx` — 只读代码字段：等宽文本 + 复制图标按钮（Tooltip），可选 `visibility`（显示/隐藏 Eye/EyeOff）与额外 `children`（如「轮换密钥」按钮）；用于 Base URL / API Key
3. `StatusBadge.tsx` — 状态点 + 文案徽章：`tone: "ok" | "idle"`，ok=emerald / idle=muted
4. `ConfirmDialog.tsx` — 基于 shadcn `AlertDialog` 的受控确认框：`open/onOpenChange/title/description/confirmText/destructive/onConfirm`；替换 `window.confirm`
5. `EmptyState.tsx` — 图标 + 主文案 + 可选引导 `children`；替换纯文字空态

验证：`tsc --noEmit` 零错误（组件暂未被引用时不强求页面接入）。

## Task 2: GatewayPage 视觉重做

- 页面骨架：`max-w-2xl` + `PageHeader`（标题「网关」+ 说明）
- 网关状态卡：shadcn `Card`（Title+Description）；`StatusBadge`（运行中/已停止）；端口等宽字体；启动/停止 → `Button`（default / destructive，lucide `Play`/`Square` 图标）
- 客户端接入卡：Base URL 与 API Key 走 `CopyField`（API Key 带显示/隐藏 + 轮换密钥）；「轮换密钥」从 `window.confirm` 改 `ConfirmDialog`（destructive）
- 接入示例：`select` → shadcn `Select`；代码块深色字面量底色保留（本页刻意设计），右上复制改 `Copy` 图标按钮 + Tooltip
- 配置迁移卡：`Button variant="outline"` + 说明文字
- 错误横幅语义化（border-destructive/50 bg-destructive/10 text-destructive）
- 移除本页对 `legacy.tsx` 的引用

验证：构建零错误；dev 下启动/停止、复制、显示/隐藏、轮换（取消+确认两条路径）、示例切换不回归。

## Task 3: StatsPage recharts 图表

- `pnpm --dir ui add recharts`（React 19 兼容版本）
- KPI 四卡 → shadcn `Card`；时间范围切换 → `Button` 组（active 态用 accent）
- 柱状图 → recharts `BarChart` 堆叠：`inputTokens`（fill `var(--primary)`）+ `outputTokens`（fill `var(--primary)` 透明度变体或 `--secondary-foreground` 系）；`XAxis` MM-DD、`Tooltip` 显示请求/输入/输出；`ResponsiveContainer` 高 280
- 空态 → `EmptyState`（`BarChart3` 图标）
- 移除本页对 `legacy.tsx` 的引用

验证：构建零错误；图表数据与原手绘一致（同日同值）；范围切换联动。

## Task 4: 视觉验收 + 台账

- 双主题（亮/暗）截图对比；侧边栏折叠态
- `.superpowers/sdd/2026-08-31-ui2-phase1-gateway-stats/progress.md` 台账更新
- 提交：Task 粒度 commit，推送
