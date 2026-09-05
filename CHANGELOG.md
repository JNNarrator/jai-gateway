# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- **日志详情与复现命令**（UX-T1）：点击日志行弹出详情（入站协议/路由/供应商/模型/耗时/token/错误），
  一键「复制为 cURL」生成可复现命令（请求体为占位模板，隐私基线不变——仍不落内容）；新增供应商筛选
- **Ctrl+K 命令面板**（UX-T2）：全局唤起——页面跳转 / 快捷操作（复制网关端点、WebDAV 立即推送/拉取）/
  搜索供应商与模型
- **健康自检横幅**（UX-T3）：网关页显示最近一轮健康检查摘要（时间 + 不可用供应商名单，
  全绿仅小徽章不打扰）；新 IPC `health_summary`
- **WebDAV 推送冲突可视 diff**（UX-T5）：推送前差异预警升级为明细弹窗——远端独有
  （将被覆盖丢失）与本地独有（将新增到远端）逐条列出后再确认；新 IPC `webdav_push_diff`

## [0.1.7] - 2026-09-05

### Added
- **能力声明 + 兼容性规划层**（`gateway-core::codec::capability`，借鉴 GodeX bridge）：协议族级
  六面能力表（参数/工具/tool_choice/响应格式/reasoning/流式 usage）+ 四级决策
  （supported/degraded/ignored/rejected），跨族转换先规划后编码，能力面拒绝统一收敛于此
  （错误码 `response_format_not_supported` / `tools_limit_exceeded` / `tool_choice_not_supported`）
- **json_schema 结构化输出降级执行**（决策翻转，见 docs/design/protocol-ir.md §3/§10）：
  openai 系上游原生外传 `response_format`（含 Responses `text.format` 形状还原）；
  anthropic/gemini 上游降级为「提示词指令注入 + 输出后 JSON 校验」（strict 时校验失败
  返回 502 `structured_output_validation_failed`）；连降级都无法表达时才 400
- **reasoning effort 完整闭环**：入站建模进 `SampleParams.reasoning_effort`（chat 的
  `reasoning_effort`、Responses 的 `reasoning.effort`）；出站 Native 档原样透传、
  anthropic 映射 `thinking` 开/关、无能力族忽略并 WARN
- **工具声明上限护栏**：工具数超目标族上限（默认 128）→ 400 `tools_limit_exceeded`
- **Codex 扩展工具折叠与还原**（§10 扩展工具降级矩阵）：shell / apply_patch / custom /
  local_shell 声明与调用折叠为 function 工具（固定名或原名 + function input_schema），
  Codex 客户端可经 JAI 接任意普通模型；回程按工具身份映射还原原始 item
  （`shell_call` / `apply_patch_call` / `custom_tool_call` / `local_shell_call`，
  含流式 item 类型与增量事件名还原）
- **能力告警进结构化日志**：跨族能力面降级 / Lenient 丢弃 / 流式丢帧从控制台
  `eprintln` 升级为 `emit_log`（error_kind `CapabilityWarn` / `SseParseWarn`），
  UI 日志页可见，不再仅落终端
- **协议标准字段静默忽略**：Responses 协议标准字段（`store` / `parallel_tool_calls` /
  `stream_options` / `metadata` / `user` / `include` / `previous_response_id`）转换丢弃
  属预期（如 `store` 请求服务端存储会话，转发网关不代管），不再刷 `CapabilityWarn`——
  告警只保留给真正的降级与陌生字段，避免 dsh 等客户端常规请求淹没日志
- **WebDAV 远端备份管理**（`webdav_backups_list` / `webdav_backup_restore` /
  `webdav_backup_delete`）：同步页新增「远端备份（WebDAV 目录）」区——PROPFIND 列出
  同目录 `jai-config.<时间戳>.json` 备份（时间/大小），可恢复指定版本到本地
  （与拉取同回声抑制，并把自动拉取基线对齐远端当前版本，防止恢复结果立刻被拉回去）、
  可删除（仅时间戳备份名，当前配置与无关文件拒绝；404 幂等）
- **远端路径透明化**：目录字段按 URL 语义百分号编码（空格/中文不再拼出非法地址），
  同步页新增「远端配置文件地址」实时预览 + 一键复制
- **自动同步失败系统通知**：自动推送/拉取失败时发系统通知（错误摘要 140 字符截断）
- **网关出站代理配置（D8）**：设置页新增「网络代理」卡片——HTTP(S)/SOCKS5 代理地址
  （可含 `user:pass@` 认证）+ 绕过列表（每行 host 或 `.suffix`，`*` 全过）+「测试连接」；
  上游模型 / 健康检查 / WebDAV 同步统一经代理出站，**保存后重启网关生效**（与端口约定一致）；
  关闭时代理行为与默认完全一致（零回归）

### Changed
- **推送前差异预警**：手动推送若远端有本机没有的供应商/模型，首次调用返回可读提示并在
  前端弹「仍然推送」确认框——防止多设备场景无意覆盖掉另一台设备刚加的内容；
  `webdav_push` 新增 `force` 参数（true 跳过预警）
- **远端备份保留策略**：`BACKUP_KEEP=10`，备份解析/清理只认 `jai-config.<digits>.json`
  形态（防误删，当前配置永不列入）

### Added
- **WebDAV 从本地快照恢复**：同步页新增「本地推送前快照」卡片与「从快照恢复」按钮
  （`webdav_snapshot_info` / `webdav_snapshot_restore`），推送前快照（`webdav_last_snapshot`）
  终于有回退入口；恢复后如开启自动推送，防抖会将其同步回远端
- **WebDAV 自动拉取 / 双向同步**：同步页新增「自动拉取」开关——与自动推送共用间隔，
  按导出 `exportedAt` 时间戳 last-write-wins（远端非空且比上次成功同步更新才导入；
  空远端不拉取，防远端空配置清空本机）；新增「上次自动拉取」状态显示

### Changed
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
- **WebDAV 备份被空配置覆盖（数据丢失）**：远端 `jai-config.json` 此前被直接 PUT 覆盖——另一台
  设备（空/新装配置 + 自动推送）会把远端完整备份覆盖成空文件（2026-09 实测事件）。现在：
  1. **推送前留存远端旧版**——每次覆盖前先把远端现有配置复制为同目录 `jai-config.<时间戳>.json`
     （备份失败即中止推送，不盲覆盖），远端备份永不丢失；
  2. **自动推送护栏**——本地为 0 供应商/0 模型而远端有内容时，自动推送跳过并把原因写入
     「上次自动推送」状态（同步页可见），改由用户手动推送（手动推送不受限）
- **流式转换无终止标记上游流的缓冲护栏**（roadmap 稳定性 finding 修复）：转换路径 SSE 行缓冲
  加双重护栏——①单行超限（>1MiB 无换行刷流）立即断开并落日志，防无界内存；
  ②持续收字节但长时间（90s，`JAI_SSE_LINE_HOLD_SECS` 可覆盖）无完整行时断开，
  防「零字节挂起」拖死下游；passthrough 不受影响（逐块转发无缓冲）
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