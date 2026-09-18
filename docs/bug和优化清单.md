# Bug / 优化清单

> 本文件是 JAI 唯一问题追踪入口。以后所有 bug 和优化都放这里。
> 完成一项就把 `[ ]` 改成 `[x]`，并尽量补一行“解决版本 / 说明 / 日期”。

## 1. Bug 清单

- [x] 1. 创建供应商报错：系统密钥环操作失败（检查本机凭据设置）: keyring: Platform secure storage failure: UNIX[Operation not permitted]
  - 已解决：密钥环不可用时自动降级为数据目录下 `vault_fallback.json`（Unix 0600），设置页可见“文件降级”状态；真实系统仍优先钥匙串。

- [x] 2. dsh 测试返回 Cloudflare 400 Bad Request
  - 现象：
    ```
    OpenAI API error (400): {"code":null,"message":"<html>... <title>400 Bad Request</title> ... cloudflare ...","type":"invalid_request_error"}
    ```
  - 根因：dsh 走 OpenAI Responses API（`/v1/responses`），one-model 也是 Responses 上游；但 JAI 把该渠道按 `openai_compat` 转成 `/chat/completions`，被上游 Cloudflare 拒 400。
  - 已解决：
    1. 新增 `openai_responses` 协议族 + 迁移 0003，Responses 入站遇到该协议族直接字节透传到 `/responses`；
    2. 顺带修复直通转发丢失 `Content-Type`/`Accept`/`User-Agent` 等头的问题；
    3. ✅ 已用 `dsh --profile headless "直接回复：JAI链路正常"` 实测通过，返回 `JAI链路正常`，退出码 0。

- [x] 3. dsh 多供应商后 400（ALB / 模型不可用）
  - 现象：
    ```
    OpenAI API error (400): <html>...<center>alb</center>...
    OpenAI API error (400): {"code":"MODEL_NOT_AVAILABLE","message":"模型不可用：基元律动/glm-5",...}
    ```
  - 根因：
    1. 跨族转换路径给上游发了两个 `Authorization` 头（一个空占位、一个真实 Key），被 ALB 拒 400；
    2. 限定模型名 `供应商/模型` 被原样转发给上游，上游不认 `基元律动/glm-5`。
  - 已解决：
    1. 转换路径去掉空占位鉴权头，只发一个真实 Authorization；
    2. 路由前把 `供应商/模型` 重写为真实模型名 `glm-5` 再转发；
    3. Responses 入站优先同协议族直通，避免被跨族渠道截胡。
  - ✅ 已用 `dsh --profile headless` 实测退出码 0。

- [x] 4. dsh Responses 请求报「messages 低于允许下限 / messages 参数非法」（400）
  - 现象：
    ```
    OpenAI API error (400): {"code":null,"message":"{\"code\":\"BAD_REQUEST\",\"message\":\"messages 低于允许下限\",...}"}
    OpenAI API error (400): {"code":null,"message":"{\"code\":\"LITELLM_ERROR\",\"message\":\"messages 参数非法。请检查文档。\",...}"}
    ```
  - 根因：Responses 解码器对 `input` 数组项要求显式顶层 `"type":"message"`；而 dsh（及 OpenAI 官方格式）
    省略该字段（直接 `{"role":"user","content":[...]}`），全部落入 `_` 分支被丢弃 → IR messages 为空 →
    转换后只有 system 消息，上游校验消息条数失败 400。
  - 已解决：input 数组项 `type` 缺失但含 `role` 字段时按 message 解析；`function_call(_output)` 仍显式按原名解析。
    新增单测 `decode_input_items_without_type_field` 防回归。
  - ✅ 已用 curl 复现 dsh 场景（限定模型名 + input 数组 + tools + 流式/非流式）实测通过，日志 200。

- [x] 5. dsh + MCP 工具循环 400：assistant tool_calls 后缺 tool 消息 / thinking 模型 reasoning_content 未回传
  - 现象：
    ```
    OpenAI API error (400): {"code":null,"message":"{\"code\":\"LITELLM_ERROR\",\"message\":\"An assistant message with 'tool_calls' must be followed by tool messages responding to each 'tool_call_id'. (insufficient tool messages following tool_calls message)\",...}"}
    OpenAI API error (400): {"code":null,"message":"{\"code\":\"LITELLM_ERROR\",\"message\":\"The `reasoning_content` in the thinking mode must be passed back to the API.\",...}"}
    ```
  - 根因（三个独立问题）：
    1. MCP 自动循环中，上游返回**混合工具调用**（MCP 工具 + 客户端工具）时旧代码立即停止循环，
       把 MCP 工具也抛给客户端（dsh 不认识、不执行）→ 下轮回传历史缺该工具结果 → 上游 400。
    2. thinking 模型（deepseek-v4-flash 等）上游返回的 `reasoning_content` 被 openai 解码器丢弃；
       MCP 循环第二轮回填 assistant 消息时不带 reasoning → 上游要求必须回传 → 400。
    3. Responses 入站解码不支持 assistant 消息**内嵌** `function_call` / `reasoning` 顶层字段
       （推理模型历史真实格式），被当普通消息忽略 → 转换后结构残缺 → 400。
  - 已解决：
    1. MCP 循环拆分工具：MCP 工具由网关执行回填、继续循环；非 MCP 工具累积到
       `pending_client_uses` 最终合并进响应交给客户端；
    2. openai `parse_response` 捕获 `reasoning_content` → IR Thinking 块；`encode_request`
       assistant 分支原样回传 `reasoning_content`；MCP 循环回填 assistant 时保留 Thinking 块；
    3. Responses 解码 is_message 分支支持顶层 `function_call` / `function_call_output` / `reasoning`。
  - 新增单测：`reasoning_content_roundtrip`、`decode_assistant_embedded_function_call`。
  - ✅ curl 实测：纯 MCP 循环（thinking 模型）成功、内嵌格式 + reasoning 回传成功、日志 200。

- [x] 6. `mcp_pool` 集成测试负载敏感 + **失败级联**（会污染 `scripts/regression.sh` 门禁）
  - 现象：`cargo test --workspace` 偶发 3 个用例 FAILED，报 `MCP initialize 超时`；**每轮失败的用例集合都不同**
    （实测两次分别为 {timeout_discards_connection, rebuilds_after_crash, env_participates_in_pool_key}
    与 {timeout_discards_connection, rebuilds_when_process_dies_after_response, idle_reclaim_reaps_process}）。
  - 根因（已定位，两段证据）：
    1. `JAI_MCP_POOL_CALL_TIMEOUT_MS` **同时管初始化握手**（`crates/gateway-core/src/mcp.rs:349` 用
       `pool_call_timeout()` 包住 `initialize`），而 `tests/mcp_pool.rs:111` 把它压到 **100ms**；机器有负载时
       「spawn 假 server + 握手」可能超过 100ms → init 超时。
       验证：`JAI_MCP_POOL_CALL_TIMEOUT_MS=30 cargo test -p gateway-core --test mcp_pool`
       → 5/7 用例报同一句 `MCP initialize 超时`（复现签名）。
    2. `tests/mcp_pool.rs:122` 的 `remove_var` 在 `assert_eq!().unwrap()` **之后**，只在成功路径执行；
       一旦 panic 就跳过清理 → 进程级环境变量停留在 100ms → **后续所有用例连带超时**。
  - 影响：门禁偶发红灯，且失败信息指向无关用例，容易误判为被测代码回归（本次即先被误判为本次改动导致；
    随后以 70 次「有/无改动」对照实验（各 35 次、6 路 CPU 负载）均 0 失败，证实与本次 codec 改动无关）。
  - 建议修法：① 用 RAII guard（Drop 时还原环境变量），别把清理写在断言之后；
    ② 初始化握手不要复用调用超时（单列 `JAI_MCP_POOL_INIT_TIMEOUT_MS`，给 spawn 留足余量）；
    ③ 该文件测试是进程级共享状态，超时阈值不宜压到 100ms 量级。
  - 【2026-09-16 v0.2.1 发版实测】门禁又红一次，失败集 `{timeout_discards_connection,
    rebuilds_when_process_dies_after_response}`，错误均为 `MCP initialize 超时`——与既有签名一致；
    处置与判据（可复用）：① `cargo test -p gateway-core --test mcp_pool` **单跑 7/7 通过**；
    ② `JAI_MCP_POOL_CALL_TIMEOUT_MS=30` 能复现该签名（证明是阈值/负载敏感而非功能回归）；
    ③ 本轮改动（`codec/ir.rs` 护栏）与该测试无调用关系。随后重跑 `release_check.sh` 全绿（EXIT=0）。
    → 结论：**这是门禁噪音，不是本次回归**；但每次发版都要靠"重跑+复现签名"来排除它，成本在持续累积，
    建议尽快按下述修法根治（属独立小任务，不阻塞本次发布）。

  - 【2026-09-16 补充】同族还有第二个坑（由本次新增测试踩到）：全局 stdio 池的连接键是
    `(cmd, args, env)`，而池里缓存的**进程句柄绑定创建它的 tokio runtime**；同一测试进程内多个
    `#[tokio::test]` 各自建 runtime，若两个测试用同一键（env 相同）就会跨 runtime 复用连接，报
    `读取 MCP 响应失败: A Tokio 1.x context was found, but it is being shutdown.`
    → registry 的 `build_proxy_tools` 静默跳过该 Server，表现为「动态工具时有时无」（本次在 6 路 CPU
    负载下 15 轮复现 10 次）。**处置**：新增测试一律给 fixture 唯一 `env`（如 `FAKE_TEST_ID`）隔离池键
    （`tests/mcp_pool.rs` 早有此先例与注释）；修复后同负载 15 轮 0 失败。
    **生产不受影响**：Tauri 命令与网关都跑在 `tauri::async_runtime` 同一个 runtime 上。

  - **已解决（2026-09-17）**：按上述建议修法 ①② 根治，不再依赖「重跑 + 复现签名」发版。
    1. **初始化握手独立预算**（`mcp.rs`）：新增 `DEFAULT_INIT_TIMEOUT` + `JAI_MCP_POOL_INIT_TIMEOUT_MS`，
       `spawn_ready` 里的 `initialize` 超时由 `pool_call_timeout()` 改为 `pool_init_timeout()`。
       默认值与调用超时一致（120s），故**生产行为不变**——只是不再被调用侧的短预算误伤。
    2. **环境变量清理改 RAII**（`tests/mcp_pool.rs`）：新增 `EnvGuard`（`Drop` 时还原原值/删除），
       取代原先「断言之后 `remove_var`」的写法，panic 与提前 return 两条路径都能收回，
       失败级联的传播链被切断。
    3. 新增回归用例 `init_handshake_uses_separate_budget`：把调用超时压到 **1ms**（握手必然超不过它），
       断言报错不含 `initialize` → 旧实现下必红。
  - 验证（2026-09-17）：
    ① 临时把 `initialize` 回退为复用 `pool_call_timeout()` → 新用例必红，且报错与生产签名逐字一致
      （`MCP initialize 超时`）；
    ② 原复现命令 `JAI_MCP_POOL_CALL_TIMEOUT_MS=30 cargo test -p gateway-core --test mcp_pool`
      → **8/8 通过**，日志中 `MCP initialize 超时` 出现 **0 次**（旧实现在此命令下 5/7 报该签名）；
    ③ 6 路 CPU 负载下连跑 15 轮 → **0 轮失败**；④ `cargo test --workspace` 全绿（EXIT=0）。

- [x] 7. Responses **出站**丢弃消息级图片（`Block::Image` 在 user 消息里）
  - 位置：`crates/gateway-core/src/codec/responses.rs` 的 `encode_request`，注释写「Responses 上游 v1 不支持
    图片内联转换，Lenient 丢弃」——**该注释与官方 schema 存疑**：Responses 的 message content 支持
    `input_image`（依据见 `docs/design/tool-result-image-protocol-factcheck.md`）。
  - 与「工具结果内嵌图片」（本次已修）属同一类静默丢失，但**本次未动**：改动会让原本「静默丢图但请求成功」
    的请求变成「带图请求」，若某些中转上游不接受可能由 200 变 4xx，需单独评估。
  - 建议：先确认目标上游对 `input_image` 的接受度，再按与本次相同的「原生承载 / 显式降级 + CapabilityWarn」口径处理。
  - **已解决（2026-09-17）**：改为**原生承载**，与「工具结果内嵌图片」同一口径。
    1. `responses.rs::encode_request` 的 `Role::User` 分支不再丢弃 `Block::Image`：content 数组按
       **块序**混排 `input_text` / `input_image`，复用已有的 `render_input_image()`（url 优先，
       否则把 base64 包回带真实 media_type 的 data URL）。
    2. 顺带修掉一个更隐蔽的后果：**只有图片没有文本**的用户消息此前连 `message` 都不产出
       （`text_parts` 为空即跳过），整轮图静默消失；现在有内容项就产出消息。
    3. 纯文本消息的产出形状与旧实现**逐字节一致**（仍是一段文本一个 `input_text` 项），
       不改动绝大多数请求的请求体口径。
  - **接受度评估**（回应原「需单独评估」的顾虑）：同一请求体里 `input_image` 已在
    `function_call_output.output`（工具结果内嵌图片）上生产使用，并通过 v0.1.9 起的跨族实测；
    message content 的 `input_image` 是**同一个 schema 类型、同一族协议**，故不新增风险面。
    能力声明侧本就把「user 消息带图」视为各族原生支持（`capability.rs` 注释），
    本次是让 encoder 与已声明能力面对齐，而非新增能力。
  - 验证（2026-09-17）：`tests/multimodal_image.rs` 新增 3 例（含图 user 消息保块序 / 仅图片消息不被丢 /
    纯文本形状不变）；临时恢复「丢弃」逻辑时前两例**必红**且症状与 bug 描述逐字一致
    （`text + image` 只产出 1 个内容项、仅图片时「应产出 message」panic）；18/18 全绿。

- [x] 8. dsh-tui 上下文占比统计不出来（走 JAI 网关时 `ctx 0/128k 0.0%`）
  - 现象：dsh-tui 状态栏 `ctx 0/128k 0.0%`（本机 dsh-tui + JAI，模型 `超算/Qwen3.8-Flash-Event`）；
    同一网关的 `基元律动/deepseek-flash` 路由显示正常 → 属**按供应商分化**，不是 dsh-tui 侧问题。
  - 根因：`crates/gateway-core/src/codec/openai.rs` 的 `parse_stream_event` 判定「usage 帧」时只看
    `choices` 是否为**空数组**。scnet（超算）的末帧形状是
    `{"choices":[{"index":0,"delta":{}}],"usage":{...}}`（choices 非空、delta 为空），整帧被当普通
    chunk 丢弃 → IR 只剩 `finish_reason` 帧的零值 Finish → 出站 `response.completed.usage` 恒 0 →
    dsh/pi-ai 的 `tokens.input` 恒 0 → ctx 占比恒 0。对照 tokenrhythm（基元律动）末帧是
    `"choices":[] + usage`，所以只有超算路由复现（同一份代码，两种供应商写法）。
  - 已解决：
    1. usage 采集与 choices 形状解耦：带非 null `usage` 的帧一律产出 IR Finish（与 content 增量同帧时
       正文照旧不丢）；
    2. `convert_streaming_response` 把 Finish **挂起合并**，上游 `[DONE]`/EOF 时一次性下发真实 usage——
       否则「先 finish_reason 帧、后 usage 帧」会先发一个 usage 全 0 的收尾帧，再补一帧就成了重复的
       `response.completed` / `message_stop`（挂起点在 `[DONE]` 与 EOF 两处，幂等 take）；
    3. 落库口径同步：零值 Finish 不再覆盖已采到的真实 usage（防「usage 帧在前、finish_reason 帧在后」）；
    4. 顺带修 `emit_log` 把 `route_mode` 硬编码成 `"passthrough"` 的问题：跨族转换请求也被记成直通，
       本次排查正是被日志这个字段误导（新增 `RouteMode` 枚举 + `emit_log_with`，转换路径 20 处调用点改标 converted）。
  - 验证：
    - 单测 4 例：非空 choices usage 帧 / 拆帧顺序 / usage 与正文同帧 / `"usage":null` 干扰帧；
    - 集成用例 `responses_to_openai_stream_usage_split_frames` 按抓包的真实帧形状跑完整链路
      （客户端 Responses 入站 → 网关 → mock chat 上游）：completed 帧唯一，`input_tokens 259132`、
      `output_tokens 1805`、`cached_tokens 258944`，且日志 `route_mode=converted`、
      `usage_input/usage_output` 落库为真值；
    - 两处 A/B 反证：还原旧 usage 判定 → usage 断言挂；还原硬编码 route_mode → converted 断言挂；
    - `cargo test -p gateway-core` 全绿（196 单测 + 各集成文件）+ clippy 无告警。
  - 诊断期证据（可复核）：直连上游 `api.scnet.cn` 的 `/v1/chat/completions`、`/v1/responses` 均返回真实
    usage；JAI `request_logs` 里超算路由 `usage_input/usage_output` 恒 0、基元律动路由为真实值；
    dsh 会话日志（`$DSH_HOME/sessions/**/session.v3.jsonl.zstd`）里 `usage` 全 0 而
    `request/context.contextWindow=128000` 正常（上下文窗口没问题，是占用数字没了）。
  - 注意：本修复需**重建并重启 JAI** 才生效（线上跑的是旧二进制）；重启前 dsh-tui 仍显示 0%。

- [x] 9. dsh 调 JAI 代理的 MCP 工具报 `MCP error -32001: Request timed out`（超时预算三方不匹配）
  - 已解决（网关侧，2026-09-16）：代理转发加独立预算 `JAI_MCP_PROXY_CALL_TIMEOUT_MS`（默认 **55s**，
    刻意低于常见客户端预算），超时**主动放弃等待**并返回工具级错误 + 可执行指引（长命令改走
    `terminal_start` + `terminal_poll`）。修复位置 `crates/gateway-core/src/server/registry.rs`
    （`DEFAULT_PROXY_CALL_TIMEOUT` / `proxy_call_timeout()` / `fmt_budget()`）。
  - 验证：新增 `tests/mcp_proxy_timeout.rs`（假 stdio server 的 `sleep` 工具 3s vs 200ms 预算）——
    断言「工具级 isError + 文案含 `未返回`/`terminal_start`/`JAI_MCP_PROXY_CALL_TIMEOUT_MS` +
    耗时 < 2s」；**负控制**：把预算改成 5000ms（> sleep 3000ms）→ 用例如期失败
    （返回 `{"content":[{"text":"slept"}],"isError":false,...}`、耗时 3.23s），还原即通过。
  - 客户端侧（本次**未做**，用户选择只改网关）：`~/.dsh/cordis.patch.yml` 的 `mcp-jai-registry`
    仍应择机加 `toolCallTimeoutMs`（建议 > 网关 55s，例如 150000），否则碰到网关刻意不接的长任务时
    agent 仍会看到客户端级 -32001；README 已补「超时预算表」把三方预算与调参入口写清。
  - 现象（dsh 会话日志实证，`$DSH_HOME/sessions/**/session.v3.jsonl.zstd`，2026-09-15 16:50，turn 10）：
    调 `mcp__jai-registry__netcatty-external__terminal_execute` 后 dsh 侧报
    `Error: MCP error -32001: Request timed out`；同一步并行发出的 `get_tool_schemas`、
    `vault_hosts_list` 连带报 `Error: tool call aborted before dispatch`（一批并行调用全废）。
  - 网关侧证据：`proxy_call_logs` 同一时刻只有一行
    `netcatty-external | terminal_execute | stdio | ok | duration_ms=60007`
    —— 网关等满 **60.007s** 才拿到上游结果，而 dsh 在 **60.000s** 已硬中止 → 差 7ms 输掉竞速。
  - 根因：**三方超时预算不匹配**，上游允许的最坏耗时 > 客户端预算，而网关比客户端更能等（既不提前失败、也不收敛）。
    | 环节 | 预算 | 出处（实证） |
    |---|---|---|
    | dsh MCP 客户端单次调用 | **60_000ms** 硬中止（MCP 错误码 -32001） | `@deepseek-ai/dsh-mcp-client/lib/index.js`：`DEFAULT_TOOL_CALL_TIMEOUT_MS = 6e4`；可用配置项 `toolCallTimeoutMs` 覆写 |
    | JAI `/mcp` 代理转发 | **120_000ms**（`JAI_MCP_POOL_CALL_TIMEOUT_MS`）；转发路径无独立预算 | `crates/gateway-core/src/mcp.rs:147`（`DEFAULT_CALL_TIMEOUT`）、`server/registry.rs:435+`（`proxy_call_tool` 直接 await） |
    | Netcatty 长时工具（`terminal.execute`：`policy.longRunning=true`） | **60_000ms 操作超时 + 5_000ms RPC 缓冲 = 65_000ms** | `/Applications/Netcatty.app/Contents/Resources/app.asar.unpacked/electron/capabilities/{rpcTimeouts.cjs,constants.cjs}`：`DEFAULT_OPERATION_TIMEOUT_MS / RPC_TIMEOUT_BUFFER_MS` |
  - 影响面：凡「接近或超过 60s 才返回」的 MCP 代理调用，agent 侧看到的是客户端级 -32001（而非工具级 isError），
    且会连带作废同批其它工具调用；agent 无法区分「工具真失败」与「网关还在等」。
  - 建议修法（可组合）：
    ① 客户端侧（立即见效、无需改网关）：`~/.dsh/cordis.patch.yml` 里 `mcp-jai-registry` 增 `toolCallTimeoutMs: 300000`；
    ② 网关侧：代理转发加独立预算 `JAI_MCP_PROXY_CALL_TIMEOUT_MS`（默认略低于常见客户端预算，如 55s），
       超时回**工具级 isError + 指引**（长命令改用 `terminal_start`/`terminal_poll`），别把 -32001 丢给 agent；
    ③ 文档：README「MCP 代理执行」补一张超时预算表 + 调参说明。

- [x] 10. JAI `/mcp` 代理把上游**工具级失败拉平成成功**（`isError` 恒 false）且 content 被二次字符串化
  - 已解决（2026-09-16）：`registry.rs` 引入 `ToolOutcome`（`Info` / `Proxied`），代理路径经
    `proxy_result_payload()` **原样透传**上游结果：`content` 逐块保留、`isError` 冒泡、
    `structuredContent` 保留，来源改为非标准 `source` 字段（不再破坏 content 结构）；
    静态台账/技能工具维持「JSON 文本」形态。
  - 验证：新增 `tests/mcp_proxy_passthrough.rs` 3 例——失败冒泡（`isError:true` + 正文原样）、
    成功透传（content 不被包裹/二次编码）、动态工具可见性 + 静态工具形态不变。
    A/B 反证：旧行为即线上 v0.1.9（`/Applications/JAI.app`）实测响应
    `{"content":[{"text":"{\"result\":{…\"isError\":true},\"source\":…}","type":"text"}],"isError":false}`。
  - 实测（curl 直打网关 `POST /mcp`）：
    `tools/call {"name":"netcatty-external__terminal_execute","arguments":{"sessionId":"<已失效 id>","command":"echo x"}}`
    网关响应（原文）：
    `{"result":{"content":[{"text":"{\"result\":{\"content\":[{\"text\":\"Error: Session \\"...\\" is not in the current scope.\",\"type\":\"text\"}],\"isError\":true},\"source\":\"jai-gateway-proxy/netcatty-external\"}","type":"text"}],"isError":false}`
    —— 上游 `isError:true`（工具确实失败）被外层 **`isError:false` 覆盖**；dsh 侧按外层判定
    （`dsh-mcp-client`：`if (result.isError === true) throw ...`），于是**把失败当成功**交给模型。
  - 代码位置：`crates/gateway-core/src/server/registry.rs:570-580`（`Ok(payload) => {"isError": false}` 硬编码）；
    `registry.rs:479-485` 把上游 result 包成 `{source, result}` 再 `to_string()` 塞进单个 text 块 →
    上游 `content` 结构、`structuredContent`、`image` 块全丢（dsh 的 `containsImage` 图片投影因此永不触发）。
  - 建议修法：代理路径**原样透传**上游 `result`（`content` 数组 + `isError` + `structuredContent`），
    来源标注改成追加一个 text 块（或不动）；静态台账/技能工具维持「JSON 文本」形态。

- [x] 11. `request_logs.tool_calls` 恒 0（1930/1930 行全 0，含真实工具循环的会话）
  - 已解决（2026-09-16）：`emit_log` / `emit_log_with` 增加 `tool_calls` 参数并落真值：
    跨族转换非流式按 IR 数 `Block::ToolUse`；跨族转换流式按 `StreamCallStart`(ToolCallStart)
    的 **id 去重**计数（容忍 Gemini 每帧都发 Start 且 index 恒 0）；同族直通非流式按入站线形状
    数响应体（openai `choices[].message.tool_calls` / anthropic `content[].type=="tool_use"` /
    responses `output[].type=="function_call"`）。`store::logs::LogRowView` 同步暴露该字段。
  - **已知未采集**：直通**流式**（`route_mode=passthrough` + `is_stream=1`）仍为 0 —— 该路径是
    字节直通、不解析 SSE 语义（只有 UsageScanner 做关键字级扫描），已在 `streaming_response`
    的代码注释里写明，避免再次误导排查。
  - 验证：单测 `count_tool_calls_in_passthrough_bodies` / `count_ir_tool_uses_counts_tool_use_blocks`；
    m6 集成 2 例（非流式工具调用落 1、流式并行两个工具调用落 2）。
  - 现象：本机 `jai.db` 全表 `select tool_calls, count(*) group by tool_calls` → 只有 `0 | 1930`；
    而同一时段 dsh 会话里有大量工具调用（含 MCP 代理调用）。
  - 影响：无法用日志判断「模型到底有没有发起工具调用」，本次排查即被该列误导（一度以为 MCP 调用没进网关）。
  - 根因（已定位）：`crates/gateway-core/src/server/proxy.rs:327` 的 `emit_log_with` 里 `tool_calls: 0` 是**写死的**，
    函数签名里根本没有该参数（对比 bug 8 给 `route_mode` 加参数的做法，这一列当时没跟着做）；`store/logs.rs:307` 同理。
  - 建议修法：给 `emit_log_with` 加 `tool_calls: usize` 参数，由各调用点从 IR 响应块（`Block::ToolUse` 计数）传入；
    直通路径可从 upstream body 里数 `tool_calls`，或在文档里把该列标注为「暂未采集」以免再次误导排查。


- [x] 12. 本地 `cargo tauri build` 与 CI 的前端钩子 **cwd 不一致**（`../ui` 只对 CI 成立）
  - 现象（本次打 release 时踩到）：本地 `cd src-tauri && cargo tauri build --bundles app` 报
    `Running beforeBuildCommand `pnpm --dir ../ui build`` → `ERR_PNPM_ENOENT: no such file or directory,
    lstat '/Users/jiangnan/Documents/workspace/ui'`（`../ui` 被解析到仓库外）。
  - 根因（两侧都有实证）：
    - **本地** tauri-cli 2.11.4 执行前端钩子的 cwd = **仓库根**。探针验证（`beforeBuildCommand`
      设成 `sh -c 'pwd > /tmp/f.txt'`，`cargo tauri` 与 `cargo-tauri` 两种调用、从仓库根与 `src-tauri`
      两个目录都试过）→ 输出恒为 `/Users/jiangnan/Documents/workspace/JAI`，故 `../ui` 落到 `<上级>/ui`。
    - **CI** `tauri-apps/tauri-action`（未设 `projectPath`）以 `src-tauri` 为 cwd：v0.1.9 的 Release 运行
      日志（run 34556631183）里 `Running beforeBuildCommand `pnpm --dir ../ui build`` **成功**、
      vite 构建通过、macOS + Windows 均出包 → 仓库里这个 `../ui` 对 CI 是**正确**的。
  - 处置（**不改仓库配置**，否则会弄坏 CI）：本地打包用覆盖配置把钩子换成「仓库根 cwd 下可用」的形式，
    做法已写进 `docs/design/release.md` §6「本地打 macOS 包」。
  - 教训：`tauri.conf.json` 的 `beforeDevCommand` / `beforeBuildCommand` 是 **CI 口径**（cwd=`src-tauri`），
    别为了本地能跑就改成 `pnpm --dir ui …`；同理本地 `cargo tauri dev` 也会踩同一坑（仓库一直用
    `scripts/dev.sh` 绕过，所以没暴露）。


- [x] 13. **长会话被工具参数护栏判死锁**：turn error `工具参数累计超过 262144 字节上限（护栏）`
  - 现象：agent 客户端（dsh）在会话进行到中后段后，**每一轮**都收到
    `OpenAI API error (400): {"code":null,"message":"工具参数累计超过 262144 字节上限（护栏）"}`，
    新建会话才能恢复；会话本身没有任何异常输入，纯属"聊得够久就必坏"。
  - 根因：`codec/ir.rs::validate_guards` 的 `tool_args_bytes` 在**整个请求的所有消息之间累加**，
    而 agent 客户端**每轮把完整历史全量重放**——历史里 tool_use 参数一旦合计越过 256KB，
    该会话后续每一轮都必然超限 → **永久 400，无自愈路径**（客户端只能看到无信息量的 turn error）。
    同文件 `blocks ≤ 64` 早已因完全相同的理由改成「按单条消息、不跨消息累计」
    （`MAX_BLOCKS_PER_REQUEST` 上方注释：「跨消息累计会把正常长会话误伤」），**args 侧漏改**。
    触发量级并不夸张：几次大文件 `write`（把整份文件塞进工具参数）+ 大 `bash` 命令 + 历史重放即可越过 256KB。
  - 已解决（v0.2.1）：
    1. `validate_guards` 改为**按单条消息**校验工具参数（累加器移入消息循环内），跨消息不再累计；
    2. 常量更名 `MAX_TOTAL_TOOL_ARGS_BYTES` → `MAX_TOOL_ARGS_BYTES_PER_MESSAGE`，名字即语义，
       并在注释里写明"整请求体大小由上游自身限制兜底"；
    3. 报错文案从「工具参数累计超过 N 字节上限（护栏）」改为**可定位可操作**：
       `单条消息工具参数累计超过 N 字节上限（护栏）：第 K 条消息 M 字节；请缩小单次工具调用参数，
       或新开会话丢弃历史中的超大参数`（block 超限同理带上消息序号与实测块数）；
    4. 回归测试 `codec::ir::tests::guards_tool_args_per_message_not_request`：4 条各 ~200KB 的历史消息
       （合计远超上限）必须放行；单条消息内多块累计超限仍拦，并断言错误里带消息序号。
  - 教训：**凡是"按整请求累计"的护栏，都必须先问一句"客户端会不会每轮重放历史"**——
    agent 客户端的请求是「历史 + 新增」，任何跨消息累计的阈值都会随会话长度单调逼近并最终锁死会话。
    同类风险点复核：`tools` 声明数上限（`tools_limit_exceeded`）是**每轮随 tools 数组重发**的，
    阈值按"单轮声明数"判定，不随会话增长，安全。


- [x] 14. **WebDAV 本地快照自引用递归**：`meta.webdav_last_snapshot` 单行涨到 595 MB，且每次推送翻一倍
  - 现象（本轮排查 Skill 时顺带撞到）：`jai.db` = 938 MB、WAL = 629 MB；`dbstat` 显示 `meta` 表占
    **596 MB**，其中单行 `webdav_last_snapshot` = **624,552,032 字节**。用
    `(length(value)-length(replace(value,'webdav_last_snapshot','')))/length('webdav_last_snapshot')`
    数出该值**内含自己 19 层**。远端同源现象：`/jai-config.json` = **42.06 MB**，时间戳备份链清晰翻倍
    `0.01→0.02→0.03→0.05→0.08→0.12→0.21→0.38→0.72→1.38→2.69→5.32→21.07 MB`（13+ 个），
    相邻间隔约 **30 分钟** = `webdav_auto_push_interval_min`，即**每轮自动推送翻一倍**。
  - 根因：`store/export.rs::build_export_json` 用 `SELECT key,value FROM meta` **全量导出 meta**，
    而 `webdav_last_snapshot` 正是「上一版导出物」本身；`src-tauri/src/main.rs::push_now()` 的顺序是
    「构建导出 → `sync::snapshot_put` 存为快照 → PUT 远端」→ 每次推送把上一版快照封进新快照。
    导入侧一直用白名单过滤该 key（`store/import.rs`），**导出侧漏了** —— 两侧不对称即根因。
  - 影响面：本地磁盘/WAL 无界增长；每次推送体量逐轮翻倍（本地 595 MB → 下次约 1.19 GB）；
    远端备份链同步膨胀；删行后 DB 文件也不会自动缩小（实测 298 MB 空闲页）。**不跨设备传染**
    （导入白名单挡住了 `webdav_last_snapshot`）。
  - 修复（v0.2.2）：
    1. 导出侧改为 `WHERE key <> ?1`（用 `sync::snapshot_meta_key()` 单一常量，杜绝字面量散落再漏改）；
    2. `sync::snapshot_put` 加体积硬上限（默认 4 MB，`JAI_SNAPSHOT_MAX_BYTES` 可覆盖）：超限
       **跳过写入并 WARN**、**不返回 Err**（快照只是回退手段，绝不能反过来阻塞用户的配置推送）；
    3. 启动自愈 `sync::heal_oversized_snapshot`：存量 > 2 MB 的快照用当前配置重建（重建失败则删 key）；
    4. `sync::try_pull` 体积护栏（默认 32 MB，`JAI_REMOTE_CONFIG_MAX_BYTES` 可覆盖）：远端被撑大时
       报出指向本条的明确错误，不把几百 MB 读进内存；
    5. 新增 `store::meta_delete`（「没有此设置」≠「设置为空」，避免回退路径误用空快照）。
  - 回归（**含守门人实证**）：`store::export::tests::export_excludes_self_snapshot_key` +
    `export_size_stable_across_push_cycles`。后者在修复前**实测必红**：连续 8 轮「构建导出 → 存快照」
    的体积序列为 `1206 → 2628 → 4334 → 6604 → 10002 → 15656 → 25822 → 45012` 字节（逐轮翻倍），
    修复后恒定 1206 字节 —— 生产 19 轮 → 595 MB 的缩影。另加
    `snapshot_put_refuses_oversized_text_without_blocking_push`、`heal_rebuilds_oversized_snapshot`、
    `heal_is_noop_for_normal_or_absent_snapshot`、`oversized_remote_error_is_actionable`
    与集成测试 `m7_import_webdav::pull_rejects_oversized_remote_config`。
  - 教训：**任何"把当前完整状态存下来"的逻辑，都必须先问一句"这份状态里有没有它自己"** ——
    快照/导出/备份三件套最容易踩。对策是把"导出物必须自引用安全"写成**断言**（连续 N 轮体积恒定），
    而不是靠 review 记住；同族风险点复核：`export_config_json` 命令（复用同一 `build_export_json`，
    随本次修复一并干净）、远端时间戳备份（内容同样来自导出物，随之变干净）。
  - 【2026-09-17 追加】永久解法已并入 v0.2.3：启动时按需回收磁盘
    （`store::retention::reclaim_if_bloated`，空闲页占比 ≥ 60% 且库 ≥ 32 MB 才触发；
    `JAI_VACUUM_ON_START=0` 可关、`JAI_VACUUM_FREELIST_RATIO` / `JAI_VACUUM_MIN_MB` 可调）。
    起因：删除大行后 SQLite 文件不会自动缩小，本机实测「895 MB 库里 894 MB 全是空洞」，
    只能靠人工 VACUUM 收尾；现在由启动路径兜底（实测回收 894.6 MB → 0.7 MB）。
    注意接线位置：必须早于日志连接建立（VACUUM 需独占），见 `main.rs` 启动段注释。


- [x] 15. **技能名未校验 → 工具名违反 MCP 规范**（严格客户端可能拒绝整份 tools/list）
  - 现象（2026-09-17 手工验证时发现，并经临时回退复现）：`skill_create` 只校验名称非空
    （前端同样只查 `!name.trim()`），插入名为 `代码 评审/甲` 的技能后，`tools/list`
    真的广告出工具名 `skill__代码 评审/甲` —— 违反 MCP 名契约 `[A-Za-z0-9_-]{1,64}`。
  - 影响面：dsh 客户端会自行 sanitize（`replace(INVALID_NAME_CHARS,'_')` + 12 位摘要）再映射回原名，
    所以**对 dsh 不致命**；但严格校验的客户端可能拒绝**整份** tools/list —— 连累全部代理工具
    （本机实测 67 个），属"一个怪名技能打翻整个 MCP 面"的高爆炸半径场景。
  - 修复（v0.2.3）：工具名走确定性编码映射 `skill_tool_names()`：
    ① 原名本身合法且未占用 → 保持 `skill__<原名>`（**向后兼容**，历史会话见过的
    `skill__code-review` 仍有效）；② 否则 → `skill__<sanitized>_<hash6>`（非法字符替换为 `_` +
    6 位名字摘要，合法且唯一）；③ 真实技能名始终出现在工具 description 里，模型仍看到可读名；
    ④ `dispatch_tool` 反解时先查映射表，再回退「原样名字」形式（兼容手工调用与旧会话）。
  - 回归（含守门人实证）：新增 `tests/skill_lifecycle.rs::advertised_tool_names_are_spec_legal`
    —— 断言**所有**广告出的工具名满足契约；临时回退为修复前逻辑时该测试必红，报错为
    `工具名违反 MCP 契约: "skill__代码 评审/甲"`（与生产症状逐字一致），修复后 6/6 通过。
  - 教训：**凡是「用户自由输入的名字会被拼进协议标识符」的地方，都要做编码或白名单**；
    命名自由与协议约束的冲突应该由**编码**吸收（保住可读名），而不是靠"客户端大概会自愈"。

- [x] 16. **`get_skill_detail` 可绕过 `enabled` 闸门**（台账与投递语义不一致）
  - 现象：`skill__<name>` 投递对 `enabled=0` 明确拒绝（「未启用，暂不投递」），但
    `get_skill_detail` 无论启用与否都返回**全文** → 「只投递给已启用技能」这道闸门可被旁路。
  - 影响：语义自相矛盾，用户以为"关掉=不投递给 Agent"，实际 Agent 换个工具就能拿到全文。
  - 修复（v0.2.3）：`tool_skill_detail` 与投递口径统一——未启用即拒绝，错误信息指引到
    「技能」页启用；`list_skills` 仍照常返回 `enabled` 标记供 Agent 判断。
  - 回归：`skill_lifecycle.rs::skill_detail_follows_enabled_gate`（未启用 → `isError=true`
    且错误文本里不得出现技能内容）。

- [x] 17. **台账类工具把失败拉平成成功**（`isError=false` + 错误埋在文本里）
  - 现象：`skill__不存在` 返回 `isError=true`，而 `get_skill_detail(不存在)` 返回
    `isError=false` 且错误藏在内容 JSON 的 `error` 字段里；`get_mcp_server_detail` /
    `get_tool_schemas` 同样（未找到/未启用/上游 tools/list 超时都被拉平）。
  - 影响：只看 `isError` 的客户端会把「未找到」当成功，进而可能拿空载荷继续推理
    （幻觉来源）；这正是 v0.2.0 bug 10「工具级失败被拉平成成功」的同族问题，只是残留在静态台账路径。
  - 修复（v0.2.3）：台账类工具失败一律走 `ToolOutcome::Err`（→ `isError=true`），
    成功路径仍 `isError=false`。
  - 回归：`skill_lifecycle.rs::ledger_tools_fail_loudly`（三类失败 + 缺参数都必须冒泡）、
    `skill_detail_follows_enabled_gate`。
  - 教训：**错误语义要按"调用方怎么判断成败"设计**——只要客户端习惯看 `isError`，
    把错误塞进成功载荷就是隐性谎言。

- [x] 18. **暗色主题下 toast 不可读**（近白字 + 浅绿底，对比度 1.01）
  - 现象：暗色主题下成功/信息类 toast（「端口已保存」「已导出 JSON」「MCP「x」连接正常」等）
    文字几乎不可见。视觉回归审计实测 `ratio=1.01`（AA 需 4.5）。
  - 根因：`ui/src/main.tsx` 把 `<Toaster>` 挂在 `<ThemeProvider>` **外面**，sonner 里的
    `useTheme()` 取不到主题 → 永远退回 `theme="system"`，`richColors` 于是给**浅色**调色板
    （成功底色 `hsl(143,85%,96%)` 浅绿）；而同一处的 `toastOptions.style` 又把文字色写死成
    `var(--foreground)`——暗色下是近白 —— 两者叠加就是「近白字 + 浅绿底」。
  - 影响：暗色主题用户完全看不到成功提示（属可读性硬伤，不是观感问题）。
  - 修复（2026-09-17）：把 `<Toaster>` 移入 `<ThemeProvider>` 内部，`useTheme()` 恢复可用，
    sonner 拿到正确主题（暗色下 `--success-bg: hsl(150,100%,6%)` 深绿）→ 实测 **16.73**。
  - 回归：`probe-toast2.mjs`（新增探针）断言 sonner 变量取自正确主题、且 toast 对比度 ≥4.5；
    `audit.mjs --theme=dark` 由「4 组不达标」→ **0 组**。
  - 教训：**主题有两种传递路径**（CSS 类 + React context），只驱动其一会让"跟随主题"的第三方
    组件静默走错分支；`richColors` 这类自带调色板的组件尤其容易踩。

- [x] 19. **换台电脑后同步「从来没成功过」**：新机器第一次拉取会把本机的自动同步开关改掉
  - 现象（用户实报「我换台电脑就没成功过」）：A 机同步正常；在 B 机（新电脑）配好 WebDAV、
    打开「自动拉取」并点一次「拉取」后，**B 机的自动拉取会变成关闭**，之后 B 再也收不到 A 的更新；
    对称地，B 机刻意关掉的「自动推送」会被翻成开，让新机器反过来覆盖远端。
  - 根因：`webdav_auto_*` 三个键（自动推送开关/间隔、自动拉取开关）被当作共享配置同步 ——
    导出侧随 meta 携带、导入侧在 `META_IMPORT_WHITELIST` 里，且 `apply_import` 对白名单键
    **无条件 `meta_set` 覆盖**。但它们是**本机调度偏好**（「这台机器多久同步一次」），
    不是共享配置。A 机 `auto_pull_enabled=0`（出厂默认关）就这样"旅行"到 B 机，
    把 B 机用户刚打开的开关在**第一次拉取时自己关掉** —— 一个自我否定的闭环：
    拉取这个动作本身把「以后自动拉取」的能力关闭了。
  - 排查过程中**先排除**的假设（都验证过，避免误判）：
    ① 密钥不随同步 → 否：0006 起凭据明文入库，导出携带 `api_key`/`gateway_key`/`webdav_password`；
    ② 导入后网关不热加载 → 否：路由每请求实时读库（`store::route_candidates`）；
    ③ 路径拼接因尾部斜杠不一致 → 否：`join_path`/`join_remote_file` 已归一化；
    ④ 远端数据缺失/损坏 → 否：实测远端 `jai-config.json` 完整（2 供应商带 Key、25 模型、网关 Key）；
    ⑤ 认证/协议不支持 → 否：curl 实测 GET 200；A 机推送正常（远端 exportedAt 与 A 机 last_sync 一致）。
  - 修复（2026-09-17）：把三个 `webdav_auto_*` 定为**本机调度偏好**，双向不参与同步：
    - 新增单一事实源 `sync::MACHINE_LOCAL_META_KEYS` + `is_machine_local_meta_key()`；
    - 导出侧剔除（`store/export.rs`）—— 这一层额外让「只升级一台机器」的半升级状态也安全
      （旧版本导入侧仍会收这些键，收不到就不会覆盖）；
    - 导入侧白名单 `7 → 4`，并保留 `!is_machine_local_meta_key(k)` 作为纵深防御
      （即便日后有人把键加回白名单也拦得住）。
  - 回归（`tests/m7_import_webdav.rs` 新增 4 条，共 13 条全绿）：
    - `pull_must_not_clobber_local_auto_switches`：**修复前必红**（实跑复现「拉取关掉自动拉取」）；
      刻意在 payload 里**注入**旧版本形态的三个键，保证导入侧单独退化也能测出来；
    - `export_omits_machine_local_switches`：导出物不得含这三个键，但 url/username/directory/password 必须随行；
    - `second_machine_pull_lands_data_and_keys`：新机器拉取后供应商/上游密钥/模型/网关 Key **全部到位**
      （证明数据链路本身是好的，把"开关被关"和"数据没到"两类问题分开）。
    - 双向守门人实证：只回退导入侧 → `pull_must_not_clobber_local_auto_switches` 必红；
      只回退导出侧 → `export_omits_machine_local_switches` 必红。
  - **注意（用户须知）**：修的是"新机器上的开关被覆盖"，要两台都升级到本版本才完全生效；
    且**新机器必须先手工填一次 WebDAV 地址/账号/密码**（凭据是"能拉取"的前提，无法自举）。
    升级后建议在新机器上按自己的意愿重新设置「自动拉取/自动推送」开关——从此不再被对方改掉。

- [ ] 20. 自动拉取的时间节奏被自动推送的间隔绑住（相邻缺陷，本次未改）
  - 位置：`src-tauri/src/main.rs::spawn_autopush` —— `let wait = push_interval.or(pull_interval)`，
    一个 tick 只取**其中一个**间隔；且定时分支里「开了自动推送就每 tick 都推」。
  - 后果：若自动推送设 360 分钟、自动拉取设 30 分钟，则拉取实际也是 360 分钟一次
    （用户会以为「自动拉取没生效」）。bug 19 修好后三个开关各自独立，这个耦合反而更容易被踩到。
  - 建议修法：tick 取启用项的最小间隔，并按各自「上次执行时间」分别判定是否该跑
    （状态已在 `autopush.last` / `autopush.last_pull` 里，`at_ms` 可直接用），
    避免把 360 分钟的推送也压成 30 分钟。需要一个可控时钟的测试。
  - 影响面：仅调度节奏，不影响数据正确性（`should_pull` 的 last-write-wins 仍会拦住重复导入）。

- [x] 21. **zcode 经 JAI 测试连接恒失败：「Provider rejected the model request.」**
  - 现象：zcode 自定义 Provider（`openai-responses` → `http://127.0.0.1:1314/v1`，模型
    `基元律动/deepseek-flash`）连接测试与真实会话全失败；报错文案由 zcode 生成
    （`zcode.cjs` 把上游 400/422 统一包装成这句），**看起来像模型名不对，实际无关**。
  - 定位过程（从日志而非猜测入手）：`request_logs` 里同文案 400 累计 53 次，`error_summary`
    是上游原文 —— `{"code":"UNSUPPORTED_FIELD","message":"DeepSeek reasoning_effort 只支持
    low、medium、high、xhigh、max","data":{"field":"reasoning_effort"}}`；zcode 的
    `rollout/model-io-*.jsonl` 显示失败请求全部带 `reasoning.effort=none`，而**直连**基元律动
    （不经 JAI）的会话带 `effort=max` 全部 200 —— 值域就是分水岭。
  - 根因：族级能力表把 `openai_compat` 的 reasoning 定为 `EffortMode::Native` ⇒ 客户端值
    **原样透传**；而「这家上游认哪些写法」无人负责。zcode 的 provider 没有 reasoning 元数据时
    按「无推理」发 `none`，恰好撞上只认 low..max 的上游。
  - 修复（2026-09-18，迁移 0011 + `crate::effort`）：供应商级/模型级「推理档位值域」声明
    （模型级覆盖供应商级，`NULL` = 未声明 ⇒ 原样透传，老供应商零影响）；
    `plan_reasoning` 归位（域内透传 / `none` 无档位则丢弃 / 其余收敛到最近档）；
    `CompatibilityPlan::resolve` 首次真正改写 `params.reasoning_effort`；
    直通路径 body 同步归一（顶层与嵌套两种形态）。
  - 回归：`tests/m9_capability.rs` 新增 6 条（`m9_7` 复刻本故障、`m9_11` 直通路径、`m9_12`
    未声明不干预），`effort.rs` 13 条单测，`import.rs` 1 条 0011 往返；真机复验上游
    不带 `reasoning_effort` → 200、带 `"low"` → 200。
  - **注意（用户须知）**：需重新构建并重启 JAI 才生效（重启自动迁移到 0011）；到「供应商」页把
    基元律动的档位填成 `low,medium,high,xhigh,max` 即可，zcode 侧不用改任何配置。
  - 附带澄清（曾被误判为根因）：`供应商/模型` 限定名是**支持**的（`split_once('/')`），
    但必须正斜杠 —— 反斜杠会当字面量 → 404（同日 08:54 的两次 404 即此）。

- [x] 22. 导入配置时 `openai_responses` 供应商被判「未知协议族」（顺手修）
  - 位置：`store/import.rs` 的 family 白名单 `matches!(family, "openai_compat" | "anthropic" | "gemini")`。
  - 后果：该族供应商无法随 WebDAV / 导出配置同步到另一台机器（0003 起它是合法族，
    `providers.family` 的 CHECK 也允许它）。
  - 修复：白名单补齐 `openai_responses`；回归并入
    `store::import::tests::roundtrip_carries_reasoning_effort_levels_and_responses_family`。


## 2. 优化清单

- [x] 1. 创建供应商弹框应该有按钮可以测试能不能获取到模型。
- [x] 2. skill 添加应该支持 zip 导入添加。
- [x] 3. UI/UX 优化：按 `docs/ui优化.md` 的 66 项建议逐项实施并勾选。
- [x] 4. 多供应商时，dsh 模型列表显示“供应商名/模型名”，请求支持按该限定 ID 路由。
- [x] 5. MCP 支持粘贴标准 `mcpServers` JSON 导入（Claude Code 格式）
  - 说明：`{"mcpServers":{name:{command,args,env,url,type}}}` 粘贴即导，同名更新、不合法条目跳过并报告；
    新增迁移 0005（`mcp_servers.env` 列），stdio 启动子进程时注入 env；导出客户端配置同步带 env。
  - 验证：131 测试全绿 + 前端构建通过。
- [x] 6. MCP 管理页两个开关缺说明（用户反馈：不知道都是干啥用的）
  - 现象：`启用` 是**裸开关** —— 界面上没有任何文字（只有 `aria-label`，肉眼与读屏之外都看不到）；
    `代理执行` 只有一个 `text-xs` 的「代理」二字，既短又没解释；两者**依赖关系**（代理执行需同时启用）
    也无处可查 → 用户不知道各自作用，也看不出为什么开了「代理」Agent 还是什么都看不到。
  - 已解决（2026-09-16）：
    1. 两个开关都加可见文字标签（`启用` / `代理执行`，原先的裸开关与「代理」孤字移除）；
    2. 列表上方加一行**常驻解释条**：启用 = 网关是否连接它（关闭后不进台账/工具列表、也不能被代理执行）；
       代理执行 = 是否让 Agent 调它的工具（工具以 `<server>__<tool>` 暴露并经网关转发，需同时启用）；
    3. 两个标签加 `Tooltip`（hover/聚焦）显示完整语义，含「默认关闭＝最小权限、有状态工具先评估」；
    4. 文案抽成 `SWITCH_HELP` 常量，解释条与 tooltip 共用同一份，避免两处漂移。
  - 验证：新增探针 `tools/visual-regression/mcp-switches.mjs`（11 项断言：解释条存在且含两条说明 /
    行内可见两个标签 / 含混的「代理」孤字已消失 / hover 出详情且文案正确 / **点标签文字能切换开关**
    （`true → false`）且 `mcp_set_enabled` 参数正确 / 无横向溢出 / 右侧操作按钮未被挤出 / 无控制台报错），
    1180×800 与 900×600 双尺寸全绿；`fold.mjs --size=1180x800` 复核 MCP 页 `foldPct 0 / belowFold 0 /
    pageHScroll 0 / mainHScroll 0`，与旧基线一致（仅剩既有的长 URL 截断 P2 项）。
  - 排查副产品（值得记住的坑）：探针 mock 若每次返回**同一个数组引用**，React 的 `setList(next)` 会因
    `Object.is(prev,next)` 跳过重渲染，表现为「切换开关后 UI 不更新」的**假 bug**；mock 必须每次返回新副本
    以模拟 IPC 反序列化边界（已在探针注释里写明）。
  - 探针自查（v0.2.1 发版复核时发现并修掉，**上面「900×600 全绿」当时的结论是错的**）：断言 ⑤
    「右侧操作按钮完整可见」原先用 `b.innerText` 匹配 `删除|编辑|列出工具|测试连接`，而行内按钮的文字标签
    带 `hidden lg:inline`（<1024px 是纯图标）→ 900×600 下 innerText 为空、匹配集为空、`worst` 为 `undefined`。
    已改为回落到 `aria-label` / `title` 匹配，并新增 `matched > 0` 断言使**空匹配不再能算作通过**；
    双尺寸复跑：12 个按钮全部落在视口内（1180 → right 1109；900 → right 859）。
    教训：断言里凡是「先筛选再取最值」，都必须显式断言**筛选结果非空**，否则「找不到元素」既可能假通过
    （`undefined && …` 恰好被写成宽松条件）又可能报出无法定位的失败，两种都在浪费排查时间。

## 3. 视觉回归（默认窗口 1180×800，最小 900×600）

> v0.2.0 起默认窗口 980×640 → 1180×800（最小 760×520 → 900×600），见 §2 第 12 条。

> 详见 [`docs/视觉回归整改plan.md`](视觉回归整改plan.md)（含 303 步动态点击遍历、715 张截图、对比度/命中区/折叠线量化数据）。
> 完成一项就把 `[ ]` 改成 `[x]`。

- [x] 1. P0 供应商「添加/编辑」弹窗主按钮打开即不可见：弹窗内容 620–780px 挤进 542px 可滚区，`创建/保存/测试连接/取消` 在可视框外（`ProvidersPage.tsx:508` 把 `max-h-[85vh] overflow-y-auto` 加在整个 DialogContent 上）
- [x] 2. P0 弹窗基座 `dialog.tsx` 缺 `max-h`/可滚动正文结构：内容一长（长技能正文、MCP env 行、大 JSON 导入）就溢出视口且无法滚动（760×520 下 MCP 添加弹窗 h=528 > 520，上下各被切 4px）
- [x] 3. P0 toast 遮挡底部控件：`bottom-center` toaster（z=999999999，rect x312–668/y565–619）在遍历中造成 150 次「被遮挡点不到」，含弹窗内输入框与 `检查更新`/`测试连接`；建议改 `bottom-right` + z-index 降到 40
- [x] 4. P1 主操作在首屏之外（同步页已修：操作条吸顶；设置页/网关页待做）：同步页首屏曾仅 38%（`保存配置/测试连接/预览变更/推送/拉取` 全在 y=761）、日志 21%、设置 34%、模型 36%、网关 55%；760×520 下同步 28%、供应商 39%
  - 已解决（2026-09-17）：**网关页**新增吸顶主操作条（`启动/停止` + `复制 MCP 配置` + 端口回显），
    并移除 MCP 卡片内那个随内容滚动、会被折叠线切 25px 的浮动「复制配置」按钮
    （同一操作不再出现两份）；状态卡内的启停按钮同步上移，卡内留一行指路说明。
  - **设置页**复核结论：**无需再改**——plan §P1-4 修复方案第 3 项（按卡片加锚点导航）早已落地：
    `SettingsPage.tsx` 的 `main nav` 为 `position: sticky`，覆盖 端口/代理/日志/跨域/凭据/数据/更新
    七个分区，每张卡片的保存按钮随卡片走。原 `[~]` 注记「设置页待做」系过时。
  - 验收判据调整（本次实测数据，`fold.mjs`）：`foldPct ≤ 25%` 这一判据对**长表单页**不成立——
    设置页内容 1848px、同步页 1630px，在 764px 可视高下必然只有 41%/47% 首屏可见。故改按
    plan 的实质目标验收：**主操作无需先滚动即可点到**。复核结果：
    网关页 `belowFold 0`（修前 1，即 `复制配置@866`）；同步页主操作由吸顶条覆盖；
    设置页由锚点导航 + 卡片内保存覆盖。9 个页面中 7 个已 `foldPct 100%`
    （MCP/技能/供应商/模型/统计/日志 等）。
  - 验证：`probe-sticky`（新增探针）实测网关页吸顶条滚动 700px 后
    「复制 MCP 配置」仍在视口内（top 72 / bottom 108，紧贴滚动容器顶部）。
- [x] 5. P1 模型表：已确认由 Table 基座横向内滚 + 加窄窗口提示；列降级待评估。原：760 宽窗口溢出 217px，行内输入被折叠线切 9–14px
  - 【2026-09-17 复核】默认窗口 1180×800 下**已无溢出**（表 894 / 容器 1004，余量 110px）；
    但最小窗口 **900×600 下仍溢出 77px**（表 801 / 容器 724），由 Table 基座横向内滚承载。
  - **已解决（2026-09-17，按用户拍板「隐藏列 + 信息降级」）**：
    1. `<1024px` 隐藏「模态（入/出）」列（`hidden lg:table-cell`），该信息降级为「模型名」列内的
       紧凑 badge —— 只显示非文本模态的单字缩写（图/音/视），完整集合放 `title`，
       入/出均为纯文本时不显示（默认情形，显示等于逐行噪音）；
    2. 三个行内输入窄窗收窄：上游模型 ID `w-32→24`、上下文 `w-28→20`、最大输出 `w-24→20`，
       补齐隐藏单列后仍差的宽度；
    3. 窄窗提示文案同步更新（模态已并入模型名列，不再是隐藏列之一）。
  - 关键教训（第一版 badge 走了弯路）：降级 badge 的**文案长度直接决定列宽**——
    首版写「模态 文本/图像→文本」把模型名列撑到 302px，隐藏整列省下的 112px 全被吃回去，
    表宽反而从 801 涨到 804。改成单字缩写后表宽 741，再配合输入收窄降到 **674**。
  - 验证（新增探针 `probe-models-cols.mjs`）：900×600 表宽 **674 = 容器 674（不再溢出）**，
    模态列 `display: none`、badge 文案「图」+ 完整 `title`；1180×800 下 7 列齐全
    （模态列 104px、编辑器可用）、badge `display: none`。`fold.mjs` 双尺寸横向溢出均为 0。
- [x] 6. P1 日志页 2931px 长表：`加载更多` 在 y=2870，表头无 sticky
- [x] 7. P2 暗色主题主按钮对比度 2.59:1（`--primary` #00A6F4 + `--primary-foreground` #FAFAFA），AA 要求 4.5:1；浅色主题同按钮 5.03:1 合格
- [x] 8. P2 浅色主题小字/状态色：600→700 级 + 日志状态码 700/800 + 提示条/toast 改前景色；79 类 → 剩余约 2 类（设置页主按钮 4.27 待复核）。原 79 类：badge「缺少凭据」3.2、日志状态码「429/499」3.38、同步「成功」3.55、设置说明 3.65、toast 文案 4.26
  - 已解决（2026-09-17）：**双主题对比度不达标项均清零**（`audit.mjs` 分组结果浅色 0 组 / 暗色 0 组）。
    修前浅色实为 **4 个根因**（不是一个），逐条处置：
    1. **日志表 12px muted 小字**（51 + 1 个元素，最差 4.24）：`--muted-foreground` 0.552 → **0.52**；
    2. **日志 `errorKind` 列硬编码 `text-red-600`**（3 个元素，4.24）：改 `text-red-700`，
      与既有「状态码 600→700」惯例一致（`ProvidersPage` 同色 badge 一并改）；
    3. 原注记的「设置页主按钮 4.27」**复核结论：是 hover 态**——shadcn 默认 `hover:bg-primary/90`
       把主色冲淡（5.03 → 4.27）。新增 `--primary-hover` token（浅色加深 / 暗色提亮），
       `button.tsx` 与 `badge.tsx` 的 default 变体改用之 → hover 态 6.16（浅）/ 8.23（暗）；
    4. 同上的弹窗内「添加」按钮，同一根因、同一修法。
  - **顺带发现并修掉一个真实缺陷（登记为 §1 第 18 条）**：暗色主题下 toast 是「近白字 + 浅绿底」
    = **1.01**，成功提示完全不可读——根因是 `<Toaster>` 挂在 `<ThemeProvider>` 外面。
  - 方法学副产品：`audit.mjs` 原先只在页面加载后 `classList.add("dark")`，next-themes 已初始化完毕，
    导致「应用暗色 + 组件库浅色」的错配（这是上面那个 toast 缺陷暴露出来的原因）。
    已改为**页面加载前**写入 `localStorage.theme`，使暗色审计结果可信。
- [x] 9. P2 命中区：Switch 已用伪元素扩到约 44×36、弹窗关闭 16→24；行内复制/勾选/官网链接待做。原文：Switch 32×18、弹窗关闭 16×16、行内复制 16×16、批量勾选 16×16、弹窗内协议 `select` 1×1（几乎不可点）
  - 已解决（2026-09-17）：三处真实小命中区全部扩到 **WCAG 2.2 AA（≥24×24）**，用与 Switch 相同的
    「视觉不变、命中区外扩」思路（避免撑开表格行高/行内间距）：

    | 位置 | 视觉 | 有效命中区 | 手法 |
    |---|---|---|---|
    | 模型页「复制模型名」 | 16×16 | **30×30** | `::after` + `-inset-2` |
    | 技能页批量勾选框 | 16×16 | **26×26** | `<label>` 包裹 + `p-1.5` / `-m-1.5`（`<input>` 是替换元素，`::after` 不生效） |
    | 供应商页「官网」链接 | 40×16 | **54×26** | `::after` + `-inset-x-2 -inset-y-1.5` |

  - **新增探针 `probe-hits.mjs`**：审计按 `getBoundingClientRect` 测量，**看不到**伪元素与 label 扩展，
    故原报告会永远显示 16×16。新探针用 `elementFromPoint` 从中心向四周逐点探测，输出**有效命中区**。
    两个坑：① 判据必须只认「元素本身 / 其后代 / 包裹它的 label」，**不能**把祖先算命中
    （否则父 `td`/`span` 都算命中，整行都算可达，数值虚高）；② 从中心采样，有效尺寸 = `2×可达半径`
    （写成 `rect + 2×reach` 会把尺寸算大）。
  - **假阳性澄清**：原注记的「弹窗内协议 `select` 1×1（几乎不可点）」是 Radix Select 的
    **无障碍隐藏代理**（`aria-hidden="true"`、`tabindex="-1"` + visually-hidden 样式：1×1、`clip: rect(0,0,0,0)`），
    真实触发按钮 223×36、完全正常——不是缺陷，无需修改。

- [x] 10. P2 字号：2 处 10px → 11px；URL 截断待加 title。原文：2 处 10px 文本；MCP 注册 URL 截 22px、供应商 base URL 截 21px
  - 已解决（2026-09-17）：`text-[10px]` 全局已清零（复核 0 处）；3 处截断补 `title`（hover 可读全量）：
    `McpPage` 的注册路径/URL（310px 容器放不下，实测被截 16 次）、`ProvidersPage` 的 base URL（截 8 次）。
  - **探针修复（关键）**：这一项此前一直无法被追踪，因为 `audit.mjs` 里 `res.truncated` **只初始化、从不 push**
    ——截断检测是死代码、字段恒为空。本次补上实现（`text-overflow: ellipsis` 或 `.truncate`
    且 `scrollWidth > clientWidth` 且**无 `title`** 才算问题），修后重跑 → **0 条**。

- [x] 11. P3 折叠线硬切与顶部无渐隐：卡片被切半（`客户端接入` 398–660、WebDAV 348–1617）、内容滚动时被标题栏切开
  - 【2026-09-17 复核】现状（`fold.mjs --size=1180x800`）：折叠线「切一半」仅剩 2 处，且都是**滚动中间态**
    的固有现象（任何滚动位置都会切到某个元素）：网关页 MCP 卡片（745–1174，卡片跨线）、
    同步页 `INPUT` 切 22px。原报告点名的「`客户端接入` 398–660 / WebDAV 348–1617 卡片被切半」
    已随窗口尺寸与吸顶条整改消失。
  - **已解决（2026-09-17，按用户拍板「现在就做」）**：新增顶部渐隐遮罩。
    实现踩了两次坑，都记在代码注释里：
    1. **sticky 方案贴不到裁切边**：滚动容器的 `padding` 也在可滚动区内，内容滚上来后被裁切的
       是 main 的**边框盒**顶边；用 `sticky top-0`（含 `-mt-6` 修正）实测恒定停在 24px 处，
       会露出未渐隐的硬边。改为「包裹 `<main>` + 绝对定位覆盖层」→ 偏移 **0**，精确贴边。
    2. **`pointer-events` 必须显式 none**：首版漏了，那条 24px 透明带会吞掉顶部区域的点击
       —— 与历史 P0-3「toast 遮挡导致点不到」是同一类问题；`elementFromPoint` 复核确认穿透。
  - 三个约束（避免修复引入新问题）：包裹层**不加 z-index**（保持与 main 内吸顶条同层叠上下文，
    吸顶条 z-10 盖住遮罩 z-5，按钮文字不被渐变冲淡）；`absolute` 不占布局高度
    （网关页内容高 1162px 修前修后一致，折叠线数据未变）；`aria-hidden` 不污染可访问性树。
  - 验证（新增探针 `probe-fade.mjs`）：三个代表性页面（技能/网关/日志）渐变均渲染、
    贴边偏移 0、命中测试穿透、`main.scrollHeight` 不变；网关页吸顶条按钮 `hitIsButton=true`。
- [x] 12. P3 默认窗口偏小：内容普遍需要 1100–1900px 高，建议默认 1080×740、`minHeight` 520→560
  - 已解决（v0.2.0，commit `90acc59`）：默认窗口 **980×640 → 1180×800**，最小 **760×520 → 900×600**。
    取 1180×800 而非建议的 1080×740，是为让首屏同时容纳左栏 + 双列内容（网关页/MCP 页在 1080 宽下仍会把
    操作按钮挤到折叠线下）。复核：`fold.mjs --size=1180x800` 全页 `belowFold=0`（设置页仍有 41% 需滚动，
    属 §3 第 4 条 P1 待办，与窗口尺寸无关）。
