# FreeLLMAPI → JAI 借鉴报告

> 对象：[tashfeenahmed/freellmapi](https://github.com/tashfeenahmed/freellmapi)（聚合 34 个免费 LLM 供应商、635 个免费端点，对外提供一个 `/v1` 兼容入口）
> 基线：JAI 当前工作区代码（代码 HEAD `32488c2`，v0.1.9；README 行号已同步至 `0dae06c`）
> 复核方式：`node scripts/verify-borrow-report.mjs` 做**机械校验**——每条 `文件:行号` 引用回仓验证文件存在且行号在范围内；每条 FreeLLMAPI 机制挂其公开文档 URL 并线上核可达。
> ⚠️ **该脚本不校验"行内容是否支持说法"**（语义正确性是另一回事）。语义层面由一次独立子代理回仓复核覆盖，其结果与由此产生的勘误见文末「§5 勘误记录」——**本报告已被该复核驳回并据此改写了两处结论**，改写过程保留在案。

## 0. 一句话结论

FreeLLMAPI 与 JAI 是**同一物种的两个变种**：都是"把一个本机入口架在多家上游前面"的网关。但两者稀缺的东西不同——FreeLLMAPI 稀缺**额度**（免费层要省着用、要在池子里挑），JAI 稀缺**信任**（用户自带 key，要的是"把 agent 指向 127.0.0.1 之后它别掉链子"）。

所以它的**池化 / 签名目录 / 社区先验**这套为"免费额度"服务的设计对 JAI 基本不适用；值得抄的是它为在恶劣条件下活下来磨出来的两样东西：**额度治理**与**接入自动化**。

值得注意的是：经独立复核后，JAI 在"失败记忆与失败归因"上比初稿判断的**扎实得多**（已有持久化失败记忆 + 逐候选错误归因）。真正被证实的空白只有两处——**渠道限额账本**与**接入生成器**，两者恰好都直接服务于"稳定"与"省心"这两个承诺。

## 1. 机制对照表

| 维度 | JAI 现状（证据） | FreeLLMAPI 做法（文档） | 差距判断 |
| --- | --- | --- | --- |
| 失败记忆 / 冷却 | **已有持久化失败记忆**：每次故障转移失败都落 `last_err_at`（[crates/gateway-core/src/server/proxy.rs:1027](../../crates/gateway-core/src/server/proxy.rs) → [crates/gateway-core/src/store/mod.rs:333](../../crates/gateway-core/src/store/mod.rs)），选路时把该渠道整体排到健康渠道之后（[crates/gateway-core/src/router/mod.rs:54](../../crates/gateway-core/src/router/mod.rs)）。**但冷却不分级**：固定 5 分钟常量（[crates/gateway-core/src/router/mod.rs:14](../../crates/gateway-core/src/router/mod.rs)）只对"从未成功过"的渠道生效（[:58](../../crates/gateway-core/src/router/mod.rs)），有过成功史的渠道失败后无时间窗自愈，靠重试成功复位 | 冷却阶梯 2m→10m→1h→24h + 来源分级（heuristic / authoritative / credit / tier），Retry-After 优先于自身猜测 | 不是"有没有冷却"，而是**冷却不分错误类型、不听 Retry-After、时长不升级、缺时间窗自愈** |
| 失败归因 | **已逐候选记录**：失败候选写 `error_kind` + 含供应商名的 `error_summary`（[crates/gateway-core/src/server/proxy.rs:1026-1043](../../crates/gateway-core/src/server/proxy.rs)；字段 [crates/gateway-core/src/store/logs.rs:37](../../crates/gateway-core/src/store/logs.rs)、[:39](../../crates/gateway-core/src/store/logs.rs)），UI 日志页可见；上游 HTTP 错误原样回传（[crates/gateway-core/src/server/proxy.rs:796](../../crates/gateway-core/src/server/proxy.rs)），`502 all_providers_failed` 仅走网络级失败路径。**边界例外**（3 例）：未知协议族的 Failover 未标记降级（[crates/gateway-core/src/server/proxy.rs:1324](../../crates/gateway-core/src/server/proxy.rs)）、转换路径缺 key 只降级不写归因（[crates/gateway-core/src/server/proxy.rs:1338](../../crates/gateway-core/src/server/proxy.rs)）、原生缺 key 的摘要不含供应商名（[crates/gateway-core/src/server/proxy.rs:916](../../crates/gateway-core/src/server/proxy.rs)） | 跨候选聚合成一句话（"检查 5 条：3 条限流/冷却、2 条无可用 key，最近恢复约 2m"），并回 `X-Fallback-Trail` 头 | 缺**聚合视图**、缺"被跳过/未尝试候选"的记录、缺回程头 |
| 额度账本 | **空白**：全仓无 RPM/RPD/TPM/TPD/quota/in-flight lease 任何实现；[crates/gateway-core/src/server/ratelimit.rs:21](../../crates/gateway-core/src/server/ratelimit.rs) 只是鉴权失败限速器 | 四维滑动窗 + 在途租约堵 check-then-act 竞态；从上游错误体学限额且只收紧 | **真实空白**：并发场景下 JAI 会集体打爆同一上游 |
| 渠道健康度 | 二值分区（健康/不健康），无"会衰减的分数"；失败会持久化并降级（见上） | 五轴连续分：可靠度（Thompson 采样后验）+ 速度 + 智力 + 额度余量 + 限流惩罚 | 缺连续分；但全量 bandit 对单人自用是过度设计 |
| 选路顺序 | priority 分组 → 健康优先 → 同优先级内 weight 加权随机打散（[crates/gateway-core/src/router/mod.rs:18](../../crates/gateway-core/src/router/mod.rs)、[:63](../../crates/gateway-core/src/router/mod.rs)；[crates/gateway-core/src/store/mod.rs:486](../../crates/gateway-core/src/store/mod.rs)；迁移 [crates/gateway-core/src/store/migrations/0004_advanced_routing.sql:1](../../crates/gateway-core/src/store/migrations/0004_advanced_routing.sql)） | 六种命名策略 + 命名链 profile，可 `auto:<profile>` 逐请求切换 | 已有可靠雏形，缺"策略"旋钮与命名链 |
| 首字节耗时 | 只有"首字节 60s 超时"这个**阈值常量**（[crates/gateway-core/src/server/proxy.rs:38](../../crates/gateway-core/src/server/proxy.rs)），不测量；日志只记总时长（[crates/gateway-core/src/store/logs.rs:34](../../crates/gateway-core/src/store/logs.rs)；前端 [ui/src/types.ts:80](../../ui/src/types.ts)） | TTFB 与吞吐共同构成速度轴（`0.6×吞吐 + 0.4×TTFB`） | **真实空白**：没有 TTFB 就没有"快慢"维度的任何决策依据 |
| 会话连续性 | 无会话粘性概念（全仓 grep 命中 0） | 会话键粘性 30 分钟；跨模型切换时注入紧凑交接便条 | 缺；对 dsh 长会话影响直接 |
| 重试编排 | 按候选序列逐渠道尝试（候选查询 [crates/gateway-core/src/store/mod.rs:471](../../crates/gateway-core/src/store/mod.rs)） | 最多 20 次尝试 + 45s 墙钟预算 + 预算耗尽即 abort 在途请求 + 连续失败熔断 | 缺"总预算"概念 |
| 客户端接入 | 网关页给接入示例、用户复制粘贴（[README.md:115](../../README.md)） | 14 个 `setup-*` 生成器：结构化合并客户端配置、写 `$DSH_HOME/.env`（0600）、改动前时间戳备份、`--dry-run` 打 diff | **真实空白**；而 JAI 第一优先客户端正是 dsh（[README.md:112](../../README.md)） |
| 协议面广度 | 入站 `/v1/chat/completions`、`/v1/models`、`/v1/messages`、`/v1/responses`、`/v1/completions`、`/mcp`、`/healthz`（[crates/gateway-core/src/server/mod.rs:90-108](../../crates/gateway-core/src/server/mod.rs)，`/mcp` 见 [:103](../../crates/gateway-core/src/server/mod.rs)） | 另有 embeddings / images / videos / audio、原生 Gemini `/v1beta`、Ollama 仿真（NDJSON）、可撤销 URL token | 按需补，非核心 |
| 密钥存储 | **明文入 SQLite**（迁移 [crates/gateway-core/src/store/migrations/0006_secrets_in_db.sql:1](../../crates/gateway-core/src/store/migrations/0006_secrets_in_db.sql)；[README.md:86](../../README.md) 明说安全性依赖数据目录文件权限）；系统钥匙串已退役（[crates/gateway-core/src/vault.rs:4](../../crates/gateway-core/src/vault.rs)） | AES-256-GCM 信封加密，密钥只在内存解密，对外只发统一 token | **慎抄**：JAI 是**故意**明文的，因为凭据要随 WebDAV 同步（[README.md:87](../../README.md)） |
| 模型目录 | 用户手动触发从上游 `/models` 拉取入库（[crates/gateway-core/src/discover.rs:99](../../crates/gateway-core/src/discover.rs)）；另有一个 10 分钟一轮的健康探测也会打 `/models`（[src-tauri/src/main.rs:1525](../../src-tauri/src/main.rs)） | 每 12 小时同步带签名的目录 feed | 不适用：BYO key，没有"池"可聚合 |
| 入口安全基线 | Host 必须回环（防 DNS rebinding）+ Origin 白名单（[crates/gateway-core/src/server/security.rs:4](../../crates/gateway-core/src/server/security.rs)） | 本地优先单用户；MCP 面可开关 | JAI 已到位，无差距 |

## 2. 值得借鉴（分级）

### P0-1 渠道限额账本 + 在途租约
FreeLLMAPI 用四维滑动窗（RPM/RPD 分钟与 UTC 日界）记账，并用**在途租约**堵住 check-then-act 竞态——否则"检查时都合格、并发发起后集体超限"；还会从上游错误体里学真实上限，且**只收紧不放宽**（[配额与冷却引擎](https://github.com/tashfeenahmed/freellmapi/blob/main/docs/en/architecture/02-quota-and-cooldown-engine.md) §2/§5）。
JAI 经独立复核确认**完全没有这个维度**（[crates/gateway-core/src/server/ratelimit.rs:21](../../crates/gateway-core/src/server/ratelimit.rs) 只是防爆破的鉴权限速，全仓 grep 无任何额度计量实现）。用户自带 key 容易让人以为"额度用户自知"，但**并发打爆是网关自己引入的问题**：多设备（WebDAV 同步那批机器）同时指向同一上游时，只有网关能看见全局并发。
- 落点：新增模块挂在 [crates/gateway-core/src/router/mod.rs](../../crates/gateway-core/src/router/mod.rs) 选路前置门，计数落 SQLite；先做 RPM/RPD 覆盖多数场景，TPM/TPD 后续
- 验证：上游声明 RPM=5、并发发起 10 个请求，断言实际外发不超过 5 次，多余请求走备用渠道或明确报错

### P0-2 dsh 接入生成器（`setup-dsh` 等价物）
FreeLLMAPI 的 `setup-dsh` 把路由**结构化合并**进 `$DSH_HOME/settings.yaml`（`api: openai-completions`、`/v1` base URL、用实时目录填 `models`），key 写进 `$DSH_HOME/.env`（0600），并支持 `--dry-run` 打 diff、改动前时间戳备份、不动用户已有配置（[客户端与编码 agent](https://github.com/tashfeenahmed/freellmapi/blob/main/docs/en/clients/01-agent-clients.md)）。
JAI 已认准 dsh 是第一优先客户端（[README.md:112](../../README.md)），但接入手段是"网关页展示配置、用户自己复制粘贴"（[README.md:115](../../README.md)），无任何生成器。**这是全清单里投入最小、用户可感知收益最直接的一条**：把"正确接入"从用户的记忆负担变成一条命令。
- 落点：`scripts/` 新增生成器，或 Tauri IPC 命令 + 设置页按钮；实现 settings.yaml 结构化合并 + 备份 + `--dry-run`
- 验证：在临时 `DSH_HOME` 下执行，断言既有其他 route 与注释被保留、`.env` 权限 0600、重复执行幂等

### P1-1 冷却分级：Retry-After 感知 + 错误类型区分 + 时间窗自愈
JAI 已有失败持久记忆与健康降级（[crates/gateway-core/src/server/proxy.rs:1027](../../crates/gateway-core/src/server/proxy.rs) → [crates/gateway-core/src/store/mod.rs:333](../../crates/gateway-core/src/store/mod.rs)；[crates/gateway-core/src/router/mod.rs:14](../../crates/gateway-core/src/router/mod.rs)、[:54](../../crates/gateway-core/src/router/mod.rs)），且 `router` 注释明确写着"最近失败的主渠道会被健康备渠道接管，而不是每次先撞一次失败"——这块的**骨架是对的**。差距在**分级**（[配额与冷却引擎](https://github.com/tashfeenahmed/freellmapi/blob/main/docs/en/architecture/02-quota-and-cooldown-engine.md) §4/§6）有三点：
1. **不听 Retry-After**：上游说"1 小时后重试"，JAI 过了 5 分钟照打——对日额度耗尽（RPD/TPD）这类错误，5 分钟窗口等于没有；
2. **冷却不分错误类型**：`classify_status` 确实给 401/429 打了不同标签（`UpstreamAuth` / `RateLimit`），但两者都汇入同一个 `provider_mark_fail` 降级，**冷却时长与恢复方式完全相同**——401（key 失效）本应长时间停用，而不是跟着一起"过一会儿再试"；
3. **时间窗自愈缺口**：[:58](../../crates/gateway-core/src/router/mod.rs) 的 5 分钟窗口只作用于"从未成功过"的渠道；有过成功史的渠道一旦失败就走 `ok >= err` 分支，没有时间到期，只能靠"被重试且成功"复位。**实际缓解**：`src-tauri` 的 10 分钟一轮健康探测成功时会写 `provider_mark_ok`——正是 `is_healthy` 所读的同一对列（[src-tauri/src/main.rs:1525](../../src-tauri/src/main.rs)），故这类渠道通常在 ≤10 分钟内被自动复位；`is_healthy` 本体没有时间窗，但系统层并非无自愈路径。
- 落点：[crates/gateway-core/src/router/mod.rs](../../crates/gateway-core/src/router/mod.rs) 的 `is_healthy` 与冷却时长决策 + `store` 增加冷却来源/到期字段
- 验证：假上游对首次请求返回 `429` + `Retry-After: 3600`，断言接下来 1 小时内该渠道不再被首选，且到期后自动恢复为首选

### P1-2 穷尽诊断聚合 + 回程头
JAI 对**失败候选**已经记了 `error_kind` 与含供应商名的 `error_summary`（[crates/gateway-core/src/server/proxy.rs:1026-1043](../../crates/gateway-core/src/server/proxy.rs)；字段 [crates/gateway-core/src/store/logs.rs:37](../../crates/gateway-core/src/store/logs.rs)），上游 HTTP 错误也原样回传（[crates/gateway-core/src/server/proxy.rs:796](../../crates/gateway-core/src/server/proxy.rs)）——**这块的原始材料是齐的**。缺的是把它**汇总成一句人能立刻读懂的话**，以及把"哪些候选被跳过、为什么跳过"也纳入记录（FreeLLMAPI 的 [路由与 bandit 评分](https://github.com/tashfeenahmed/freellmapi/blob/main/docs/en/architecture/01-routing-and-bandit-scoring.md) §9 做的是这件事）。
- 落点：[crates/gateway-core/src/server/proxy.rs](../../crates/gateway-core/src/server/proxy.rs) 失败收尾处聚合 + 日志详情页新增"本次尝试轨迹"视图
- 验证：构造"3 条渠道分别因 401/429/超时失败"，断言最终错误体或日志给出聚合归因（含各原因条数与"从未被尝试的候选数"）

### P1-3 会话粘性 + 上下文交接便条
FreeLLMAPI 用会话键做 30 分钟粘性，并在**确实换模型时**注入一条紧凑交接便条，防止新模型"重新开始任务"（[客户端与编码 agent](https://github.com/tashfeenahmed/freellmapi/blob/main/docs/en/clients/01-agent-clients.md) 的 Context Handoff 节）。
JAI 的健康优先排序（[crates/gateway-core/src/router/mod.rs:18](../../crates/gateway-core/src/router/mod.rs)）意味着健康状态一变就可能换上游。对 dsh 的长会话，换上游可能表现为"它突然忘了刚才在干嘛"。
- 落点：[crates/gateway-core/src/router/mod.rs](../../crates/gateway-core/src/router/mod.rs) 选路引入会话键；[crates/gateway-core/src/server/proxy.rs](../../crates/gateway-core/src/server/proxy.rs) 跨渠道切换时注入便条（默认关闭，与 FreeLLMAPI 的 opt-in 一致）
- 验证：同一会话连发两轮，中途让渠道 A 故障，断言第二轮仍优先 A 之外的**同一**渠道，且请求体中出现交接便条

### P1-4 TTFB 采集 → 轻量速度/可靠性评分
FreeLLMAPI 的速度轴是 `0.6×吞吐 + 0.4×TTFB`；可靠度用 Thompson 采样（[路由与 bandit 评分](https://github.com/tashfeenahmed/freellmapi/blob/main/docs/en/architecture/01-routing-and-bandit-scoring.md) §3/§4）。
JAI 侧 `proxy.rs` 只有"首字节 60 秒超时"阈值（[crates/gateway-core/src/server/proxy.rs:38](../../crates/gateway-core/src/server/proxy.rs)），日志只有总时长（[crates/gateway-core/src/store/logs.rs:34](../../crates/gateway-core/src/store/logs.rs)、[ui/src/types.ts:80](../../ui/src/types.ts)）。**建议只抄"测什么"与"轴怎么定义"，不抄 Thompson 全量 bandit**——单人自用样本稀疏，后验会长期停在先验附近，复杂度换不来决策质量；EWMA 成功率 + TTFB 足够。JAI 已依赖 `rand`/`sha2`（[crates/gateway-core/Cargo.toml:21-22](../../crates/gateway-core/Cargo.toml)），实现无新依赖。
- 落点：流式路径旁路记录首字节时刻 → 日志加列 → 作为健康度输入
- 验证：断言日志出现 TTFB 字段，且慢渠道在同等 priority 下排序靠后

### P1-5 重试墙钟预算 + 熔断
FreeLLMAPI：最多 20 次尝试、45s 墙钟预算、预算耗尽即中止在途请求、可选连续失败熔断（[路由与 bandit 评分](https://github.com/tashfeenahmed/freellmapi/blob/main/docs/en/architecture/01-routing-and-bandit-scoring.md) §9）。JAI 是纯序列尝试（候选查询 [crates/gateway-core/src/store/mod.rs:471](../../crates/gateway-core/src/store/mod.rs)），候选多、上游都慢时会长时间挂住客户端。
- 落点：[crates/gateway-core/src/server/proxy.rs](../../crates/gateway-core/src/server/proxy.rs) 编排循环加预算参数与超预算提前返回
- 验证：3 条候选都故意慢，断言总耗时被压在预算内且返回聚合归因（与 P1-2 共用结构）

### P2-1 命名链 profile 与策略旋钮
FreeLLMAPI 有六种命名策略与可保存的 profile 链，可 `auto:<profile>` 逐请求选链（[路由与 bandit 评分](https://github.com/tashfeenahmed/freellmapi/blob/main/docs/en/architecture/01-routing-and-bandit-scoring.md) §2/§12）。JAI 的 priority+weight（[crates/gateway-core/src/store/mod.rs:486](../../crates/gateway-core/src/store/mod.rs)）已能表达"主备+均衡"，缺"起个名、按场景切换"。
- 落点：`store` 增 profile 表 + 路由入口解析 `auto:<profile>`；UI 网关页加切换器
- 验证：两条 profile 指向不同 priority 序，断言同名模型在两 profile 下走不同渠道

### P2-2 响应缓存与幂等重放
FreeLLMAPI 有精确匹配缓存（key 已到 v4，会把默认值参数归一化以共享缓存条目）与 `Idempotency-Key` 重放（[路由与 bandit 评分](https://github.com/tashfeenahmed/freellmapi/blob/main/docs/en/architecture/01-routing-and-bandit-scoring.md) §15）。JAI 无响应缓存（`cache` 命中仅为 `Cache-Control: no-cache` 与 usage 里的 cache token 统计）。价值中等（agent 流量缓存命中率低），但"重试导致重复副作用"在多渠道故障转移下是真实风险。
- 落点：缓存进 `store`；幂等键仅对非流式请求生效
- 验证：同 `Idempotency-Key` 重放，断言只真正外发一次

### P2-3 协议面补齐（embeddings / audio / Ollama 仿真 / Gemini 入站）
FreeLLMAPI 这些面是为"让更多客户端接得进来"（[客户端与编码 agent](https://github.com/tashfeenahmed/freellmapi/blob/main/docs/en/clients/01-agent-clients.md)）。JAI 出站已有 gemini codec，入站路由表（[crates/gateway-core/src/server/mod.rs:90-108](../../crates/gateway-core/src/server/mod.rs)）尚无 `/v1beta`、无 embeddings、无 Ollama 面。按目标客户端需求驱动，不必齐备。

### P2-4 MCP 面扩展为"运行时可观测"
JAI 的 `/mcp`（[crates/gateway-core/src/server/mod.rs:103](../../crates/gateway-core/src/server/mod.rs)）目前是 MCP Server / Skill 台账的只读视图；FreeLLMAPI 的 MCP 让 agent 能内省"可用模型、供应商健康、用量与路由策略"，且**做成可开关的设置**（[客户端与编码 agent](https://github.com/tashfeenahmed/freellmapi/blob/main/docs/en/clients/01-agent-clients.md)）。JAI 可加"渠道健康/冷却状态"查询工具——与 P1-1、P1-2 的数据同源，边际成本低。

## 3. 已有 / 不适用（避免抄错东西）

**JAI 已经领先或已到位、无须借鉴的**

- **协议双轨制（同族直通 / 跨族才转换）**：这是 JAI 相对 FreeLLMAPI 的**结构性优势**。FreeLLMAPI 是"统一转成 OpenAI 语义再分发"，而 JAI 同族走字节级直通，未建模特性（缓存控制、citations）原样保留（[README.md:58-63](../../README.md)）。FreeLLMAPI 协议面更"广"，JAI 的更"保真"——广可以补，保真补不回来。
- **失败记忆与逐候选归因**：JAI 已持久化 `last_err_at` 并据此降级渠道，也已逐候选记录错误类型与供应商名摘要。本报告初稿曾误判为空白，经独立复核纠正（见 §5）。
- **入口安全基线**：Host 回环校验防 DNS rebinding + Origin 白名单（[crates/gateway-core/src/server/security.rs:4](../../crates/gateway-core/src/server/security.rs)），比 FreeLLMAPI 文档所述"本地优先"更具体。
- **WebDAV 多设备同步**：FreeLLMAPI 完全没有这个能力。JAI 的同步是差异化资产，**任何借鉴都不能以破坏它为代价**。

**不适用（照抄会做无用功）**

- **免费额度池化 + 签名目录 feed + 付费目录时效差**：FreeLLMAPI 的工程重心与商业模式都建立在"聚合 34 家免费额度"上。JAI 是 BYO key，没有池可聚合，目录随用户自己的上游走——手动触发的 `/models` 入库发现（[crates/gateway-core/src/discover.rs:99](../../crates/gateway-core/src/discover.rs)）在这个定位下是**正确**的，不是缺陷。（另有一个 10 分钟一轮的健康探测也会访问 `/models`，见 [src-tauri/src/main.rs:1525](../../src-tauri/src/main.rs)——但那是连通性探测，不等于目录同步。）
- **社区先验（community prior）**：需要多实例汇总服务做后盾，且与"本地优先、不上报"的隐私姿态冲突。
- **34 家供应商适配器矩阵**：JAI 的四协议族已覆盖"任意 OpenAI 兼容 / Responses / Anthropic / Gemini"，供应商数量不是 JAI 的变量。
- **provider-wide 额度池、免费层日额度过期、60 语言 i18n**：都是为"免费层 + 大众分发"服务的，与 JAI 定位无关。

**慎抄（方向可取，但不能照搬）**

- **密钥静态加密**：FreeLLMAPI 用 AES-256-GCM 信封加密（[高层架构索引](https://github.com/tashfeenahmed/freellmapi/blob/main/docs/en/architecture/00-high-level-index.md)）。但 JAI 是**故意明文**的——密钥要在设备间同步（[README.md:87](../../README.md)），凭据入 DB 的迁移也是为此（[crates/gateway-core/src/store/migrations/0006_secrets_in_db.sql:1](../../crates/gateway-core/src/store/migrations/0006_secrets_in_db.sql)），系统钥匙串已因此退役（[crates/gateway-core/src/vault.rs:4](../../crates/gateway-core/src/vault.rs)）。**照抄会直接打断多设备同步**。真要做，得先设计"导出即加密 / 设备级密钥 / 端到端加密"，属架构级改动。
- **冷却探针提前恢复**：机制好（节省闲置额度），但依赖"探针能低成本验证 key 有效性"。JAI 的上游是用户付费/自有 key，提前探测可能产生真实调用成本——建议先做 P1-1 的分级冷却，探针留到确认有需求再说。

## 4. 边界与不确定性

- 本报告对 FreeLLMAPI 的判断**全部来自其公开文档**（README 与 `docs/en/**`），未读其源码、未运行其本体。文档与实现若有偏差，以文档为准的结论需重核。
- FreeLLMAPI 仓库处于活跃演进中（其文档提到多个 commit/issue 编号）；本报告引用的机制描述对应**本次读取时点**的 `main` 分支。
- JAI 侧结论基于工作区 HEAD `32488c2`；`grep` 无命中类判断（如"无会话粘性"）只能证明**当前代码未出现该概念**，不排除以其他命名实现。
- 机械校验（`verify-borrow-report.mjs`）**只保证引用位点存在、行号在范围内、URL 可达**，不保证语义正确——这一盲区已实际导致初稿两处误判，见 §5。
- 按本轮约定，未评估 FreeLLMAPI 的商业面与各供应商 ToS。

## 5. 勘误记录

本报告初稿完成后，由一名**独立子代理**（无本会话上下文）回仓逐条核验，结论为 `reject`，指出两处"把 JAI 已有能力说成缺失"的错误。两处均已证伪并改写：

| 初稿说法 | 复核证据 | 改写结果 |
| --- | --- | --- |
| "429 只触发换下一个候选，**没有任何持久记忆**；每个新请求都会先撞它一次" | [crates/gateway-core/src/server/proxy.rs:1027](../../crates/gateway-core/src/server/proxy.rs) 的 `provider_mark_fail` → [crates/gateway-core/src/store/mod.rs:333](../../crates/gateway-core/src/store/mod.rs) 把 `last_err_at` 落库；[crates/gateway-core/src/router/mod.rs:14](../../crates/gateway-core/src/router/mod.rs)、[:54](../../crates/gateway-core/src/router/mod.rs)、[:58](../../crates/gateway-core/src/router/mod.rs) 据此把渠道降级；`router` 注释原文即"而不是每次先撞一次失败" | 改为：**已有持久化失败记忆与健康降级**；真实差距是冷却不分错误类型、不听 Retry-After、缺时间窗自愈（→ P1-1） |
| "日志没有每个候选为何失败；对外是一个 502 `all_providers_failed`" | [crates/gateway-core/src/server/proxy.rs:1026-1043](../../crates/gateway-core/src/server/proxy.rs) 对失败候选写 `error_kind` + 含供应商名的 `error_summary`，字段见 [crates/gateway-core/src/store/logs.rs:37](../../crates/gateway-core/src/store/logs.rs)；[crates/gateway-core/src/server/proxy.rs:796](../../crates/gateway-core/src/server/proxy.rs) 上游 HTTP 错误原样回传，502 仅网络级失败 | 改为：**已逐候选记录归因**；真实差距是缺跨候选聚合、缺未尝试候选记录、缺回程头（→ P1-2） |

复核同时确认了其余判断成立（密钥明文与 WebDAV 的关系、TTFB 缺失、无额度账本、无会话粘性、渠道健康二值分区、dsh 第一优先、以及"不适用/慎抄"清单本身），并验证了两条 FreeLLMAPI 文档 URL 可达且内容与描述一致。

**方法学教训**：机械校验（存在性 + 行号范围）不能替代语义核验。"引用行号真实存在"与"该行内容支持我的结论"是两件事——本次两处错误都发生在后者，而前者全绿。这条盲区已写入报告开头的复核方式声明。

**改写后的第二轮复核（同样由 fresh 子代理执行）结论为 `pass`**：两处修订的链路逐跳核对成立；"有过成功史的渠道无时间窗自愈"经 `is_healthy` 四分支逐条核对**判定为准确**；§5 本身经核验**不存在被掩盖的第三处错误**；另抽查 6 个引用位点全部支持原文。复核同时提出两条非阻塞观察，均已并入本报告：① 10 分钟健康探测会写同一对列，故成功史渠道实际 ≤10 分钟自动复位（已补入 P1-1 第 3 点）；② 归因存在配置/边界例外，故全文不再使用"每个失败候选"这类全称量词（已补入 §1 表与 P1-2）。
