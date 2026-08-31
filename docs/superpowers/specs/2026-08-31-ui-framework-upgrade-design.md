# JAI UI 2.0 — UI 框架引入与界面升级设计

- 日期：2026-08-31
- 状态：已评审（brainstorming 产出，待用户确认）
- 范围：`ui/` 前端（Tauri 2 + React 19 + Vite 6 + Tailwind），不涉及 Rust 后端与 `api.ts`/`types.ts` 数据层

## 1. 背景与目标

当前 UI 全部集中在 `ui/src/App.tsx`（2328 行，9 个 Tab）：组件手搓（`Card`、按钮/输入框样式类、CustomEvent 全局 toast）、无图标库、统计页柱状图用 div 手绘、仅暗色单主题。

本次升级三重目标：

1. **视觉质感升级**：现代、专业的开发者工具观感；明暗双主题；蓝紫主色
2. **交互组件补齐**：模态框、下拉、开关、表格、表单校验等组件的行为质量（键盘导航、焦点管理、无障碍）
3. **代码重构**：App.tsx 拆分为 `pages/ + components/` 结构，为后续迭代打基础

## 2. 决策记录

| 决策点 | 结论 | 备选与否决理由 |
| --- | --- | --- |
| 组件路线 | **shadcn/ui**（Radix 无头组件 + 源码拷入仓库） | Ant Design：观感强势、体积大、需 React 19 补丁、双样式体系；Mantine：与 Tailwind 双样式体系冗余 |
| 图标库 | lucide-react | — |
| 全局通知 | sonner | 替换现有 CustomEvent 方案 |
| 图表 | recharts | 替换统计页手绘 div 柱状图 |
| 表单 | react-hook-form + zod（仅复杂表单） | 简单表单保持受控 state，不强行套 |
| 主题策略 | CSS 语义变量（shadcn 约定）+ class 切换 + next-themes | — |
| 主色 | **蓝紫色系**（GitHub/Raycast 气质） | 放弃琥珀橙；青绿系落选 |
| 导航布局 | **左侧边栏**（图标+文字，可折叠为纯图标模式） | 顶部 9 页签在 760px 最小宽度下拥挤 |
| 迁移策略 | **分阶段逐步迁移**，每阶段结束应用可运行可验收 | 一次性重写风险不可控 |
| Tailwind 版本 | 3 → **4**（阶段 0 升级） | 项目 Tailwind 面小（一个 config + 3 行 css），升级风险低；shadcn 当前默认面向 v4。兜底：若升级受阻，退回 v3 + shadcn legacy 配置（hsl 变量），设计不受影响 |

## 3. 主题系统

- 所有颜色收敛为 shadcn 语义化 CSS 变量（`--background`、`--foreground`、`--primary`、`--card`、`--muted`、`--border`、`--destructive` 等约 20 个），明暗两套值定义在 `index.css` 的 `:root` 与 `.dark`
- 主色取蓝紫色系，明暗两套分别调校对比度（目标正文对比度 WCAG AA）
- `ThemeProvider` 采用 next-themes：默认跟随系统偏好，手动切换后持久化 localStorage，无闪白
- 迁移各 Tab 时，现有硬编码色值（`neutral-950`、`amber-600` 等）逐一替换为语义变量；阶段 0 结束后新代码禁止新增硬编码色值

## 4. 布局与导航

- **左侧边栏**：JAI 标识 + 9 个导航项（图标 + 文字，lucide 图标），底部为「设置」与主题切换器
- 可折叠为纯图标模式（约 56px），折叠状态存 localStorage；窗口最小宽度 760px 下两种模式均不换行
- 自建轻量侧边栏组件（`components/layout/SidebarNav.tsx`），不引入 shadcn 重型 Sidebar 组件——Tauri 窗口无移动端场景，YAGNI
- 保留系统原生标题栏（Tauri 默认 decorations），不自绘标题栏
- 主内容区：各页面自带 `PageHeader`（标题 + 说明 + 页级操作按钮区），统一各页头部的排版节奏

## 5. 目录结构

```
ui/src/
├── api.ts / types.ts        # 数据层边界，本次不动
├── main.tsx                 # 挂 ThemeProvider + Toaster（sonner）
├── App.tsx                  # 只剩外壳：SidebarNav + 页面挂载
├── index.css                # Tailwind v4 入口 + 主题变量（@theme）
├── lib/
│   ├── utils.ts             # cn()（clsx + tailwind-merge）
│   └── toast.ts             # toast(msg, kind) 封装，内部调 sonner；签名与现有 toast() 一致，调用点零改动
├── components/
│   ├── ui/                  # shadcn 基础件（button、input、dialog、switch、select、table、tooltip、badge、skeleton、card、label、dropdown-menu 等，按需拷入）
│   ├── layout/              # SidebarNav、ThemeToggle、AppShell
│   └── common/              # StatusBadge、CopyField（输入框+复制/显隐）、ConfirmDialog、EmptyState、PageHeader
└── pages/
    ├── GatewayPage/  SyncPage/  McpPage/  SkillsPage/
    ├── ProvidersPage/  ModelsPage/  StatsPage/  LogsPage/  SettingsPage/
```

- **拆分两步走**：阶段 0 先做纯机械搬移（代码原样挪入 `pages/`，视觉不变、`toast`/`goTab` 等跨文件调用改走 `lib/`），之后每个 Tab 的重写是独立小改动，可验收、可回退

## 6. 组件映射（手搓 → 新体系）

| 现状 | 替换为 |
| --- | --- |
| `btnPrimary / btnGhost / btnDanger` 样式类 | shadcn `Button`：default / outline / destructive 变体 |
| `inputCls` + 手写 label | `Input` + `Label`（复杂表单再加 `FormField`） |
| 手搓 `Card` | shadcn `Card`（Header / Title / Description / Content 结构） |
| `toast()` CustomEvent + App 内监听 | `lib/toast.ts` → sonner（成功/失败/加载中三态） |
| `window.prompt`（如供应商环境变量名输入） | `Dialog` 表单 |
| 危险操作无二次确认或仅 `window.confirm` | `ConfirmDialog`（轮换密钥、删除供应商/技能/MCP 等） |
| 统计页手绘 div 柱状图 | recharts：堆叠柱状图（输入/输出 token 分色）+ 时间范围切换联动 |
| checkbox 形式的启用开关 | `Switch` |
| 纯文字按钮 | lucide-react 图标按钮 + `Tooltip`（复制/眼睛/刷新/删除） |
| 空状态纯文字 | `EmptyState` 组件（图标 + 主文案 + 引导操作按钮） |

表单策略：供应商（新建/编辑）、MCP（导入/表单）、技能表单属复杂表单，迁移时引入 react-hook-form + zod（字段校验、错误提示就地展示）；网关、同步等简单表单保持受控 state。

## 7. 阶段划分

每阶段结束应用必须可运行、可验收（`pnpm build` 零错误 + 功能手测不回归）。

| 阶段 | 内容 | 验收要点 |
| --- | --- | --- |
| **0 基建** | Tailwind 3→4（`@tailwindcss/vite` 插件，删除 postcss 链）；shadcn 初始化 + 拷入基础件；双主题变量与 `ThemeToggle`；sonner 替换旧 toast（签名不变）；App.tsx 机械拆分到 `pages/`；侧边栏导航上线 | 双主题可切换且持久化；9 个页面功能与拆分前一致；`tsc --noEmit` 零错误 |
| **1 网关 + 统计** | 门面页视觉重做（状态卡片、接入卡片、代码片段区块）；统计页 recharts 图表 | 截图前后对比；图表数据与原手绘一致 |
| **2 供应商 + 模型** | 供应商新建/编辑表单迁 RHF+zod；列表信息分层 + 状态徽章；模型表表格化（`Table`）+ 搜索/排序/批量操作 | 表单校验错误就地展示；模型行保存流程不回归 |
| **3 MCP + 技能** | 列表 + `Dialog`/侧滑模态表单；空状态与批量操作升级 | 导入/导出/测试连接流程不回归 |
| **4 同步 + 日志 + 设置** | WebDAV 表单与预览流程；日志表着色/筛选/导出；设置页 toggle 化 | 同步与日志功能不回归 |
| **5 打磨** | 空状态统一、键盘导航、aria-label 补齐、删除死代码（旧样式类）、（可选）字体栈优化 | 全页走查清单通过 |

阶段顺序依据：先门面（视觉价值最高）→ 再高频交互（表单/表格）→ 后低频页面。

## 8. 数据流与错误处理

- `api.ts` / `types.ts` 完全不动——这是行为不回归的边界
- 所有异步操作的失败路径统一走 `lib/toast.ts` 的 error 态（保持现有 `toast(msg, "err")` 习惯）
- 列表类页面加载态用 `Skeleton`，替换现有的空白/闪烁
- `goTab` 跨页跳转（如「同步」页跳「网关」导出）改为页面级路由回调，去掉 CustomEvent

## 9. 验证标准（对齐 AGENTS.md）

- 每阶段硬门槛：`pnpm --dir ui build`（含 `tsc --noEmit` + `vite build`）**零错误零警告级类型问题**
- 行为不回归：各阶段验收清单逐项手测（对应 Tab 的完整功能流程）
- 视觉验收：关键页面截图前后对比；明暗两套主题各过一遍
- 完成声明前必须实际运行上述命令并贴出结果，禁止凭预期断言

## 10. 非目标（本次不做）

- 移动端适配（Tauri 桌面窗口，最小 760px）
- i18n / 英文界面（保持中文）
- 自绘标题栏、窗口毛玻璃等平台特效
- `api.ts`/`types.ts` 数据层改动、后端改动
- 单测框架引入（现有工程无 ui 测试设施，验收以编译 + 手测为准；可作后续独立事项）

## 11. 风险与兜底

| 风险 | 应对 |
| --- | --- |
| Tailwind v4 升级遇阻（类名变更影响现有 markup） | 兜底退回 v3 + shadcn legacy 配置（hsl 变量），阶段划分不变 |
| sonner 替换 toast 后行为差异（叠加多条、时长） | `lib/toast.ts` 统一封装层隔离，调一处即全局生效 |
| recharts 与 React 19 兼容 | recharts ≥2.15 已支持 React 19，锁版本验证 |
| 分阶段迁移期间新旧样式并存显得割裂 | 阶段 0 完成主题变量基座后，未迁移页面应用兼容样式兜底（全局基础样式微调），且阶段 1–4 间隔尽量短 |
