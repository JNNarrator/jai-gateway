# SDD ledger — plan: docs/superpowers/plans/2026-08-31-ui2-phase2-providers-models.md

Task 1: complete — FormField（label+hint/error 就地展示）入库；react-hook-form + zod + @hookform/resolvers 安装
Task 2: complete — ProvidersPage 重做：新建/编辑统一居中 Dialog（RHF+zodResolver，create/edit 双 schema）；window.prompt 换 EnvVarDialog；列表信息分层（Switch/ family Badge/ 凭据警示/ 最近成败徽章/ 时间与错误消息）；删除接 ConfirmDialog；操作按钮 lucide 图标化
Task 3: complete — ModelsPage 重做：shadcn Table 表格化；供应商 Select、搜索 Input+图标、排序 Button、批量启用/禁用；行内编辑 Input/Switch/保存流程保留；空态 EmptyState（区分无模型/无匹配）
Task 4: complete — 验收：
- tsc+vite 零错误
- 表单校验就地展示实测通过：空表单提交，「名称不能为空 / Base URL 不能为空 / API Key 不能为空」红色文字显示在各自字段下方（dev server + IAB 截图）
- 真实应用（debug 构建）暗色走查：供应商页 2 渠道信息分层渲染正常；模型页 14 行表格 + 行内编辑 + 保存流程无回归（保存后列表刷新无错误）
- 已知瑕疵（登记）：ModelsPage 描述文案首版混入统计页残留，已修正并重建

环境备注：
- zodResolver 与 z.coerce.number() 的 input/output 类型不兼容 → 改 z.number + register valueAsNumber
- 桌面端 Radix DropdownMenu/Dialog 仍不响应 AXPress；本阶段校验与列表验收分别走浏览器与真实应用互补
