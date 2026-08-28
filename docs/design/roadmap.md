# 开发路线图：MVP 里程碑切分 — v2

> 状态：已评审定稿（2025-08，v2 依第一梯队客户端与多设备同步动机扩编）。每个里程碑独立可验收、可演示。
> 关联：《JAI — 桌面 AI API 网关.md》、[protocol-ir.md](protocol-ir.md)、[storage-schema.md](storage-schema.md)。

## 1. 总览

| 里程碑 | 名称 | 相对规模 | 前置 | 可演示产物 |
| --- | --- | --- | --- | --- |
| M0 | 工程骨架 | S | — | `cargo tauri dev` 出窗口，healthz 通 |
| M1 | 单渠道直通端到端 | L | M0 | DeepSeek harness / OpenAI 客户端经 JAI 对话成功 |
| M2 | 多渠道路由 | M | M1 | 杀掉主渠道，请求自动走备用 |
| M3 | Claude Code 接入 | M | M1 | Claude Code 真实 session 全程经 JAI |
| M4 | 跨族转换·上半场 | XL | M1（可与 M2/M3 并行设计） | Cline × Claude/Gemini 工具循环 |
| M5 | 跨族转换·下半场 | L | M4 | Claude Code × GPT/Gemini 干活 |
| M6 | Responses API 入站（Codex 接入） | L | M4、M5 | Codex CLI 经 JAI 正常干活 |
| M7 | 配置导入 + WebDAV 同步 | M | M2（依赖导出 JSON） | 新设备一键拉平全部配置 |
| M8 | 收尾加固 | M | M6、M7 | 48h 常驻稳定性 + 性能数据 |
| M9 | 发布工程 | M | M8 | 公开 beta 安装包 |

关键依赖事实：**同族直通路径不含协议转换工作量**，故 M3 提前、价值密度高；跨族重活集中在 M4–M5 展开；M6/M7 是两个功能补强里程碑，之后才进入加固与发布。

## 2. 第一梯队客户端覆盖

来自需求文档「重点客户端」节，逐条落到里程碑：

| 客户端 | 协议线 | 覆盖里程碑 | 验收要点归属 |
| --- | --- | --- | --- |
| DeepSeek harness | OpenAI chat completions | M1 | 直通链路 + SSE + 多渠道顺延 |
| Claude Code | Anthropic messages 直通 | M3 | 真实编码 session；count_tokens；529/429 形状 |
| zcode | 待实测（chat 或 Anthropic 均已覆盖） | M1 或 M3 完成即具备条件，实测后归入对应验收 | — |
| Codex | OpenAI **Responses API** 入站 | M6 | 真实编码 session 经 `/v1/responses` |

过渡通道（不承诺长期支持）：Codex 在 provider 配置里设 `wire_api = "chat"` 可先走 M1 的 chat completions 端点救急；M6 落地后回归原生线。

## 3. 各里程碑详情

### M0 工程骨架（S）

**范围**
- Cargo workspace：`crates/gateway-core`（codec / store / router 占位模块）、`src-tauri`
- 前端脚手架：React + TS + TailwindCSS + shadcn/ui，单空页面
- SQLite 迁移执行器（`user_version` 机制）+ v1 DDL 应用
- 网关监督进程：启停控制、`127.0.0.1:1314` 起监听、端口占用自动顺延
- `GET /healthz` → `{ok, version}`
- CI：fmt / clippy `-D warnings` / `cargo test` / 前端构建，macOS + Windows 矩阵

**验收标准**
1. 冷克隆仓库 → `pnpm i && cargo tauri dev` 出应用窗口与占位托盘菜单
2. 迁移幂等测试通过（`user_version` 重复执行不重复建表）
3. 端口 1314 被占时自动起在 1315，UI 显示实际端口
4. CI 双平台全绿

### M1 单渠道直通端到端（L）

**范围**
- Provider CRUD（IPC）+ 密钥环写入/读取/删除生命周期（先写环再插行，失败回滚）
- **测试连接**按钮（探测对应协议的模型列表接口）
- 自动发现模型入库 + LiteLLM 元数据快照静态 JSON 内嵌做默认填充
- gateway_key：首次启动生成 `sk-jai-*`；鉴权中间件（SHA-256 常量时间比对）；UI 前缀展示/复制/重新生成（吊销+新建）
- OpenAI 入站 **passthrough 代理**：仅当路由结果为单渠道 `openai_compat` 时启用（同名冲突暂报错）；SSE 直通管道 + usage 旁路扫描
- 本地安全三件套：Host/Origin 校验、CORS 默认拒绝、强制鉴权
- `request_logs` 异步攒批写入 + 只读日志页
- 托盘正式启停菜单

**不做**：多渠道路由（见 M2）、导出、任何跨族转换。

**验收标准**
1. 黄金夹具：直通模式下上游实际收到 body 的 SHA-256 == 客户端发出值
2. 流式与非流式 `/v1/chat/completions` 均通；上游中途断流时客户端收到明确断开，日志落 `error_kind`
3. 错误鉴权 → 401，body 为 OpenAI 错误 schema（`type/code: invalid_api_key`）
4. `Origin: http://evil` 与非法 `Host` 均 403
5. 100 并发流式 × 60s：无内存异常增长，日志行数等于请求数
6. **真实客户端冒烟：DeepSeek harness 类 OpenAI 客户端（chat 模式 + 流式）走通一次完整对话**【第一梯队验收】

### M2 多渠道路由（M）

**范围**
- 路由执行器：按 `priority, rowid` 序逐渠道尝试；同名多渠道合法化
- 故障转移规则集：{连接拒绝、超时、UpstreamAuth、RateLimit、Overloaded、上游 5xx} → 下一渠道；InvalidRequest / ContextTooLong → 即刻返回不切换；**首个字节下发下游后禁止切换**
- 单轮按序遍历一遍即止，全部失败返回最后一个错误
- Provider 健康徽标（`last_ok_at / last_err_*` 维护与展示）
- 设置页：端口修改、日志开关与级别、保留策略展示
- **导出 JSON**（meta + providers + models，零敏感字段）
- 保活 timer：30 天 / 5 万行裁剪、`tool_id_map` TTL 清理
- 鉴权失败限速：同源 10 次/分钟失败 → 封禁该源 5 分钟

**不做**：配置导入（提前至 M7，见 §6 调度决策）。

**验收标准**
1. Mock 双渠道同名模型：一级渠道恒 500 → 请求成功，日志 `provider_id/upstream_model_id` 记录二级渠道命中
2. 反转 priority 后行为随之反转
3. `response_format` 类确定性 400 错误不触发切换
4. 导出文件全文扫描（grep 断言）：无 `sk-jai` 全文、无 keyring 引用、无上游密钥
5. 保留裁剪任务的单测（mock clock）

### M3 Claude Code 接入（M）

**范围**
- 入站 Anthropic passthrough 路由：`POST /v1/messages` → `family=anthropic` 渠道直通（SSE 同 M1 管道）
- `x-api-key` / `anthropic-version` 头处理（缺省注入默认版本）
- `POST /v1/messages/count_tokens` → 粗估返回（`ceil(chars/4)` + system/tools 开销常量），避免 CC 降级
- 错误形状 Anthropic 化：枚举 type、Overloaded→HTTP 529
- 日志 `inbound_family='anthropic'` 区分
- README 快速接入节（`ANTHROPIC_BASE_URL` + key 环境变量示例）

**验收标准**
1. 真实 Claude Code 完整编码 session（读代码→改文件→跑命令）全程经 JAI → 官方 API，流式与工具均正常【第一梯队验收】
2. zcode 实测确定协议线并按归属跑通一例对话【第一梯队】
3. `count_tokens` 恒返回正整数，误差可接受（不对精确度承诺）
4. Anthropic 系错误（429/529）以 Anthropic schema 回给 CC
5. 多轮对话中 prompt caching 生效时日志 `usage_cache_read > 0`

### M4 跨族转换·上半场：OpenAI 客户端 × Claude/Gemini 模型（XL）

**范围**
- `InboundCodec::OpenAI` 全量实现（decode / render_response / render_stream_event + RenderState）
- `UpstreamCodec::Anthropic` 与 `UpstreamCodec::Gemini` 全量实现
- StreamEvent IR 贯通；tools 三段（定义/发起/结果）跨族矩阵
- 结构合规变换：相邻同角色合并、system 提升、tool_result 打包、块序约束
- Gemini 无调用 id 的合成与 name 关联；http 图片拉取转 base64（8s 超时，失败明确 400）
- `response_format`(json_schema) 跨族 400（OpenAI 族目标仍透传）；Extensions Lenient 丢弃 + 单条 WARN 汇总
- 护栏常量：单请求 blocks ≤ 64、累计 args ≤ 256KB，越界 400
- `GET /v1/models` 跨族聚合输出去重

**内部闸门（顺序推进）**：G-a 文本链路 → G-b 流式 → G-c tools 循环 → G-d 图片。

**验收标准（黄金夹具矩阵跑满所有转换链路）**
1. 性质测试 `decode(encode(ir)) == ir` 三协议全绿
2. Cline 连 JAI 分别指定 claude / gemini 模型，完整执行一次工具编辑循环
3. OpenAI 交错 tool_calls 分片输入 → Anthropic 输出严格嵌套顺序（缓冲重排断言）
4. Gemini 同名函数合成 id 冲突场景（`_k{n}` 后缀）结果关联正确
5. `response_format` 跨族请求得到 400 且 message 列出受支持子集
6. 携带 `cache_control` 跨族请求：功能不受影响 + 恰好一条 WARN
7. 护栏越界场景稳定 400 不 panic

### M5 跨族转换·下半场：Anthropic 客户端 × OpenAI/Gemini 模型（L）

**范围**
- `InboundCodec::Anthropic` render 侧：响应与 SSE 渲染（嵌套顺序缓冲纪律在此方向落地）、错误 Anthropic 化、嵌入版 tool id（`toolu_`+base58）反解闭环
- 入站携带 thinking 字段：Lenient 丢弃 + WARN（Thinking 块仅存储不转换）
- Anthropic `message_start` 的 input_tokens 前置填 0、`message_delta` 终局补齐（CC 已验证兼容）

**验收标准**
1. Claude Code 以 `ANTHROPIC_BASE_URL` 指 JAI、model 设为 OpenAI/Gemini 型号，完成子任务含工具使用【第一梯队 × 杂牌源组合验证】
2. 多轮会话中工具 id 回传稳定；构造超长 id 场景验证 `tool_id_map` 回落读取
3. 上游中途出错时客户端收到 Anthropic 形状的 `event: error` 帧
4. 请求参数映射单测跑满（top_k 忽略告警、max_output 缺省填充等）

### M6 Responses API 入站 · Codex 接入（L）（2025-08 新增）

**背景**：Codex 为第一梯队客户端，其原生线格式为 OpenAI Responses API。依拍板将 `/v1/responses` 从二期提前至此实现。

**范围**
- Inbound 解码侧扩展一个 Responses Codec：
  - `POST /v1/responses` 请求结构解码 → 复用 CanonicalRequest IR（instructions / input items / tools 映射进现有 Block/ToolSpec）
  - 响应渲染：非流式 response 对象 与 SSE 事件流（`response.created` / `output_item.added` / `output_text.delta` / function call 参数增量 / `response.completed` 等）← 由 StreamEvent IR 推导
  - 错误响应按 Responses 错误形状输出
- 路由层挂接新端点；`GET /v1/models` 不变
- 序列号/响应 id 规范化管理（每个请求生成统一 response id，事件内引用一致）

**验收标准**
1. 真实 Codex CLI 以 JAI 为 base（responses 线）完成一次完整编码任务（含工具调用）【第一梯队验收】
2. 黄金夹具：Responses ↔ IR 往返测试全绿；同一夹具下 M4/M5 的既有入站输出保持不变（防回归）
3. 流式中断场景：客户端收到明确的 error 终态事件而非静默悬死
4. 与同模型的 chat completions 输出做语义一致性对比（文本+工具调用一致）

### M7 配置导入 + WebDAV 同步（M）（2025-08 提前至公测前）

**背景**：多设备配置同步是项目核心动机之一（macOS/Windows 多机）。原定二期，复议提前。

**范围**
- 导入：解析导出 JSON → 名称+Base URL 去重 upsert → 「待录入密钥」状态引导补录密钥环凭据
- WebDAV 客户端：URL/账号/密码（密码存密钥环）/目录路径配置；手动推/拉按钮先行，定时同步可选
- 冲突策略 last-write-wins（v1）；推送前本地快照留存一份用于误操作回退
- 导入/同步完成后触发配置校验报告（缺失凭据清单、无效 base_url 清单）

**验收标准**
1. 新装机器 A → 手动导入 B 机导出的 JSON → 补录 Key 后即可用，无手工建供应商动作
2. WebDAV 拉取：A 改动 → 推送 → B 拉取后 providers/models/meta 一致（行级 diff 断言）
3. 互斥保护：两端同时推送时后写胜出且前一份有快照可查
4. 导入含脏数据（坏 URL、重复名）的文件：成功部分生效 + 明确的差异报告，不整体失败

### M8 收尾加固（M）

**范围与验收**
1. 全矩阵夹具回归脚本一键执行（CI 接入，覆盖四类入站线）
2. 解码器鲁棒性：quickcheck/fuzz 简表 500 随机 body 不 panic
3. 资源与性能基线：空闲 RSS 目标 < 80MB；冷启动到 healthz 就绪 < 3s；1MB/s SSE 连续 10 分钟无积压无泄漏式增长；数据记入本文档附录
4. 48h 本机常驻自用观察期零崩溃（期间第一梯队四客户端各至少一例）
5. clippy 收尾清零、护栏常量集中审计、文档一致性 pass

### M9 发布工程（M）

**范围与验收**
1. macOS 签名 + 公证流水线；Windows 代码签名（EV 证书成本与取舍记录进发布 checklist）
2. Tauri Updater 更新通道（公钥内嵌、feed JSON）
3. 全新虚拟机验证：下载 → 安装 → 配一个供应商 → 第一梯队四客户端各打通一例
4. 杀软误报排查流程文档化
5. README / 接入指南终稿（Claude Code、Codex、DeepSeek harness/zcode、ChatGPT Next Web 例）、CHANGELOG、tag `v0.1.0-beta`

## 4. 需求覆盖矩阵（追溯 MVP 清单，防遗漏）

| 需求文档条目 | 承接里程碑 |
| --- | --- |
| 1 供应商管理 + 测试连接 | M1 |
| 2 模型发现 + 元数据快照默认值 | M1 |
| 3 代理服务：OpenAI 入站 | M1 |
| 3 代理服务：Anthropic 入站 + count_tokens | M3 |
| 3 代理服务：Responses API 入站（Codex） | M6 |
| 3 双协议路由 / 流式 / tool calling / 多渠道顺延 | M1 / M4 / M2 ✅ |
| 4 数据持久化 + 导出 JSON + 日志脱敏 | M0(库) / M2 ✅(导出) / M1(日志) |
| 7 WebDAV 同步 + 配置导入（提前） | M7 |
| 5 基础 UI（供应商/模型/状态卡/日志/设置/托盘） | M1 / M2 ✅ |
| 6 网关进程（端口顺延、自定义端口） | M0 / M2 ✅(自定义端口) |
| 非功能：本地安全四项 | M1（鉴权/Host-Origin/CORS）、M2 ✅（限速） |
| 非功能：稳定性/可用性（一票否决） | 全局基线 §5，量化验收在 M8 |
| 非功能：低资源占用 | M8（量化验收） |
| 发布工程 | M9 |

## 5. 稳定性工程基线（全局，适用所有里程碑）⛳

> 2025-08 确立为一票否决项：进度可以延迟，稳定性不允许妥协——网关一旦在工作流里挂掉，用户的全部 agent 会话中断。

结构性要求（随所属里程碑首次落地后持续保持）：

1. **每请求独立任务隔离**：单个请求处理 panic 不得拖垮网关进程；handler 层捕获并转为 500 + 日志
2. **超时三件套**：上游连接 10s / 上游首字节 60s / 流空闲读取 120s，均可配置；超时走故障转移或明确报错，绝不悬挂
3. **日志永不反压 HTTP 层**：有界队列洪峰丢弃最旧 + 计数告警（存储设计定案的推广约束）
4. **SSE 首字节下发后禁止切换渠道**（M2 定案的推广原则）
5. **DB 忙碌降级**：SQLite busy 时带退避重试；配置读写失败不影响进行中的转发
6. **进程看门狗**：Tauri 侧监督网关状态，异常退出自动重启并以 UI 横幅告知（重启计数可见）
7. **每次发版必过**：黄金夹具矩阵 + fuzz 简表 + 48h 自用（对 beta 版本要求）
8. **错误路径即验收路径**：每个功能的验收标准必须包含至少一条故障注入用例（断流/超时/上游 500 已分布于 M1–M7）

## 6. 全局 DoD 与调度决策记录

全局 DoD：双平台 CI 绿灯、clippy 零警告、新增行为有夹具或单测、需求覆盖矩阵更新、可复现 demo 步骤、相关设计文档同步修订。

| 决策 | 内容与理由 |
| --- | --- |
| 细粒度里程碑 | （2025-08）每步可独立验收演示 |
| ~~导入推迟二期~~ → 提前 | （2025-08 复议，覆盖上一决策）多设备同步为核心动机，「导入 + WebDAV」提前至 M7、公测前交付 |
| Responses API 提前 | （2025-08 拍板）Codex 属第一梯队，直接实现 `/v1/responses`（M6），不以 wire_api=chat 兼容模式为主方案；该模式仅作 M6 前救急 |
| M4 可与 M2/M3 并行设计 | 编码器可在 M1 落地后纯函数化开发，集成挂在 M4 闸门 |

## 7. 主要风险与护栏

| 风险 | 缓解 |
| --- | --- |
| 客户端版本迭代发送未知字段导致脆断 | Extensions 默认 Lenient（IR 文档 §7）；黄金夹具锁定主流客户端当前版本样本 |
| 重点客户端行为漂移（CC/Codex/zcode/harness） | 每次发版用当日版本回归 §2 对应验收脚本；破坏性漂移记 CHANGELOG 并评估兼容垫片 |
| Responses API 表面大于预期（事件类型繁多） | M6 先落 Codex 实际触达的事件子集，未触达事件夹具标记 skip 而非臆造行为；预算超支则砍图片输入先行 |
| Gemini functionCall 无 id 的并发同名歧义 | 合成 id 冲突后缀策略 + 专项夹具（M4 验收 4） |
| 流式重排缓冲内存失控 | 护栏常量（≤64 blocks / ≤256KB args）统一在 M4 落地、M8 审计 |
| 平台密钥环差异 | M0 起启动三连探测；不支持环境在添加供应商前拦截（storage §4） |
| WebDAV 误覆盖造成配置丢失 | M7 推送前快照留存；LLW 先于定时自动同步开放 |

## 8. 实施进度快照（下班交接记录）

### M1（单渠道直通端到端）—— 已完成 ✅

核心代码落地并通过本地 clippy（-D warnings）与 gateway-core 单测；UI 通过 `tsc --noEmit && vite build`。真机上游冒烟待有环境时执行。

- 存储 CRUD 扩展：providers / models / gateway_keys / meta、路由候选 SQL、`Db::with_any`
- 异步日志管道：有界 1024 队列、≥64 行或 500ms 攒批、独立连接写、洪峰丢弃计数、`logs_recent`
- OS 密钥环封装：set/get/delete/启动探测 + 共享 mock 单测（不触真实钥匙串）
- OpenAI 旁路工具：请求 peek、增量 usage 扫描器、URL/错误形状
- 模型快照：内嵌 JSON（14 个常见模型），`snapshot::lookup`
- 安全中间件：Host 回环校验、Origin 白名单（默认拒绝）、Bearer/x-api-key + sha256 常量时间认证
- 直通代理：单渠道 openai_compat、字节级 body 透传、SSE 管道、首字节 60s/空闲 120s、错误分类与 provider 状态回写
- 模型发现：openai_compat / anthropic / gemini 三家 `/models` 归一入库（保留用户已调默认值）
- Tauri IPC 全套：provider CRUD/测试/发现、model 限值/开关、网关密钥 info/reveal/轮换、日志查询、导出 JSON、CORS 设置、families
- UI 五标签页：网关 / 供应商 / 模型 / 日志 / 设置（React+Tailwind，M0 卡片升级）
- src-tauri 启动自举网关密钥、密钥环先写后库+失败回滚

### M2（多渠道路由）—— 已完成 ✅

- 路由执行器：`router` 模块实现 `AttemptVerdict` 分类（Stop/Failover/Success）；`proxy` 逐渠道循环尝试
- 故障转移规则集：{连接拒绝、超时、UpstreamAuth、RateLimit、Overloaded、上游 5xx} → 下一渠道；
  InvalidRequest / ContextTooLong → 即刻返回不切换；首字节下发后禁止切换（SSE 管道纪律不变）
- 集成测试（真实 HTTP 双渠道 mock）：一级恒 500 → 顺延命中二级并落日志（`provider_id` 断言）、
  priority 反转行为反转、确定性 400 不切换不尝试二级、流式顺延 —— `tests/m2_failover.rs` 4 项全绿
- 保活 timer：`store::retention::run_retention`（30 天 / 5 万行 / tool_id_map TTL，mock clock 可测）+
  `spawn_retention_loop` 常驻循环（src-tauri 启动时拉起，每日一次）
- 导出构建器下沉：`store::export::build_export_json`，单测扫描全文无 `sk-`/`keyring`/`provider/` 敏感串（M2 验收 4）
- 鉴权限速：`server::ratelimit::AuthRateLimiter`（同源 10 次/分钟失败 → 封禁 5 分钟），
  中间件接入 ConnectInfo（`into_make_service_with_connect_info`）；封禁期直接 429
- 设置能力：`settings_get` / `settings_set_port` / `settings_set_logs_enabled` IPC + UI 设置页
  （端口保存重启生效、日志开关实时切换、保留策略展示）；启动时从 meta 恢复端口与日志开关
- 提供商健康徽标 UI：最近成功/失败徽章 + 时间 + 失败摘要（数据源为已有 last_ok_at / last_err_*）

### M3（Claude Code 接入）—— 已完成 ✅

- 入站 Anthropic 直通：`POST /v1/messages` → `family=anthropic` 渠道（`tests/m3_anthropic.rs` 5 项全绿）
- 双线复用同一流水线：`InboundWire::{OpenAi,Anthropic}` 收敛路径/鉴权头/错误形状/日志族差异；
  OpenAI 线回归不破坏（M1/M2 测试保持绿）
- 上游认证：`x-api-key` + `anthropic-version`（缺省注入 `2023-06-01`）
- `POST /v1/messages/count_tokens`：粗估 `ceil(chars/4)` + 消息/工具开销常量，恒返回正整数（验收 3）
- 错误 Anthropic 化：`{"type":"error","error":{...}}` 形状；Overloaded → **HTTP 529** 保留（验收 4）
- 全渠道失败语义修正：最后失败若携带 HTTP 状态，原样回传（保留 429/529 与上游方言），
  网络级失败才折叠 502 `all_providers_failed`（对齐 roadmap M2「返回最后一个错误」）
- 日志 `inbound_family='anthropic'` 区分；usage 旁路扫描照常落库（cache_read 校验留待真机）
- README 快速接入节（`ANTHROPIC_BASE_URL` + 网关 Key 示例）

### M4（跨族转换·上半场：OpenAI 客户端 × Claude/Gemini 模型）—— 已完成 ✅

- IR 类型落地（`codec/ir.rs`）：CanonicalRequest/Response、Block、ToolSpec、ToolChoice、
  SampleParams、StreamEvent、StopReason、Usage + 结构合规变换（相邻同角色合并）+ 护栏
  （blocks ≤ 64 / args ≤ 256KB，`validate_guards`）
- InboundCodec::OpenAI 全量（`codec/openai.rs`）：decode_request（含 image_url/base64、tool_calls、
  tool 结果）、render_response、render_stream_event + RenderState
- UpstreamCodec::Anthropic 全量（`codec/anthropic.rs`）：encode_request（system 提升、tool_result
  块序、temperature 截断）、parse_response、parse_stream_event（message_start/content_block_*）
- UpstreamCodec::Gemini 全量（`codec/gemini.rs`）：encode_request（systemInstruction、functionDeclarations、
  functionCallingConfig）、parse_response、parse_stream_event（functionCall 整对象 → Start+Args+End、
  合成 id `{name}_k0`）、HTTP 图片拉取转 base64（8s 超时，400 明确报错）
- proxy 跨族接线：`try_converted_candidate`（解码→护栏→response_format 400→Lenient WARN→编码→
  解析→渲染回入站形状），流式 SSE 事件级转换（首字节纪律保持）
- 集成测试 `tests/m4_conversion.rs` 7 项 + `tests/m4_golden.rs` 5 项全绿
  （Anthropic/Gemini 文本/工具/流式端到端、429 全败 OpenAI 形状、参数映射、往返不变性）
- 全量回归：57 单测 + M2/M3/M4 集成共 78 项全绿，clippy -D warnings 零警告

### M5（跨族转换·下半场：Anthropic 客户端 × OpenAI/Gemini 模型）—— 已完成 ✅

- InboundCodec::Anthropic 全量（`codec/anthropic.rs` M5 段）：decode_request（system 字符串/块、
  tool_use/tool_result 反解、thinking Lenient 丢弃 + WARN）、render_response、render_stream_event
  + AnthropicRenderState（message_start input_tokens 前置 0、message_delta 终局补齐、message_stop）
- UpstreamCodec::OpenAI（`codec/openai.rs` M5 段）：encode_request（system 还原首条消息、tool_calls、
  role=tool、max_completion_tokens、stream_options 注入）、parse_response、parse_stream_event
- proxy 转换接线：Anthropic 入站 → OpenAI/Gemini 上游，非流式与流式全链路；流式转换修复
  「首字节已含完整 SSE 流时未消费行缓冲」问题（单 chunk 场景不丢内容）
- tool id 闭环：`canonical_to_anthropic_id` 改为 `toolu_` + base58("jai1."+原始 id) 确定性编码，
  `decode_anthropic_tool_id` 识别 magic 并还原；超长 id（>64）回落 `tool_id_map`（store 新增
  tool_id_put/tool_id_get + 7 天 TTL），流式/非流式出站映射入站回传还原
- 流式中途错误：Anthropic 入站收到 `event: error` 帧（OpenAI 线 `data: {"error":...}`），
  不再静默断流
- 集成测试 `tests/m5_anthropic_inbound.rs` 6 项全绿：text/tool/stream/长 id 多轮回传
  （OpenAI + Gemini 上游）；proxy 侧新增 error SSE 帧与 tool_id_map 单测
- 全量回归：67 单测 + M2/M3/M4/M5 集成共 27 项全绿，`cargo fmt --check` + clippy -D warnings 零警告

### M6（Responses API 入站 · Codex 接入）—— 已完成 ✅

- 新增 `codec/responses.rs`：Responses 请求解码（instructions/input items/tools → IR）、
  非流式 response 对象渲染、SSE 事件流渲染（response.created / output_item.added /
  output_text.delta / function_call_arguments.delta / response.completed）、Responses 错误形状
- `InboundWire::Responses` 接入统一 dispatch：`POST /v1/responses` 挂路由，永远走跨族转换路径，
  复用 M4/M5 的 IR 管道（上游可接 OpenAI chat / Anthropic / Gemini）
- 集成测试 `tests/m6_responses_inbound.rs` 4 项全绿：文本 / 工具 / 流式 / 404 错误形状
- 全量回归：71 单测 + M2/M3/M4/M5/M6 集成共 31 项全绿，`cargo fmt --check` + clippy -D warnings 零警告

### M7（配置导入 + WebDAV 同步）—— 已完成 ✅

- 导入：`store/import.rs` 解析 `jai-export/v1`，按 (name, base_url) 去重 upsert，
  生成「待录入密钥」清单与导入报告（新增/重复/模型/无效供应商）
- WebDAV：`sync.rs` PUT/GET 推拉 `jai-config.json`，Basic Auth，密码存系统钥匙串；
  推送前本地快照留存（`webdav_last_snapshot`），last-write-wins
- Tauri IPC：`config_import` / `webdav_config_get/set` / `webdav_push/pull`
- UI：新增「同步」页 —— 粘贴导入、WebDAV 配置、手动推/拉
- 测试：`tests/m7_import_webdav.rs` 3 项 + import/sync 单测全绿
- 全量回归：77 单测 + M2/M3/M4/M5/M6/M7 集成共 34 项全绿，`cargo fmt --check` + clippy -D warnings 零警告

### M8（收尾加固）—— 自动化部分已完成 ✅（48h 常驻观察待公测）

- 全矩阵回归一键脚本：`scripts/regression.sh`（fmt / clippy / test / 前端 build）
- fuzz 简表：`tests/m8_hardening.rs` 500 个确定性随机 body 覆盖四类入站解码器与
  SSE 解析器，无 panic/悬挂
- 护栏常量审计：现有 `ir::validate_guards` 单测（blocks ≤ 64 / args ≤ 256KB）持续全绿
- 文档一致性：README / roadmap 随 M1–M8 同步更新
- 资源/性能基线与 48h 本机观察：列入 M9 发布前真机验收项（本仓库无法自动完成）

### M9（发布工程）—— 文档与 CI 已就绪 ✅（签名/公证需真实 secrets 执行）

- `docs/design/release.md`：签名/公证/更新通道/发布流程检查单
- `.github/workflows/release.yml`：tag 触发 macOS/Windows 构建，接入
  Apple/Windows 签名 secrets 与 Tauri Release 草稿
- `CHANGELOG.md`：M1–M8 变更记录
- README 与 roadmap 同步
- 实际签名/公证/更新 feed 发布：需要仓库 secrets 与真实证书后执行

### MCP（Model Context Protocol）管理 —— 基础管理 + 工具发现/调用已完成 ✅

- 新增 `mcp_servers` 表（stdio/sse/http、命令/参数/URL、启停）
- store CRUD + Tauri IPC（`mcp_list/create/update/set_enabled/delete`）
- `gateway-core::mcp`：轻量 MCP 客户端，支持 initialize / tools/list / tools/call
  （stdio 走子进程 JSON-RPC，http/sse 走 HTTP JSON-RPC）
- Tauri IPC：`mcp_tools_list` / `mcp_tools_call`；UI「MCP」页可列出远端工具
- 网关请求自动合并 MCP 工具/执行工具循环：已完成 ✅（请求注入 MCP 工具；
  上游发起 MCP 工具调用时网关自动执行并回填结果，非流式/流式入站均支持）

### 技能（Skill）管理 —— 基础管理 + 自动注入已完成 ✅

- 新增 `skills` 表（名称/描述/内容/启停）
- store CRUD + Tauri IPC（`skill_list/create/update/set_enabled/delete`）
- `gateway-core::skills`：读取启用技能并格式化为 system 文本
- 跨族转换路径自动将启用技能追加到 system（直通路径保持字节不变）
- UI「技能」页：添加/编辑/启停/删除技能定义

### 高级路由（第二阶段抽做）—— 已完成 ✅

- 模型别名/映射：`models.upstream_model_id` 现在会实际用于发给上游的 model 字段（同族直通与跨族转换均生效），UI 模型表可编辑
- 权重负载均衡：供应商新增 `weight` 字段（迁移 0004），同 `priority` 内按 weight 加权随机打散
- 健康感知排序：同优先级内基于 `last_ok_at / last_err_at` 把近期失败渠道排到健康渠道之后（5 分钟冷却窗口）
- 测试：router 单测覆盖优先级分组、健康优先、权重排序保持全量候选

### 用量统计（第二阶段抽做）—— 已完成 ✅

- `store::logs::usage_stats`：按天聚合请求数 / 输入输出 Token / 缓存读取
- Tauri `stats_usage` + UI「统计」页：近 7/30/90 天切换与柱状图
- 单测覆盖聚合正确性

### 待办（下一里程碑）

1. 真机验收 M1–M9 + MCP/Skill（Claude Code/Codex 跨族链路、WebDAV、签名安装包、48h 常驻）

> 历史快照：M1–M9 + MCP/Skill 管理（含工具发现/调用、技能自动注入）快照已并入本节；更早的记录见 git 历史。

## 9. 铁律：客户端优先级（2025-08 用户拍板）

1. **DeepSeek Harness（dsh）是最高优先级客户端**。它是开源的，必须保证能通过 JAI 网关完美使用；
   任何改动不得破坏 dsh 链路，dsh 验证不通过不得宣称完成。
2. **zcode 次重点支持**。使用量也大，应与 dsh 一并纳入回归；其余客户端相对靠后。
3. dsh 相关验收必须覆盖：注册/添加 DeepSeek 渠道、模型发现、真实对话、SSE 流式、工具调用（如 harness 触达）。
