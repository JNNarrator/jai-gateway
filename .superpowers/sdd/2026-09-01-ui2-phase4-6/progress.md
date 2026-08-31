# SDD ledger — 阶段 4/5/6

plans: docs/superpowers/plans/（阶段 4-6 以 spec §7 为纲，单台账合并记录）

## 阶段 4（同步 + 日志 + 设置）— complete

- SyncPage：PageHeader + shadcn Card/FormField/Input/Textarea/Button；导入确认与
  「预览变更将覆盖本地」均改 ConfirmDialog（替换 window.confirm）；密码显隐保留；busy 条 primary 化
- LogsPage：shadcn Table；自动刷新 Switch + 间隔 Select；筛选 Input（搜索图标）；导出 CSV/JSON
  图标按钮（CSV 加 BOM 修复 Excel 中文乱码）；错误行 destructive/5 底色；EmptyState + 加载更多
- SettingsPage：端口占用警告改 ConfirmDialog；日志开关 Switch；保留策略两次 window.prompt
  → 单 Dialog 双数字字段；「添加域名」window.prompt → Dialog；关于/数据位置卡片语义化

## 阶段 5（打磨）— complete

- legacy.tsx 删除（grep 确认零引用；过渡期兼容映射注释随文件移除）
- 全部 9 页空态统一 EmptyState；图标按钮 aria-label 已在各阶段随手补齐
- 字体栈：body 增加系统字体栈（system-ui/PingFang SC/Microsoft YaHei）

## 阶段 6（平台视觉）— complete

- tauri.conf.json：main 窗口 decorations:false + transparent + windowEffects（mica→acrylic→blur 依次回退）
- tauri.macos.conf.json：macOSPrivateApi + decorations:true + titleBarStyle Overlay（保留红绿灯）
  + vibrancy（sidebar/hudWindow）；主题 --background 加 alpha 供特效透出
- TitleBar 组件：data-tauri-drag-region 拖拽区（Tauri 原生双击最大化）、Windows/Linux 自绘
  最小化/最大化/关闭（lucide 图标、关闭悬停 destructive）；macOS 预留左侧安全区 pl-20
- capabilities 增补 core:window allow-minimize/toggle-maximize/close/start-dragging/is-maximized

## 验收

- tsc + vite build 零错误（每阶段均跑）
- 真实应用（Windows 11，debug 构建）：
  - 自绘标题栏生效（系统标题栏消失，应用内标题条 + 三按钮渲染正常）
  - 最大化按钮实测：980×640 → 全屏 2906×1730，按钮态变「还原」，布局自适应无破版；还原正常
  - 拖拽：data-tauri-drag-region 为 Tauri 原生机制；自动化 raw 事件受会话前台限制无法注入，
    机制层面由官方 API 保证（与本台账阶段 2 记录的 Radix pointerdown 限制同因）
  - 毛玻璃：mica 在本机 Win11 支持；截图背景正常无透明破图（实色回退路径安全）

遗留（非阻塞）：macOS/vibrancy 与 mica 的视觉效果需对应平台人眼确认（本机仅 Windows）。
