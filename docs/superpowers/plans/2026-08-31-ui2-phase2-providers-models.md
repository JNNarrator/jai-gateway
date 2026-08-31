# JAI UI 2.0 — 阶段 2（供应商 + 模型）Implementation Plan

> 前置：阶段 0/1 已验收。范围：spec §7 阶段 2。
> 边界：`api.ts`/`types.ts`/Rust 零改动；禁止新增硬编码色值（状态绿/红等功能性色与阶段 1 同约定）。

**验收要点（spec）**：表单校验错误就地展示；模型行保存流程不回归。

---

## Task 1: 表单基建

- 依赖：`react-hook-form` + `zod` + `@hookform/resolvers`
- `components/common/FormField.tsx`：label + 控件 + hint/error 就地展示（error 用 destructive 色）

## Task 2: ProvidersPage 重做

- **新建供应商 → 居中 Dialog**（评审决定模态形态），表单迁 RHF+zod：
  - schema：name 必填；baseUrl 必填且 URL；family 枚举；apiKey 必填（创建）；priority int ≥0；weight int ≥1；追加请求头行 name 必填若 value 非空
  - 校验错误就地展示在每个字段下方
- **「从环境变量导入」window.prompt → 小 Dialog**（受控 Input，替代浏览器原生弹窗）
- **编辑供应商 → 复用同 Dialog**（编辑模式：apiKey 可空=不变）
- **列表卡片信息分层**：第一行 名称 + family `Badge` + 优先级/权重 + 缺少凭据警示 Badge + 最近成功/失败徽章；第二行 baseUrl 等宽；第三行最近成功/失败时间与错误消息
- 启用开关 checkbox → shadcn `Switch`；操作按钮 lucide 图标 + 文案；删除接 `ConfirmDialog`
- 页面骨架 PageHeader + 空态 EmptyState（KeyRound 图标）

## Task 3: ModelsPage 重做

- 供应商切换 select → shadcn `Select`；搜索框 → `Input` + Search 图标；排序 → Button + ArrowUpDown；批量启用/禁用保留 + toast
- 手写 table → shadcn `Table`（TableHeader/TableRow/TableCell/表头 muted 样式）
- 行内编辑保留流程：alias/ctx/out 用 `Input`；启用 checkbox → `Switch`；保存按钮带已保存反馈
- 空态 → `EmptyState`（Boxes 图标，区分「无模型」与「无匹配」）

## Task 4: 验收 + 台账

- 构建零错误；表单空提交实测校验错误就地展示；模型行保存/批量操作不回归
- 双主题截图；台账 `.superpowers/sdd/2026-08-31-ui2-phase2-providers-models/progress.md`
