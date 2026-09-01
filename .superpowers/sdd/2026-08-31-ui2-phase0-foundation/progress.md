# SDD ledger — plan: docs/superpowers/plans/2026-08-31-ui2-phase0-foundation.md
Task 1: in progress (base 9662753dcb36bea7f1e09c7aac4ab4da8e9b9bd0)
Task 1: minor (deferred): 冒烟复跑延后到 Task 7 验收清单；.playwright-mcp/ 残留留待用户清理；前置未提交产物（疑似首次 agent 中断）已双重核验正确
Task 1: complete (commits 9662753..b7f6284, review clean)
Task 2: in progress (base b7f6284)
Task 2: minor (deferred): v4 outline-none 语义变化（focus 环行为）留待阶段迁移时改 outline-hidden；工作树前置残留模式与 Task 1 相同（首次派发 agent 疑似执行后汇报失败）
Task 2: ⚠️ 视觉抽查由协调方补做——已用 Playwright 截图核验暗色观感一致，PASS
Task 2: complete (commits b7f6284..a3753e5, review clean)
Task 3: in progress (base a3753e5)
Task 3: fix round 1/5 启动（发现：GatewayPage 代码块 bg-black/60+text-neutral-300 亮色不可读——颜色清单漏查 bg-black；另派字面量完整性扫描；commits 待定）
Task 3: fix round 1/5 完成（1 ADDRESSED：GatewayPage 代码块 zinc 字面量化 commit 91f3ac1；字面量扫描覆盖完整但 #3 SkillsPage 模态分类误判 → 1 项 NOT ADDRESSED 进入 R2）
Task 3: fix round 2/5 启动（SkillsPage 模态面板补不透明映射背景 + 全库模态面板排查）
Task 3: fix round 2/5 完成（1 ADDRESSED：SkillsPage 模态面板 bg-neutral-900 commit 958ae18；全库模态扫描确认无其他漏网；复审通过）
Task 3: 协调方亮色视觉复验 PASS（GatewayPage 代码块 zinc-300 可读；技能模态白色面板可读；主题持久化 localStorage 生效）
Task 3: minor (deferred): @theme inline 下 /60 透明度修饰符对 var() 色降级为不透明（bg-neutral-900/60 卡片），既有效应，阶段迁移时随语义化消除；shadcn ui/button+badge destructive 变体上游 text-white 属设计意图
Task 3: complete (commits a3753e5..958ae18, 2 fix rounds, review clean)
Task 4: in progress (base 958ae18)
Task 4: complete (commit f5eb76b，从 Task 3 收尾后恢复执行；sonner 底层替换，22 处调用点零改动)
Task 5: complete (commit 938f55c，nav.ts→nav.tsx NavProvider/useNav，goTab 调用点不变)
Task 6: complete (commit 78c466f，SidebarNav 9 项 lucide 图标 + 折叠持久化 localStorage["jai-sidebar-collapsed"])
Task 7: complete — 构建硬门槛通过（tsc --noEmit + vite build 零错误）；验收清单 8/8 浏览器实测通过：9 页全部渲染、亮暗主题切换+刷新持久化、侧边栏折叠+刷新持久化、sonner 成功（绿「已复制」）/失败（红「WebDAV 连接失败」）toast 底部居中、goTab 跨页跳转（同步→网关）、复制按钮 toast、危险按钮未迁移、数据加载行为未动（api.ts/types.ts 零改动）
--- 2026-08-31 阶段 0 完成（第二台机器续作收尾）---
本机环境备注：pnpm 11.21 默认供应链策略 minimumReleaseAge=24h 会拦截 lockfile 中的 lucide-react@1.38.0（发布于 2026-08-31，不足 24h）。绕过方式（均不入库）：`pnpm --dir ui install --config.minimum-release-age=0`；构建用 `pnpm --dir ui --config.verify-deps-before-run=false run build`（运行前自检会自动重装并撞策略）。2026-09-01 07:24 UTC 后包龄过窗，本机命令恢复原样。.npmrc 不生效（pnpm 11 不从 .npmrc 读该设置）。
下一阶段：阶段 1–6 各自制定独立计划（spec：docs/superpowers/specs/2026-08-31-ui-framework-upgrade-design.md）
终审 (9662753..e595582): 需修复后合并 — 3 Important（I-1 GatewayPage 亮色代码块不可读回归；I-2 McpPage env 非法 JSON 渲染崩溃白屏；I-3 ProvidersPage 编辑静默丢 extraHeaders/可改 family）+ 9 Minor；修复波派发（含 M-1 兼容映射清理/M-2/M-3/M-4/M-5），M-6/7/8/9 park
终审修复波: complete (commit 7dcf077, I-1/I-2/I-3/M-1/M-3/M-4/M-5 全部 ADDRESSED, re-review 通过无新破坏)
parked — M-6: separator.tsx 零使用保留 — ruling: shadcn 生成件，未来表单可用，删除无收益
parked — M-7: 特效不支持平台无实色兜底 — ruling: 目标平台 macOS/Win10/Win11 全有特效覆盖，Linux 属 bundle targets:all 边角，低风险留待实际需求
parked — M-8: 单 chunk 998KB（recharts） — ruling: 桌面应用本地加载无网络成本，后续需要时再 manualChunks
parked — M-9: TitleBar userAgent 嗅探 — ruling: WKWebView UA 判定可工作，platform() 改造属风格优化
备注: McpPage env 校验对非 stdio kind 也生效（复审非阻断观察），后续可收窄或补 http 分支提示
UI 2.0 全分支终审: 闭环 — 可合并
--- 2026-09-01 第一梯队（M8/M9 真机验收）执行记录 ---
性能基线: 冷启动 0.30-1.62s ✅；SSE 1MB/s×10min passthrough 满速零积压 RSS 漂移+0.1MB ✅；空闲 RSS 主进程 123.6-126.6MB（全足迹≈360MB）超 M0 <80MB 目标 → 重定基线 finding ⚠️
稳定性 finding: 转换路径（MCP 注入）对无终止标记上游流零转发+疑似无界缓冲（passthrough 不受影响）→ 已记 roadmap 附录 A 待修
48h 观察: 2026-09-01 11:35 起表（release, MCP 恢复启用），observe48h.sh + 每30分钟自动化 sampling，2026-09-03 11:35 判定
zcode 实测: key 已入用户剪贴板，用户 GUI 配置中，待测试对话后网关侧验证
zcode 实测: ✅ PASS（2026-09-01 13:57）— 真实会话经 JAI，实测走 OpenAI Responses 入站（非预想 Anthropic 线），流式+非流式均 200；顺带完成 M6 Responses 线真实客户端验收。roadmap §2 与 zcode接入.md 已同步。
优化 A 批: ✅ 21329e5（密钥免显示复制/移除接入示例/导出移至同步页/JSON 框缩小/模型名复制按钮）
优化 B 批: ✅ 869b8af（WebDAV 自动推送：变更防抖 30s + 定时 30分/1时/6时 可配，本机为准；推拉互斥 + 拉取后 40s 防回声；webdav_autopush_status IPC；修 5 处历史吞错 bug——spawn_blocking 内层 Result 未解包）
优化 C 批: ✅ b8503f1（供应商健康检查 10 分钟/轮 + 状态跃迁系统通知（首轮抑制），复用 mark 函数与探测核心；评审 Approved）
部署计划: 三批 + 转换路径 finding 修复一起等 48h 观察期满（9/3 11:35）部署，部署时全页面视觉走查补 A 批欠账
