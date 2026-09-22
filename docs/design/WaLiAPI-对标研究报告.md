# WaLiAPI 对标研究报告

> 调研对象：<https://github.com/fuzhengwei/WaLiAPI>（MIT，v0.3.5，2026-09-21）
> 调研方式：全量文本源码静态阅读（455 个文件 / 7.2 MB）+ GitHub API 元数据 + 逐模块源码走查（6 路并行探查）
> 调研日期：2026-09-22
> 定位：**同品类、同技术栈、同核心命题的直接竞品**；JAI 的协议内核更深，WaLiAPI 的产品面更宽、跑得更快。

---

## 0. 结论摘要

**确实是同类产品。** 一样的品类（本地 LLM API 网关桌面软件）、一样的技术栈（Tauri 2 + Rust/Axum + React）、一样的三协议命题（Chat Completions / Responses / Anthropic Messages 互转）、一样的「同族直通 + 跨族转换」双轨制，甚至连 `docs/superpowers/` 的 spec-driven 工作流都同源。默认监听、密钥前缀、页面划分也高度对应（它 `127.0.0.1:8777` + `sk-waliapi-*`，我们 `127.0.0.1:1314` + `sk-jai-*`）。

**但两边的「重心」完全不同：**

| | WaLiAPI | JAI |
| --- | --- | --- |
| 核心赌注 | **上游获取 + 知识资产 + 交付形态** | **协议保真 + 跨设备配置同步** |
| 路线 | 广度扩张（个人 → 团队/云） | 深度正确性（单一场景做到真的通） |
| 协议层 | 逐对有向 codec，ADR 明确**拒绝**大 IR | 统一 IR + 能力表六面 + 四级决策 |
| 独有资产 | Auth 账号（4 家 OAuth）、知识库 RAG、Wiki、安全审计、headless/Docker/Web 面板、29 个 MCP 工具 | 双轨制协议内核、WebDAV 多设备同步、dsh/zcode 真机联调、UI 门禁 + 视觉回归 + 负控制 |
| 协议内核质量 | 够用就好（`handlers.rs` 5,522 行、`driver.rs` 3,311 行） | **更系统、更严谨**（`protocol-ir.md` 定稿 + 逐字段映射总表） |
| 工程门禁 | 测试多（~1,117），**无 UI 门禁、无视觉回归** | 测试少（339），**有 UI 门禁 + 22 个视觉探针 + 负控制** |
| 组织形态 | 16 位贡献者 / 571 commits / 26 releases / 2 个月 | 单人 / 221 commits / ~1 个月 |

**一句话判断：JAI 的内核更强，WaLiAPI 的产品更宽。该抄它的「面」，同时保住自己的「深」——不要因为看它功能多就动摇双轨制 + IR 这条已经走对的路线。**

**最该立刻动手的三件事**（详见 §4）：
1. **headless / Docker 交付形态** —— 可同时绕开我们当前两个发布阻塞（macOS 只有 aarch64、签名公证未做），而我们的 `gateway-core` 已经不含 tauri 依赖，差距比它当初更小。
2. **渠道草稿测试**（保存前用真实最小推理请求逐端点探测）—— 消灭「配错一个字段，请求时才发现」。
3. **本机 AI 工具配置扫描导入**（`scan_local_ai_configs`）—— 直接命中我们「收敛杂牌 token 来源」的立项动机。

---

## 1. 项目速览（客观数据）

| 项 | WaLiAPI | JAI |
| --- | --- | --- |
| 创建 / 当前版本 | 2026-07-17 / **v0.3.5**（2026-09-21） | / **v0.3.1**（2026-09-22） |
| License | MIT | MIT |
| 提交数 | **571** | 221 |
| Release 数 | **26** | 3（tag 制） |
| 贡献者 | **16** | 1 |
| Stars / Forks | 136 / 43 | — |
| 分发渠道 | 官网 `walicode.xiaofuge.cn`、GitHub Releases、网盘、Docker Hub、GHCR、阿里云镜像 | 无 |
| 作者 | 小傅哥（fuzhengwei，技术教育博主，渠道优势明显） | — |

### 1.1 代码规模

| 项 | WaLiAPI | JAI | 倍率 |
| --- | --- | --- | --- |
| Rust 行数 | **103,558** | 37,372 | 2.8× |
| 前端 TS/TSX | **19,559** | 8,486 | 2.3× |
| 主要页面行数 | 12,449（9 页） | 4,845（9 页） | 2.6× |
| 数据库迁移 | **42** | 12 | 3.5× |
| `#[test]` / `#[tokio::test]` | **~1,117** | 339 | 3.3× |
| 文档行数（docs/） | 15,713 | 5,952 | 2.6× |

### 1.2 WaLiAPI 模块规模

```
server/            9,510   （handlers.rs 5,522 / router.rs 973 / admin_routes.rs 957）
protocol/         19,081   （codec 矩阵 + sse_bridge + directions 双向转换）
endpoint_executor/ 9,568  （driver.rs 3,311 / mod.rs 1,708 / sse.rs 799）
auth_provider/    13,250   （codex_login 2,093 / service 2,062 / grok 1,474 / kimi 1,146 …）
services/         18,076   （knowledge RAG + wiki + mcp + channel_test）
core/              7,601   （route_plan.rs 3,272 / attempt.rs 1,565 / channel_identity.rs 859）
db/                3,450
security/          3,372
adaptor/           1,659   （OpenAI / Claude / DeepSeek / Gemini / Custom）
commands/          9,426   （132 个 Tauri 命令）
```

---

## 2. WaLiAPI 架构剖析

### 2.1 分层与双产物形态

三层：`src/`（React 19 前端）→ `src-tauri/`（Tauri 2 壳）→ 同一套 Rust 代码库编译出**两种产物**：

- **桌面端**：`main.rs` + `lib.rs`，feature `default = ["desktop-ui"]`，产物 `.dmg` / `.msi` / `.deb` / `.AppImage` / `.rpm`。
- **Headless 服务端**：`bin/waliapi-web.rs`，`--no-default-features --features embed-web`，用 `rust-embed` 把 Web 面板资源嵌进二进制，无窗口纯 HTTP，用于 Docker / systemd。release profile 开 `lto="fat" + codegen-units=1` 以剔除桌面代码。

关键设计：`AppState` 是全局状态容器；headless 通过 `tauri::test::MockRuntime` 拿到 `State<'static, Arc<AppState>>` 复用命令分发；桌面专属代码一律 `#[cfg(feature = "desktop-ui")]` 门控。

**Web 面板复用同一套前端**（最值得学的一招）：

- `web/` 是 pnpm 子包 `waliapi-web`，`web/vite.config.ts` 用 alias `@app → ../src` 直接复用全部 pages / components，只把 `@tauri-apps/api/*` 与各 plugin 换成 `web/src/lib/*-shim.ts`（tauri / event / app / opener / dialog / updater / process）。`web/src` 自身仅约 310 行。
- 后端只暴露两个通道：`POST /admin/api/invoke`（命令名 + 参数透传）与 `GET /admin/api/events`（SSE）。
- 前端统一传输层 `src/lib/runtime.ts`（186 行）：桌面走 Tauri IPC `invoke`，浏览器走 `fetch /admin/api/invoke`。**新增前后端交互必须走该层**（AGENTS.md 硬约定）。

**对我们的直接意义**：我们 `ui/src/api.ts` 已有 73 个 `invoke` 包装、与 73 个 IPC 命令一一对应，`gateway-core/Cargo.toml` 确实没有 tauri 依赖，`examples/jai_standalone.rs` 已证明可无桌面壳启动网关。**我们做 headless 的改造成本比 WaLiAPI 当初更低**，缺的只是一个 `/admin/api/invoke` 等价端点 + 前端传输层抽象（我们目前直接 import `@tauri-apps/api`）。

### 2.2 协议层：关键分歧点

**WaLiAPI 不是统一 IR，而是「有向成对 codec 矩阵 + 静态策略注册表」。**

`protocol/codec/registry.rs::CodecRegistry::direction(downstream, upstream)` 用 `match (Protocol, Protocol)` 选静态策略：

- 对角线是 `CHAT_IDENTITY` / `MESSAGES_IDENTITY` / `RESPONSES_IDENTITY`（`codec/identity.rs`）。
- 非对角线是 `CHAT_TO_MESSAGES` / `MESSAGES_TO_CHAT` / `CHAT_TO_RESPONSES` / `RESPONSES_TO_CHAT`，以及 `directions::MESSAGES_TO_RESPONSES_V2` / `RESPONSES_TO_MESSAGES_V2`。
- Gemini 只做**出站**（`Protocol::Gemini` 不在入站枚举里），单独 `CHAT_TO_GEMINI`。

它在 ADR 里**明确拒绝建大 IR**（`docs/channel-refactor-tasks/00-architecture-decisions.md` 第 ① 条）：只建最小请求包络（`RequestEnvelope` / `AuditedRequest` / `PreparedAttempt`），理由是 IR 会丢保真度、且跨族方向本来就各不相同。

**这与我们的 `docs/design/protocol-ir.md` 是正面路线分歧。** 我的判断是**不需要改**，但要认清代价：

- 我们的 IR（`CanonicalRequest` / `Block` / `ToolSpec` / `StopReason`）+ 能力表六面 + 四级决策（Supported / Degraded / Ignored / Rejected）是 N+N 结构；它是 N² 结构。
- 它的优势是**逐方向可单独调优保真度**（`messages_to_responses` 写着「one pass，独立于其他方向，避免引入中间转换」）；我们的优势是**加一族 = 加两个适配器**，且能力规划集中、可测。
- 两边踩过同一批坑，说明这类问题与 IR 选择无关：tool_result 载体、stop_reason 枚举、system 位置、SSE 登记帧与增量帧的 id 硬契约、`[DONE]` 协议。
- **它有几处我们可核对的补丁**：`protocol/codec/sse.rs::record_end` 同时识别 `\r\n\r\n` 与 `\n\n` 取最小边界、`MAX_PENDING_BYTES = 32MB` 超限即换渠道；`identity.rs::capture_trailing_usage` 处理 `[DONE]` **之后**到达的尾部记录（仍记账但不投递）；`normalize_done` 把 `data: "[DONE]"` 规范为未加引号。

**同族直通两边几乎一模一样，是个好印证：**

| 环节 | WaLiAPI | JAI |
| --- | --- | --- |
| body | `build_upstream().body(body.clone())` 原样转发 | 同 |
| 流式 | `mpsc::channel::<Bytes>(64)` 逐块转发，全程无重编码 | 同 |
| 旁路观测 | `PassthroughStreamProbe` + `StopReasonProbe` + `UsageScanner` | `PassthroughStreamProbe` + `UsageScanner` |
| 直通路径上的例外 | `channel_policy_of()` 命中才做：`max_tools` 超限 400、`effort::normalize_body` 值域归一 + `CapabilityWarn`、`replay::inject_placeholders` 补非空推理占位 | `channel_policy_of()` / `effort::normalize_body` / `replay::inject_placeholders` |

结论：**「同族直通 + 只做渠道声明的归一」这个设计，两家独立收敛到了同一个形态**。我们的架构选择是对的。

### 2.3 路由与重试模型（差距最大的一块）

WaLiAPI 的模型远比我们精细：

- **六类失败分类**（`core/attempt.rs::FailureClass`）：`CallerTerminal` / `ChannelAuthTerminal` / `EndpointUnsupported` / `Retryable` / `UpstreamProtocolError` / `CommittedStreamError`。
- **两档预算**：`DEFAULT_MAX_ATTEMPTS_PER_GROUP = 3`、`DEFAULT_MAX_ATTEMPTS_TOTAL = 6`，由 `AttemptFlow::next_step` 状态机执行「组内预算耗尽 → 若可跨组才进下一组」。
- **候选分组的核心概念 `GroupTier::{Native, Conversion}`**：原生协议组与转换组分开，且**Native 组永不因优先级被 Conversion 组跳级**（测试 `higher_priority_first_but_conversion_never_leapfrog_native` 守着）。
- **跨组规则由 `is_degradable()` 决定**：只有 `EndpointUnsupported | Retryable` 允许跨组。Anthropic 容量错误（429/529）→ `Retryable` → 可跨到 Chat 组；而 **401/403 → `ChannelAuthTerminal` → 只在组内换候选，绝不跨组**（注释明确：避免语义/权限错误被另一协议掩盖）。
- **`non_idempotent` 请求不放宽预算**（Responses 带 `background` / `store` 时不重试）。
- **`Retry-After` 遵从**：解析 delta-seconds 与 RFC 7231 日期，重试前等待，上限 5 秒 + ±20% jitter。
- **配额 429 响应体按端点协议返回**并携带 `used/limit` 字段。

**JAI 现状**：`router/mod.rs`（294 行纯函数）→ 健康分组 → priority → `weighted_shuffle`，配 `classify_status` 真值表（`Failover{UpstreamAuth}` / `Stop{ContextTooLong, InvalidRequest}`）。**没有组概念、没有两档预算、没有跨组规则**。已核实：**我们没有 `Retry-After` 处理**（grep 只命中 `replay.rs` 里无关的 `should_retry_after_rejection`）。

> 这是**最高 ROI 的代码级借鉴**：不涉及架构变更，只是把 `proxy.rs` 的顺延循环改造成带组 / 预算 / 跨组语义的状态机，就能一次性解决「429 密集重试触发上游限流」「auth 错误被跨协议掩盖」「非幂等请求被重试」三类真实风险。

### 2.4 渠道体系

`channel_presets.rs`（1,172 行）定义了完整的领域身份：

- `ChannelProtocol{openai, anthropic, ollama}`、`ChannelProvider{openai, google, deepseek, qwen, zhipu, doubao, doubao_coding_plan, moonshot, stepfun, anthropic, ollama, custom}`、`NativeEndpoint{chat_completions, responses, messages, count_tokens, embeddings, api_chat}`、`AuthScheme{bearer, x_api_key, query_key, optional_bearer}`、`RegionGroup{custom, international, domestic, local}`、`ModelEnumStrategy{StaticOnly, StaticPlusSync, SyncOnly}`。
- 预设同时携带 `native_base_url` / `legacy_base_url` / `native_endpoints` / `default_checked_endpoints` / **`model_suggestions`（每条带 `verified_at` + `source_url`）** / `preset_revision`。排序恒为 custom → international → domestic → local，`custom` 每个协议都有且置顶默认。
- 身份解析由 `core/channel_identity.rs::resolve_channel_identity()` 唯一负责，`identity_revision = 0` 表示 legacy 未初始化（迁移 `015` 用 `AFTER UPDATE OF type, base_url, config ON channels` 触发器强制清空身份并要求事务内两步 UPDATE，旧 `type`/`base_url` 继续双写）。
- **多 Key 加权负载**：`channel_api_keys` 表（迁移 023），每 Key 独立权重、独立启用/禁用，主 Key + 扩展 Key 共同参与加权随机；两条转发路径（`proxy.rs` 与 `endpoint_executor/driver.rs`）都已覆盖。
- **渠道复制**（一键，名称加 `(副本)` 后缀，密钥清空待填）。
- **从 curl 导入渠道**：粘贴任意 OpenAI / Anthropic / Ollama 兼容的 curl 命令，自动解析并填充协议、Base URL、API Key、模型（前端 `lib/curl.ts::parseCurlToChannel`）。
- **复制测试 curl**：按渠道协议 / URL / 模型生成含真实 Key 的可执行 curl。
- **模型映射增强**：一对一 + 一对多数组（单目标 → 多目标，同优先级渠道间随机负载均衡）；**每条映射可单独停用**（迁移 041）；Auth 账号也有 `model_mapping_json`；`/v1/models` 聚合时把映射**源别名**也暴露，让客户端选得到别名。

### 2.5 渠道草稿测试（`services/channel_test.rs`，2,057 行）——强烈建议借鉴

这是我认为最实用的一块：**保存渠道之前**就能知道每个端点能不能用。

- `run_draft_test`：`build_draft_identity` → `validate_draft_url`（SSRF：仅 http/https；`localhost` / `127.` / `::1` 仅对 `ollama` / `custom` 放行；恒拦 `169.254/16`、`100.64/10`、`0/8`、组播/保留段、`fc00::/7`、`fe80::/10`）→ 逐端点 `probe_endpoint`。
- `probe_body` 构造**真实最小推理请求**（`max_tokens` / `max_output_tokens = 1`、`stream: false`），注释明确写「绝不用 `/models` 代替」。
- `DraftTestConfig` 默认 `per_probe_timeout = 15s`、`probe_wait_margin = 5s`、`total_cap = 60s`；`count_tokens` 端点被排除出探测；无模型则全部 `skipped`。
- 结果：`DraftEndpointTestResult{endpoint, status(passed|failed|skipped), category, message, latency_ms, tested_model, cost_possible}`；`classify_failure` 映射为 `authentication` / `endpoint_unsupported` / `model` / `request` / `timeout` / `network` / `protocol`；`sanitize_message` 同时脱敏明文与 percent-encoded Key（截 300 字符）。
- **保存门禁**：`compute_draft_fingerprint` = SHA-256(protocol/services/URL/端点/模型/timeout) + SHA-256(api_key)；`validate_save_receipt` + 进程内 `TestReceiptStore`（TTL，重启即失效）——即「测过了才能存，改了配置就得重测」。
- 本地拒绝规则：OpenAI 协议未勾选 Chat/Responses 直接拒绝。

**已知空白**：探测全部 `stream: false`，**没有流式与工具调用的专项测试**（这块覆盖在 `protocol/` codec 的测试里，不在渠道测试内）。借鉴时我们要自己补这两项——而这恰好是我们更有能力做的（我们已有 codec 层测试基建）。

### 2.6 Auth 账号体系（`auth_provider/`，13,250 行）——JAI 完全缺失的一大块

对象安全 trait `Provider`（`kind` / `login` / `import` / `import_all` / `refresh` / `outbound` / `list_models` / `fetch_quota`）+ `ProviderRegistry` 注册 4 家：

| Provider | 登录方式 | 关键常量 / 后端 |
| --- | --- | --- |
| **Codex** | 浏览器回调（PKCE S256，回调端口 1455/1457）+ **设备码**双通道 | `CODEX_CLIENT_ID="app_EMoamEEZ73f0CkXaXp7hrann"`、scopes 含 `api.connectors.read/invoke`、`codex_cli_simplified_flow=true`；额度走 `…/backend-api/wham/usage` 与 `x-<limit_id>-primary/secondary-*` 头解析 |
| **Kimi Code** | 纯 RFC 8628 设备码 | `KIMI_OAUTH_HOST=https://auth.kimi.com`、`KIMI_LOGIN_TIMEOUT=15min`；出站 `https://api.kimi.com/coding` |
| **Antigravity（Gemini）** | 浏览器 loopback OAuth（PKCE + state + `access_type=offline`） | client id/secret 由环境变量注入；出站 `https://daily-cloudcode-pa.googleapis.com`（`v1internal:generateContent/streamGenerateContent/loadCodeAssist/…`） |
| **Grok** | 设备码 + OIDC discovery | `GROK_ISSUER=https://auth.x.ai`、scope 含 `grok-cli:access`；`validate_oauth_endpoint` 仅允许 `https://*.x.ai` |

要点：

- **账号与渠道同为路由候选**（`RouteCandidate::{Channel, AuthAccount}`，ADR-9/11）。**没有全局「当前账号」概念**——按 priority/weight 抽样，`auth_reorder_accounts` 只改 `sort_order`。
- `auth_accounts` 表（迁移 019）用通用列 + `payload_json`，`UNIQUE(provider, account_id)`；`ProviderPayload` 的 `Debug` 输出固定为 `<redacted>`。
- 刷新：**每账号一个 Mutex** 串行化轮换（`refresh_locks` + `prune_idle_refresh_locks`），`needs_refresh` 阈值 5 分钟；失败 401 → 强制刷新 → 只重试一次 → `Unauthorized` + `schedule_maintenance_retry(12h)`。
- `maintenance.rs`：`MAINTENANCE_INTERVAL = 12h` 跑 `run_maintenance_cycle`（active 账号同步模型，invalid 账号按 `next_retry_after` 重试）。
- **错误语义分层（很精细）**：Auth 账号（凭证属用户本人，如 Kimi Code）上游 401/403 **保留真实状态码**，让调用方知道重新登录即可恢复；而渠道 Key 的终态失败**统一脱敏为 502**。故障转移语义不变（新增 `failure_from_auth_upstream` 只调状态码透传）。错误响应体新增 `failure_class` 字段。
- **多格式导入**：`AuthFileFormat::{Codex, Cpa, Sub2api}`。`CPA = CLIProxyAPI`（导入其 `auths/*.json`）；`sub2api` 识别 `type == "sub2api-data"`，`account_id` 优先序 `account_id > chatgpt_account_id > chatgpt_user_id`。**注意**：Gemini/Grok/Kimi 的 `import()` 实际一律返回 `ImportFailed`，与其设计文档声称的 `supports_import=true` 不一致（文档漂移）。
- **跨平台原子写入**（`write_auth_json_with_rename`）：备份 `auth.json.bak-<stamp>` → 写临时文件（`create_new`）→ `write_all` → `sync_all` → `set_private_permissions`(0o600) → `rename` → `sync_parent_directory`（Windows 为 no-op，原子性由 `rename` 保证）。

**这是 WaLiAPI 最大的产品级差异点**，也是它 2 个月拿到 136 star 的主要原因之一：让用户用**已有的订阅额度**（ChatGPT Plus/Pro、Kimi、Gemini、Grok）而不是按量付费的 API Key。

**但我建议谨慎对待**，理由有三：① 属于合规灰区（用订阅额度提供 API 服务）；② 依赖上游**私有/未公开端点**（Code Assist base URL、`wham/usage`），随时可能变，维护成本是持续性的；③ 需要真实账号才能测试，CI 无法覆盖。**是否做、做哪几家，是产品决策而非技术决策。** 若要做，`AuthAccount` 应复用我们已有的 `Provider` 抽象与路由候选模型，而不是新起一套。

### 2.7 安全审计引擎（`security/`，3,372 行）——JAI 完全没有

- **扫描对象**：对**原始下游协议 JSON 全树**逐字符串节点扫描（`walk_json`），按路径记录 `location`（如 `$.messages[0].content`）。分请求侧 `scan_request` 与响应侧 `scan_response` 两阶段（响应预算更宽）。
- **规则类别**：凭证（`sk-` / `ghp_` / `AKIA` / `AIza` / JWT / `Bearer` / PEM 私钥头）、敏感路径（`.env` / `~/.ssh` / `id_rsa` / `.aws/credentials`）、Unicode 隐写（零宽 U+200B/200C/200D/2060/FEFF、Bidi U+202A-202E、变体选择符）、外联工具（`ifconfig.me` / `webhook.site` / `ngrok`）、可疑工具调用（`curl` / `bash -c` / 外传组合）、追踪像素、**提示注入**（`prompt.injection` / `prompt.fingerprint_context`）。
- **六档等级 + 0-100 评分**：severity 基数 `info 5 / low 15 / medium 35 / high 65 / critical 90` 取 max，组合加成（credential+network +25、sensitive_file+network +25、unicode+network +15、shell+sensitive_file +20）封顶 100；阈值 ≥90 Critical / ≥65 High / ≥35 Medium。
- **五种策略**：只审计 / 警告 / 脱敏 / 阻断 / 确认（`Confirm` → 409 `approval_required`，fail-closed）。默认只审计。
- **预算与 fail-closed**：`ScanBudget{max_total_bytes: 32MiB, max_string_nodes: 50_000, max_depth: 256, max_elapsed: 800ms, max_text_bytes_per_string: 64KiB}`；超限返回 429 `security_scan_budget_exceeded`，**绝不报 clean**。
- **白名单短路 + 黑名单**：`is_whitelisted("keyword", …)` 命中即整段 return（豁免全部内置扫描）；`wl_path` / `wl_domain` / `wl_tool` 各自跳过对应扫描器；自定义规则 `CustomRule{rule_type: blacklist|whitelist, category: domain|tool|path|keyword, pattern, severity, action}` 小写子串匹配——**只对 blacklist 生效**（whitelist 不产生 finding）。
- **信任边界**：闸门产出 `forward_json` 与 `sanitized_log_json`，日志侧恒经 `redact_json_for_logging`（独立信任边界）；`body_hash = SHA256`。`gate_dispatch` 对 8 个 `DownstreamProtocol` 变体做**无通配 match（编译期清单守卫）**——新增协议忘了接审计会编译失败，这个手法很干净。
- `audit_delta_strings()` 供 codec / 流式路径做增量重扫。
- 落库：`request_logs` 加 `risk_level` / `risk_score` / `risk_summary` / `security_action` / `sanitized` / `blocked_reason`，明细进 `request_security_findings` 表（带 `evidence_masked` + `evidence_hash`）。

**对 JAI 的意义**：我们的定位是个人开发者本地代理，但 **agent 编程场景下 DLP 是真实需求**——防止 agent 把 `~/.ssh/id_rsa`、`.env`、云凭据发给模型。我们的日志本来就不落内容（隐私设计），所以审计的价值在**阻断 / 脱敏**而非记录，反而更契合。若要抄，建议**只抄「阻断 / 脱敏」这一半**，不要抄完整的审计日志与规则管理 UI。

**要引以为戒的一点**：它的 `security_builtin_rules` 表**只是 UI 元数据**——运行时扫描器不查该表，真正的开关是 `settings_store` 的 `security.scan_unicode|scan_tools|scan_network`，`toggle_key` 只是镜像设置键名。**即界面里显示的规则清单与实际生效的规则是两套东西。** 这是我们应该主动避免的「表里不一」。

### 2.8 知识库 RAG / Wiki / MCP（`services/`，18,076 行）

**这三块与我们的 MCP 定位是互补而非竞争关系**，需要分清：

| | WaLiAPI | JAI |
| --- | --- | --- |
| MCP 的定位 | 把**内置知识资产**暴露给 Agent（29 个工具 = KB 13 + Wiki 16） | 把**外部 MCP Server / Skill 台账**暴露给 Agent（5 个只读工具 + `<server>__<tool>` 代理转发 + `skill__` 投递） |
| 知识来源 | 网关自己爬 / 导入 / 索引（Git 仓库、URL 批量、本地目录、PDF / 代码 / Markdown） | 无内置知识库 |
| 代理执行语义 | 内置工具直接返回 JSON 文本 | **原样透传上游 `content` 块并冒泡 `isError`**（注释明确说早期「包成 `{source,result}`」是错误） |

**我们的代理转发语义更正确**（那条「错误语义要按调用方怎么判断成败设计」的教训我们已经在 `proxy_result_payload` 里落地了）。我们缺的是「本地知识资产」这一块。

WaLiAPI 实现细节里最有价值的是它的**踩坑记录**：

- **中文检索的两处硬功夫**：① `text.rs::normalize_radicals()` 只对 U+2E80–2FFF（CJK 部首补充）做 NFKC 归一——因为 PDF 文字层会把「日 / 方」提取为部首「⽇ / ⽅」；② `search_projection()` 生成中文重叠双字 + 单字投影，落到独立的 `kb_chunks.search_text` 列（迁移 037，不改历史正文 / 哈希 / 向量），启动时 `backfill_search_text()` 每批 128 条补建。查询与文档共用同一套分词（`query_tokens()` 去重、上限 64），FTS5 查询生成 `"tok"* OR "tok"*`（**用引号中和 AND/OR/NEAR 运算符**）。
- **Unicode 切片 panic 曾导致整个进程退出**：`ingest_wiki_source` / `search_wiki` 按字节下标在 Unicode 文本上切片触发 `core::str::slice_error_fail`，在 release `panic = "abort"` 下**整个 WaLiAPI 进程死掉**（PR #58）。对策是 `utils/text.rs` 字符边界安全切片工具，覆盖 wiki ingest / repository / security scanner 三条路径。—— 我们也要核对：`panic = "abort"` 下的任何 `str` 切片都必须走字符边界。
- **向量索引的工程取舍**：HNSW 是**自研单层图**（`DEFAULT_M=16` / `ef_construction=200` / `ef_search=50`），bincode 落盘 + 原子 rename，≤1024 节点直接精确排序；**索引与库内切片不一致时自动回退线性检索**（`index.len() != chunk_map.len()` 或有存活节点缺失），维度不符也回退。增量索引按 `content_hash` 复用未变块的 embedding + HNSW 单点插入 + 墓碑摘除，**跳过付费 embedding 调用**。
- **融合打分**：默认 RRF（`RRF_K = 60`，Σ 1/(k+rank)），可选加权（默认 0.7 / 0.3），两路各取 `top_k * 2` 后融合。
- **扫描版 PDF 的 VLM OCR**：页级判定（文字层 < 50 字符/页才走 VLM），pdfium 渲 JPEG（全局 Mutex 串行），按目标渠道协议适配（claude → `/messages`，其余 → `/chat/completions`），**不做全渠道 fallback**，无视觉渠道直接报错；输出注入 `<!-- page: N -->` 锚点并按页切片。
- **Wiki 与 KB 的本质区别**：KB 是 chunk 级向量 top-k；Wiki 是**页面级 + 图谱级**——由 LLM 把原始资料**编译成结构化知识条目**（Markdown + YAML frontmatter + `[[wikilinks]]`），落文件系统 + SQLite，检索是 `LIKE` + 逐页读盘子串扫描（**无向量、无 FTS5**）。这是一条「用 LLM 做编译而非做检索」的独立路线，与 agent 场景（生成 `SKILL.md`、项目知识卡）比 RAG 更契合。

### 2.9 交付形态与运维（第二个高 ROI 借鉴点）

WaLiAPI 的发布矩阵远宽于我们：

| 形态 | 实现 |
| --- | --- |
| 桌面 | Tauri 2，托盘常驻，`close_to_tray` 默认 true，自动更新（`tauri-plugin-updater` + GitHub/GitCode 双源 `latest.json`） |
| Headless | `waliapi-web` 二进制 + 子命令 `repair-stream-logs`（`--apply/--limit/--status-only/--usage-only`） |
| Docker | 三阶段构建：`node:22` builder（仅编译期装 GTK/WebKit）→ `pnpm --filter waliapi-web build` + `cargo build --bin waliapi-web --no-default-features --features embed-web` → `debian-slim` 运行时（非 root uid 10001、`/data` 卷、`EXPOSE 8777`、healthcheck `/health`） |
| systemd | `deploy/systemd/`，`StateDirectory=waliapi` 沙箱 + root-only env 文件（0600） |
| 反向代理 | `deploy/caddy/Caddyfile.example`，HTTPS 终止后反代到 `127.0.0.1:8777` |
| CI | 6 个 workflow：`macos-arm64-v*` / `all-v*`（Intel）/ `all-win-v*` / `linux-v*` / `web-v*`（tar.gz + GHCR + SBOM/attestation）/ `v*`（Docker Hub 多架构） |

**Web 管理面的鉴权设计值得单独看**：

- 三条**互不通用**的凭证域：数据面 `/v1/*`（`sk-waliapi-*`）/ Web 管理面 + KB/Wiki REST（管理员会话 + `WALIAPI_ADMIN_TOKEN`）/ MCP 端点（`WALIAPI_MCP_TOKEN`，须与 ADMIN 不同）。
- 管理员首次启动自动生成随机密码（stdout + 数据目录 `INITIAL_PASSWORD` 文件，**首次登录成功后文件即删除**）；密码用 argon2id 存 `admin_users`；会话是内存 Bearer + HttpOnly Cookie（`SESSION_TTL` 7 天），登录失败 `LoginThrottle` 指数退避，改密吊销全部旧会话。
- **CORS 作用域**：宽松 CORS（`Access-Control-Allow-Origin: *`）**仅**作用于 API Key 鉴权的数据面；管理面 / KB / Wiki / MCP 不带宽松 CORS，变更类请求另需 `X-Requested-With` 头（CSRF 防护）。这个「按信任域切分 CORS」的做法比一刀切干净。

**对我们发布阻塞的直接价值**：我们目前卡在 macOS 只有 `aarch64`（Intel Mac 无产物）与签名 / 公证未做。**Linux 二进制 + Docker 镜像完全不需要签名与公证**，是一条可以立刻打通的分发路径；而 `macos-13` 已下架、`-intel` 属 larger runners 按分钟计费的问题也随之绕开。

### 2.10 可观测性与日志策略

- **请求 ID 标准化**：`server/request_id.rs`，优先级 `X-Request-Id > Wali-Trace-Id > UUIDv4`，`MAX_REQUEST_ID_LEN = 128`，中间件回写并回显，落库为 `trace_id`。
- **OTLP 导出器**：`otlp_exporter.rs` **不引 OTel SDK**，用 OTLP/HTTP JSON 把 `request_logs` 增量导为 span；游标存 `meta["otlp.export_cursor"]`，**失败退避且不推进游标**（保证不丢）。这是个很轻的落地方式，值得参考。
- **流式内容段持久化**：SSE 生成内容**逐段落库**，连接中断后已生成内容仍可查看（迁移 032）。
- **Responses 断线续传**：`/v1/responses/{id}/events` 以 `response_id` 为锚、`offset` 起回放逐帧持久化内容；`stream.resume_ttl_secs` 默认 86400（过期 410）；无终止帧时补合成 `response.completed`(`status=incomplete`)。
- **日志三级**：基本 / 详情 / **简要**（简要只保留最新 3 条消息，长对话场景显著降低存储占用）+ `detail_level` / `started_at` 字段与索引 + 覆盖索引优化聚合查询 + 按日期删除 / 清空。
- **探测日志降噪**：健康探测**仅在状态翻转时**写审计行，恢复状态**就地更新**，不为每次探测新增日志。
- **语义缓存**：`semantic_cache.rs`，exact（SHA-256 + temp/max_tokens 分档）+ semantic（余弦）两层，命中回 `X-Cache: hit` 并照写日志行，默认关闭。
- **超时策略**（这条我们已踩过同样的坑）：流式与非流式用**不同的 reqwest client**——流式只设 `connect_timeout`(10s) 不设总超时，非流式另有渠道级 `timeout_secs`(默认 60s)。`driver.rs::StreamTimeouts`：首帧 60s + 空闲 120s。

### 2.11 工程化与组织方式

**WaLiAPI 的强项在组织，不在门禁：**

- **测试规模** ~1,117 个（我们 339 个）。数据库测试用**内存 SQLite + 跑真实迁移 SQL**，因此 cwd 必须是 `src-tauri/`。
- **门禁弱**：CI 只跑 `cargo fmt --check` + `clippy -D warnings` + `cargo test` + 前端 `tsc && vite build`。**没有 UI 静态门禁，没有视觉回归。** 这一点我们显著领先。
- **版本号四处同步**：`package.json` / `Cargo.toml` / `tauri.conf.json` / `Cargo.lock`（与我们三处同步类似）。
- **迁移前自动备份 DB，保留最近 3 份**（我们目前没有，见 §4）。
- **发布产物与 CHANGELOG 联动**：workflow 从 `tauri.conf.json` 取版本 + `awk` 抽 CHANGELOG 段落作为 release notes。

**组织方式（最值得研究的一块）**：

- **贡献者看板**：README 顶部有完整表格——提交数、代码变更行数、主要贡献，每人一个 @handle。**16 位贡献者 / 571 commits / 2 个月。**
- **每条 CHANGELOG 条目带 PR 号 + @贡献者**，可追溯到人。
- **功能被切成任务卡**：`docs/channel-refactor-tasks/T01–T14`（每个任务一份 `00-architecture-decisions.md` 级别的设计 + 验收），`docs/changes/<feature>/{plan,execution}.md`，`docs/superpowers/{plans,specs}/<日期>-<feature>.md`。**一个任务卡 ≈ 一个 PR**，外部贡献者准入门槛极低。
- **AGENTS.md（173 行）是给 AI 编码代理的上手文档**：项目概览、实际目录结构、构建命令、feature 开关、测试方式、认证体系、迁移规则、开发约定，一页说清；并**主动承认「README 的项目结构一节可能滞后于代码，以实际目录为准」**。

> 我们已经有 `docs/superpowers/{specs,plans}/` 与 `.pi/goal/`，方法论同源。差距在**任务卡粒度**与**贡献者可见性**。如果 JAI 打算开源运营，这套组织方式可直接复制；如果不打算，则价值主要在「任务卡粒度」这一半——它本身就是很好的上下文管理手段。

### 2.12 WaLiAPI 的明显短板（快速迭代的代价）

这些是「2 个月 26 个 release + 16 人协作」的副作用，值得我们引以为戒：

1. **巨型文件**：`handlers.rs` 5,522 行、`driver.rs` 3,311 行、`route_plan.rs` 3,272 行、`codec/gemini/mod.rs` 847 行。
2. **UI 显示与实际生效不一致**：`security_builtin_rules` 表只是 UI 元数据（见 §2.7）。
3. **文档与代码不同步**：模型上游同步的设计文档标注「真实前后端尚未实现」，但 `services/upstream_models.rs` 已实现；Gemini/Grok/Kimi 的 `import()` 与设计文档声称不符；README 项目结构与实际目录不符（AGENTS.md 自己承认）。
4. **陈旧产物**：`Dockerfile.tp` 的 COPY 目标与 ENTRYPOINT 路径不一致；`build.sh` 里 `IMAGE_TAG="0.2.5"` 落后于 0.3.5；`web/package.json` 仍列着已被主应用移除的 `zustand` / `@tanstack/react-query`。
5. **注册但从未调用的插件**：`tauri-plugin-notification` 有权限配置，但全仓找不到实际发送调用。
6. **无单实例保护**：SQLite 不支持多实例写同一数据目录，但没有单实例锁。（我们也没有，见 §4。）
7. **SSRF 覆盖不均**：知识库 URL 导入有完整的 `is_forbidden_ip`，但**渠道 `base_url` 未走同一校验**。
8. **导出含明文密钥**：渠道导出 JSON 包含明文 `api_key`（界面有警告，但与我们「导出剔除敏感字段」的主张相反）。
9. **README 塞进完整 CHANGELOG**：README 达 77 KB，阅读体验差。我们独立 `CHANGELOG.md`（104 KB）的做法更好。

---

## 3. 逐面对标表

> 图例：🟢 JAI 领先 ｜ 🟡 各有取舍 ｜ 🔴 WaLiAPI 领先（我们缺）

| # | 维度 | 判定 | 说明 |
| --- | --- | --- | --- |
| 1 | 协议转换架构 | 🟡 | 我们：统一 IR + 能力表六面 + 四级决策（N+N）。它：有向成对 codec 矩阵（N²），ADR 明确拒绝大 IR。各有取舍，**不必改**。 |
| 2 | 同族直通 | 🟡 | 两边几乎完全同构（字节转发 + 旁路探针 + 仅做渠道声明归一）。**相互印证了方向正确**。 |
| 3 | 路由与重试 | 🔴 | 它有六类失败分类、两档预算（组内 3 / 总 6）、`GroupTier{Native,Conversion}`、跨组规则（只有 Retryable / EndpointUnsupported 可跨组）、`Retry-After` 遵从 + jitter、非幂等不放宽。我们只有顺延 + 健康排序。 |
| 4 | 上游凭证获取 | 🔴 | 它有 4 家 OAuth（Codex / Kimi / Antigravity / Grok）+ 账号作为路由候选 + 额度查询 + 多格式导入（Codex / CPA / sub2api）。**我们完全没有。** 合规与维护风险需产品决策。 |
| 5 | 渠道配置体验 | 🔴 | 它有草稿测试（保存前逐端点真实推理探测 + fingerprint / receipt 门禁）、从 curl 导入、复制测试 curl、渠道复制、多 Key 加权负载、预设注册表（带 `verified_at` / `source_url`）。 |
| 6 | 模型映射 | 🟡 | 它多了「一对多数组」「每条映射可停用」「`/v1/models` 暴露映射源别名」。我们是单一 `upstream_model_id`。 |
| 7 | 模型元数据 | 🟢 | 我们有 LiteLLM 快照 + 输入/输出模态集合 + 供应商/模型级 reasoning 值域声明。它的能力表是**协议族级静态**，模型级 `target_model_capabilities` 明确未做。 |
| 8 | 多模态 | 🟢 | 我们有 `modality.rs` + `image.rs` 跨族转换。它的视觉能力路由是「长期方案 #15」，当前只在错误信息里追加诊断提示，**不改变路由行为**（fail-open）。 |
| 9 | 出站网络代理 | 🟢 | 我们有 `netcfg.rs`（HTTP/SOCKS5 + 认证 + 绕过列表 + 测试连接）。它只在渠道 `config.proxy` 层面按需解析。 |
| 10 | 配置同步 | 🟢 | 我们有 WebDAV 多设备同步（推/拉独立间隔 + 远端备份 + 冲突 diff + LWW 护栏）。**它没有跨设备同步。** |
| 11 | 桌面壳健壮性 | 🟢 | 我们有网关监督循环（watch + 崩溃 1s 重启 + 重启计数）。它只有服务级 `restart_server` 命令，无进程级看门狗。 |
| 12 | 系统通知 | 🟢 | 我们真实使用 `tauri-plugin-notification`（供应商健康跃迁通知）。它注册了插件但**全仓找不到调用**。 |
| 13 | 前端交互质量 | 🟢 | 我们有脏状态 guard（`UnsavedGuard` + `useDirtyGuard`，切页 / 关窗 / `beforeunload` 三层拦截）、`Ctrl+K` 命令面板、日志轮询随可见性暂停。它没有命令面板与统一脏状态机制。 |
| 14 | UI 门禁与视觉回归 | 🟢 | 我们有 `ui_lint.sh` + 22 个视觉探针 + 双尺寸双主题 + **负控制**（证明门禁真能红）。它**完全没有**。 |
| 15 | 测试规模 | 🔴 | 它 ~1,117 个测试，我们 339 个。我们的 `gateway-core/tests` 9,411 行覆盖不差，但**协议 codec 的边界用例密度不如它**。 |
| 16 | 交付形态 | 🔴 | 它有 headless 二进制 + Docker 多阶段 + systemd + Caddy 示例 + Web 管理面板（复用同一套前端）。我们只有桌面端。 |
| 17 | 团队 / 多人使用 | 🔴 | 它有密钥配额 + 渠道/模型黑白名单 + 知识库授权（`api_key_knowledge_access`）+ 管理员会话体系。我们只有单一网关 Key。 |
| 18 | 本地知识资产 | 🔴 | 它有知识库 RAG（HNSW + FTS5 + RRF + 中文部首归一）+ Wiki 引擎（LLM 编译结构化页面）+ 29 个 MCP 工具。我们只有 MCP 台账 / 代理。 |
| 19 | MCP 代理语义 | 🟢 | 我们**原样透传 `content` 块 + 冒泡 `isError`**（语义正确）。它的内置工具返回 JSON 文本（不同场景，但我们的转发语义更严谨）。 |
| 20 | 安全审计（DLP） | 🔴 | 它有完整的请求/响应双阶段扫描 + 六档评分 + 五策略 + fail-closed 预算 + 编译期清单守卫。我们完全没有。 |
| 21 | 可观测性 | 🔴 | 它有 `X-Request-Id` 标准化 + OTLP/HTTP JSON 导出（无 SDK，游标不推进保证不丢）+ 流式内容段持久化 + Responses 断线续传。我们只有元数据日志 + cURL 复现。 |
| 22 | 日志策略 | 🟡 | 它有三级日志（含「简要」）+ 探测日志降噪 + 覆盖索引。我们有 30 天 / 5 万行封顶 + VACUUM 回收。可互补。 |
| 23 | 迁移与运维 | 🟡 | 它有**迁移前自动备份 DB（保留 3 份）**。我们有快照机制但迁移前不备份。 |
| 24 | 单实例保护 | 🟡 | **两家都没有。** SQLite 不支持多实例写同一数据目录，这是共同风险。 |
| 25 | 导入导出的信任模型 | 🔴 | 它的 v2 身份字段走「白名单 + `identity_revision > 0` 才逐字信任，否则回落 legacy 推断」。我们的 WebDAV 是整份 LWW，schema 演进时的回灌风险需核对。 |
| 26 | 本机配置扫描导入 | 🔴 | 它有 `scan_local_ai_configs`（扫描本机已装的 AI 工具配置并导入渠道）。**直接命中我们「收敛杂牌 token 来源」的立项动机。** |
| 27 | 客户端接入脚本 | 🟢 | 我们的 `scripts/setup_dsh.mjs` 更严谨（dry-run diff + 幂等 + 只改自己那条 route + 运行时 schema 校验 + 时间戳备份）。它的应用配置胜在**覆盖 8 款工具**与**恢复语义的 absent 标记**。 |
| 28 | 组织与协作 | 🔴 | 16 贡献者 / 571 commits / 2 个月 / 26 releases + 贡献者看板 + 任务卡粒度 + 每条改动带 PR 与 @handle。我们是单人 221 commits。 |

**计分（粗算）**：🟢 JAI 领先 9 项，🟡 各有取舍 8 项，🔴 WaLiAPI 领先 11 项。**领先项集中在「协议内核 / 同步 / 桌面壳 / 门禁 / 交互」；落后项集中在「功能广度 / 交付形态 / 团队能力 / 组织」。**

---

## 4. 借鉴清单（按 ROI 排序）

### P0 — 立刻可做，成本小、收益直接

| # | 事项 | 来源 | 为什么值得做 |
| --- | --- | --- | --- |
| P0-1 | **渠道草稿测试**：保存前逐端点发真实最小推理请求（`max_tokens=1`、`stream:false`），结果按 `passed/failed/skipped` + 失败分类呈现；配 `draft_fingerprint` + 进程内 receipt（改了配置就要重测） | `services/channel_test.rs` | 消灭「配错一个字段，请求时才发现」。**我们要额外补它缺的两项：流式探测 + 工具调用探测**（我们已有 codec 测试基建，比它更有条件做对） |
| P0-2 | **从 curl 导入渠道** + **复制测试 curl** | `lib/curl.ts` / 渠道列表操作栏 | 用户从 DevTools 拷一条 curl 就能建渠道，摩擦极低；复制 curl 让排障不依赖 UI |
| P0-3 | **本机 AI 工具配置扫描导入**：扫描 `~/.claude/settings.json`、Codex `auth.json` / `config.toml`、Continue、dsh `settings.yaml` 等，识别其中已配置的 base_url + key + model，一键导入为渠道 | `commands/import_export.rs::scan_local_ai_configs` | **直接命中我们的立项动机**（「把杂乱的 token 来源收敛为本地稳定入口」）。用户本来就在多个客户端里散着配了各种 key，这是最短的迁移路径 |
| P0-4 | **迁移前自动备份 DB，保留最近 N 份** | `sqlx::migrate!` 前置步骤 | 我们已有 `store/snapshot.rs` 基建，接上成本极低，但能救「迁移炸了」这一类不可逆事故 |
| P0-5 | **重试模型升级**：引入 `GroupTier{Native, Conversion}` 分组 + 组内预算 + 跨组预算 + `is_degradable()` 跨组规则 + `Retry-After` 遵从（上限 + jitter）+ 非幂等请求不放宽预算 | `core/attempt.rs` / `route_plan.rs` | **最高 ROI 的代码级借鉴。** 不改架构，把 `proxy.rs` 的顺延循环改造成状态机即可，一次解决「429 密集重试触发上游限流」「auth 错误被跨协议掩盖」「非幂等请求被重试」三类真实风险。已核实：**当前没有 `Retry-After` 处理** |
| P0-6 | **导入 / 同步文件的身份信任模型**：外部导入的协议 / 供应商 / 端点等身份字段不逐字信任，走白名单 + 版本号校验，否则回落 `resolve_channel_identity` 推断 | `commands/import_export.rs::is_trusted_v2_identity` | 我们的 WebDAV 是整份 LWW；一旦 schema 演进（例如渠道模型加字段），旧文件回灌可能带进不一致的身份。加这层护栏成本很小 |
| P0-7 | **探测日志降噪**：健康探测仅在状态翻转时写日志，恢复状态就地更新 | CHANGELOG v0.3.2 | 我们的健康检查每 10 分钟一轮，需核对是否每轮都落库；若是，日志会被探测噪声淹没 |
| P0-8 | **模型映射增强**：每条映射可单独停用 + `/v1/models` 暴露映射源别名 | 迁移 041 / `handlers.rs` | 停用开关让「临时改映射」不用删配置；暴露别名让客户端能直接选到别名（否则别名只对知道内情的人有用） |
| P0-9 | **单实例锁**（两家都缺，我们顺手补上） | — | SQLite 不支持多实例写同一数据目录。桌面端用户双击图标两次就可能踩到 |
| P0-10 | **`panic = "abort"` 下的 `str` 切片审计** | PR #58 教训 | 它因为字节下标切片在 Unicode 文本上 panic，**整个进程退出**。我们也要 grep 一遍所有 `str` 切片点，确认走字符边界 |

### P1 — 架构级，中成本、高收益

| # | 事项 | 来源 | 判断 |
| --- | --- | --- | --- |
| P1-1 | **headless / Docker 交付形态** | `bin/waliapi-web.rs` + `web/` 子包 + `/admin/api/invoke` | **优先级最高的架构级借鉴。** 可同时绕开我们两个发布阻塞（macOS 只有 aarch64、签名公证未做）——Linux 二进制与 Docker 镜像都不需要签名公证。我们的 `gateway-core` 已不含 tauri 依赖、`jai_standalone.rs` 已证明可无壳启动、73 个 IPC 与 73 个前端 `invoke` 一一对应，**改造成本比它当初更低**。需要做的：① `runtime.ts` 等价传输层抽象；② `/admin/api/invoke` + `/admin/api/events` 两个端点；③ `--no-default-features` 的 feature 切分与 `rust-embed` 内嵌前端 |
| P1-2 | **Responses 断线续传**：`/v1/responses/{id}/events` + offset 回放 + `resume_ttl_secs` | `endpoint_executor/driver.rs::persist_resume_frame` | 补齐我们明确未实现的 `GET` / `DELETE /v1/responses/{id}`。Codex 类客户端的长任务断线重连是真实痛点。**注意**：与「日志不落内容」的隐私主张冲突，建议做成**可选的、独立于日志表的** resume 存储，带 TTL 与显式开关 |
| P1-3 | **安全扫描（只做阻断 / 脱敏这一半）** | `security/` | agent 编程场景的真实需求：防止 agent 把 `.env`、`~/.ssh/id_rsa`、云凭据发给模型。**只抄「扫描 + 脱敏 / 阻断」，不抄审计日志与规则管理 UI。** 关键设计要抄对：fail-closed 预算、`gate_dispatch` 式的编译期清单守卫、脱敏日志与转发体分离的信任边界 |
| P1-4 | **多 Key 加权负载** | `channel_api_keys` 表 | 单渠道多 Key 按权重随机，分散并发压力。对「一个中转站给了多个 key」的场景直接有用 |
| P1-5 | **Auth 账号体系** | `auth_provider/` | 最大的产品级差异，也是**最需要产品决策的一项**。合规灰区 + 依赖上游私有端点 + CI 无法覆盖，是持续性维护成本。若做，复用我们已有的 `Provider` 抽象与路由候选模型，**不要新起一套**；且建议**先只做 1 家**（Kimi 或 Codex）验证需求真伪 |
| P1-6 | **本地知识资产 + MCP 暴露** | `services/knowledge` / `services/wiki` | 不必做完整 RAG。**最小可用版本**：本地目录 / 代码库索引 + 一个 `search_local_docs` MCP 工具 + 一个 `read_doc` 工具，让 Agent 能查我们的 `docs/design/*.md` 与代码。这比它的「文档上传 + 向量库」轻得多，但价值相似。若要更进一步，「用 LLM 编译结构化页面（Wiki 路线）」比纯 RAG 更契合 agent 场景 |
| P1-7 | **密钥级白 / 黑名单 + 配额** | `api_keys` + `api_key_knowledge_access` | 单人本地用价值低；但「把网关给团队内几个人用」时，限制某客户端只能用某几个模型 / 渠道是刚需 |
| P1-8 | **OTLP/HTTP JSON 导出（不引 SDK）** | `otlp_exporter.rs` | 极轻量的可观测性落地方式：把 `request_logs` 增量导为 span，游标存 `meta`，**失败退避且不推进游标**（保证不丢） |

### P2 — 观望 / 按需

| # | 事项 | 判断 |
| --- | --- | --- |
| P2-1 | 语义缓存（exact + semantic 两层） | 默认关闭的设计是对的。对 agent 编程场景（每轮 prompt 都不同）命中率可能很低，先观察 |
| P2-2 | 日志三级（含「简要」级别） | 我们日志本来就只落元数据，价值有限；但「简要」这个思路可用于**未来的可开关内容日志** |
| P2-3 | 渠道预设注册表（`model_suggestions` + `verified_at` + `source_url`） | 我们目前是「模型自动发现」，够用。若要降低首次配置摩擦，预设仍有价值 |
| P2-4 | 组织与协作（贡献者看板 + 任务卡粒度） | 开源运营才需要。**但「任务卡粒度」本身值得学**——一个任务 = 一份 design + 一份验收，是很好的上下文管理手段 |
| P2-5 | `--help` / `repair-stream-logs` 类运维子命令 | 我们做 headless 时顺手加 |

---

## 5. 不建议跟的地方

1. **不要因为它的功能多而动摇双轨制 + IR 路线。** 我们的 `protocol-ir.md`（逐字段映射总表 + 能力表六面 + 四级决策）在**可测性、可扩展性、可解释性**上都优于「N² 对 codec 矩阵」。它的优势只在「逐方向可单独调优保真度」，而这个优势可以用「IR 之外给个别方向留逃生舱」来获得，不需要推倒重来。
2. **不要把 CHANGELOG 塞进 README。** 它 README 77 KB，阅读体验很差。我们独立 `CHANGELOG.md` 的做法更好。
3. **不要接受巨型文件。** `handlers.rs` 5,522 行、`driver.rs` 3,311 行是它最明显的技术债。我们 `proxy.rs` 4,180 行也偏大，但至少没有继续恶化——**这条要警惕，不要往那个方向走**。
4. **不要做「表里不一」的 UI。** 它的 `security_builtin_rules` 表只是 UI 元数据、运行时扫描器根本不查，是明确的教训。
5. **不要接受「文档声称」与「代码实现」的漂移。** 它多处文档与代码不一致（模型同步、Auth 导入、项目结构）。我们 `docs/bug和优化清单.md` 的口径对齐习惯更好，要保住。
6. **不要用「导出含明文密钥」的设计。** 它的渠道导出 JSON 含明文 `api_key`；我们主张导出剔除敏感字段，这是对的，不要为了「换机即用」的便利退让（我们已经用 WebDAV 同步解决了同一个问题——但值得再核对一次：同步文件里敏感字段的处理是否与「导出剔除」的主张一致）。
7. **不要在渠道 `base_url` 上省掉 SSRF 校验。** 它知识库 URL 做了完整校验但渠道 base_url 没做。我们如果引入「从 curl 导入渠道」（P0-2），就必须同时接上 `validate_draft_url` 等价校验，否则这个便利会变成一个 SSRF 面。

---

## 6. 竞品态势与我们的应对

### 6.1 它真正的优势不是代码

WaLiAPI 最可怕的不是 103K 行 Rust，而是**组织能力**：2 个月、16 位贡献者、571 commits、26 个 release，且每条改动都能追溯到人和 PR。这背后是一套可复制的机制：

1. 作者个人品牌（小傅哥）带来的初始流量与信任；
2. **任务卡粒度**（T01–T14）让外部人「拿一张卡就能开工」；
3. 每条 CHANGELOG 带 PR 号 + @handle，**贡献有可见回报**；
4. `AGENTS.md` 让 AI 编码代理也能直接上手（这在这个时间点是稀缺的）。

如果 JAI 保持单人 + 不开源运营，这条优势无法直接对抗；如果 JAI 要开源，这套机制可以整套照搬。

### 6.2 我们的护城河在哪

按「别人抄起来的难度」排序：

1. **协议保真度**：`protocol-ir.md` 的逐字段映射总表 + 能力表六面 + 四级决策 + Codex 扩展工具折叠还原 + 截断语义对齐（`incomplete` + `incomplete_details.reason`）。**这是最硬的一条**，因为它要求把三套协议的行为规范吃透，不是加功能能追上的。
2. **WebDAV 跨设备同步**（含推 / 拉独立间隔、远端备份、冲突 diff、LWW 护栏）。WaLiAPI 完全没有，而这是我们的立项动机之一。
3. **真机联调记录**：`docs/test-report-dsh.md`（dsh 7 项全绿）+ `docs/zcode接入.md`（zcode 实测通过）+ 那些「坑 / 教训」清单。这些是**用真实流量换来的**，抄不走。
4. **工程质量方法论**：UI 门禁 + 视觉回归 + 负控制 + 「一直红的断言比没有断言更危险」这类教训。WaLiAPI 完全没有这一层。

### 6.3 建议的定位表述

> WaLiAPI 是「**功能最全的本地 LLM 网关**」；
> JAI 应该守住「**协议最保真的本地 LLM 网关，且配置能跟着你换机器**」。

对应地，近期投入应该偏向：**把「协议保真」的可验证性继续做强（补测试密度，P0-5 / P0-10）、把「换机器即用」做完整（WebDAV 收尾）、把分发路径打通（P1-1 headless / Docker）**；而不是去堆功能广度（Auth 账号 / 知识库 / 安全审计这三块都先只取最小可用版本或延后）。

---

## 7. 附录：调研方法与局限

### 7.1 方法

- **源码获取**：`codeload` tarball 多次断流（卡在 5.4 MB），改为用 GitHub Tree API 列出全部 512 个 blob，过滤二进制与 lockfile 后**逐文件从 `raw.githubusercontent.com` 并行拉取 455 个文本文件（7.2 MB）**，100% 成功。二进制（图标 / pdfium）未取。
- **阅读**：6 路并行探查（网关与协议层 / 安全与知识库与 MCP / 账号与渠道预设 / 桌面壳与前端与部署 / JAI 现状 / JAI 文档），每路给出带文件路径与标识符的证据；关键结论由我逐条交叉核对。
- **JAI 侧**：对当前工作区 HEAD（`3f063d9`，v0.3.1）做源码走查 + 文档精读，并用 grep 实测确认了「无 `Retry-After` 处理」「无单实例锁」「无迁移前备份」「无密钥黑白名单 / 配额」「无渠道草稿测试」五条。

### 7.2 局限（明确声明）

1. **只做静态阅读，没有编译、没有运行、没有真机验证 WaLiAPI。** 所有「行为」结论来自代码阅读，可能存在分支未覆盖。
2. **未读 WaLiAPI 的二进制资源**（`src-tauri/icons/*`、`resources/pdfium/*`），因此无法评估其打包体积与 OCR 依赖的实际可用性。
3. **未逐行通读的部分**：`handlers.rs` 中段（`handle_embeddings` 等非聚焦路径）、`mcp/handlers.rs` 的 29 个工具逐个实现体、`knowledge/importer.rs` 的 git / url 克隆实现、`code_parser.rs` 的 tree-sitter 查询、各文件 `#[cfg(test)]` 测试正文（占 `service.rs` / `codex_login.rs` 大半篇幅）。
4. **未读的设计文档**：`docs/channel-refactor-tasks/` 的 03/04/05/06/08/10/11/12/13、`docs/auth-codex/` 的 `01-ui-spec.md` / `glossary.md` / `prompts/` / `work/`、`docs/plans/2026-09-18-gemini-auth.md`。
5. **JAI 侧未读**：`CHANGELOG.md` 中间版本（0.1.7–0.2.13）未逐条精读；`docs/视觉回归整改plan.md` 的 303 步遍历数据未细看；`docs/superpowers/plans/` 中 UI 2.0 阶段文档未读；`sync.rs`（1,539 行）仅读表头。
6. **数字口径**：行数含空行与注释；测试数为 `grep -c '#\[test\]\|#\[tokio::test\]'` 的命中数（可能含少量注释中的示例）。
7. **Stars / 贡献者数据**为调研时点（2026-09-22）快照，会变化。

### 7.3 建议的后续动作

如果要从本报告继续推进，建议顺序：

1. 把 §4 的 **P0 清单**过一遍，逐条标注「已有 / 部分 / 无」并估工时；
2. 对 **P0-5（重试模型）** 与 **P0-1（草稿测试）** 先写 spec（走我们已有的 `docs/superpowers/specs/` 流程）；
3. 对 **P1-1（headless / Docker）** 单独立项论证——它牵涉 feature 切分、传输层抽象、CI 与发布流程，是唯一一个「值得单独写一份 design 文档」的项；
4. **P1-5（Auth 账号）** 不做技术评估，先做产品决策（合规 + 维护成本 + 是否真的有用户需要）。