# SDD ledger — plan: docs/superpowers/plans/2026-08-31-ui2-phase3-mcp-skills.md

Task 1: complete — McpPage 重做：新建/编辑 MCP Dialog、粘贴 JSON 导入 Dialog（剪贴板粘贴保留）、
  删除 ConfirmDialog、Switch、kind Badge、图标按钮组、EmptyState（Puzzle 图标）
Task 2: complete — SkillsPage 重做：新建/编辑技能 Dialog（替换常驻展开内联表单，编辑改点「编辑」弹出）、
  批量操作条语义化、批量删除 ConfirmDialog、Switch、拖拽导入 ZIP 原样保留、
  EmptyState（Sparkles + 创建示例技能引导）；补 shadcn Textarea 组件
Task 3: complete — 验收：
- tsc+vite 零错误
- 真实应用暗色走查：MCP「列出工具」真实流程不回归（jai-tools 13 个工具正常显示在信息条）；
  技能页 2 技能卡片渲染正常（暗号/代码审查），常驻编辑表单已消除

环境备注：SkillsPage 删除确认的联合类型窄化触发 TS2367，改用直接窄化写法。
