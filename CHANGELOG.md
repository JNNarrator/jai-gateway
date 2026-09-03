# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- **供应商官网字段**：供应商表单新增「官网」（可空），列表卡片一键跳转系统浏览器
  （tauri-plugin-opener）
- **WebDAV 密码回显**：同步页密码框回显已保存密码（明文入库后与网关 Key 同级展示语义）

### Changed
- **密钥迁入 SQLite，钥匙串退场**（migration `0006_secrets_in_db`）：供应商凭据明文存
  `providers.api_key`（与网关 Key / MCP env 同级安全模型，安全性依赖数据目录文件权限）。
  启动时一次性迁移钥匙串存量（`jai/provider/{id}`、`jai/webdav`）→ 入库 → 删除钥匙串项 →
  置 `keyring_migrated` 标记位；迁移改为后台执行且不持 DB 锁，授权弹框不再阻塞启动。
  此后转发、导入导出、启动**零钥匙串访问**；`vault_storage_kind` 命令、文件降级存储
  （vault_fallback.json）与 UI 存储类型显示全部移除
- **WebDAV 同步协议扩展（jai-export/v1）**：导出 providers 携带 `api_key`/`website`，
  顶层新增 `gateway_key`（当前 active），meta 全量导出（WebDAV 密码随行）；
  导入时远端 `api_key` 非空才覆盖本地、新建供应商直接带凭据（导入后立即可路由）、
  `gateway_key` 与本地不同才轮换（吊销旧 key 保留审计）、meta 按白名单应用
  （仅 WebDAV 连接配置与密码）
- 网关 Key 手动重新生成后触发自动推送防抖通知（配置 WebDAV 即自动更新远端）

### Fixed
- **日志「输入/输出」token 数恒空**：修复流式 usage 抽取器两处缺陷——
  `"usage":null` 后 32 字节窗口误命中下一 SSE 行触发错误收集且线索被丢弃
  （glm-5.3-flash 中转流 17 个 null + 末尾 usage 帧实测恒空）；现在 `null` 显式跳过、
  关键字与值跨 feed 分割时悬挂判定（64 字节上限保护），回归测试覆盖整段/逐字节喂入。
  转换路径流式结束时透传 IR 累计 usage 落日志（此前硬编码 None）

### Added
- **MCP 导入自动识别三种格式**（`mcp_import` 命令，原 `mcp_import_from_json` 更名）：
  1. `{"mcpServers": {...}}` JSON（Claude Code / Claude Desktop，另兼容无包装的裸对象）
  2. Codex CLI 命令行：`codex mcp add <名称> --env K=V -- "命令" [参数...]`
     （`claude mcp add` 同构兼容；无 `--` 时名称后第一个位置参数为命令/URL）
  3. Codex `config.toml` 片段：`[mcp_servers.<名称>]`（支持 `command`/`args`/`env`
     与 `url`/`transport`），TOML→JSON 后复用同一条目解析
- **应用内更新**：设置页「软件更新」卡片——打开时静默检查 + 手动检查更新、
  进度条下载安装（tauri-plugin-updater，minisign 签名校验）、完成后一键重启
  （tauri-plugin-process）；更新源为 GitHub Releases 的 `latest.json`
- `/mcp` 元数据 MCP Server（Streamable HTTP，与网关共用端口与鉴权）：
  把网关登记的 MCP Server / Skill 台账以 MCP 协议暴露给 Agent——五个只读工具
  （`list_mcp_servers` / `get_mcp_server_detail` / `get_tool_schemas` /
  `list_skills` / `get_skill_detail`），不注入对话链路、不代执行工具；
  env 仅回键名不回值。网关页提供 `mcpServers` 接入配置一键复制（复制时自动填入真实密钥）

### Changed
- **MCP / Skill 不再注入对话链路**：删除网关侧 MCP 工具自动合并与自动执行循环、
  Skill 注入 system 逻辑；同族请求恢复纯字节直通。工具执行统一由客户端侧完成，
  消除网关代执行导致的对话链路污染。管理页面保留（连接测试/工具查看/导入导出）

### Fixed
- 修复启动崩溃（v b8503f1 引入）：Tauri `setup` 闭包不在 tokio runtime 上下文，
  `spawn_autopush` / `spawn_health_check` 裸 `tokio::spawn` 启动即 panic；
  改用 `tauri::async_runtime::spawn`

### Added
- UI 2.0 界面升级（阶段 0–6，spec 见 `docs/superpowers/specs/2026-08-31-ui-framework-upgrade-design.md`）
  - 基建：Tailwind 3→4、shadcn/ui 基座、蓝紫明暗双主题（next-themes）、sonner 通知、
    可折叠侧边栏导航、App.tsx 拆分 pages/ + components/
  - 全部 9 页迁移语义化组件（Card/Button/Dialog/Switch/Select/Table/Tooltip），
    移除过渡期 legacy 样式；列表加载态 Skeleton
  - 供应商/MCP/技能表单 Dialog 化：react-hook-form + zod，校验错误就地展示；
    window.prompt/confirm 全部替换为 Dialog/ConfirmDialog
  - 统计页 recharts 堆叠柱状图（输入/输出分色 + 单日明细 Tooltip）
  - 阶段 6 平台视觉：自绘标题栏（Windows/Linux 自绘窗口三键，macOS overlay 保留红绿灯）、
    窗口毛玻璃特效（mica/acrylic/vibrancy，不支持平台实色回退）
- 全新品牌 logo（J + AI 星火，蓝紫渐变）与应用图标全套（ico/icns/PNG/Square）、UI favicon
- M6: OpenAI Responses API 入站（`POST /v1/responses`，Codex 原生线）
  - Responses ↔ IR 编解码、SSE 事件流、错误形状
- M7: 配置导入 + WebDAV 同步
  - `jai-export/v1` 导入、按名称+Base URL 去重 uplift、缺失密钥报告
  - WebDAV 手动推/拉、推送前快照、last-write-wins；UI「同步」页
- M8: 收尾加固
  - 全矩阵回归脚本 `scripts/regression.sh`
  - 500 随机 body 解码器 fuzz 简表
- M9: 发布工程
  - CHANGELOG、发布检查单、tag 触发 CI（签名/公证需真实 secrets）
- MCP Server 管理：`mcp_servers` 表 + CRUD IPC + UI「MCP」页；支持 tools/list、tools/call 客户端调用（stdio/http/sse）
- MCP 工具自动合并 + 自动执行循环：网关把启用 MCP 工具注入请求工具定义；上游发起 MCP 工具调用时自动执行并回填结果继续生成
- 高级路由：模型别名/映射（upstream_model_id）、同优先级权重负载均衡、基于最近成功/失败的健康感知排序
- 用量统计：新增「统计」页，展示近 7/30/90 天请求数与 Token 用量柱状图
- 旧版 `POST /v1/completions`：支持 openai_compat 渠道字节级直通
- MCP 工具列表缓存：自动合并路径增加 30s TTL 缓存，避免每个请求都去 MCP Server 拉取工具
- 发布门禁脚本 `scripts/release_check.sh`：一键检查工作区/版本/CHANGELOG/tag 并跑全量回归
- dsh 真机联调：修复 Responses+MCP 合成 SSE 与真实 API 形状不一致的问题（event 行、output_text.done、content_part.done、[DONE]）
- Responses 流式转换也补齐真实 SSE 事件行与 done 事件，保证 dsh/zcode 经 JAI 与直连体验一致
- 新增 `docs/test-report-dsh.md` dsh 真机测试报告
- README 明确：优先支持国产 Agent（dsh / zcode）
- MCP 配置托管：新增「复制客户端配置」，导出标准 `mcpServers` JSON，供 Claude Code / Continue 等 Agent 加载
- Skill 导出：新增「复制技能包」，导出启用技能 Markdown 文本，供 Agent 加载/查看
- zcode 接入指南：`docs/zcode接入.md`，按本机 zcode 配置确认 Anthropic 协议线
- 技能（skill）管理：`skills` 表 + CRUD IPC + UI「技能」页；跨族转换请求自动注入启用技能到 system

### Changed
- README/roadmap 同步至 M9 + MCP/Skill 基础管理
- README 更新：国产 Agent 优先支持说明、新 logo

### Fixed
- MCP stdio 客户端把服务端通知行误当响应帧，导致「列出工具/工具调用」报
  「MCP 响应缺少 result」；现跳过无 id 的通知帧与非 JSON 噪音行
  （dsh 真机回归：server-everything 13 工具列出、echo 工具循环端到端通过）
- 流式转换首字节含完整 SSE 流时未消费行缓冲的问题
- Anthropic SSE 渲染缺失 `content_block_stop`、交错 tool_calls 顺序问题

### Added
- **统一 MCP 代理（M1）**：`/mcp` registry 动态聚合可代理 Server 的全部工具为
  `<server>__<tool>` 命名（按首个双下划线分割，server 名可含下划线），dsh 只注册
  `jai-registry` 一个入口即可发现并调用所有已配置 MCP 工具；网关显式转发
  `tools/call` 到真实 Server（复用 mcp.rs 客户端），`description` 标注
  `[proxy: <server>]`，30s TTL 缓存动态工具列表，单个 Server 拉取失败跳过不阻塞
- **Skill 投递（M2）**：registry 动态生成 `skill__<name>` 工具，agent 按名调用即投递
  Skill 全文（32KB 截断并注明），保持「目录 + 按名加载」拉取模式，不注入 system、
  不代跑工具循环
- **MCP 代理权限边界**：`mcp_servers.proxy_allowed` 开关（默认关，migration 0007），
  未开启的 Server 工具不可见、不可调用；UI「MCP」页每行新增「代理」Switch

### Changed
- **MCP stdio 进程池**：`run_stdio_jsonrpc` 重构为「懒初始化 + 进程复用」——
  每个 `(cmd,args,env)` 内容指纹维护一个可复用进程（env 值只哈希不落池防密钥泄漏），
  首次调用 spawn+initialize、后续 tools/list、tools/call 复用（JSON-RPC id 递增）；
  空闲超时回收（默认 30s，另挂 30s 后台清扫，Server 停用后不留孤儿进程）、
  进程崩溃/无响应自动重建、每次调用超时保护（默认 120s，旧实现无限挂起）；
  同连接请求锁串行化，不同 Server 独立连接。冷启动从每次 ~198ms 降到仅首次/回收后
- **代理转发审计**：独立表 `proxy_call_logs`（migration 0008），每次代理调用记录
  `{ts, server, tool, kind, status, duration_ms, error}`，失败也记、异步落库，
  与 `request_logs` 的 `route_mode` CHECK / usage 聚合完全隔离

### Fixed
- `/v1/models` 暴露 `contextWindow`、usage 输出 cache 细分（prompt_tokens_details
  cached_tokens）、解析 `choices:[]` 的 usage 末帧，修复 ctx 占用指标
- **WebDAV「测试连接」误判**：原用 OPTIONS 探测，DUFS 等服务器匿名放行
  （实测免认证 200），凭据无效也误报「连接成功」；改用带凭据的
  `PROPFIND Depth:0` 真实校验（sync::probe），401/403 → 「认证失败：用户名或
  密码不正确」，404 → 「路径不存在」，拉取/推送错误同步分类提示
  （401/403 认证失败、拉取 404 远端尚无配置文件、推送 404 目录不存在）