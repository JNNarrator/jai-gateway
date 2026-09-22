# 下一迭代：渠道草稿测试 + 5 项缺口修复（Channel Draft Probe & Gap Fixes, D9）

> 提案日期：2026-09-22 · 前置：v0.3.1（已交付）
>
> 一句话目标：把「配错了要等真请求才知道」变成「保存前就知道」，并顺手补掉 4 项经实测确认的工程缺口。
>
> 全部「现状事实」均由源码实读取证（路径:行号），不是文档口径复述。开工前请先看 §6 的拍板项。

---

## 0. 任务与需求映射

> **决策已拍板（2026-09-22）**：D1–D4 按本方案建议锁定；**D5 配额不做**。详见 §6.1。

| 任务 | 对应你说的 | 性质 | 预估 |
| --- | --- | --- | --- |
| **T1 渠道草稿测试** | 渠道草稿测试 | 新功能（核心） | 3–5 天 |
| **T2 Retry-After 遵从** | 缺口① | 小修 | 0.5–1 天 |
| **T3 重试模型升级**（分组 + 双档预算 + 跨组规则） | 缺口①的完整形态 | 重构 | 2–3 天 |
| **T4 单实例锁** | 缺口② | 小修 | 0.5 天 |
| **T5 迁移前自动备份 DB** | 缺口③ | 小修 | 1 天 |
| **T6 网关密钥多密钥 + 白/黑名单**（~~配额~~ 已砍） | 缺口④ | 大（有前置缺失） | 3.5–4.5 天，**建议拆两迭代** |

> 你说的「5 个缺口」里第 5 条本身就是渠道草稿测试。我把它当 T1 单列，把重试拆成 T2（可单独发布的小修）与 T3（完整形态），于是正好 6 项任务。
>
> **T6 有前置缺失**（见 §2.4）：当前「请求用了哪个密钥」在鉴权中间件里被直接丢弃，所以白名单没有地方可挂；且「密钥」当前实质只有一把，单密钥谈「分级权限」没有意义。建议拆成 T6a（多密钥地基）+ T6b（白/黑名单）两个迭代。
>
> **配额（D5）已砍**，因此不再需要 `gateway_keys.quota_*` 三列与 `request_logs.key_id` 的配额聚合用途（迁移 0014 取消）。

---

## 1. 背景与动机

- 对标 WaLiAPI 后确认：它在**渠道配置体验**与**上游调度健壮性**两处明显强于我们（详见 `docs/design/WaLiAPI-对标研究报告.md` §2.3 / §2.5）。本迭代只取其中「成本可控、收益直接」的部分。
- 我们现在的「测试连接」只打 `/models`——**它证明不了渠道能不能真的推理**。上游 `/models` 返回 200 而 `/chat/completions` 返回 400 是常见情况（模型名不对、协议族选错、缺 `max_tokens`、只支持 Responses 不支持 Chat）。用户要等到在客户端发第一条消息才发现。
- 4 项工程缺口都是「已经踩过或即将踩到」的：
  - 上游 429 时我们**没有退避**，单轮遍历全部候选会把限流窗口打满（`proxy.rs` 内 `grep sleep` 零命中）；
  - 用户双击图标两次就能起两个网关进程（启动路径**零单实例保护**），两个网关监听不同端口，用户不知道客户端该连哪个；
  - 迁移失败 = `main.rs:3510` panic 退出，**无备份、无 UI 提示**，用户只看到一个闪退的图标；
  - 网关密钥实质只有一把（`gw_key_active` 只取最新一条），无法给团队内不同人发不同权限的 key。

---

## 2. 现状事实（探索取证）

### 2.1 重试与路由

| 事实 | 证据 |
| --- | --- |
| 入口 | `dispatch(wire, ctx, req)` @ `proxy.rs:1114`；四个数据面端点全部转发到此 |
| 候选获取 | `store::route_candidates(c, &model_key)` @ `proxy.rs:1177-1179` |
| 主循环 | `for cand in &candidates` @ `proxy.rs:1252` —— **单轮遍历一遍即止**，注释见 `:1251` |
| 隐式分组 | `candidates.sort_by_key(\|c\| c.family != wire.family())` @ `proxy.rs:1211` —— **同族/跨族分组已存在，只是没显式化** |
| 失败判定 | `AttemptVerdict::{Stop{kind}, Failover{kind}, Success}` @ `router/mod.rs:87-98`；`classify_status(status, body_excerpt)` @ `router/mod.rs:117-153` |
| 状态码映射 | 401/403→`Failover{UpstreamAuth}`；404/408→`Failover{ProviderOther}`；429→`RateLimit`；529→`Overloaded`；500..600→`Overloaded`；400→`ContextTooLong`（正文含 `context_length`/`context_window`）否则 `InvalidRequest`；400..500→`InvalidRequest`；2xx→`Success` |
| **死代码** | `first_byte_verdict(err_is_connect)` @ `router/mod.rs:103-111` **全仓从未被调用**（只在自己的单测里出现）——即「首字节失败应 failover」这个语义**从未落地** |
| 候选间退避 | **没有**。唯一常量是连接重试 `UPSTREAM_CONNECT_RETRY = 1` @ `proxy.rs:69` + `should_retry_connect` @ `83-85`，且**无 sleep** |
| Retry-After | 已被读取但**只回传客户端**：`FORWARD_ERROR_HEADERS` @ `proxy.rs:1059` = `["retry-after","retry-after-ms","x-request-id"]`，取值 @ `1645-1652`，回传 @ `1734-1736` / `1324-1326`。**不参与内部调度** |
| **跨族丢头** | `try_converted_candidate` @ `proxy.rs:2158-2164` 只取 body，**不收集错误响应头** → 跨族失败时 Retry-After 直接丢失 |
| 流式 commit | `streaming_response` @ `proxy.rs:3355` 有「首字节持票」（`UPSTREAM_FIRST_BYTE_TIMEOUT` = 60s @ `proxy.rs:38`）；首字节失败三分支（`:3370` / `:3395` / `:3420`）全部 `return wire.error_response(...)` → 被包成 `Attempt::Delivered` @ `1814`，**不 failover** |
| 超时常量 | 首帧 60s、`STREAM_IDLE_TIMEOUT` 120s @ `:40`、`NONSTREAM_READ_TIMEOUT` 300s @ `:42`、connect 10s @ `:203` |
| 配置 | **没有 settings 表**，只有 `meta(key,value)` @ `0001_initial_schema.sql:4-7`；`GatewayCtx` @ `proxy.rs:184-197` 无任何重试字段 |
| 错误体 | `InboundWire::error_response(status, message, err_type, code)` @ `proxy.rs:147-167`；三方言构造函数见 `codec/openai.rs:273-282` / `codec/anthropic.rs:119-127` / `codec/responses.rs:27-36`。**`failure_class` 字段不存在** |

### 2.2 渠道测试与表单

| 事实 | 证据 |
| --- | --- |
| 现有测试命令 | `provider_test` @ `main.rs:398`、`provider_test_draft` @ `main.rs:440` —— **两者都只调 `discover_models`，即只打 `/models`** |
| `probe_provider` | @ `main.rs:2196`，就是 `discover_models(...)`，注释 `:2195` 明说「HTTP 200 即视为连通（0 个模型也算通）」 |
| 草稿入参/出参 | `ProviderTestDraftInput{base_url, family, api_key}` @ `main.rs:423`；`ProviderTestDraftResult{ok, count, model_names}` @ `main.rs:431` |
| 无端点概念 | provider 只有 `base_url + family` 二元组，**不能声明多个端点**；family 受 CHECK 约束四选一 |
| provider 列 | `id,name,base_url,family,enabled,priority,weight,extra_headers,api_key,website,reasoning_effort_levels,max_tools,last_ok_at,last_err_at,last_err_msg,created_at,updated_at`（`PROVIDER_COLS` @ `store/mod.rs:159`） |
| 上游请求构造可复用点 | `url_join` @ `codec/openai.rs:264`；`InboundWire::upstream_path()` @ `proxy.rs:118`；`InboundWire::apply_auth()` @ `proxy.rs:130`；`build_upstream` @ `proxy.rs:1570`（直通）/ `:2082`（转换） |
| client | 全局唯一 `AppCore.http`，启动时 `netcfg::build_client(proxy, 10s)` @ `main.rs:3336-3339`，**请求期不重建**（代理自动继承） |
| **URL 校验** | **对 `base_url` 没有任何校验**——无 scheme 白名单、无内网/回环拦截。`netcfg::validate_proxy_url` @ `netcfg.rs:59` 只管代理地址；`security::check_host` @ `security.rs:49` 是**入站**防 DNS rebinding。**存在 SSRF 敞口** |
| 前端表单 | `ProviderDialog` @ `ProvidersPage.tsx:426-764`；「测试连接」按钮 @ `:735`；`testConnection()` @ `:484-507`；react-hook-form + zodResolver，schema @ `:97-124`；脏 guard `useDirtyGuard` @ `:475` |
| 脱敏 | `ProviderRow.api_key` 标 `#[serde(skip_serializing)]` @ `store/mod.rs:114-116`；`ProviderDto` @ `main.rs:165-183` 只给 `has_key` @ `:201`。**但导出物不脱敏**：`store/export.rs:62-63` 显式写明文（WebDAV 同步契约，`sync.rs:12` 注释） |

### 2.3 启动、数据目录、迁移

| 事实 | 证据 |
| --- | --- |
| 入口 | `main()` @ `main.rs:3227`；`.run(...)` @ `:3509`；`.expect("error while running jai")` @ `:3510` |
| 插件注册 | @ `main.rs:3229-3232`，仅 4 个：`updater` / `process` / `notification` / `opener` |
| 启动顺序 | ①数据目录 `:3244-3263` → ②`db_path = data_dir.join("jai.db")` `:3264` → ③`Db::open`（含全部迁移）`:3267` → ④VACUUM 回收 `:3271-3282` → ⑤钥匙串存量迁移 `:3287-3294` → ⑥`logs::spawn_logger` `:3296` → ⑦快照自愈 `:3301` → ⑧读设置 `:3306` → ⑨保留循环 `:3325` → ⑩`AppCore` `:3332` → ⑪`ensure_gateway_key` `:3346` → ⑫`spawn_autopush` `:3349` → ⑬`spawn_health_check` `:3352` → ⑭托盘 `:3354-3407` → ⑮`app.manage` `:3410` → ⑯**启动即拉起网关** `:3428` |
| **单实例** | **完全没有**。全仓 grep `single.instance` / 文件锁 / `tauri-plugin-single-instance` **零命中**。唯一「占用探测」是 `port_in_use` @ `main.rs:3057`（`TcpListener::bind` 判占用），**只在设置页保存前调用，不参与启动路径** |
| 数据目录 | `app.path().app_data_dir()`，失败/不可写则回退 `std::env::temp_dir()/jai-data`（`main.rs:3244-3263`）。identifier = `app.jai.gateway` |
| 目录内文件 | **只有 `jai.db`**（WAL 模式下另有 `-wal` / `-shm`）。**没有独立日志目录**，请求日志进 `request_logs` 表 |
| 迁移执行器 | `migrate(conn)` @ `store/mod.rs:47-65`：`PRAGMA user_version` 逐条比对，每条一个 `BEGIN; <sql>; PRAGMA user_version = N; COMMIT;`（**版本号与 schema 同事务，原子**）；迁移期 `foreign_keys=OFF`。`MIGRATIONS` @ `migrations.rs:5-51`，**共 12 条**（0001–0012） |
| 迁移失败 | 返回 `Err`（**不是 panic**），但调用方 `main.rs:3267` 用 `?` → `setup` 返回 `Err` → `main.rs:3510` panic。**无弹窗、无通知、无备份** |
| `store/snapshot.rs` | **不是 DB 备份**！是内嵌的「模型元数据快照」（上下文窗口/最大输出默认值，`include_str!("snapshot.json")`）。别被文件名骗了 |
| 真正的快照 | WebDAV 推送前留存，存 **DB 的 meta 表** key = `webdav_last_snapshot`（`sync.rs:16`），`snapshot_get` @ `sync.rs:242` / `snapshot_put` @ `:266`（超 4 MiB 跳过并告警）；调用点 `main.rs:1518-1526` |
| **已有但未接线** | `BACKUP_KEEP = 10` @ `sync.rs:590` + `backup_evict_candidates(names, keep)` @ `sync.rs:595-606`（按时间戳升序取最老 `len-keep` 个）——**生产代码从未调用**，只有自己的单测用。且它作用于 **WebDAV 远端**备份 |
| 依赖 | `src-tauri/Cargo.toml` 无 `fs2` / `fd-lock` / `nix` / `sysinfo` / `single-instance`；`tracing` 与 `log` 只在 `Cargo.lock` 里作为传递依赖出现，源码 `tracing::` / `log::` 零命中 —— **全部日志是 `println!`/`eprintln!`** |
| 托盘菜单 | `gw-start` / `gw-stop` / `show` / `quit` @ `main.rs:3354-3407` |
| 网关启停 | `gateway_start` @ `:3521`（有 `state.running` 进程内幂等 @ `:3526`）、`spawn_supervisor` @ `:3099-3181`（`st.supervisor.lock().unwrap().is_some()` 幂等 @ `:3104`，Crash 后 1s 重启 @ `:3168-3171`） |

### 2.4 网关密钥与鉴权

| 事实 | 证据 |
| --- | --- |
| 表结构 | `gateway_keys(id, key, prefix, label, created_at, revoked_at, last_used_at)` @ `0001_initial_schema.sql:38-46`；**0002–0012 无任何 ALTER 到此表** |
| **实质单密钥** | `gw_key_active` @ `store/mod.rs:638-653` 的 SQL 是 `... WHERE revoked_at IS NULL ORDER BY created_at DESC LIMIT 1` —— **只返回一条** |
| 前缀 | `gen_gateway_key` @ `main.rs:727-734`：`sk-jai-` + 28 随机字符；`prefix` 取前 14 字符 @ `store/mod.rs:669` |
| 鉴权 | `security::authenticate(db, headers) -> Result<AuthedKey, Response>` @ `security.rs:125`；`bearer_token` @ `:170` / `x_api_key` @ `:182`；`ct_eq` @ `:20`（双方 SHA-256 后逐字节异或折叠） |
| **关键缺口** | `AuthedKey{id}` @ `security.rs:116-118` 在中间件 `proxy.rs:260` 被**直接丢弃**：`Ok(_key) => next.run(req).await`。既不进 `req.extensions()` 也不进 `GatewayCtx`。**全仓 grep `extensions_mut` 零命中** |
| 限速 | `ratelimit.rs`：`MAX_FAILS_PER_WINDOW=10` @ `:14`、`FAIL_WINDOW_MS=60_000` @ `:16`、`BAN_MS=300_000` @ `:18`；**只针对鉴权失败** |
| 候选查询 | `route_candidates(c, model_name)` @ `store/mod.rs:582-619`，**参数只有 model_name**；唯一调用点 `proxy.rs:1178` |
| request_logs | 17 列 @ `0001_initial_schema.sql:48-69`：`id,ts,inbound_family,route_mode,model_name,provider_id,upstream_model_id,http_status,stop_reason,usage_input,usage_output,usage_cache_read,usage_cache_write,duration_ms,is_stream,tool_calls,error_kind,error_summary`。**无 key 归属列**；索引只有 `idx_logs_ts(ts DESC)` 与 `idx_logs_model_ts(model_name, ts DESC)` |
| 配额 | **完全不存在**。`grep quota/配额` 的命中全是「单次输出截断诊断」`OutputBudgetClipped` @ `proxy.rs:564-625`，与账户配额无关 |
| 聚合可复用 | `usage_stats(db, days)` @ `logs.rs:269` 按天聚合 usage，**但不按 key/provider 分组** |
| 缓存模式可复用 | `CorsAllowlist` @ `security.rs:226-282`：`Arc<Mutex<Option<(i64, Vec<String>)>>>` + 5s TTL + `note_missing()` 每 600 次打一条日志。**这是热路径配置查询的现成模板** |
| 密钥命令 | `gateway_key_info` @ `main.rs:769`（恒不带全文）/ `gateway_key_reveal` @ `:782` / `gateway_key_regenerate` @ `:796`（= 轮换，创建+吊销旧的） |
| 前端 | `GatewayPage.tsx` 常态显示 `prefix…（点击右侧显示全文）` @ `:271-277`；`doCopyKey` @ `:89-97`；`confirmRotate` @ `:293` |

### 2.5 顺带发现的两个真实 bug

1. **`ui/src/types.ts:19` 的 `Family` 类型漏了 `openai_responses`**：
   ```ts
   export type Family = "openai_compat" | "anthropic" | "gemini";
   ```
   后端 `Family` 枚举（`codec/mod.rs:28-33`）与 providers 表的 CHECK 都是**四个值**。这个类型定义会让 TS 层无法正确表达 Responses 族渠道（依赖 `as any` 或类型断言绕过）。一行修复。
2. **`first_byte_verdict` @ `router/mod.rs:103-111` 是死代码**，且它想表达的语义（「首字节失败可以 failover」）当前**没有实现**——首字节失败走 `Attempt::Delivered` 直接返回错误。这是 T3 要补的一块。

---

## 3. 迭代范围

### T1 渠道草稿测试（核心）

**目标**：在「新建/编辑供应商」弹窗里，除现有「测试连接」（打 `/models`）之外，新增「端点探测」——对每个候选端点发一次**真实的最小推理请求**，逐端点给出「通过 / 失败 + 失败分类 + 延迟」。

#### T1.1 新增模块 `crates/gateway-core/src/probe.rs`

现有 `discover.rs`（325 行）只负责列模型，**不要塞进去**——它的职责已经很清晰。新建独立模块。

```rust
/// 探测目标（不落库，纯草稿）
#[derive(Debug, Clone)]
pub struct ProbeTarget {
    pub family: String,                 // 四族之一
    pub base_url: String,               // 已 normalize_base
    pub api_key: Option<String>,
    pub model: String,                  // 必填，探测必须有模型名
    pub extra_headers: Option<String>,  // JSON map，与 provider 同格式
}

/// 探测结论
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus { Passed, Failed, Skipped }

/// 失败分类（对用户可解释）
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeCategory {
    Authentication,       // 401/403
    Model,                // 404 且正文含 model / not found
    EndpointUnsupported,  // 404/405/501 且正文不像模型问题
    Request,              // 400/422 其它
    RateLimit,            // 429
    Overloaded,           // 5xx / 529
    Timeout,              // 超时
    Network,              // 连接失败 / DNS
    Protocol,             // 200 但 body 无法解析成预期结构
    UrlBlocked,           // 被 SSRF 校验拦下
    NoModel,              // 未填模型 → Skipped
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProbeOutcome {
    pub endpoint: &'static str,       // "chat_completions" / "responses" / "messages" / "gemini_generate"
    pub status: ProbeStatus,
    pub category: Option<ProbeCategory>,
    pub message: String,              // 已脱敏，≤300 字符
    pub latency_ms: u64,
    pub tested_model: Option<String>,
    pub cost_possible: bool,          // 是否可能产生了计费
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DraftProbeReport {
    pub run_id: String,               // uuid v4
    pub tested_at: i64,
    pub fingerprint: String,          // 见 T1.4
    pub results: Vec<ProbeOutcome>,
}

pub async fn probe_draft(
    client: &reqwest::Client,
    target: &ProbeTarget,
    cfg: &ProbeConfig,
) -> DraftProbeReport;

#[derive(Debug, Clone)]
pub struct ProbeConfig {
    pub per_probe_timeout: Duration,  // 默认 15s
    pub total_cap: Duration,          // 默认 60s
    pub allow_loopback: bool,         // 默认 false，见 T1.3
}
```

#### T1.2 端点与 payload 派生（按 family）

**注意**：provider 现在没有「端点」概念（只有 `base_url + family`），所以**从 family 派生主端点**，不引入新的数据模型（这是刻意的范围收敛——加 `native_endpoints` 是另一个迭代的事）：

| family | 探测端点 | 路径（用 `url_join` @ `codec/openai.rs:264`） | 鉴权（照抄 `InboundWire::apply_auth` @ `proxy.rs:130`） |
| --- | --- | --- | --- |
| `openai_compat` | `chat_completions` | `{base}/chat/completions` | `Authorization: Bearer <key>` |
| `openai_responses` | `responses` | `{base}/responses` | `Authorization: Bearer <key>` |
| `anthropic` | `messages` | `{base}/v1/messages` | `x-api-key: <key>` + `anthropic-version: 2023-06-01` |
| `gemini` | `gemini_generate` | `{base}/v1beta/models/{model}:generateContent` | `x-goog-api-key: <key>`（头传递，避免 key 进 URL/访问日志，与 `discover.rs:173-177` 一致） |

探测 body（全部 `stream: false`，`max*_tokens = 1`，**最小成本**）：

```jsonc
// openai_compat
{"model":"<M>","messages":[{"role":"user","content":"ping"}],"max_tokens":1,"stream":false}
// openai_responses
{"model":"<M>","input":"ping","max_output_tokens":1,"stream":false}
// anthropic
{"model":"<M>","max_tokens":1,"messages":[{"role":"user","content":"ping"}],"stream":false}
// gemini
{"contents":[{"parts":[{"text":"ping"}]}],"generationConfig":{"maxOutputTokens":1}}
```

**判定「通过」的标准**：HTTP 2xx **且** body 能被解析出预期的最小结构（例如 chat 有 `choices` 数组、messages 有 `content` 数组、responses 有 `output` 或 `status`）。只判 2xx 不够——上游返回 HTML 拦截页也是 200 的情况我们踩过（见 `docs/bug和优化清单.md`）。

**附加信息性探测（不进 passed 门禁）**：当 family 是 `openai_compat` 时，额外探一次 `/responses`，结果标 `status: Failed` 但 `category: EndpointUnsupported` 并在 UI 上灰显「不支持 Responses」——这让用户知道这个中转站能不能接 Codex。**这是可选加分项，做不完可以砍**。

#### T1.3 SSRF 校验（**必做，不能砍**）

现状：**对 `base_url` 零校验**（§2.2）。草稿测试是第一个「用户填什么就打什么」的入口，**必须同时建立校验能力**，否则这个便利会变成一个 SSRF 面。

新建 `crates/gateway-core/src/netguard.rs`（或放 `netcfg.rs`，但职责不同，建议独立）：

```rust
/// 校验出站 URL 是否允许访问。返回规范化 URL 或中文原因。
pub fn validate_outbound_url(raw: &str, allow_loopback: bool) -> Result<reqwest::Url, String>;

/// 是否是被禁止的目标 IP
pub fn is_blocked_ip(ip: &std::net::IpAddr, allow_loopback: bool) -> bool;
```

必须拦截的地址族（逐条写单测）：

- 非 `http` / `https` scheme
- IPv4：`0.0.0.0/8`、`10/8`、`127/8`、`169.254/16`（**link-local，云元数据端点 169.254.169.254 在这里**）、`172.16/12`、`192.168/16`、`100.64/10`（CGNAT）、`192.0.2/24`、`198.18/15`、`224/4`（组播）、`240/4`（保留）
- IPv6：`::1`、`fc00::/7`（ULA）、`fe80::/10`（link-local）、`ff00::/8`（组播）、未指定 `::`
- **IPv4-mapped IPv6**（`::ffff:127.0.0.1` 必须被识别为 loopback —— 这是最常见的绕过手法）
- 主机名为 `localhost` / `*.localhost` / `*.internal` / `*.local` 时按 loopback 处理

`allow_loopback = true` 时放行 loopback / `localhost`（用于 Ollama、LM Studio、vLLM 等本机部署），**但绝不放行 link-local 与云元数据**。

**接线范围（决策点 D2）**：
- **必须**：`probe_draft` 内部（T1）
- **建议**：`provider_create` / `provider_update` 保存时校验并拒绝非法 `base_url`（一次性校验，不做热路径每请求校验，避免「已存渠道突然被拦」）
- **不做**：热路径（`try_candidate`）每请求校验——性能与可用性风险不值得

> 顺带修掉 WaLiAPI 的同一个坑：它知识库 URL 做了完整校验，但渠道 `base_url` 没做。我们不要重复这个不一致。

#### T1.4 Fingerprint 与 receipt（可选门禁）

```rust
/// 不可逆指纹：family + 规范化 URL + model + 超时 + SHA256(api_key)
pub fn compute_probe_fingerprint(target: &ProbeTarget, cfg: &ProbeConfig) -> String;
```

进程内 receipt（**不落库**，重启即失效——这是刻意的，避免「半年前测过的渠道」被信任）：

```rust
pub struct ProbeReceiptStore { /* Mutex<HashMap<String /*fingerprint*/, (i64 /*at*/, DraftProbeReport)>> */ }
impl ProbeReceiptStore {
    pub fn put(&self, report: &DraftProbeReport);
    /// TTL 默认 30 分钟；且要求「本次全部 Passed 或 Skipped」
    pub fn validate(&self, fingerprint: &str, now_ms: i64) -> bool;
}
```

门禁开关（meta KV `require_probe_pass`，**默认 `false` = 只提示不强制**）：

- `false`：UI 在「测试连接」旁展示探测结果，保存不做拦截（推荐默认，避免本地/特殊渠道被卡死）
- `true`：`provider_create` / `provider_update` 校验 receipt，未通过则拒绝并提示「请先完成端点探测」

#### T1.5 后端接线

- 新 IPC `provider_probe_draft(input: ProbeDraftInput) -> DraftProbeReport`
  - `ProbeDraftInput { base_url, family, api_key, model, extra_headers, allow_loopback }` @ `main.rs` 新增
  - **复用 `core.http`**（`main.rs:3336-3339`）→ 自动继承出站代理与 10s connect timeout
- **保留** `provider_test_draft`（`main.rs:440`）不改，向后兼容（它是「列模型」语义，仍有用）
- 命令注册位置：`generate_handler!` @ `main.rs:3434-3508`，插在 `provider_test_draft` 之后

#### T1.6 前端

- `ProvidersPage.tsx` 的 `ProviderDialog`（`:426-764`）：在「测试连接」按钮（`:735`）右侧新增「端点探测」
- 新增 `ProbePanel` 组件（放 `ui/src/components/`），展示每个端点一行：

  ```
  ✓ chat_completions   通过    412ms   gpt-4o-mini
  ✗ responses          失败    380ms   404 model not found
  ⚠ messages           跳过           未填写模型
  ```

- 失败行 hover 显示完整 `message`（已脱敏）
- **模型下拉**：`model` 字段用已有的模型发现结果（`provider_discover_models`）填充；若用户还没发现模型，提供「先发现模型」按钮
- 新增 `allow_loopback` 复选框，文案：「允许访问本机地址（用于 Ollama / LM Studio 等本机部署）」，默认不勾
- `ui/src/types.ts` 补 `ProbeStatus` / `ProbeCategory` / `ProbeOutcome` / `DraftProbeReport` 类型；`ui/src/api.ts` 补 `providerProbeDraft`
- 脏状态：探测不改变表单，不影响 `useDirtyGuard`（`:475`）

#### T1.7 测试

单测（`probe.rs` 内联）：
- payload 生成矩阵（四族 × 有/无 key）
- `compute_probe_fingerprint`：同输入同指纹；改 api_key / model / URL 后指纹变化；确认不可逆（不含明文 key）
- 失败分类矩阵（用构造的 status + body 直接喂给分类函数）
- `netguard::is_blocked_ip` 全地址族矩阵（含 IPv4-mapped IPv6）
- `validate_outbound_url`：非法 scheme / 缺 host / 用户名密码 / 合法 https 通过

集成测试（`crates/gateway-core/tests/m13_probe.rs`）：
- 起本地假上游（仿 `bin/mcp_fake_server.rs` 或 tests 里已有的 hyper 假服务器）
- 200 + 合法 body → `Passed`
- 200 + HTML 拦截页 → `Failed{Protocol}`（**这条最重要**，防止「假绿」）
- 401 → `Failed{Authentication}`
- 404 + `{"error":{"message":"model not found"}}` → `Failed{Model}`
- 响应延迟超过 `per_probe_timeout` → `Failed{Timeout}`
- 连接拒绝 → `Failed{Network}`
- `base_url = http://169.254.169.254/latest/meta-data/` → `Failed{UrlBlocked}`（**不发请求**）
- `base_url = http://127.0.0.1:PORT` + `allow_loopback=false` → `Failed{UrlBlocked}`；`=true` → 正常探测

> **注意**：本机跑集成测试时必须 `.no_proxy()`——`docs/bug和优化清单.md` 记过这个坑（Clash 会把本地黑洞端口接走并回 502）。

**预估**：3–5 天（后端 2 天、前端 1 天、测试 1 天、真机验收 0.5 天）

---

### T2 Retry-After 遵从

**目标**：上游 429/503 返回 `Retry-After` 时，网关在切换下一个候选**之前**等待指定时间（有上限 + jitter），而不是立刻打下一个。

#### T2.1 新增纯函数（放 `router/mod.rs`，与 `classify_status` 同处）

```rust
pub const RETRY_AFTER_CAP_MS: u64 = 5_000;      // 单次等待上限
pub const RETRY_AFTER_JITTER_PCT: u32 = 20;     // ±20%
pub const MAX_TOTAL_BACKOFF_MS: u64 = 10_000;   // 整个 dispatch 的累计等待上限

/// 解析 Retry-After：支持 delta-seconds 与 HTTP-date（IMF-fixdate）。
/// 非法 / 负数 / 已过期 → None
pub fn parse_retry_after(value: &str, now_unix: i64) -> Option<u64>;  // → ms

/// 计算实际等待：retry_after 优先，否则用 attempt 次数的指数退避；加 jitter；封顶 CAP
pub fn backoff_delay_ms(retry_after_ms: Option<u64>, attempt: usize, jitter_pct: u32) -> u64;
```

**实现注意**：
- `delta-seconds` 是主路径（覆盖绝大多数上游）。HTTP-date 作为 fallback：**已确认 `httpdate 1.0.3` 就在我们的 `Cargo.lock` 里**（`:1791`，作为传递依赖被拉进来），所以只需在 `crates/gateway-core/Cargo.toml` 显式声明 `httpdate = "1"` 即可，**不会引入新的编译单元**。**不要手写日期解析**。
- jitter 用 `rand`（`crates/gateway-core/Cargo.toml:21` 已有 `rand.workspace = true`，工作区锁 `0.8`）：`delay * (100 ± jitter_pct) / 100`
- 超大值（如 `Retry-After: 3600`）必须被 `RETRY_AFTER_CAP_MS` 截断——否则一个坏上游能让客户端挂 1 小时

#### T2.2 修掉跨族丢头（bug）

`try_converted_candidate` @ `proxy.rs:2158-2164` 现在只取 body，**不收集错误响应头**。改为与直通路径一致，收集 `FORWARD_ERROR_HEADERS`（`proxy.rs:1059`）并放进 `UpstreamError.headers`。

#### T2.3 接线

- `Attempt::Failed` 变体增加字段 `retry_after_ms: Option<u64>`（`proxy.rs`，直通 `:1769-1774` 与转换 `:2158` 两处都要填）
- `dispatch` 主循环 @ `proxy.rs:1252`：在 `Failed` 且**还有下一个候选**时：
  ```rust
  if let Some(ms) = retry_after_ms {
      let delay = router::backoff_delay_ms(Some(ms), attempt_idx, RETRY_AFTER_JITTER_PCT);
      if total_backoff + delay <= MAX_TOTAL_BACKOFF_MS {
          tokio::time::sleep(Duration::from_millis(delay)).await;
          total_backoff += delay;
      }
  }
  ```
- **只对 `Retryable` 类失败等待**（429 / 5xx / 529）。`401/403`（认证错）与 `400`（请求错）不等——等了也没用。
- **流式已 commit 后不适用**（那时已经没候选了）

#### T2.4 测试

- `parse_retry_after` 矩阵：`"120"` / `"0"` / `"0.5"` / `"-1"` / `"abc"` / 合法 HTTP-date / 过去的 HTTP-date / `"999999"`（截断）
- `backoff_delay_ms`：无 retry_after 时的指数退避；jitter 在 ±20% 区间内（用固定 seed 或断言区间）
- 集成（`tests/m13_retry_after.rs`）：假上游 A 返回 `429 + Retry-After: 1`，假上游 B 返回 200 → 断言 (a) 请求落到 B；(b) A 与 B 的**接收时间差 ≥ 900ms**（考虑 jitter）
- 回归：`m2_failover.rs` / `m11_connect_retry.rs` / `m12_qualified_provider_fallback.rs` 全绿
- 保留现有测试 `m3_anthropic.rs:283 upstream_retry_after_is_forwarded_to_client`（**回传客户端的行为不能被破坏**——T2 是「同时用于内部调度」，不是「改为内部调度」）

**预估**：0.5–1 天

---

### T3 重试模型升级：候选分组 + 双档预算 + 跨组规则

**目标**：把「单轮遍历一遍」换成带预算与跨组语义的状态机；**同时把 `first_byte_verdict` 这块死代码落地**（首字节失败允许 failover）。

#### T3.1 关键兼容约束（先说清楚）

现有的 `AttemptVerdict::{Stop, Failover}` 里那个 `kind: &'static str`（`UpstreamAuth` / `RateLimit` / `Overloaded` / `ProviderOther` / `InvalidRequest` / `ContextTooLong`）**会落进 `request_logs.error_kind`**（`0001_initial_schema.sql:48-69`），前端日志页与统计依赖它。

所以引入 `FailureClass` 时，**必须提供 `.as_kind() -> &'static str` 保持字符串完全不变**，否则会破坏日志语义与既有测试。

#### T3.2 新增 `FailureClass`（`router/mod.rs`）

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    CallerTerminal,        // 400/422 → 不换候选，直接回客户端
    ChannelAuthTerminal,   // 401/403 → 组内换候选，不跨组
    EndpointUnsupported,   // 405/501 → 可跨组
    Retryable,             // 404/408/409/429/529/5xx → 可跨组
    UpstreamProtocolError, // 响应无法解析 → 可跨组
    CommittedStreamError,  // 已 commit 后断流 → 无法 failover
}

impl FailureClass {
    /// 保持与现有 error_kind 字符串完全一致（兼容 request_logs 与前端）
    pub fn as_kind(self) -> &'static str;
    /// 是否允许跨组（从同族组跳到转换组）
    pub fn degradable(self) -> bool;
    /// 是否值得等待 Retry-After
    pub fn retryable_with_backoff(self) -> bool;
}

/// 由现有 AttemptVerdict + 状态码派生（保留 classify_status 不变，新增包装）
pub fn classify_failure(status: u16, body_excerpt: &str) -> FailureClass;
```

映射（在现有 `classify_status` 基础上收敛，**`Stop`/`Failover` 的判定边界不变**）：

| 状态码 | FailureClass | `as_kind()`（不变） |
| --- | --- | --- |
| 401 / 403 | `ChannelAuthTerminal` | `UpstreamAuth` |
| 404 / 408 | `Retryable` | `ProviderOther` |
| 429 | `Retryable` | `RateLimit` |
| 529 / 5xx | `Retryable` | `Overloaded` |
| 400 含 context_length | `CallerTerminal` | `ContextTooLong` |
| 400 其它 / 4xx 其它 | `CallerTerminal` | `InvalidRequest` |
| 405 / 501 | `EndpointUnsupported` | `ProviderOther` |
| 2xx | （不适用） | — |

#### T3.3 显式分组（复用已存在的隐式排序）

`proxy.rs:1211` 已经有 `candidates.sort_by_key(|c| c.family != wire.family())`。把它显式化为：

```rust
pub enum GroupTier { Native, Conversion }

pub struct CandidateGroups {
    pub native: Vec<StoreRouteCandidate>,
    pub conversion: Vec<StoreRouteCandidate>,
}

pub fn group_candidates(cands: Vec<StoreRouteCandidate>, inbound_family: &str) -> CandidateGroups;
```

**行为不变保证**：`Native` 组内的顺序 = 现在 `order_candidates` + family 排序的结果；`Conversion` 同理。即**重构后不改变现状行为**（除预算限制生效外）。这是可回归验证的。

#### T3.4 双档预算与状态机

```rust
pub struct RetryBudget {
    pub per_group: usize,
    pub total: usize,
}
pub const DEFAULT_MAX_ATTEMPTS_PER_GROUP: usize = 3;
pub const DEFAULT_MAX_ATTEMPTS_TOTAL: usize = 6;

/// 从 meta 读；缺失用默认值；设为 (1,1) 等价于「关闭重试」= 现状
pub fn retry_budget_from_meta(get: impl Fn(&str) -> Option<String>) -> RetryBudget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowStep { NextInGroup, NextGroup, Stop }

pub struct AttemptFlow { /* group, per_group_used, total_used, budget, non_idempotent */ }

impl AttemptFlow {
    pub fn new(budget: RetryBudget, non_idempotent: bool) -> Self;
    /// 一次尝试失败后决定下一步
    pub fn next(&mut self, class: FailureClass) -> FlowStep;
}
```

`next()` 的规则（**这就是要写进真值表测试的东西**）：

| FailureClass | 组内未超预算 | 组内已超预算 |
| --- | --- | --- |
| `CallerTerminal` | `Stop` | `Stop` |
| `ChannelAuthTerminal` | `NextInGroup` | **`Stop`（不跨组）** |
| `EndpointUnsupported` | `NextInGroup` | `NextGroup`（若 total 未超） |
| `Retryable` | `NextInGroup` | `NextGroup`（若 total 未超） |
| `UpstreamProtocolError` | `NextInGroup` | `NextGroup`（若 total 未超） |
| `CommittedStreamError` | `Stop` | `Stop` |

`total` 用尽时一律 `Stop`。

**非幂等请求不放宽**：`non_idempotent` 为真时，budget 强制为 `(1, 1)`。判定：

```rust
/// Responses 协议带 background=true 或 store=true（会产生服务端持久化副作用）
/// 以及 stream=true 且已 commit 的情形 → 不重试
pub fn is_non_idempotent(wire: &InboundWire, body: &[u8]) -> bool;
```

#### T3.5 落地 `first_byte_verdict`（补上从未实现的语义）

现在 `streaming_response`（`proxy.rs:3355`）首字节失败三分支（`:3370` / `:3395` / `:3420`）全部返回错误响应，被包成 `Attempt::Delivered` @ `1814`。

改为：**首字节阶段失败（此时还没有任何字节下发给客户端）→ 返回 `Attempt::Failed{class: Retryable, ..}`**，允许进入下一个候选。

依据：`first_byte_verdict`（`router/mod.rs:103-111`）的语义就是「首字节失败是 failover 候选」——它写了但没接线。落地后**删掉死代码**，或让它成为 `classify_failure` 的一个入口。

**边界**：只有在 `tx.send(Ok(first_chunk))` @ `proxy.rs:3463` **之前**失败才可 failover；之后一律 `CommittedStreamError` → 断流 + 落日志（现状行为，不改）。

#### T3.6 接线点

- `dispatch` @ `proxy.rs:1114`：把 `for cand in &candidates` 改成 `AttemptFlow` 驱动的循环
- `try_candidate` / `try_converted_candidate` 的失败分支：从返回 `AttemptVerdict` 改为返回 `(FailureClass, UpstreamError)`
- 保留 `classify_status` 与 `AttemptVerdict` 的**公开签名**（其它调用点与测试还在用），新增 `classify_failure` 作为上层包装

#### T3.7 测试

单测（`router/mod.rs` 内联）：
- `AttemptFlow::next` **真值表**（上表 6 行 × 3 种预算状态）
- `as_kind()` 与现有字符串逐一对照（**防止日志语义漂移**）
- `degradable()` / `retryable_with_backoff()` 分类矩阵
- `is_non_idempotent`：Responses + `background:true` → true；+ `store:true` → true；普通 chat → false

集成测试（`tests/m13_retry_model.rs`）：
- **401 不跨组**：同族候选全部 401 → **不尝试**转换族候选（断言转换族上游零请求）
- **429 跨组**：同族候选全部 429 → 尝试转换族候选
- **`EndpointUnsupported` 跨组**：同族候选全 405 → 尝试转换族
- **组内预算**：3 个同族候选全 500 → 恰好尝试 3 次后跳到转换组（不是全部遍历）
- **总预算**：8 个候选全 500 → 总尝试次数 = 6
- **非幂等**：Responses + `store:true` + 上游 500 → **只尝试 1 次**
- **首字节 failover**：候选 A 建连后立刻断流（首字节前）→ 落到候选 B 并成功
- **回归**：`m2_failover.rs` / `m3_anthropic.rs` / `m11_connect_retry.rs` / `m12_qualified_provider_fallback.rs` 全绿

**预估**：2–3 天


---

### T4 单实例锁

**目标**：用户双击图标两次（或从 Finder/Dock 再点一次）时，不再起第二个网关进程，而是把已有窗口置前。

#### T4.1 方案选择

现状（§2.3）：启动路径**零单实例保护**，且依赖里没有任何文件锁 crate。

| 方案 | 评价 |
| --- | --- |
| **A. `tauri-plugin-single-instance`（官方插件）** | **推荐。** 跨平台、行为正确（第二个实例触发第一个实例的回调 → 可以 focus 而非 exit）、`tauri.conf.json` 已有 `plugins` 段可放配置 |
| B. 锁文件 + `fs2` / `fd-lock` | 需新依赖；要处理陈旧锁（进程被 kill 后锁残留）；收益不比 A 大 |
| C. 固定端口互斥（`TcpListener::bind`） | 零依赖，但语义粗糙（端口被别的程序占了就误判），且无法传递「请置前」的信号 |

选 **A**。

#### T4.2 实施

1. `src-tauri/Cargo.toml` 加 `tauri-plugin-single-instance = "2"`（`[dependencies]`，与其它 `tauri-plugin-*` 同段，`src-tauri/Cargo.toml:14-19`）
2. `main.rs` 的插件注册链（`:3229-3232`）**首位**插入：
   ```rust
   .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
       // 第二个实例启动 → 走到这里 → 聚焦已有窗口，然后第二个实例自行退出
       show_main_window(app);      // 见下方第 3 点：从托盘 "show" 分支抽出
   }))
   ```
   **注意**：托盘菜单的 `"show"` 分支（`main.rs:3397-3402`）现在是**内联**的 `app.get_webview_window("main")` + `show()` + `set_focus()`，**并没有**一个现成的 `restore_main_window` 函数。所以先把这个三分支逻辑抽成一个 `fn show_main_window(app: &AppHandle)`，托盘与单实例回调共用它（**不要复制粘贴两份**）。
3. **时序正确性（必须验证）**：插件必须在**任何会碰数据目录的代码之前**生效。插件的单实例判定发生在 Tauri runtime 初始化阶段，**早于 `setup()`**（`:3241`），因此第二个实例不会执行 `Db::open`（`:3267`）。**这一点要在真机上确认**——如果发现第二个实例仍然打开了 DB，就说明判定时机晚于 `setup`，需要改成在 `setup` 开头手动调 `app.try_state::<...>()` 检查（回退方案）。
4. 第二实例的退出行为由插件负责（它会 `std::process::exit`）。**不要**在回调里做任何写库操作。

#### T4.3 边界与不做的事

- **不要**把单实例保护加到 `gateway-core`：它是库，桌面壳才有「单实例」概念。将来若做 headless（P1-1），headless 用锁文件而非这个插件——那时再把 `InstanceGuard` 抽象出来。
- **`port_in_use`（`main.rs:3057`）保持原样**，它服务于设置页的端口冲突提示，与单实例无关。
- **`spawn_supervisor` 的进程内幂等**（`:3104` 的 `is_some()` 检查）**保留**——它防的是「同一进程内重复点启动」，与跨进程单实例是两层防护。

#### T4.4 验收（必须真机，无法单测）

- macOS：Finder 双击两次 → 只有一个窗口、只有一个网关监听（`lsof -i :1314` 单条）、第二次双击把窗口置前
- Windows：开始菜单连续启动两次 → 同上
- 已有窗口最小化到托盘后再次启动 → 窗口恢复显示（不是新开一个）
- 启动第二个实例后，第一个实例的 `request_logs` **没有**出现重复/异常写入
- 关闭第一个实例（托盘 → 退出）后再启动 → 能正常启动（锁正确释放）

**预估**：0.5 天（含双平台真机）

---

### T5 迁移前自动备份 DB

**目标**：应用任何迁移之前，先把当前 DB 备份一份；迁移失败时用户能看到「已备份到 X」而不是一个闪退的图标。

#### T5.1 关键事实澄清

- `store/snapshot.rs` **不是** DB 备份（§2.3），别改它。
- `BACKUP_KEEP = 10` + `backup_evict_candidates(names, keep)`（`sync.rs:590-606`）**已存在但生产代码从未调用**，且作用于 **WebDAV 远端**备份。本任务**复用这个纯函数**做本地备份的滚动清理（避免重写），但**不改它的语义**。
- 迁移执行器 `migrate(conn)`（`store/mod.rs:47-65`）**不知道 DB 路径**——必须改签名。

#### T5.2 备份实现

用 SQLite 的 `VACUUM INTO`（rusqlite bundled 0.32 的 SQLite 版本支持），**不要用 `fs::copy`**——WAL 模式下直接拷文件可能漏掉未 checkpoint 的数据。

```rust
// store/mod.rs
pub const MIGRATION_BACKUP_KEEP: usize = 3;
const BACKUP_DIR: &str = "backups";

/// 返回本次是否产生备份及路径
pub fn migrate(conn: &Connection, db_path: Option<&std::path::Path>)
    -> Result<Option<std::path::PathBuf>, StoreError>;

/// 备份当前 DB 到 <data_dir>/backups/jai.db.<unix_ms>.bak
/// 仅当「已存在的库且有迁移待应用」时执行
fn backup_before_migrate(conn: &Connection, db_path: &std::path::Path) -> Result<std::path::PathBuf, StoreError>;
```

`backup_before_migrate` 的判定（**三条都要满足才备份**）：

1. `db_path` 非 `:memory:` 且文件已存在
2. `PRAGMA user_version > 0`（**新库不备份**——没有数据可丢，备份只会产生垃圾文件）
3. `user_version < MIGRATIONS.len()`（**没有待应用的迁移就不备份**——避免每次启动都产生一个备份）

执行：
```sql
VACUUM INTO '<data_dir>/backups/jai.db.<unix_ms>.bak'
```
（`VACUUM INTO` 要求目标文件不存在；`backups/` 目录需先 `create_dir_all`）

清理：列出 `backups/` 下匹配 `jai.db.*.bak` 的文件名 → 喂给 `sync::backup_evict_candidates(names, MIGRATION_BACKUP_KEEP)` → 删除返回的名字。

#### T5.3 失败处理（**决策点 D4**）

推荐策略：

| 场景 | 行为 |
| --- | --- |
| 备份失败（磁盘满 / 权限） | **继续启动**，但 `eprintln!` 醒目告警 + 记入 meta（`last_migration_backup_error`），下次启动在 UI 上提示 |
| 备份成功 + 迁移成功 | 正常启动，`eprintln!` 记一行「已备份到 X」 |
| 备份成功 + 迁移失败 | **中止启动**，并通过系统通知（`tauri-plugin-notification` 已装，`main.rs:3231`）提示「数据库迁移失败，已备份到 X，请回滚或反馈」 |
| 备份失败 + 迁移失败 | 中止启动 + 通知提示「迁移失败且备份失败，请手动备份 <data_dir>」 |

> 现在迁移失败走 `main.rs:3510` 的 `.expect(...)` panic，**用户什么都看不到**。至少要加通知。这是本任务的隐性一半价值。

#### T5.4 测试

`store/mod.rs` 内联或 `tests/m13_migration_backup.rs`：
- 内存库（`":memory:"`）→ 不备份
- 全新文件库（`user_version = 0`）→ 不备份，但 12 条迁移全部应用
- 已存在且 `user_version = 12`（= `MIGRATIONS.len()`）→ 不备份（无待应用迁移）
- 已存在且 `user_version = 11` → 产生备份，且备份里的 `user_version` **= 11**（迁移前状态）、不含第 12 条迁移的表结构变更
- 造 5 个备份文件 → 清理后剩 3 个**最新**的
- 备份路径不可写（把 `backups/` 设为只读或指向非法路径）→ 返回错误但不 panic

**预估**：1 天

---

### T6 网关密钥：多密钥 + 白/黑名单（配额已砍）

> **建议拆成两个迭代。** 现状有一个硬前置缺失：**「请求用了哪个密钥」在鉴权中间件里被丢弃**（§2.4），所以白名单没有地方可挂；而「密钥」当前实质只有一把（`gw_key_active` 只取最新一条），单密钥谈「分级权限」没有意义。先做 T6a 打通地基，再做 T6b。

#### T6a 多密钥地基（独立迭代，约 2 天）

**T6a.1 表结构**：现有 `gateway_keys` 七列（`0001_initial_schema.sql:38-46`）**已够用**，不需要迁移。`label` 字段承担「发给谁」的备注。

**T6a.2 store 层**（`store/mod.rs`）：

```rust
/// 新增：创建一把新密钥，不吊销旧的
pub fn gw_key_create(c: &Connection, key: &str, label: Option<&str>) -> Result<GatewayKeyRow, StoreError>;

/// 新增：列出全部未吊销密钥
pub fn gw_keys_active(c: &Connection) -> Result<Vec<GatewayKeyRow>, StoreError>;

/// 新增：按 id 吊销
pub fn gw_key_revoke(c: &Connection, id: &str) -> Result<(), StoreError>;

/// 保留：轮换 = 创建新 + 吊销旧的（现有语义，main.rs:796 在用）
pub fn gw_key_rotate(c: &Connection, key: &str, label: Option<&str>) -> Result<GatewayKeyRow, StoreError>;

/// 改语义：从「取最新一条」改为「取全部未吊销」（鉴权要遍历比对）
pub fn gw_key_active(c: &Connection) -> Result<Option<GatewayKeyRow>, StoreError>;  // 保留给旧调用点
```

**T6a.3 鉴权改造**（`security.rs:125-166`）：

现在是「取唯一活跃 key → `ct_eq` 一次」。改为「取全部活跃 key → 逐个 `ct_eq`，命中即返回」：

```rust
let keys = spawn_blocking(...store::gw_keys_active...).await??;
for row in &keys {
    if ct_eq(&row.key, token) && ct_eq(&row.id, &row.id) { /* 命中 */ }
}
```

**注意安全语义**：遍历时**不要短路优化**（不要「先比 prefix 再比全文」）——prefix 是 14 字符明文，用它做快速筛选会让「哪个 prefix 存在」可通过时序区分。密钥数量是个位数，全量遍历成本可忽略。保持 `ct_eq` 的常量时间比较语义（`security.rs:20`）。

**T6a.4 新 IPC**：`gateway_key_create` / `gateway_key_list` / `gateway_key_revoke`（`main.rs`，注册在 `:3455-3457` 附近）

**T6a.5 前端 `GatewayPage.tsx`**：从「单卡片」改为「密钥列表 + 新建按钮」
- 每行：`prefix…` + label + 创建时间 + 最后使用 + 「显示全文」「复制」「吊销」
- 保留现有「轮换」（`confirmRotate`，`:293`）语义与脱敏规则（`:271-277`）
- 吊销需二次确认（复用现有 `ConfirmDialog`）

**T6a.6 测试**：
- 创建 3 把 → `gw_keys_active` 返回 3；旧密钥仍能鉴权通过
- 吊销 1 把 → 该密钥鉴权失败，其余 2 把正常
- 轮换 → 新的可用、旧的立即失效
- 全吊销 → 鉴权一律失败（**但桌面端 `ensure_gateway_key` 会在启动时自举一把，注意别把自举逻辑搞乱**，`main.rs:737-746`）

#### T6b 白/黑名单（独立迭代，约 1.5–2.5 天）

**T6b.1 把 key id 注入请求上下文（必做前置）**

`security_mw` @ `proxy.rs:260` 现在把 `AuthedKey` 丢掉：
```rust
Ok(_key) => next.run(req).await,   // ← 丢弃
```
改为：
```rust
Ok(key) => { req.extensions_mut().insert(key); next.run(req).await }
```
`AuthedKey`（`security.rs:116-118`）需 `#[derive(Clone)]`（已有 `Send + Sync + 'static` 隐含）。`dispatch` @ `proxy.rs:1114` 从 `req.extensions().get::<AuthedKey>()` 取 id。

**T6b.2 表结构（迁移 0013）**

```sql
-- 0013
CREATE TABLE key_provider_rules (
  key_id      TEXT NOT NULL,
  provider_id TEXT NOT NULL,
  mode        TEXT NOT NULL CHECK (mode IN ('allow','deny')),
  PRIMARY KEY (key_id, provider_id)
);
CREATE TABLE key_model_rules (
  key_id     TEXT NOT NULL,
  model_name TEXT NOT NULL,
  mode       TEXT NOT NULL CHECK (mode IN ('allow','deny')),
  PRIMARY KEY (key_id, model_name)
);


-- 可选（见下方说明）：给请求日志加密钥归属，便于排查「这条为什么被拒」
ALTER TABLE request_logs ADD COLUMN key_id TEXT;
CREATE INDEX idx_logs_key_ts ON request_logs(key_id, ts DESC);
```

> **关于 `request_logs.key_id`（可选，建议做）**：原本它是为配额聚合准备的。配额砍掉后，它的用途变成**可观测性**——出问题时能回答「这条请求用的是哪把密钥、被谁的规则拒了」。成本只有 1 个 `ALTER` + 1 个索引，且和 0013 同一个迁移文件，几乎免费。**建议做；若想再省，可以砍。**
>
> **已砍**：`gateway_keys.quota_limit` / `quota_used` 两列（D5 决策，配额不做）。

**T6b.3 规则语义**（照 WaLiAPI 的收敛口径，简单可解释）

- `deny` 命中 → 该渠道/模型**不可用**
- `allow` 列表**非空** → 只允许列表内的
- 两者都为空 → **不限制**（向后兼容：现有密钥行为完全不变）
- `deny` 优先于 `allow`

**T6b.4 过滤点**（`proxy.rs`）

在 `dispatch` 拿到 candidates 后、`router::order_candidates`（`proxy.rs:1208`）**之前**做 `retain`：

```rust
let rules = ctx.key_rules.get(&ctx.db, &key_id).await;   // 5s TTL 缓存
candidates.retain(|c| rules.allows_provider(&c.id));
```

**过滤后 candidates 为空** → 返回 **403**（不是 404 `model_not_found`），错误码 `model_not_allowed`，走 `InboundWire::error_response`（`proxy.rs:147-167`）以保持三方言一致。

**热路径缓存**：照抄 `CorsAllowlist`（`security.rs:226-282`）的模式：
```rust
pub struct KeyRulesCache { cache: Arc<Mutex<HashMap<String, (i64, KeyRules)>>>, ... }
```
5s TTL，避免每个请求都查库。

**`/v1/models` 也要过滤**（`models_list` handler）——否则客户端会看到自己用不了的模型，体验更差。

**T6b.5 配额 —— 已砍（D5）**

不做配额，因此：
- 不加 `gateway_keys.quota_limit` / `quota_used`
- `dispatch` 入口**不加**任何用量校验
- `logs.rs` 批写**不加** `quota_used` 累加（保持现在的纯插入路径，不动事务边界）
- 不做 `gateway_key_quota_reset` 命令

保留记录：若将来要做，§3 T6b 的原始设计（累计 token 封顶 + 手动重置 + 周期重置延后）仍在 `docs/design/WaLiAPI-对标研究报告.md` §2.4 的对照里可回溯。

**T6b.6 测试**
- `KeyRules` 语义矩阵：deny 命中 / allow 非空且命中 / allow 非空且未命中 / 都为空
- 集成：给 key 配 `deny` 某 provider → 该 provider 的上游**零请求**；candidates 全被过滤 → 403 `model_not_allowed`
- `/v1/models` 按 key 过滤
- **向后兼容**：无规则的密钥行为与改造前**完全一致**（用现有测试回归）

**预估**：T6a 2 天；T6b 1.5–2.5 天（配额砍掉后约省 1–1.5 天）

---

## 4. 建议的施工顺序

```
Day 1        T2 Retry-After  +  T4 单实例锁          （两个小改动，可并行/同日完成）
Day 2        T5 迁移前备份
Day 3–5      T1 渠道草稿测试（核心，独占）
Day 6–8      T3 重试模型升级
Day 9–10     T6a 多密钥地基
Day 11–13    T6b 白/黑名单
```

理由：
- **T2 + T4 先做**：都是「改完当天能验证」的小活，先拿到确定性收益，也为后面的重构建立回归基线。
- **T5 在 T6 之前**：T6 要加 3 个迁移，正好让「迁移前自动备份」在实践中被真实验证一次——**自己先吃自己的狗粮**。
- **T1 独占**：它是本迭代唯一的新功能，前端 + 后端 + SSRF 校验三块，不适合与别的任务并行改同一批文件。
- **T3 在 T1 之后**：两者都动 `proxy.rs`，串行避免冲突。
- **T6 最后**：有前置缺失，且需要产品决策（见 §6）。

---

## 5. 验收标准

### 5.1 每项任务的硬性标准

| 任务 | 验收 |
| --- | --- |
| T1 | 四族渠道各能正确探测；**200 + HTML 拦截页必须判失败**；云元数据地址被拦且不发请求；`provider_test`/`provider_test_draft` 行为不变；新增测试全绿 |
| T2 | `Retry-After` 被解析并用于内部退避（集成测试断言时间差）；**回传客户端的行为不变**（`m3_anthropic.rs:283` 仍绿）；跨族路径不再丢头 |
| T3 | `AttemptFlow` 真值表全绿；**401 不跨组**（断言转换族零请求）；`as_kind()` 字符串与改造前逐字一致；`m2`/`m3`/`m11`/`m12` 全绿；首字节 failover 生效 |
| T4 | 双平台真机：双击两次只有一个窗口一个网关；第二实例不写 DB；退出后能重启 |
| T5 | 新库不备份、无待应用迁移不备份、有待应用迁移必备份且备份是迁移前状态；保留 3 份；迁移失败有可见通知 |
| T6a | 多密钥可创建/列出/吊销；全部密钥都能鉴权；轮换语义不变 |
| T6b | 规则语义矩阵全绿；被过滤时返回 403 而非 404；`/v1/models` 同步过滤；无规则密钥行为不变 |

### 5.2 全局门禁（每项任务收尾都要跑）

```bash
bash scripts/regression.sh     # cargo fmt --check + clippy -D warnings + cargo test --workspace + 前端 tsc && vite build
bash scripts/ui_lint.sh        # T1/T6a/T6b 动了 UI，必须过
node tools/visual-regression/gate.mjs   # 需 vite:5173；T1/T6a/T6b 动了 UI，必须过
```

**特别提醒（来自我们自己的教训，`docs/bug和优化清单.md`）**：
- 新增的 UI 判据必须**跑负控制**（故意改坏 → 门禁必须变红），否则是「一直绿的假断言」
- 探针跑前要 `rmSync` 旧 JSON，否则会读到上次结果报假绿
- 新断言不要插进恒不执行的 `if` 块

---

## 6. 决策记录与风险

### 6.1 已拍板决策（2026-09-22）

| # | 决策 | 结论 |
| --- | --- | --- |
| **D1** | 草稿测试的探测结果是否强制 | ✅ **不强制**。`require_probe_pass` 默认 `false`，探测结果只在 UI 展示，不拦截保存。T1.4 的 receipt 机制**照做**（保留开关能力），只是默认关 |
| **D2** | SSRF 校验的接线范围 | ✅ **探测 + `provider_create` / `provider_update` 保存时校验**；**不做热路径**（`try_candidate` 不加校验） |
| **D3** | 是否允许访问 loopback | ✅ **允许，但需用户显式勾选**（`allow_loopback` 默认 `false`）。用于 Ollama / LM Studio / vLLM 等本机部署；**link-local 与云元数据地址在任何情况下都不放行** |
| **D4** | 备份失败时是否阻止启动 | ✅ **不阻止**。备份失败 → `eprintln!` 告警 + 记入 meta `last_migration_backup_error`，继续启动（迁移本身仍有事务保护） |
| **D5** | 配额 | ❌ **不做**。整个配额能力（`quota_limit` / `quota_used` / 429 校验 / 手动重置 / 周期重置）**全部砍掉**，见 §3 T6b.5 与 §7 |

**因 D5 产生的连带调整**（已在正文同步）：
- 迁移 **0014 取消**，只保留 `0013_key_rules.sql`
- `request_logs.key_id` 从「配额聚合必需」降级为「**可观测性可选**」（仍建议做，与 0013 同文件）
- `logs.rs` 的批写路径**不动**（不加 `quota_used` 累加、不改事务边界）
- T6 总量从 4–6 天降到 **3.5–4.5 天**；T6b 从 2–4 天降到 **1.5–2.5 天**


| 风险 | 应对 |
| --- | --- |
| T3 动了 `proxy.rs` 主循环，可能破坏 dsh/zcode 真机链路 | 严格保持 `as_kind()` 字符串不变；`m2/m3/m11/m12` 作为回归闸门；改完在 dsh 上跑一次真机（`scripts/setup_dsh.mjs` 流程） |
| T2 引入 sleep 会让客户端等待变长 | `RETRY_AFTER_CAP_MS = 5s` + `MAX_TOTAL_BACKOFF_MS = 10s` 双重封顶；只对 `Retryable` 等待 |
| T1 的探测请求会消耗用户额度 | `max*_tokens = 1`、非流式、单次；报告里带 `cost_possible` 字段并在 UI 上提示「探测会产生极小量计费」 |
| T1 的 SSRF 校验误伤合法渠道 | `is_blocked_ip` 写全地址族单测；`allow_loopback` 开关兜底；校验失败只影响探测与保存，不影响已有渠道的转发 |
| T6b 的规则过滤可能让用户「自己把自己锁死」 | 必须至少保证「被过滤时返回 403 且错误信息说明是密钥规则导致」；UI 上规则配置要有「当前密钥可访问的模型」预览 |
| `tauri-plugin-single-instance` 与现有 4 个插件的注册顺序冲突 | 放在链首；真机验证；若异常则回退到锁文件方案 |

---

## 7. 非目标（另立迭代）

明确**不做**，避免范围蔓延：

- ❌ **`native_endpoints` 数据模型**（provider 声明多个可用端点 + 端点勾选 UI）——T1 用 family 派生端点即可，加数据模型是独立迭代
- ❌ **从 curl 导入渠道**（对标报告的 P0-2）——与 T1 共用 SSRF 校验器，建议紧接 T1 之后单独做
- ❌ **本机 AI 工具配置扫描导入**（P0-3）——独立迭代
- ❌ **Auth 账号体系 / OAuth 登录**（P1-5）——需先做产品决策
- ❌ **headless / Docker 交付形态**（P1-1）——最大的一项，必须单独立项
- ❌ **Responses 断线续传**（P1-2）——独立迭代
- ❌ **安全扫描 / DLP**（P1-3）——独立迭代
- ❌ **流式探测与工具调用探测**（T1 只做非流式最小推理探测；这两项需要更复杂的假上游与断言，建议 T1 落地后作为 T1.1 追加）
- ❌ **网关密钥配额**（D5 决策，2026-09-22）——`quota_limit` / `quota_used` / 429 用量校验 / 手动重置 / 周期重置**全部不做**。连带取消迁移 `0014`，`logs.rs` 批写路径不动。若将来重启此议题，原始设计（累计 token 封顶 + 手动重置 + 周期重置延后）可从 `docs/design/WaLiAPI-对标研究报告.md` §2.4 与本次 git 历史回溯
- ❌ **`ui/src/types.ts` 的 `Family` 漏 `openai_responses`**（§2.5 bug 1）——一行修复，**建议随手带上**，但不算任务

---

## 8. 交付物清单（预估）

### 新增文件

| 文件 | 内容 |
| --- | --- |
| `crates/gateway-core/src/probe.rs` | T1 探测核心（类型 + payload 派生 + 分类 + fingerprint + receipt） |
| `crates/gateway-core/src/netguard.rs` | T1 SSRF 校验（`validate_outbound_url` / `is_blocked_ip`） |
| `crates/gateway-core/tests/m13_probe.rs` | T1 集成测试 |
| `crates/gateway-core/tests/m13_retry_after.rs` | T2 集成测试 |
| `crates/gateway-core/tests/m13_retry_model.rs` | T3 集成测试 |
| `crates/gateway-core/tests/m13_migration_backup.rs` | T5 集成测试 |
| `crates/gateway-core/src/store/migrations/0013_key_rules.sql` | T6b |
| `ui/src/components/ProbePanel.tsx` | T1 前端探测结果面板 |

### 修改文件

| 文件 | 涉及任务 |
| --- | --- |
| `crates/gateway-core/src/router/mod.rs` | T2（`parse_retry_after` / `backoff_delay_ms`）、T3（`FailureClass` / `AttemptFlow` / `GroupTier`） |
| `crates/gateway-core/src/server/proxy.rs` | T2（跨族收头 + sleep）、T3（主循环改造 + 首字节 failover）、T6b（key id 注入 + 候选过滤） |
| `crates/gateway-core/src/server/security.rs` | T6a（多密钥鉴权遍历）、T6b（`AuthedKey` 注入） |
| `crates/gateway-core/src/store/mod.rs` | T5（`migrate` 签名 + 备份）、T6a（`gw_key_*` 新函数）、T6b（`key_rules` 查询） |
| `crates/gateway-core/src/store/logs.rs` | T6b（可选：`LogEvent.key_id`，仅落库不做累加） |
| `crates/gateway-core/src/discover.rs` | T1（复用其鉴权头拼法，可能抽出公共函数） |
| `src-tauri/src/main.rs` | 全部任务（新 IPC、插件注册、通知、命令注册表） |
| `crates/gateway-core/Cargo.toml` | T2（显式声明 `httpdate`） |
| `src-tauri/Cargo.toml` | T4（`tauri-plugin-single-instance`） |
| `ui/src/pages/ProvidersPage.tsx` | T1（探测按钮 + 面板 + allow_loopback 复选框） |
| `ui/src/pages/GatewayPage.tsx` | T6a（密钥列表）、T6b（规则 UI） |
| `ui/src/api.ts` / `ui/src/types.ts` | 全部前端任务（含 §2.5 的 `Family` 一行修复） |
| `CHANGELOG.md` | 全部 |

### 迁移编号

当前 `MIGRATIONS` 共 **12 条**（0001–0012）。本迭代新增：
- `0013_key_rules.sql`（T6b）

**T1–T5 不需要迁移。** 如果 T6 拆分，T6a 也不需要（复用现有七列）。

---

## 附：开工前的第一件事

建议按这个顺序做，能最快暴露风险：

1. **先跑一遍现有门禁**，确认基线是绿的：`bash scripts/regression.sh`
2. **从 T2 开始**（最小改动 + 有明确集成测试），验证「改 `proxy.rs` → 跑回归 → 全绿」这条链路是通的
3. **决策已拍板**（D1–D4 按建议、D5 配额不做），可直接开工，无需再确认
4. 若时间紧，**优先级：T2 > T4 > T5 > T1 > T3 > T6**（前三个是「当天可见收益」，T1 是核心新功能，T3 是重构，T6 可延后）
