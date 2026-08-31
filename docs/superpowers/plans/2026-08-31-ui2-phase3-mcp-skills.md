# JAI UI 2.0 — 阶段 3（MCP + 技能）Implementation Plan

> 前置：阶段 0–2 已验收。范围：spec §7 阶段 3。
> 边界：同前（数据层/Rust 零改动；语义变量）。

**验收要点（spec）**：导入/导出/测试连接流程不回归。

---

## Task 1: McpPage 重做

- 新建/编辑 MCP Server → 居中 Dialog（表单受控 state 即可，args JSON 校验保留就地展示）
- 「粘贴 JSON 导入」→ Dialog（textarea + 从剪贴板粘贴按钮保留）
- 删除 → ConfirmDialog；启用 checkbox → Switch；操作按钮图标化（测试连接/列出工具/编辑/删除）
- 空态 EmptyState（Puzzle 图标）；PageHeader + 三个页级操作（复制客户端配置 / 粘贴 JSON 导入 / 添加）
- 移除 legacy 引用

## Task 2: SkillsPage 重做

- 新建/编辑技能 → 居中 Dialog（替换常驻展开的内联表单——编辑改为点「编辑」按钮弹出）
- 批量操作条语义化（选中数 + 批量启用/禁用/删除/取消）；批量删除 → ConfirmDialog
- 启用 checkbox → Switch；选择 checkbox 保留；删除按钮 → ConfirmDialog + 图标
- 空态 EmptyState（Sparkles 图标 + 创建示例技能引导按钮）；拖拽导入 ZIP 流程原样保留
- 移除 legacy 引用

## Task 3: 验收 + 台账

- 构建零错误；真实应用走查：MCP 导入/列出工具/删除、技能创建/批量操作不回归
- 台账 `.superpowers/sdd/2026-08-31-ui2-phase3-mcp-skills/progress.md`
