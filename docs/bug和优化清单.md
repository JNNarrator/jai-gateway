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

- [x] 20. 自动拉取的时间节奏被自动推送的间隔绑住（2026-09-20 修复）
  - **先修正原条目的判断**：原文说「若自动推送设 360 分钟、自动拉取设 30 分钟，则拉取实际也是
    360 分钟一次」。核对代码后确认：**这个场景当时根本无法配置出来** —— 全项目只有一个间隔字段
    `auto_push_interval_min`（`WebDavConfig`），`current_autopush_interval` 与
    `current_autopull_interval` 都读它，同步页也只有一个「定时间隔」选择器。
    所以 `push_interval.or(pull_interval)` 两边恒等，`or` 取哪个都一样 ——
    原文点名的 `or` 是**潜在隐患**（一旦有人把两个间隔拆开就会静默踩中），不是当时的活跃缺陷。
  - **但症状是真的，根因是另一个（且当时可复现）**：`spawn_autopush` 每轮循环都**新建**
    `tokio::time::sleep(wait)`，`rx.changed()` 一到就在 `select!` 里丢弃它并重建 ——
    即「任何一次变更唤醒都会把定时倒计时重置」。而 `notify_change()` 在
    **15 处**调用（供应商增删改、模型改别名/限额/模态、MCP、技能、网关 Key 重生成、
    导入、快照恢复……），全是日常编辑动作。
    ⇒ 只要用户**编辑得比间隔勤**（间隔 30/60 分钟时很常见，360 分钟几乎必然），
    定时 tick 永远等不到，**自动拉取被饿死**，症状正是原条目写的「用户以为自动拉取没生效」。
    连带自动推送的定时分支也一起失效（只剩变更触发）。
  - 处置（一次做完，两件事同源）：
    1. **拆出独立的拉取间隔**（本次按用户拍板加）：`WebDavConfig` 新增
       `auto_pull_interval_min` + `normalized_pull_interval()`，meta 键
       `webdav_auto_pull_interval_min`；`normalized_interval()` 更名 `normalized_push_interval()`
       （消除「一个名字管两边」的歧义）。同步页从「一个共用间隔」改为**推送/拉取各一个**选择器。
       新增键**必须**进 `MACHINE_LOCAL_META_KEYS`（本机调度偏好，双向不参与同步）——
       漏加就是 bug 19 的同类问题（A 机间隔静默改写 B 机），已在常量注释里写明这条纪律。
    2. **重写调度**：抽成纯函数 `plan_sync` / `next_wake` + `ActionClock`
       （记录「上次实际执行时刻」与「启用起点」，单调时钟 `Instant`）。
       到期点 = `anchor + interval`，**只有真正执行过才会往后推** ——
       变更唤醒、配置重读都不再重置它，饿死路径从结构上消失。
       定时唤醒点取**最近的一个到期点**（不是 `or` 取其一，也不是各间隔的最小值）：
       既修掉拉取被长间隔绑死，也修掉旧代码「开了自动推送就每 tick 都推」
       （那会让 360 分钟的推送被 30 分钟的拉取拽成 30 分钟一次，反复覆盖远端）。
       挂钟毫秒（`at_ms`）**只用于界面展示**，不参与判定：系统时间被 NTP 校正或手改
       不该让同步饿死或瞬间风暴（这是没照抄「直接用 `at_ms`」建议的原因）。
       顺带把两处重复的结果记录/通知收敛成 `record_auto_sync`，配置读取由「每轮两次」
       并为一次（避免两次读之间配置被改而拿到半个旧配置）。
    3. **自查改调度时自己引入的两个风险**（各配单测）：
       ① **拿不到锁时的空转**：改成「睡到到期点」后，若动作已到期而手动推/拉正持锁，
       `next_wake` 返回 `Some(ZERO)` → `sleep(0)` 立即返回 → 又拿不到锁 → `continue`，
       变成空转 + 反复打日志（**旧实现每轮固定睡满一个间隔，没有这个问题** ——
       是本次改成「睡到到期点」才引入的）。新增 `AUTOSYNC_LOCK_BACKOFF`（5s）显式退避。
       ② **配置改动生效滞后**：`webdav_config_set` **不**触发变更通知（它改的是调度参数本身，
       不是要同步的业务数据），若睡满一个长间隔（推送设 6 小时），用户刚把「拉取间隔」
       改成 30 分钟也要等最长 6 小时才生效，表现为「改了没反应」。
       新增 `AUTOSYNC_MAX_SLEEP`（60s）封顶 —— 只是「醒来重新评估配置」，
       **不会提前执行动作**（是否该跑由 `plan_sync` 按绝对到期点判定，与睡眠时长无关）。
       两条都有单测：`sleep_duration_caps_long_waits`（去掉 `.min()` 即变红）、
       `schedule_round_trip_for_360_push_and_30_pull`（360/30 组合串一遍决策与锚定）。
    4. **对抗性代码审查（`code-reviewer` 子代理）后补掉的两个真缺陷**：
       ① **瞬时读库失败被当成「用户关掉了」**：`current_autosync_intervals` 原先把
       「读配置失败」与「读到了但未启用」都返回 `(None, None)`，调用方据此
       `sync_enabled(false, _)` → **清空锚点** → 恢复后重新计时，
       把下一次执行整体推迟一个完整间隔（最长 6 小时）。触发源是偶发 IO 错误
       （SQLite busy、磁盘抖动、`spawn_blocking` join 失败），概率低但后果正是本条目
       要消灭的症状。改为返回 `Option<...>`：`None` = 读失败 → **保持时钟不动**、
       睡 `AUTOSYNC_CONFIG_POLL` 后重试；`Some((None, None))` = 确定未启用。
       ② **「关掉再打开」可能整段被漏采样**：`webdav_config_set` 原先不通知调度循环，
       状态跃迁只能靠睡眠到期才发现 —— 短窗口内的 off→on 会让
       `sync_enabled(false)` 从未执行，锚点残留，重新启用可能立刻同步一次
       （与 `re_enable_restarts_the_interval` 的语义相悖）。
       新增**独立的**配置变更信号 `AutopushHub::cfg_tx` + `notify_config_change()`：
       调度循环 `select!` 多一路 `Wake::Config`，只回到循环顶部重读配置，
       **不推送、不走防抖**（若复用 `tx` 会变成「改个间隔就顺带推一次远端」）。
       顺带让「改间隔」立刻生效，`AUTOSYNC_MAX_SLEEP` 退化为纯安全网。
       配套单测 `config_change_signal_is_independent_from_data_change`
       （两个通道任一方向被合并即变红）。
  - 验证：`src-tauri/src/main.rs` 新增 `mod autosync_schedule_tests`（12 个用例，
    **可控时钟**：全部用「基准时刻 + 偏移」构造 now，不依赖真实时间流逝，不会 flaky）。
    关键几条：`pull_is_not_throttled_by_push_interval`（360/30 → 唤醒点取 30，拉取到期而推送不到期）、
    `push_is_not_over_triggered_by_pull_ticks`（30…330 分钟只拉不推，第 360 分钟才推）、
    `change_wakes_do_not_push_back_the_deadline`（第 m 分钟变更唤醒后剩余时长必须是 `30-m`
    而不是恒为 30 —— 这正是饿死回归）、`re_enable_restarts_the_interval`、
    `interval_change_reanchors_from_last_run`、`never_run_action_fires_after_one_full_interval`
    （保住「刚启用不立刻同步」的既有语义）。
    `sync.rs` 新增 2 条（独立归一化 + 缺键反序列化与 `Default` 一致）；
    `m7_import_webdav.rs` 的 bug 19 守门人测试扩到新键：B 机推送间隔 360 / 拉取间隔 30，
    A 机 payload 里**注入** `webdav_auto_pull_interval_min=360`，
    断言拉取后 B 仍是 30（导入侧若退化就精确变红）。
    写测试时自己踩了一次坑并已修正：`mark_run(now)` 之后再断言「立刻到期」是错的 ——
    刚跑完应当剩一整个间隔；把期望值写对后两条用例才真正测到「从上次执行起算」。
    全量回归 **380 通过 / 0 失败 / 2 ignored**（原 366，+14 = 调度器 12 + sync.rs 2），
    四段门禁全绿（`cargo fmt --check` / `clippy -D warnings` / `cargo test --workspace` /
    前端 `tsc --noEmit + vite build`）。
  - 影响面：修前仅调度节奏受影响、不影响数据正确性（`should_pull` 的 last-write-wins
    仍会拦住重复导入；推送侧有「空配置不覆盖远端」护栏）。修后两个节奏各自独立可控。
  - 用户须知：**两个间隔都从「上次成功执行完成」起算**（固定延迟：远端慢/卡导致一次同步耗时超过间隔时，
    不会背靠背连跑）；刚启用或刚关掉再打开，都重新等一个完整间隔（避免打开瞬间立刻同步一次）。
    缩短间隔**即时生效**（按上次执行时刻重算，若已超过新间隔则立即到期）。

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

- [x] 23. **发布后 updater 通道仍推上一版**（v0.2.6 发布时实测踩到）
  - 现象：v0.2.6 的 Release 已由草稿转正式，但应用轮询的
    `releases/latest/download/latest.json` 仍返回 **0.2.5** —— **全程无任何报错**，
    表现为「新版本发了但没人收到更新」。
  - 根因（两层）：
    1. `release.yml` 建草稿时硬编码 `-F prerelease=true`，而 GitHub 的 `/releases/latest`
       **不含 prerelease** → feed 指针停在上一版；草稿态本已由 `draft=true` 表达，
       该标志纯属冗余且有害；
    2. 仅翻 `prerelease=false` 后 `latest` 指针仍可能停在上一版（实测需 `--latest` 显式置位）。
  - 修复（2026-09-18）：`release.yml` 改为 `-F prerelease=false`；`docs/design/release.md` §5 新增
    第 7 步「发布后必须校验 updater 通道」（给出 curl 校验与 `--prerelease=false --latest` 兜底命令）。
  - 本次事故的现场处置：对 v0.2.6 执行 `gh release edit v0.2.6 --prerelease=false --latest`，
    复验 feed 返回 **0.2.6**、5 个平台条目签名齐全（darwin-aarch64/app、windows-x86_64{,-msi,-nsis}），
    且 macOS 更新包 `JAI_aarch64.app.tar.gz` 可 200 下载。
  - 教训：`scripts/release_check.sh` 覆盖不到这一环（它只看仓库内状态），
    「发布是否真的可达」只能在发布后从**公网端点**验。

- [x] 24. **网关自己发明的工具数上限（128）把上游能跑的请求拦成 400**
  - 现象：zcode 经 JAI 发 140 个工具声明 →
    `400 provider_code=tools_limit_exceeded 工具声明数 140 超过上游上限 128`，
    整个 turn 失败（`reason=invalid_request`）。
  - 根因：`codec/capability.rs` 的族级能力表把 `max_tools` 定死 128
    （`DEFAULT_MAX_TOOLS`，四族同值，出自 protocol-ir 能力对齐表的经验值），
    `plan_tools` 超限直接 `Rejected`。**实测上游基元律动接受 140 与 300 个工具均 200**
    —— 这条限制是**网关自己的臆测**，误杀了上游完全能跑的配置。
  - 教训（与 bug 21 同源）：网关不该**发明**上游限制，只能**执行上游声明的**限制。
    族级能力表只该回答「协议本身支不支持」，不是「这家上游具体怎么校验」。
  - 修复（2026-09-18，迁移 0012）：
    1. 四族 `max_tools` 一律 `None`（删掉 `DEFAULT_MAX_TOOLS`）⇒ **未声明即不拦**，
       放行由上游裁决（上游真报错时其错误原文照常回给客户端，链路不再被网关截断）；
    2. 新增 `providers/models.max_tools` 渠道声明（模型级覆盖供应商级，`NULL`/0 = 未声明）；
       声明了超限仍 400，文案含实际个数与上限并提示可调整；
    3. 规划层新增 `ChannelPolicy`（0011 的 effort + 0012 的 max_tools 统一为「渠道覆盖」），
       `plan_compatibility_with(req, caps, Option<&ChannelPolicy>)`；
    4. **同族直通路径同样生效**（`capability::body_tool_count` 数 `tools` 数组），
       避免「声明了上限却被直通路径绕过」。
  - 回归：`capability.rs` 单测 `max_tools_only_enforced_when_channel_declares_it`
    （未声明放行 / 声明 128 拦 / 声明 300 放行）、`body_tool_count_reads_tools_array`；
    `m9_capability.rs` 改 `too_many_tools_rejected` → `declared_tool_cap_rejects_overflow`，
    新增 `undeclared_tool_cap_lets_140_tools_through`（bug 复刻：140 个应完整透传）、
    `declared_tool_cap_applies_on_passthrough_path_too`。
  - 真机复验：上游直连 140 / 300 个工具均 200；修复后经 JAI 的 140 工具请求 200 且上游实收 140 个。
  - UI：供应商卡片「工具上限 …」+ 模型表「≤N / 上限?」芯片（留空 = 不拦）。


- [x] 25. **Responses 流式渲染的 item id / 状态错乱：zcode 每轮都「Model request failed.」**
  - 现象：zcode 经 JAI（`openai-responses` 入站）**每次**请求都失败，客户端已收到
    `200 + text/event-stream`（`x-jai-mode: converted`），随后在流里拿到 error chunk，
    报 `reason=unknown retryable=false`（**无 HTTP 状态**）。而 JAI 的 `request_logs`
    记的是 `200 + tool_calls=N`（看起来成功）→ 极易误判成「上游抖动」。
  - 真因（从客户端 cause 链坐实，而非猜测）：
    ```
    cause: UnknownError: text part msg_resp_jai_942 not found
    context: errorPhase=stream source=provider transport=sse
    ```
    即 AI SDK 收到引用某 item 的帧，但该 item 从未登记（无 `output_item.added` /
    `content_part.added`）。`codec/responses.rs` 流式渲染状态机有两处错：
    1. 「文本 item 已开」的标志（`item_started`）被 `ToolCallStart` 复用置 true。于是
       **只调用工具、没有文本**的回复（agent 主 turn 的常态，实测 out_tok=178/tc=2）
       在收尾时，`Finish` 分支会为一个从未登记的 `msg_*` 发出 `output_text.done` /
       `content_part.done` / `output_item.done` → 客户端查不到该 text part → 整轮失败。
       错误发生在**流尾**，与线上时间戳完全吻合。
    2. `ToolCallStart` 登记的 item id 是 `fc_{index}_{call_id}`，但
       `ToolCallArgsDelta` / `ToolCallEnd` 写死 `fc_{index}_pending` ⇒ 参数增量引用
       不存在的 item（同一 bug 的另一半，严格客户端同样会炸）。
  - 教训：**SSE 是协议，不是「看起来像就行」**。凡是「按 id 索引的增量流」，登记帧
    与增量帧的 id、顺序都是硬契约；用单标志表示多种 item 的状态，早晚会串。
    「只调用工具、没有文本」是最常见形态，不该靠客户端宽容才不失败。
  - 修复：
    1. 状态拆开：`msg_started`（**只由文本路径读写**）+ `active_tool_item_id`（登记工具
       item id），`Start` 一并重置；`ToolCallEnd` 清空；
    2. 文本增量只据 `msg_started` 决定是否补发 `output_item.added` + `content_part.added`；
    3. 参数增量/结束帧复用登记的 id，`item.call_id` 从 id 反解（不再退化成 `call_{index}`）；
    4. 纯工具调用回复不再发 `msg_*` 收尾帧。
  - 回归：`codec::responses` 新增 3 条单测——
    `render_stream_tool_only_emits_no_bogus_message_frames`（**最精确复刻**：纯工具调用不得
    出现 `msg_*` 伪帧、不得有 `_pending`）、`render_stream_tool_then_text_keeps_item_ids_consistent`、
    `render_stream_text_then_tool_keeps_ids`。已做**反证**：把旧行为加回去，前两条立刻变红。
  - 真机复验：以 zcode 原始请求体重放 + 构造纯工具/混排请求，逐流跑 id 一致性校验脚本全过；
    纯工具流里 `msg_*` 伪帧数为 0、参数增量 id 与登记 id 一致。
  - 排查提示：**遇到「客户端失败、JAI 记 200 时，不要看 JAI 日志下结论**，要看客户端
    的 cause 链（zcode：`~/.zcode/cli/log/zcode-*.jsonl` 的 `turn.failed` → `error.cause.cause`）。
  - **同族第三处（同日续修）**：修完上面两处后 zcode 能拿到回复了，但工具调用报
    `fault.runtime.toolLifecycleIncomplete`「Tool call ended without a terminal event.」
    —— 因为 **openai 族上游没有「工具调用结束」事件**（`openai.rs`：`Ev::ToolCallEnd => None`），
    而 `Finish` 分支只关 reasoning / 文本 item，**不关还开着的工具 item**，于是每条工具流都是
    `output_item.added → …delta… → response.completed`，永远没有终结帧；严格客户端只认
    `output_item.done` 来终结工具调用，缺了就把工具行标成"未终结"。
    另：终结 item 早期还把 `name`/`arguments` 传成空串（客户端靠它构造 tool-call）。
    修复：抽出 `close_tool_call()`，在 `ToolCallEnd`、**`Finish`**、以及新 item 开始前（文本/工具）
    都补发 `function_call_arguments.done`（带完整 arguments）+ `output_item.done`（完整最终 item：
    id/call_id/name/arguments），并推进 `output_index` 防撞号。
    回归：`render_stream_finish_closes_open_tool_call`、
    `render_stream_closes_every_open_tool_call`（连续两个工具调用、上游始终不给结束事件）；
    真机逐流校验「每个 added 必有对应 done 且带 name/arguments」全过。
  - **同族第四处（真正的拦路虎，同日定位）**：补上终结帧后 zcode 仍报同一句话，于是搭了
    **本地 SDK 测试台**（用真实 `ai` + `@ai-sdk/openai` 回放 JAI 实际发出的 SSE，
    见 `scripts/verify_responses_with_ai_sdk.mjs`），一次就拿到铁证：
    ```
    _TypeValidationError: Type validation failed:
      {"item":{...,"id":"fc_0_call_00_…","name":"Bash","type":"function_call"},"type":"response.output_item.done"}
    —— invalid_union ...（所有变体都不匹配）
    ```
    读 SDK 的 zod schema 后确认：**`response.output_item.done` 里 function_call 变体的
    `status` 是必填**（`z.enum(["in_progress","completed","incomplete"])`，无 `.nullish()`），
    而 JAI 从没发过 `status` → 该帧整体校验失败被**静默丢弃** → SDK 永不产生
    `tool-input-end`/`tool-call`（日志里 `chunkCounts` 只有 `tool-input-start`/`tool-input-delta`）
    → 工具行永远关不掉。同期还对齐了其它 item 形状：`custom_tool_call.input` 必须是**字符串**
    （JAI 原来发对象）、`shell_call`/`apply_patch_call` 的 `status` 同样必填、
    `apply_patch_call.operation` 必须是对象（原来发字符串）。
  - 教训：**"看起来像"不等于"协议合规"**。客户端用 zod 逐事件校验时，缺一个必填字段就是整帧丢弃，
    而且**丢弃是静默的**（客户端不报错、JAI 记 200）——只有拿真实客户端/SDK 回放才能发现。
    这类"形状契约"必须用真实 SDK 验证，不能只靠自家单测断言"我发了什么"。
  - 回归：`render_stream_tool_items_carry_status`（added=in_progress / done=completed）；
    真机：用 `scripts/verify_responses_with_ai_sdk.mjs` 消费修复后的真实流 →
    `tool-input-end: 2`、`tool-call: 2`、name/input 正确、无 TypeValidationError。

- [x] 26. **推理内容下发用了「非 modeled 事件」，客户端拿不到 → 上游要求回传 reasoning_content 时 400**
  - 现象：zcode 报 `Provider rejected the model request.`，JAI 侧只是透传上游 400
    （`{"code":"LITELLM_ERROR","message":"The `reasoning_content` in the thinking mode must be
    passed back to the API."}`）。
  - 根因：JAI 用 `response.reasoning_text.delta` 下发思考内容，而严格客户端
    （zcode 内 AI SDK）的 modeled chunk 列表里**没有**这个事件（只有
    `response.reasoning_summary_part.added/done` 与 `response.reasoning_summary_text.delta`）
    → 推理文本被**静默丢弃** → 客户端回传历史时不可能带上 reasoning → thinking 模式上游拒绝。
    与 bug 25 同源：**事件名/形状必须落在客户端的 modeled 集合内**，否则等于没发。
  - 修复：改用 modeled 事件（`reasoning_summary_part.added` → 每个 delta
    `reasoning_summary_text.delta` → 收尾 `reasoning_summary_part.done`），reasoning item 的
    `summary` 带上文本（流式 `output_item.done` + 非流式 body；`content` 保留兼容既有客户端）。
  - 验证：真实 SDK 回放 → 客户端捕获推理文本（修前 0 字符，修后 1011 字符）；
    链路测试断言回传的 reasoning item 变成上游 assistant 消息的 `reasoning_content`+`tool_calls`。
  - 附注：用户失败请求体原样重放两次都 200 → 上游**非确定性**（部分后端强制校验），
    故"客户端能拿到并回传"是唯一可靠解法。
  - 待办（未做）：`discover.rs`「发现模型」的 `/v1/models` 调用仍是单次发送，上游抖动即失败
    （用户同日遇到 `请求失败: error sending request for url (https://tokenrhythm.studio/v1/models)`）；
    计划加传输层重试（只重试发不出去的错误，不重试 HTTP 错误码）。

- [x] 27. **`m4_conversion` 的 flood 护栏回归随机失败，会打红 CI**（bug 25 修复过程暴露）
  - 现象：`cargo test --workspace` 偶发 `conversion_disconnects_on_newline_flood_upstream`
    失败（实测 **8 轮 4 红**），但**单跑该文件恒绿** —— 典型负载敏感 flake。
    断言期望「已断开」护栏文案，实际拿到 `stream aborted by upstream: ...`，
    看起来像上游流中断或护栏失效，**方向极具误导性**。
  - 根因（分诊取证，非推测）：**mock handler 不消费请求体** → hyper 在
    `poll_drain_or_close_read` 走 `_ => self.close_read()`（`hyper-1.11.0/src/proto/h1/conn.rs:849`）
    → 服务端 close 时接收缓冲区仍有客户端发来的未读字节 → 内核发 **RST 而非 FIN**
    → **客户端接收缓冲区里已到达但应用尚未读走的数据被一并丢弃**。
  - 证据链（三步排除法，每步实测）：
    1. mock 侧显式打点确认 1056774 字节**全部写完** → 排除「上游没发完」；
    2. 网关侧 `buf_len` 每次都不同（685204 / 751158 / 908054 / 924126 / 997438），
       而 `content_length` 声明值一直是正确的 1056774；错误链为
       `error decoding response body <- ... unexpected EOF during chunk size line`
       → 排除「护栏逻辑错」，实为部分丢失；
    3. **单变量验证**：只让 handler 消费请求体（`_req_body: axum::body::Bytes`）
       → 连跑 6 轮全绿。
  - 负载相关性由此解释：机器越忙 → 客户端读得越慢 → 接收缓冲区积压越多 → 被 RST 丢得越多。
  - 修复：**生产代码零改动**（真实上游会读请求体，这是测试夹具缺陷）。两个 mock 都补
    `_req_body`，根因写进 `m4_conversion.rs` 注释防复发。
  - 教训：①「单跑绿、全量红」要优先怀疑**并发/时序/资源竞争**，而不是被测逻辑；
    ② 断言里带**完整响应体**（而非截断 200 字符）是本次能一次定位的关键——
    截断把真正的错误帧藏在了视野外；③ 分诊打点要打在**两侧**（mock 发完 / 网关读了多少），
    单侧打点无法区分"没发"与"没收到"。
  - 附带：同批 bug 25 的 4 个回归测试有 `unused_mut`（clippy `-D warnings` 门禁失败）
    ——该批改动当初未过 clippy 就提交了，`release_check.sh` 首次运行即暴露。

- [x] 28. **视觉回归探针的「无控制台报错」断言长期恒假**（2026-09-20 修复，做 bug 20 时发现）
  - 现象：新增 `sync-intervals.mjs` 后跑 1180×800 与 900×600，**功能断言全绿**，
    但末尾「无控制台报错」恒红：`pageerror: Cannot read properties of undefined
    (reading 'unregisterListener')`。拿既有探针做对照 —— `mcp-switches.mjs` **一模一样地红**。
  - 根因：`@tauri-apps/api` 的 `_unlisten()` 直接读
    `window.__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener`（真实运行由 Tauri 注入），
    而探针 mock 只装了 `__TAURI_INTERNALS__` / `__TAURI__`。
    `TitleBar` 的 `win.onResized()`（自绘标题栏监听窗口尺寸）在每个页面都会注册并在
    cleanup 时 unlisten → 于是**每个页面**都抛该 pageerror。
  - 后果（比报错本身严重）：三个断言「无控制台报错」的探针
    （`mcp-switches` / `gateway-endpoints` / `sync-intervals`）**永远不可能变绿**，
    等于这条断言从未生效 —— 真正的控制台错误会被这条已知噪音淹没。
    这属于「一直红的断言比没有断言更危险」：它训练人忽略红色。
  - 修复：四个 mock（`mcp-switches` / `gateway-endpoints` / `sync-intervals` / `run.mjs`）
    补上 `window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener(_e, id) { cb.delete(id); } }`。
    修后三者全绿（`sync-intervals` 1180×800 与 900×600 **全部通过**、
    `mcp-switches` 双尺寸全部通过、`gateway-endpoints` 17/17）。
    `audit.mjs` 的 `page errors` 输出行也随之为空。
  - **同批发现的第二个坑（更隐蔽）**：`audit.mjs` / `deep.mjs` / `deep2.mjs` / `fold.mjs` /
    `probe-*.mjs` 共 **12 个**探针，是从 `path.resolve(".vr/run.mjs")` 读 `installMock` 的
    —— 即读 `.vr/` 下那份**未跟踪的本地镜像**，不是仓库里 `tools/visual-regression/run.mjs`。
    `audit.mjs` 的注释写「复用 run.mjs 里的 invoke mock（避免两份实现漂移）」，
    但实际读的是本地副本，**恰恰会漂移**：我改了仓库里那份后重跑 audit，
    pageerror 依旧 —— 因为它用的还是 9/16 的旧 mock。
    **已彻底修掉**（2026-09-21）：12 处全部改为**相对本脚本解析** ——
    `fs.readFileSync(new URL("./run.mjs", import.meta.url), "utf8")`（`audit.mjs` 读 `./audit.mjs`），
    与 `fixtures.mjs` 一直以来的写法一致：不依赖 cwd，也不再受镜像漂移影响。
    同时删掉了 8 个探针里因此变成死代码的 `import path from "node:path"`。
    验证：`audit` light/dark 的 `page errors` 行**消失**（此前每次必现），
    `fold` / `deep2` / `probe-hits` / `probe-fade` / `probe-sticky` / `probe-models-cols` /
    `probe-colors3` / `probe-toast2` 全部正常跑出结果 —— 证明换路径后 mock 确实来自仓库内那份。
  - 教训：探针的 mock 必须与「真实注入的全局对象集合」对齐；
    缺一个全局对象就会让**断言整体失效**，而失效方式是「一直红」，最容易被当成噪音。

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

- [x] 7. 网关页缺「完整请求地址」：用户复制 Base URL 填进要求精确地址的客户端会 404（Reasonix 实测踩到）
  - 现象：Reasonix 报 `JAI · Responses: Request endpoint not found (HTTP 404). Check the API format and request address.`
    该文案**不是 JAI 产生的**（全仓库 grep 无此字符串），是 Reasonix 自己的报错模板，
    `JAI · Responses` 只是「连接显示名 · API 格式」标签，不是 JAI 的响应内容。
  - 根因（客户端侧）：Reasonix 桌面版「自定义供应商」表单把 **API 地址**当**精确请求地址**存进 `request_url`，
    官方文档原文 *“Reasonix does not append or rewrite its path”*。用户填了网关页展示的 Base URL
    `http://127.0.0.1:1314/v1`，请求就打到 `/v1` 上 —— JAI 无该路由 → 404。
    实测对照：`POST /v1/responses` → 200；`POST /v1` → 404。
  - 已解决（2026-09-20）：网关页「客户端接入」卡片**保留** Base URL 复制字段，另增「完整请求地址」区块
    （5 条：Chat Completions / Anthropic Messages / Responses / 模型列表 / MCP 元数据，每条独立复制按钮）；
    顶部常驻操作条加「复制完整地址」，一键复制接入清单（Base URL + API Key + 全部完整地址）；
    卡片文案点明「精确请求地址（不会再补路径）」与填错会 404。
  - 验证：新增探针 `tools/visual-regression/gateway-endpoints.mjs`（17 项断言：Base URL 复制功能仍在且内容正确 /
    完整地址区块存在且恰好 5 条 / 每条都是完整端点而非裸 `/v1` / 端点集合与顺序一致 /
    逐条点复制图标后**剪贴板内容逐条相符** / 一键复制含 Base URL + API Key + 全部完整地址 /
    文案点出「精确请求地址」与「404」/ 无横向溢出且地址行内部无溢出 / 无控制台报错），
    1180×800 与 900×600 双尺寸 **17/17 全绿**；`tsc --noEmit` 与 `vite build` 通过；
    `fold.mjs --size=1180x800` 复核网关页 `pageHScroll 0 / mainHScroll 0 / clippedTextCount 0`
    （新增的 4 个行内复制按钮落在折叠线以下，属既有「客户端接入卡片较长」范畴，主场景已由常驻条按钮覆盖）。
  - 附带结论（**未改 JAI**）：JAI 的 `/v1/responses` 本身可用 —— 带 tools / streaming / reasoning 的 Agent 载荷
    实测 200，SSE 事件链完整（`response.created` → `output_item.added` → `reasoning_summary_text.delta` →
    `function_call_arguments.done` → `output_item.done` → `response.completed`），
    且 dsh 就是走 `openai-responses` 线稳定在用。若日后仍要 JAI 侧兜底，可加 `/responses`（不带 `/v1`）别名路由；
    **不建议**把裸 `/v1` 直接映射成 Responses —— 会掩盖客户端的配置错误，反而更难排查。
  - 同批发现的 3 个 JAI 侧真实缺口（本次未改，待评估）：① 流式 `response.completed.response.output` 恒为 `[]`
    （`RenderState` 不累积 output items，靠增量 `output_item.done` 解析的客户端无碍，读最终对象的有影响）；
    ② 无状态：`GET`/`DELETE /v1/responses/{id}` 未实现，`store` / `previous_response_id` 被忽略；
    ③ `web_search` 工具被静默忽略（`responses.rs` / `capability.rs` 无处理，返回 200 但不检索）。

- [x] 8. 上游「连接类」失败零重试：单渠道模型一次抖动就整回合失败（Reasonix 报「本轮已中断」）
  - 现象：Reasonix 改对 `request_url` 后**能拉到模型、也能对话**，但**时不时**提示
    「本轮已中断。上方的部分输出会永久保留供查看；只有完整工具调用及结果和有界恢复摘要会进入模型下一轮。」
  - 排查手法：临时起一个本地抓包代理（`127.0.0.1:1399` → `1314`，只记 `Authorization` 是否存在、
    不记其值），把 Reasonix 的 API 地址指过去复现一次。抓到 3 条：拉模型 200 → 对话 **502**
    （199 字节 `all_providers_failed`）→ **2s 后同体请求 200**（22450 字节，流完整）。
    两条请求体 SHA-256 相同 ⇒ 第二条是**客户端自己的重试**。
    （注意：抓包期间日志里另有一批 30 万 token 的大请求，那是**本会话的 dsh** 直连 1314 的流量，
    不是 Reasonix 的 —— 排查时先按端口区分客户端，否则极易误判。）
  - 定位过程（两次自我纠错，记下来免得再踩）：
    ① 先用抓到的请求体**原样重放**，JAI 返回 45 帧完整 SSE（`response.created` → `reasoning_summary_text.delta`
    → `function_call_arguments.done` → `output_item.done` → `response.completed` → `[DONE]`）并正确产生
    `bash` 工具调用 ⇒ **协议层没问题**，问题不在 JAI 的 Responses 实现；
    ② 再统计全量日志：**502 占 9.78%（460/4705），当天 20%（66/329）**，错误为
    `上游连接失败: error sending request for url (https://tokenrhythm.studio/v1/chat/completions)`，
    另有 `LITELLM_UNAVAILABLE` / `SERVICE_BUSY` / `429` / `503` / `504`(ALB) / `403` ⇒ **上游本身不稳**；
    ③ 关键结构发现：`route_candidates` 用 `m.model_name = ?1` **精确匹配**，该模型只命中 1 个渠道；
    且客户端用的是限定名 `基元律动/deepseek-flash`，`proxy.rs` 里 `candidates.retain(|c| c.provider_name == …)`
    会把候选**过滤到只剩这一家** ⇒ 「故障转移」形同虚设；
    ④ 而 `proxy.rs` 逐候选**只发一次**、无任何同渠道重试 ⇒ 单渠道 = 零重试，
    上游一抖就整回合失败。日志里成对的 `converted` + `passthrough` 两行**不是两次尝试**：
    `passthrough` 那行 `provider_id` 为空，是整请求的汇总行。
  - 已解决（2026-09-20，**按用户要求只做重试、不动故障转移**）：新增 `UPSTREAM_CONNECT_RETRY = 1`
    与 `should_retry_connect(&e, tries)`，在 `try_candidate`（直通）与 `try_converted_candidate`（转换）
    两处发送点对连接类失败**同渠道重试 1 次**。
    判据 `tries < 1 && !e.is_timeout() && !e.is_builder()`：排除超时（等待预算已花掉，重试等于翻倍）
    与请求构造失败；其余 send 阶段错误一律算连接类 —— **有意不收窄到 `is_connect()`**，
    因为实测「accept 后立刻断开」这类错误 reqwest 报 `is_connect()=false / is_request()=true`，
    只看 `is_connect()` 会漏掉真实场景。HTTP 层失败（含 5xx）**不重试**：链路已通，重试只会放大上游压力。
    顺带把两处请求组装收敛为闭包（`RequestBuilder` 一次性，重试必须重建），
    并去掉转换路径 `(url, (out, body))` 的多余嵌套与只为消警告的 `drop(url)`。
  - 验证：新增 `crates/gateway-core/tests/m11_connect_retry.rs`（3 用例）——
    黑洞上游（accept 后立刻断开）+ **建连计数**证明「同渠道恰好重试 1 次」（直通 / 转换各一条），
    外加反证「HTTP 500 不重试」（请求计数 == 1）。反证实测：`UPSTREAM_CONNECT_RETRY` 置 0 →
    两条重试用例精确变红（`实际 1 次`）、5xx 用例仍绿。
    全量回归 **357 通过 / 0 失败**（原 354，+3）；`cargo fmt --check`、`clippy -D warnings`、
    `tsc --noEmit`、`vite build` 全绿。
  - 测试环境坑（值得记住）：本机开着系统代理（Clash `127.0.0.1:7890`）时，
    用 `reqwest::Client::new()` 打本地黑洞端口会被代理接走并**回一个 HTTP 502**，
    于是网关走 5xx 分支不重试，用例以「只建连 1 次」的假象失败。
    集成测试里凡是打本地上游，客户端都必须显式 `.no_proxy()`，否则失败形态不确定。
  - 已排除的假设：**不是 JAI 走了系统代理**。排查时看到 JAI 进程常驻 11 条到 Clash
    `127.0.0.1:7890` 的连接，一度怀疑上游请求被系统代理接走（JAI 自己的代理设置是「关闭」，
    若真走了则完全不可见，而且会是最合理的 10% 失败解释）。但用户核对 Clash Party 连接页后确认
    **没有任何 `tokenrhythm` 流量** ⇒ JAI 对上游是**直连**；那 11 条连接属于 JAI 内的其它组件
    （webview / 更新器走的是另一份带默认 feature 的 reqwest 0.13.4）。
    结论：**502 是上游侧或到上游的网络路径问题，与 JAI 的代理配置无关**。
    （这条假设之所以难以自证，是因为同一次构建里 `reqwest::Client::new()` 在测试中**确实**会被代理
    接走 —— 见上方「测试环境坑」；生产侧由 reqwest 0.12.28 的 `default-features = false` 关掉了
    `system-proxy`。两边行为不一致，所以**不能靠 feature 推断，必须看实际流量**。）
  - 遗留（**本次未动**）：① 限定名 `供应商/模型` 会 `retain` 掉其它供应商候选，
    等于**关掉故障转移** —— 语义上更像「优先该供应商」而非「只用该供应商」，待评估；
    ② 该模型只有 1 个渠道，根治要加第二家供应商；
    ③ Reasonix 会话已 30 万 token（51.2 万窗口的 59%），每次请求重传约 1.4MB，放大了失败代价，可开 compact。

- [ ] 9.（**观察中**，2026-09-20）客户端「答非所问」：模型回答的是**上一轮/上一会话的另一个问题**
  - 现象：Reasonix 里问「了解当前项目」，模型整轮都在回答「让我测试下你的并行工具调用能力」
    （那是**更新 JAI 之前**那个会话里的测试内容）。该会话 `totalTurns=1`、`input_tokens=4254`，
    本不该知道那件事。
  - 物证（Reasonix 自己的会话存储
    `~/.reasonix/desktop-sessions-v5/by-id/.recovery-cache/<sid>/recent-v1.json`）：
    `[03] USER raw_content="了解当前项目"`，紧随其后的 `[04] ASSISTANT` 其 `reasoning_content`
    原文为 *"The user says **"让我测试下你的并行工具调用能力。"** (Let me test your parallel tool
    calling ability.)"*；该会话 **3 个** assistant 轮次的推理里，**没有一次**提到用户真正说的那句话。
    另注：推理全程为英文，而该请求的 user 消息里带着 `<reasoning-language>必须使用简体中文…`
    指令 —— 说明模型连这条也没照做。
  - **已逐条排除 JAI 侧**（这是本次排查的主要产出，避免下次重复劳动）：
    | 假设 | 结论 | 证据 |
    |---|---|---|
    | 转换把历史搞乱 | ❌ | 新增诊断器 `crates/gateway-core/tests/diag_responses_conversion.rs`（`#[ignore]`）：
    喂「Reasonix 形状」的多轮输入 + **并行 4 工具调用**，实测顺序保持、最后一条就是用户的问题、
    推理落在 `reasoning_content` 未混入 `content`、工具调用与结果一一配对、无多余插入 |
    | JAI 有跨请求缓存 | ❌ | 全仓库无 `prompt_cache` / `idempotency` / `response_cache`；
    转发时不带 `user` / `prompt_cache_key` |
    | 上游前缀缓存串台 | ❌ | 被污染那次 `usage_cache_read = 0`（对比：前一会话命中 19.9 万） |
    | 发出去的内容不对 | ❌ | `ui=4254` ≈ system(2820字) + session-context(1713字) + 用户话 + 15 个工具，
    与 Reasonix 记录完全对得上 |
    | 内容本身诱发错答 | ❌ | 把记录原文**重建后原样重放 3 次**，全部切题（推理都在讨论「了解当前项目」） |
    | 现在能否复现 | ❌ | 「回显测试」探针（让模型复述收到的用户消息）**逐字正确** |
  - 当前判断：**正确的提示词进去、零缓存命中，却出来另一个话题的答案** ⇒ 形态指向**上游返回了一条
    陈旧/串台的补全**（基元律动 = tokenrhythm.studio 那层聚合网关；同一家上游 502 率 9.78%，本身不稳）。
    **注意措辞**：只能确定**不是** JAI 的转换/缓存/转发，不能断言「就是上游」。
  - **结论（2026-09-20 抓包后定论）**：**不是 JAI 的问题，是会话历史被污染后自我维持。**
    把 Reasonix 的 API 地址指到本机抓包代理（`127.0.0.1:1399` → 1314）复现一次，抓到的真实
    `input` 有 26 条，结构如下（`[01]` 与 `[25]` 逐字相同、`[00]` 与 `[24]` 逐字相同）：
    ```
    [00] user       session-context
    [01] user       <reasoning-language> + 了解当前项目      ← 用户的提问
    [02..23]        assistant 的一整轮：reasoning×3 + assistant×3 + 8 个 function_call
                    + 8 个 function_call_output，内容全是「并行工具调用测试」
    [24] user       session-context（与 [00] 相同）
    [25] user       <reasoning-language> + 了解当前项目（与 [01] 相同）
    ```
    JAI 回给它的流**完整正确**：500 帧、`response.completed` 与 `[DONE]` 齐全、usage 正常。
    把这份 `input` **原样重放**，**稳定复现**同一错误答案（说明模型是在忠实地延续它收到的历史，
    而非 JAI 转发错乱）。逐段拆开重放定位到触发点：
    | 用例 | 输入 | 结果 |
    |---|---|---|
    | A 原样 26 条 | 全量 | ❌ 又答「并行工具调用」（**稳定复现**） |
    | B 去掉尾部重复对 `[24][25]` | 24 条 | ❌ 又答并行 |
    | C 去掉开头那对 `[00][01]` | 24 条 | ❌ 又答并行 |
    | D **去掉中间那轮 assistant**（只留两对用户消息） | 4 条 | ⚪ **不再答并行**（改为去探索项目） |
    | E 只留 `[00][01]`（无历史） | 2 条 | ✅ **完全切题**（`in=4254`，与最初那轮的 `ui=4254` 一致） |
    ⇒ **那轮 assistant 输出就是污染源**：Reasonix 把它当作模型的历史输出存了下来，
    此后每一轮都随历史重放，模型于是不断「继续」那个话题 —— **自维持**，不会自己恢复。
    最初为什么会答错（第一轮输入就是 `[00][01]`，`cached_tokens=0`）**未能复现**（用例 E 同输入答对），
    判断为一次性抖动，但**它已被写进历史，因此持续生效**。
  - 处置：**在 Reasonix 里开新会话**（或删除那轮 assistant 输出）即可恢复；JAI 侧无需改动。
  - 顺带在抓包里确认了一个**真实的 JAI 缺口**：流式响应的 `response.completed.response.output`
    **恒为 `[]`**（`RenderState` 不累积 output items）—— 同一帧里 448 个 `response.output_text.delta`
    带着真实文本，但最终对象是空的。靠增量事件解析的客户端（Reasonix/dsh）无碍，
    **只读最终对象的客户端会拿到一个空回合**。
    → ✅ **已于 v0.2.11 修复**（`RenderState` 新增 `completed_items`，在三处 `output_item.done`
    累积，收尾帧改用 `completed_response(status)` 回填 `output`；新增 5 个单测）。
    此处原写「已登记待修」系过时注记，2026-09-21 更正（本条属「观察中」的客户端问题，
    JAI 侧无需改动；顺带发现的这个缺口已修）。
  - 抓包脚本 `/tmp/jai-capture-proxy.py`（临时文件，分析完即删；踩坑记录：转发时必须丢掉
    `Transfer-Encoding`，urllib 已自动解开 chunked，原样转会让客户端解析乱码并提前断开；
    SSE 判定要看**响应头 Content-Type**，不要在 body 上猜）。

- [ ] 10. **macOS 产物只有 Apple Silicon（Intel Mac 既无安装包也无法自动更新）**（2026-09-21 发 v0.2.12 时确认）
  - 现象：Release 产物固定为 `JAI_<ver>_aarch64.dmg` + `JAI_aarch64.app.tar.gz(.sig)`，
    updater feed 里只有 `darwin-aarch64` / `darwin-aarch64-app`，**没有 `darwin-x64`**。
    Intel Mac 用户下载不到 macOS 安装包，应用内「检查更新」也拿不到版本。
  - 根因：`.github/workflows/release.yml` 的 macOS 矩阵是 `macos-latest`，
    该 runner 现已是 **arm64**，`tauri-action` 未指定 `--target` 时只产出宿主架构。
  - **不是回归**：v0.2.10 / v0.2.11 / v0.2.12 三版产物形状完全一致，自始如此。
  - **建议修法（先纠正一个错误结论）**：我最初写的「加一条 `macos-13`」**是错的** ——
    `macos-13` 已被 GitHub 下架（`actions/runner-images` README 已无该 label）。
    现存 x64 标签是 `macos-15-intel` / `macos-26-intel`，但它们属 **larger runners**：
    **按分钟计费（公开仓库也不免费）**，且必须先在 org/repo 设置里创建该 runner，
    否则 `runs-on` 找不到匹配 runner 直接失败。
  - 两条可行路径：
    - **路 A（便宜，推荐先试）**：在现有 arm64 runner 上**交叉编译** ——
      `rustup target add x86_64-apple-darwin`，构建参数加 `--target x86_64-apple-darwin`。
      同一个 job 产出两套 bundle，feed 多一个 `darwin-x64`。不额外占 runner、不额外计费。
    - **路 B（贵，需先配置）**：用 macOS x64 larger runner（`macos-15-intel`），
      多一条矩阵 + 计费 + 设置里先建 runner。
  - 无论哪条路，`latest.json` 的平台键由 tauri-action 合并，两条产物需上传同一 release
    （现有 `releaseId` 机制已支持）。**改动必须靠一次真实发版验证**（feed 里要出现 `darwin-x64`）。
  - 影响面：仅影响 Intel Mac 用户；Apple Silicon 与 Windows 用户不受影响。
  - 已同步记入 `docs/design/release.md` §1（避免下次发版又当成新问题排查）。

- [x] 11. **输出被 `max_output_tokens` 截断时误报为「可重试错误」→ 客户端重发同一请求最多 10 次**
  （PI-Desktop 会话里同一句话被拼 2/4/…/176 遍的真凶放大器）（2026-09-21）
  - 现象：PI-Desktop 会话里 assistant **正文**出现同一句话重复，份数恒为偶数（2/4/6/…/176），
    而 thinking 块（0/579）与工具参数（0/1164）从不重复。客户端日志：
    `agent.turn.failed … "Response incomplete without a provider reason", retriable:true, retryAttempt:10`。
    JAI 日志里同一个 `usage_input` 连续出现 3–10 次，每次 `usage_output` 跑满上限（8192）、每次约 40s。
  - 根因（JAI 侧）：`codec/responses.rs` 的 `Ev::Finish` 分支只把 `response.status` 写成
    `"incomplete"`，事件名**恒为** `response.completed`，且**从不输出 `incomplete_details`**
    （全仓库零命中）。只读 `status` 的严格客户端（PI-Desktop 的 Responses 适配器）把
    「incomplete 且无 reason」映射成 `stopReason:"error"` + 可重试，于是**重发同一 prompt
    最多 10 次**（`PROVIDER_RETRY_MAX_RETRIES=10`）。每次重试都让模型重新生成一遍
    （该渠道在长上下文 agentic 场景下会退化复读），客户端又把重试结果拼进同一条消息
    → 复读份数翻倍滚雪球。OpenAI 语义里截断**不是错误**：`status:"incomplete"` +
    `incomplete_details.reason:"max_output_tokens"`，客户端据此判 `length`（不重试）。
  - 已解决：新增 `finish_status()` 统一口径 —— `MaxTokens → ("incomplete","max_output_tokens")`、
    `SafetyBlock → ("incomplete","content_filter")`，并给 `incomplete` 补上
    `incomplete_details`（此前该字段全仓库零命中）；非流式
    `render_response` 与合成流 `render_response_sse` 同口径。
  - **终局事件名保持 `response.completed`（刻意不跟 OpenAI 的 `response.incomplete`）**：
    实测 `openai@6.x` 的 `ResponseStream` 只在 `response.completed` 上累积快照，
    `response.incomplete` 落进 `default:` 被忽略 → 快照停在 `status:"in_progress"` 且**丢 usage**
    （截断响应反而记不到用量）。判「截断」靠 `status` + `incomplete_details`（规范语义给全），
    事件名是客户端分支依据，等目标客户端（zcode/Reasonix 等）都验证过再切规范名。
    单测 4 个（截断/安全/正常/非流式）
    + M6 集成测试 3 个（流式截断、非流式截断、正常结束）。
  - 顺带澄清责任边界：上游**确实**在复读（该中转的 usage 经核验是准的：同一句话重复 810 遍
    ≈ 8900 tokens ≈ 上限 8192），JAI 是 1:1 转发、不制造重复（转换流 delta 逐帧对应上游
    `delta.content`；缓冲破坏性 drain；无中途重试/重放）。本条修的是「把一次截断放大成
    10 次重试风暴」这一侧。

- [x] 12. **`request_logs.stop_reason` 恒为 NULL**（诊断列形同不存在）（2026-09-21）
  - 现象：6434 行全空 —— 该列在 schema 与 INSERT 里都有，但 `emit_log_with` 里写死 `None`。
    排查「模型为什么反复重发」时看不到是 `max_tokens` 截断（本次只能靠 `usage_output`
    顶满上限反推）。
  - 已解决：`emit_log`/`emit_log_with` 增加 `stop_reason` 实参并落库；采集口径——
    转换流式取 IR `Finish`（与 `pending_finish` 同裁决：先到的显式 `finish_reason` 为准）、
    转换非流式取 `CanonicalResponse.stop_reason`、直通非流式按响应体形状取、
    直通流式用 `StopReasonProbe` 做关键字级扫描（与「直通流式 `tool_calls` 恒 0」
    同源约束：字节直通不解析 SSE 语义，只读不写、绝不影响转发）。
    统一词表为 IR 口径（`end_turn`/`max_tokens`/`tool_use`/`safety`/`other`）。
  - 直通流式探针的两个坑（对抗性审查发现，各配回归测试）：
    ① Responses 终局帧把**整个 response（含全部正文）**嵌在同一帧里 —— 截断响应正文
    可达 30KB+，固定 16KB 窗口会把帧首的 `status`/`incomplete_details` 挤出窗口，
    恰好丢掉最想看的结论 → 改为**逐块扫描**（末尾窗口只留作跨块拆分的兜底）；
    ② 同帧里 `output[*]` 的 item 也带 `status`，且截断响应末尾 item 反而是 `completed`，
    「取最后一次 `status`」会把截断静默记成 `end_turn`（比 NULL 更坏）→
    `incomplete_details.reason` **优先**且加值域白名单（`max_output_tokens`/`content_filter`），
    `status` 仅兜底。另外上游挂死/中途断流的行也带上已见到的结束原因（不再 NULL）。
  - 暴露面：`LogRowView` + 日志页 CSV 导出新增「结束原因」列（JSON 导出与 `logs_recent`
    自动带上 `stopReason`）；`ui/src/types.ts` 同步补字段。日志页表头/详情**未**加列
    （避免动 UI 门禁，字段已可从 JSON 导出取）。
  - 单测 5 个（词表映射、非流式取体、流式逐块扫描、巨型终局帧、reason 优先于 item status）
    + 末尾窗口有界性测试。
  - 已知偏离（未改，留作独立变更）：`output[*].status` 在截断时仍是 `completed`（OpenAI 会
    镜像成 `incomplete`）。现有客户端一直看到 `completed`，改它属于行为变更，需单独验证。

- [x] 13. **WebDAV 404 一律被说成「目标目录不存在」：把「服务器没就绪」伪装成「用户少建了目录」**（2026-09-22 真机）
  - 现象：推送失败提示
    `WebDAV 推送失败 HTTP 404（目标目录不存在，请先在远端创建该目录）: <!DOCTYPE html> … 整页 nginx 404 …`。
    按提示去远端建目录不会有任何改善（那边本来就有目录，且远端 `jai-config.json` 一直在），
    错误正文还把一整页 HTML 灌进 UI。
  - 根因：`sync.rs` 把**任何** 404 都翻译成「目录不存在」。实测那台服务器是 **DUFS**
    （无认证 `GET /` → `401 WWW-Authenticate: Digest realm="DUFS"`），对 `PUT` 到不存在的目录
    返回 **201 自动建目录** ⇒ 404 在该服务器上**不可能**指「目录不存在」。独立用 curl 复现：
    事发那两分钟里 `GET`/`OPTIONS`/`PROPFIND /` **全是** nginx 自己的 HTML 404 页
    （内容与用户贴的一字不差），是 vhost 没路由到后端（同机另一个站点同时 502），
    请求根本没到 WebDAV 处理器；约两分钟后自愈（`PROPFIND /` 207、远端 `jai-config.json`
    的 `exportedAt` 与本地 `webdav_last_sync_exported_at` 完全一致，即上一次 08:14 UTC 推送是成功的）。
  - 已解决（**未发版**，等下次发版带上）：404 先看正文再下结论。新增
    `looks_like_web_page`（网页 vs WebDAV 的 XML/纯文本错误；剥 UTF-8 BOM、只看开头 1KB）、
    `http_hint`（按状态码 + 正文给提示）、`brief_body`（折叠空白 + 截断 200 字符）。
    正文是网页 ⇒「该地址当前不是可用的 WebDAV 端点（服务未就绪，或根地址/路径不对；
    请检查设置，稍后重试）」；正文为空或 XML/纯文本 ⇒ 保留原措辞。同一口径覆盖：推送主文件、
    推送前留存备份、连接测试、备份列表/读取/删除、显式拉取（`pull` 把「端点不可用」与
    「远端还没配置」分开说）。**对照实测**：同一台 DUFS 对「文件不存在」的 404 是
    `content-type: text/plain` + 正文 `Not Found`，与网页 404 判然有别，分类因此可判。
  - **刻意不 fail-closed**（对抗性审查发现的坑，已用反向控制钉住）：`try_pull` 对「网页 404」
    仍返回 `Ok(None)`。若按分类直接报错，`push` 的第 1 步（留存远端旧版）与 `webdav_push`
    的差异预警都会中止 —— **首次推送永远建不出远端文件**（建文件正是推送要做的事）。
    而 HTML 404 不等于端点坏了：nginx 的 dav_module 就用自带 HTML 页回答「文件不存在」，
    反向代理加一行 `error_page 404 /404.html;` 也会把所有 404 正文改写成网页。
    所以分类只用来选措辞，控制流与修复前一致。
  - 验证：单测 3 个 + 集成 8 个，含 5 个**反向控制**（空正文 404 仍说「目录不存在」、
    原「路径不存在」「远端尚无配置文件」措辞不变、`try_pull` 的 `Ok(None)`、幂等删除的
    `Ok(())`）。变异验证：`looks_like_web_page` 改成恒 `false` ⇒ **8 个用例变红**
    （2 单测 + 6 集成）、5 个反向控制仍绿；`try_pull` 改回 fail-closed ⇒
    `try_pull_html_404_is_not_fail_closed` 变红（证明不是空洞通过）。
  - 顺带发现（**未处理**，已单列为第 14 条）：远端根目录已堆积约 180 个
    `jai-config.<时间戳>.json`（9/17 起，30 分钟一次推送各留一份备份）——
    不是「清理没清干净」，而是滚动清理**压根没接上调用方**（见第 14 条）。

- [ ] 14. **远端备份保留策略形同虚设：`BACKUP_KEEP` / `backup_evict_candidates` 没有任何生产调用方**（2026-09-22 发现）
  - 现象：远端根目录堆积约 180 个 `jai-config.<时间戳>.json`（9/17 起，每 30 分钟一次推送
    各留一份），从未被清理。
  - 根因：`crates/gateway-core/src/sync.rs` 里的 `BACKUP_KEEP = 10` 与
    `backup_evict_candidates()` **只有测试在调用**（全仓 grep 无生产调用点）；
    `delete_backup` 只挂在手动命令 `webdav_backup_delete` 上。也就是说「推送后滚动清理
    （保留最近 10 份）」这个设计只落了一半：候选集算得出来，没人去删。
  - 影响：远端目录无界增长；备份列表页/`PROPFIND Depth:1` 的响应体积随之变大
    （本次实测一次列表 180+ 条目）。
  - 待办（独立变更，需真机验证）：把清理接到推送成功之后（或备份列表页的显式按钮），
    失败只记日志不阻塞推送（与 `snapshot_put` 的「尽力而为」同口径）；
    删除动作本身已有幂等语义与防误删白名单（`backup_timestamp` 只认时间戳形态）。
  - 注意：清理会真的删远端文件，属于破坏性动作，须先在真机上跑一遍保留份数与白名单判据。

- [x] 15. **集成测试用固定 `sleep(700ms)` 等异步日志落库 → Windows runner 上偶发假失败**（2026-09-22 v0.3.1 发版时撞上）
  - 现象：tag `v0.3.1` 的 `CI`（main push）在 **windows-latest** 上红：
    `m3_anthropic.rs:244` `called Option::unwrap() on a None value`
    （`logs_recent(...).find(|r| r.http_status == 200).unwrap()`）。同一份代码 30 分钟前
    在 Windows 上是绿的，macOS job 同轮也绿 ⇒ 与产品行为无关的时序抖动。
  - 根因：日志落库是**异步**的（后台线程 + 批量写入），而测试写死
    `sleep(700ms)` 后立刻查库；负载高/机器慢时 700ms 不够 ⇒ 查到空集 ⇒ `unwrap()` panic。
    全仓共 5 处同一写法（`m2_failover` / `m3_anthropic` / `m4_conversion` /
    `m6_responses_inbound` / `output_truncation_diagnostic`），每处都是一颗定时炸弹。
  - 已解决：新增 `crates/gateway-core/tests/common/mod.rs::logs_settled(db, limit, timeout)` ——
    有界轮询到「行数连续两次相同」即认为本批写完（正常约 100ms 返回，比原来还快），
    超时**不 panic**（把当前快照交给调用方，让真正的断言判断对错；只有一条都没有才报错，
    那说明日志管道根本没启动）。5 处固定 sleep 全部替换。
  - 验证：5 个受影响的测试文件全绿，且更快（m6 由约 2s → 0.71s）；`fmt`/`clippy -D warnings`
    干净。**注意**：`v0.3.1` tag 指向的提交仍带这颗炸弹（产物不受影响，纯测试代码），
    修复在 tag 之后的提交上。
  - 教训（与 §4.2「探针自身不可信」同源）：**测试里的固定等待等于把失败概率留给负载**；
    异步写入一律用有界轮询 + 明确的超时语义，不用 `sleep` 赌时间。

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

## 4. UI 优化专题：双尺寸验收清零 + UI 规范门禁化（2026-09-21）

> 触发来源：用户提出「做一次 UI 优化专题」。
> 先跑现有探针取基线，发现**老清单在默认窗口 1180×800 下基本已闭环**（双主题对比度 0 类不达标、
> 横向溢出 0、弹窗主按钮全可见、日志/MCP/技能/统计首屏 100%），剩下的问题分两类：
> ① 最小窗口 900×600 从没被当成一等公民验收过；② 一批规范**只写在文档里**，
> 新功能一加就静默回退。故本轮的重点是「把规范变成可执行门禁」，而不是再修一遍老条目。
>
> 验收基准：**1180×800 与 900×600 双尺寸 × light/dark 双主题**，一条命令 `gate.mjs`。

### 4.1 本轮修掉的问题

> **逐条索引（解决日期 / 状态 / 验证证据）** —— 本轮全部改动落在**工作区**（按约定不提交、不打 tag、不发版），
> 故「解决版本」一栏记为 `工作区（基于 v0.2.12 之后）`；所有证据都能由 `gate.mjs` / 独立探针现场复现，见 4.3。

| # | 问题 | 状态 | 解决日期 | 验证证据（见 4.3） |
|---|---|---|---|---|
| 1 | 模型页无操作区、无列降级的最长页 | 已解决 | 2026-09-21 | ③（行内控件 194 → 68、主操作折叠线下 0、吸顶位移 0px）、⑪ |
| 2 | 新功能引入 4 处 `text-[10px]` | 已解决 | 2026-09-21 | ⑤#2（负控制）；`grep -rn "text-\[10px\]" ui/src` = **0** |
| 3 | 命中区「已修」项实测不达标 | 已解决 | 2026-09-21 | ②（406 控件、不达标 0、最小有效命中区 24px）、⑤#3（负控制） |
| 4 | 模型页表在 900×600 横向溢出 8px | 已解决 | 2026-09-21 | ③（表 674/容器 724，溢出 **-50**）、⑤#4（负控制） |
| 5 | 探针自身不可信（死字段 / 噪音 / 路径） | 已解决 | 2026-09-21 | ⑦（可移植性）、⑪（探针崩溃不再假绿） |
| 6 | 900×600 toast 压住弹窗 footer 主按钮 | 已解决 | 2026-09-21 | ④（四轮「弹窗 footer ∩ toast 带」无交集） |
| 7 | 无脏状态 guard（切页/关窗静默丢失） | 已解决 | 2026-09-21 | ⑥（独立探针 14/14） |
| 8 | 日志轮询不随可见性暂停 | 已解决 | 2026-09-21 | ⑥（切后台 4→4、恢复可见 4→5） |

> 另：本轮**新发现并修掉**两个「门禁自己不可信」的问题（吸顶表头判据恒真、探针崩溃却报绿），
> 见 4.2 与 4.3 ⑪ —— 不在原始 8 条里，但属于同一主题（规范门禁化）。

- [x] 1. **模型页是唯一既无操作区、又无列降级的「最长页」**（P1）
  - 实测（改前）：1180×800 下 **194 个可交互控件、117 个在折叠线下**、首屏 44%；
    900×600 下 173 控件 / 114 在折叠线下，**6 个控件被折叠线切 28px**。
  - 处置：表格改**只读** + 右侧**详情抽屉**（`ui/src/components/ui/sheet.tsx` 新建），
    全部编辑项（上游模型 ID / 上下文 / 最大输出 / 别名 / 模态 / 推理档位值域 / 工具上限 / 启用）
    收进抽屉一次保存；补**吸顶表头**。
  - **顺带发现（重要）**：本来只是「照抄 `LogsPage.tsx:235` 的吸顶表头写法」，但把「吸顶」写成
    **可测量**的判据后实测发现 —— **日志页的吸顶表头此前从未真正生效**（确实声明了
    `sticky top-0`，实际滚动 300px 后 `th.top` 照旧跟着滚走）。根因见 4.2「声明 ≠ 生效」。
    两页现在用同一条修法（把 `max-h + overflow:auto` 加到 `[data-slot=table-container]` **那一层**），
    并由 `fold.mjs` 的**实测**判据守着（`stickyHeads[].works`，见 ④/⑤）。
  - 验收：行内控件 194 → **68**；双尺寸下模型页 `belowFold` 主操作为 0；
    模型页与日志页表头在纵向滚动 150px 后位移 **0px**。

- [x] 2. **新功能把已建立的规范又破坏了**（P1，根因见 4.2）
  - 证据：bug 21/24 引入的 0011/0012 两个编辑器写出 **4 处 `text-[10px]`**
    （`EffortLevelsEditor.tsx:111,135`、`MaxToolsEditor.tsx:86,111`）——
    而 P2-10 早已把全项目 10px 清零过一次。
  - 处置：4 处改 `text-[11px]`；并把「字号 ≥11px」变成 `scripts/ui_lint.sh` 的一条硬检查。

- [x] 3. **命中区「已修」项实测不达标，且手法本身不可靠**（P1）
  - 关键澄清（**推翻了本轮早先的初判**）：早先用 `audit.mjs` 的 rect 数字报的
    「推理档位值域 148×16 完全没扩展」「从环境变量导入 102×24」**都是假问题** ——
    rect 看不到 `::after` 外扩，实测**有效**命中区分别是 88×26 与 ≥24×24，本来就达标。
    这正是「audit 与 probe-hits 结论互相矛盾」的根源，也是本轮把判据**唯一化**的直接动因。
  - 真实缺陷只有两处，且**同因**（伪元素热区被相邻元素抢走）：
    | 控件 | 视觉 rect | 有效命中区（实测） | 根因 |
    |---|---|---|---|
    | 模型页「复制模型名」×21 | 16×16 | **6×30** | 右侧紧邻的「推理档位值域」触发器也有 `after:-inset-2`，DOM 在后、paint 在上，把那一侧 8px 抢走 |
    | 供应商页「官网」×2 | 40×16 | **54×18**（高 18 < 24） | 下方「推理档位值域 / 工具上限」同理，抢走下半截 |
  - 处置：不再依赖伪元素外扩，改**真实盒子** + 负 margin 抵消布局影响：
    复制按钮 `-m-1 grid size-6`（24×24）+ `z-10`；「官网」与两个 verbose 编辑器
    `-my-1 py-1`（高 24）+ `z-10`。`z-10` 用来赢下重叠区（否则仍会被邻居抢）。
  - 验收：`probe-hits` 在 1180×800 扫 **406** 个控件 → **命中区不达标 0 处**；
    900×600 同样 0 处（见 4.3）。
  - 教训：**命中区必须靠真实盒子**。伪元素外扩在「多个小控件相邻」时会被 DOM 在后的那个吃掉，
    而 rect 类审计永远看不见这件事 —— 只有逐点 `elementFromPoint` 才能发现。

- [x] 4. **模型页表在最小窗口横向溢出 8px**（P2，P1-5 的回归）
  - 证据：900×600 下表宽 **732 > 容器 724**。P1-5 当时验收是 674 = 674「恰好放下」，
    之后新增的「推理档位值域 / 工具声明数上限」芯片把它撑宽了 ——
    正是 P1-5 自己记下的那条教训（「降级 badge 的文案长度直接决定列宽」）。
  - 处置：随第 1 条一并解决（表格只读后不再需要给行内输入预留宽度）。
  - 验收：双尺寸下模型页 `表格: 横向溢出 ≤ 1`。

- [x] 5. **探针自身不可信：死字段 + 噪音 + 路径不可移植**（P2，**本轮的地基**）
  - `fold.mjs:36` 的 `headerBtns` 选择器（`main [data-slot=page-header] button, main h1 ~ * button, main header button`）
    **永远匹配不到** —— `PageHeader` 既不渲染 `data-slot` 也没有 `<header>`，
    于是报告里的「顶部操作区」恒为「—」，是个**死字段（假绿）**：这条验收其实从未生效。
    与 bug 清单第 28 条（`truncated` 只初始化、从不 push）**是同一类问题，又犯了一次**。
    处置：`PageHeader` 补 `data-slot="page-header"`；网关/同步的吸顶操作条与设置页的吸顶锚点导航
    补 `data-slot="page-actions"`（都写成**探针契约**并注释清楚）；`fold.mjs` 改用这两个契约，
    并额外断言「本页主操作数 > 0」，让选择器再失效时**直接报错**而不是静默变绿。
    修后网关/同步/MCP/技能/供应商/模型/统计/日志/设置九页都能列出真实主操作。
  - `clippedRows` 在 **45/102 步**非空，全是噪音：旧实现把 `document.documentElement` / `document.body`
    也当容器，并为它们特判「子元素取 `main > *`」→ 拿 main 的子元素去和 **html 的底边（=视口高）** 比，
    而 main 是滚动容器、内容本来就越过视口底 → 每页稳定产出 `cutBy: 700 / container: html` 的假阳性，
    把真问题淹掉。处置：只测真正的布局容器（main / 弹窗 / 侧栏 / `.overflow-y-auto`），
    子元素一律 `:scope > *`，滚动容器里的「看不见」归 `focusBelowFold`。
    顺带修掉同一行的死选择器：`overflow-y-auto` 是 Tailwind 类名，`querySelectorAll("overflow-y-auto")`
    按**标签名**匹配、永不命中（已改 `.overflow-y-auto`）。
  - 15 个探针各自把 Playwright/Chrome 路径写死成某台机器上的 pnpm 绝对路径 → 换机/CI 全跑不起来。
    处置：新增 `_env.mjs` 统一解析（`PLAYWRIGHT_PATH`/`CHROME_PATH` 环境变量 → `ui/node_modules`
    （`playwright` → `playwright-core`）→ 仓库根 `node_modules` → Playwright 自带 chromium），
    15 个探针全部改为从它取 `launchBrowser` / `parseArgv` / `parseSize` / `SHOTS_DIR` / `VR_DIR` / `installMockSrc`。
  - 验收：`grep -rn "deepseek-harness" tools/ scripts/ ui/src/` 只剩 `_env.mjs` 里的一行**历史注释**；
    `PLAYWRIGHT_PATH=<旧位置>` 覆盖生效、无 env 时回落到 `ui/node_modules/playwright-core`（实测记录见 4.3）。

- [x] 6. **900×600 下 toast 视觉压住弹窗 footer 主按钮**（P2；**口径已更正**）
  - 早先我把它判成 P0-3 那类「点不到」，**这是错的**：`main.tsx:32,36` 给 Toaster 与每条 toast
    都设了 `pointerEvents: "none"`，浏览器实测 `li.pointerEvents = "none"`、容器同为 `none`，
    toast 中心的 `elementFromPoint` 返回的是**它下面的元素**而不是 toast 本身。
    所以真实影响是**「可点、但 2.4 秒内看不清」**（可读性问题），不是遮挡。
  - 几何证据：900×600 打开「添加供应商」，弹窗 `h=568 / y16–584`，footer 按钮 `y523–559`；
    toast 带 `x520–876 / y522–576` → 「创建」「取消」各 `60×36` **完全落在带内**。
    1180×800 下恰好不重叠（仅剩 2px 余量）。
  - 处置（按用户拍板）：`dialog.tsx` 的 `max-h` 由 `calc(100dvh-2rem)` 改为 **`calc(100dvh-8rem)`**，
    让弹窗底边（垂直居中 → `(vh+h)/2`）恒在 toast 带上方 ≥10px。
    **默认窗口行为不变**：1180×800 下 max-h = 672 > 现有最高弹窗 639。
  - 验收：`gate.mjs` 新增几何断言「任一弹窗 footer 按钮矩形 ∩ toast 占位带 = ∅」，
    双尺寸 × 双主题全部无交集（toast 带几何取自 sonner 的 `ol.toaster` 自身，**即使当前没有 toast 也成立**）。

- [x] 7. **无脏状态 guard：改表单不保存就切页/关窗 → 静默丢失**（P2）
  - 处置：新增 `ui/src/lib/dirty.ts`（模块级脏源注册表 + `useDirtyGuard`）+ 
    `ui/src/components/common/UnsavedGuard.tsx`（确认框 + `beforeunload`）；
    `NavProvider.setTab` 改走 `requestLeave(...)` 拦截切页；
    供应商 / MCP / 技能 / 设置**四处表单**接上（弹窗内表单与页内表单均覆盖）。
  - 验收：临时探针实测「打字 → 切页 → 出现确认框 → 留在此页 / 放弃改动并离开」两条路径（见 4.3）。

- [x] 8. **日志轮询不随可见性暂停**（P2）
  - 证据：`LogsPage.tsx:54` 的 `setInterval(refresh, intervalMs)` 无 `visibilitychange`，
    应用切后台仍每 3s 拉一次 IPC。
  - 处置：`document.hidden` 时停轮询，恢复可见时立即刷新一次并重启定时器
    （不误开用户手动关掉的自动刷新开关）。
  - 验收：临时探针用 `__JAI_CALLS__` 计数实测「切后台后 `logs_recent` 不再增长、切回来恢复」（见 4.3）。

### 4.2 本轮的方法学产出（比上面 8 条更重要）

**规范必须门禁化，否则一定回退。** 本轮把三类规范从「只写在文档里」变成可执行检查：

| 门禁 | 覆盖 | 位置 |
|---|---|---|
| `scripts/ui_lint.sh` | 字号 ≥11px、图标按钮有可访问名、列表 key 不用下标、命中区不靠伪元素外扩 | 零依赖，任意目录可跑 |
| `tools/visual-regression/probe-hits.mjs` | **有效命中区 ≥24×24**（命中区判据的唯一归属）+ 控件类型覆盖缺口 | 逐点 `elementFromPoint` |
| `tools/visual-regression/gate.mjs` | 双尺寸 × 双主题 一把收口：对比度 AA / 字号 / 截断有 title / 无横向溢出 / 弹窗几何 / 弹窗 footer 不与 toast 带重叠 / 主操作首屏可达 / 行内控件不被切半 / 表格不溢出 / 命中区 / 静态规范 | **单一退出码**，判据集中一处 |

- 已接进 `scripts/release_check.sh` 第 6 步（本地发版前必跑）。CI 接入留待下一步（本轮按用户拍板不改 workflow）。
- `gate.mjs` 在**当前代码上必须能红**：本轮做了三条**负控制**实证（见 4.3），
  证明它真能抓到「字号回退」「命中区回退」「表格溢出回退」，而不是一条永远绿的装饰。

**判据必须唯一归属。** 命中区曾有两个判据（`audit.mjs` 的 rect 与 `probe-hits.mjs` 的有效命中区），
结论长期互相矛盾（audit 报 42 处，probe-hits 复核后大多合格）—— 结果是**两边都不能用**。
现在 `audit.mjs` 只负责能用 rect 可靠测量的事（对比度/字号/截断/溢出/弹窗几何），
命中区一律以 `probe-hits.mjs` 为准，`audit` 里写了注释说明为什么它不再给这个判定。

**「一直红的断言比没有断言更危险」。** 见 bug 第 28 条与本轮第 5 条：两者都是
「字段/选择器恒为空 → 断言永不生效 → 训练人忽略红色」。对策是把
「选择器真的匹配到了东西」本身也写成断言（`primaryActionCount > 0`、`missingKinds` 为空）。

**「声明某属性」不等于「该属性真的生效」，而这类判据最容易写成恒真。** 本轮两次踩到同一件事：
① 「吸顶表头」只检查 `getComputedStyle(thead).position === "sticky"` 就会**永远通过**，
但真去滚一下最近的纵向滚动容器就会发现表头照旧滚走 —— 日志页因此假绿很久。
根因是 `Table` 基座自带 `div[data-slot=table-container].overflow-x-auto`，其 `overflow-y`
随之计算成 `auto` → 它是 thead 的**最近可滚动祖先**，而它自身没有纵向溢出，于是 `sticky`
完全失效。修法是把 `max-h + overflow:auto` 加到**那一层**（用 arbitrary variant），
而不是在外面再套一个 div。（已排除 `border-collapse` 与「sticky 放 thead 还是 th」两个假设，都试过无效。）
对策：判据改成**实测**（真的滚 150px，看 `thead` 位移是否 ≤4px），并加负控制（见 ⑤ 第 4 行）。
② 我自己新写的那条判据一开始也**跑不到**：`gate.mjs` 里 `for (const h of f.stickyHeads)` 被误插进了
`if (cutPrimary.length) { … }` 块**内部** —— 语法合法、语义错（只有主操作被切半时才顺带检查吸顶），
于是 `stickyHeads` 常年不被读。**又一次「恒真的断言」**，与上面第 28 条同型。已移到正确层级。

**「探针崩溃了，门禁却更绿了」——门禁必须能区分「探针这次跑过」和「读到了上次的旧结果」。**
本轮末段真实踩到：`fold.mjs` 在一次编辑里被误删了一个变量（`tables`），探针**每次都崩**，
而 `gate.mjs` 只做 `readJson(.vr/fold-*.json)` → 读到**上一次**留下的旧 JSON，
于是在一个已经崩掉的探针上报出 **「✓ 全部通过、退出码 0」**。
对策（已落进 `gate.mjs`）：跑探针前先 `fs.rmSync` 删掉它的输出文件，
且**探针退出码非 0 直接判失败**（判据名「探针可运行」）。
`--skip-probes`（只复算判据、不重跑探针）保留给「手工重跑单个探针后验证判据」的场合 ——
它读旧 JSON 是**显式选择**，不再是默认路径。

### 4.3 验证证据（可复核）

> 命令均在仓库根目录执行；探针需先起前端 `cd ui && npx vite --host 127.0.0.1 --port 5173`。

**① 门禁本体（最终一次全量复跑）**

```
$ node tools/visual-regression/gate.mjs        # 修复「吸顶判据被误插」+「fold 崩溃假绿」之后的全量复跑
UI 门禁：尺寸 1180x800 / 900x600，主题 light / dark
· [信息·非失败] 1180x800 gateway：1 个控件跨在折叠线上（可滚动到达，属正常滚动提示）：button「复制」切5px
· [信息·非失败] 1180x800 sync：1 个控件跨在折叠线上（可滚动到达，属正常滚动提示）：input「INPUT」切22px
· [信息·非失败] 1180x800 models：3 个控件跨在折叠线上（可滚动到达，属正常滚动提示）：button「复制模型名 kimi-k2.7-code」切17px ;
  button「启用 kimi-k2.7-code」切13px ; button「编辑详情 kimi-k2.7-code」切19px
· [信息·非失败] 900x600 settings：1 个控件跨在折叠线上（可滚动到达，属正常滚动提示）：textarea「代理绕过列表」切37px
✓ 全部通过
FINAL_GATE_EXIT=0        # 单一退出码 0；无 ✗ 行（吸顶/命中区/对比度/折叠线/弹窗几何 全部通过）
```

单独复测吸顶（真实滚动 500px，比门禁里的 150px 更严）：

```
[日志] 纵向滚动容器=relative w-full overflow-x-auto | scrollTop 0→500 | th.top 181→181 | thead.top=0px → ✅ 吸顶生效
[模型] 纵向滚动容器=relative w-full overflow-x-auto | scrollTop 0→500 | th.top 213→213 | thead.top=0px → ✅ 吸顶生效
```

门禁**不读旧结果假绿**的实证（故意让探针崩溃）：

```
$ node tools/visual-regression/gate.mjs --sizes=1180x800 --themes=light   # 本轮实测
✗ 探针可运行  —— 1 处
   [probe-hits --size=1180x800] 退出码 3（读旧结果会假绿，故直接判失败）：
✗ 命中区 probe-hits  —— 1 处
   [1180x800] 未产出结果文件（探针未跑或崩溃）
✗ 不通过：2 处，涉及 2 条判据        # 退出码 1；修复此行为之前，同样的崩溃会报「✓ 全部通过」
```

**② 命中区（`probe-hits.mjs`）**

| 尺寸 | 扫描控件 | 命中区不达标 | 覆盖缺口 | 控制台报错 |
|---|---|---|---|---|
| 1180×800 | 406 | **0** | 无 | 0 |
| 900×600 | 406 | **0** | 无 | 0 |

类型覆盖（缺一即失败）：`switch 46 / checkbox 3 / inlineCopy 33 / externalLink 2 /
effortLevels 11 / maxTools 11 / dropdownTrigger 22 / dialogClose 16 / select 8 / textInput 63`。

**③ 折叠线与表格（`fold.mjs`）**

| 页面 | 1180×800 控件 / 主操作折叠线下 | 900×600 控件 / 主操作折叠线下 |
|---|---|---|
| 网关 | 13 / **0** | 13 / **0** |
| 同步 | 31 / **0** | 31 / **0** |
| MCP / 技能 / 统计 | 21 / 15 / 3，均 **0** | 同左，均 **0** |
| 供应商 | 24 / **0** | 24 / **0** |
| **模型** | **68** / **0**（表 894/容器 1004，溢出 **-110**） | **68** / **0**（表 674/容器 724，溢出 **-50**） |
| 日志 | 8 / **0** | 8 / **0** |
| 设置 | 22 / **0** | 22 / **0** |

模型页行内控件 **194 → 68**；`clippedRows`（曾被误报淹没真问题）在双尺寸下均为 **0**。

**吸顶表头的实测证据**（`fold.mjs` 的 `stickyHeads`：真的把最近纵向滚动容器滚 150px，再测 `thead` 位移）：

| 页面 | 1180×800 | 900×600 |
|---|---|---|
| 模型 | `sticky:true scrollable:true scrolledBy:150 moved:0 works:true` | 同左 |
| 日志 | `sticky:true scrollable:true scrolledBy:150 moved:0 works:true` | 同左 |

**④ 对比度 / 字号 / 截断 / 横向溢出（`audit.mjs`）**

双尺寸 × 双主题（1180×800、900×600 × light、dark）四轮审计：`contrast`、`tiny`、`truncated`、
`hScroll`、弹窗几何全部 **0**；**「弹窗 footer ∩ toast 带」四轮均无交集**。

**⑤ 四条负控制（证明门禁真的能红，而不是永远绿的装饰）**

| # | 故意回退 | 结果（本轮实测，逐字） |
|---|---|---|
| #2 字号 | 在 `StatsPage.tsx:64` 的 className 里注入 `text-[10px]` | `ui_lint` 红：`pages/StatsPage.tsx:64  text-[10px]：最小字号应为 11px` → `gate` 红：`✗ 静态规范 ui_lint —— 1 处`（退出码 1）；恢复后绿 |
| #3 命中区 | 把「复制模型名」退回伪元素外扩（`after:-inset-1`，视觉 16×16），并复刻旧布局里紧邻的同款小芯片 | `probe-hits` 报 **42 处不达标**，签名 `rect 16×16 → 有效 14×22「复制模型名 deepseek-v4-flash-0731」`、`rect 16×16 → 有效 22×22「占位竞争按钮」`（均 < 24×24）→ `gate` 红：`✗ 命中区 ≥24×24`（`--skip-probes` 口径复算，共 12 处）；恢复后绿 |
| #4 表格溢出 | 去掉 `table-fixed` 并注入 560px 不换行单元格 | `fold` 报 `7列 宽1173/容器724 横向溢出449` → `gate` 红：`✗ 表格不溢出容器 —— 1 处 [900x600 models] 7列 宽1173/容器724 溢出449`；恢复后绿 |
| 吸顶表头（本轮新加判据） | 把日志页那条「让 sticky 生效」的 arbitrary variant 去掉（回到「声明了 sticky 但实际不吸顶」） | `fold` 报 `{"sticky":true,"scrollable":true,"scrolledBy":150,"moved":-150,"works":false}` → `gate` 红：`✗ 吸顶表头真的吸顶 —— 1 处 [1180x800 logs] 滚动 150px 后表头位移 -150px（sticky 失效）`（退出码 1）；恢复后绿 |

> #2/#3/#4 的「gate 变红」用 `gate.mjs --skip-probes` 复算判据（先手工跑对应探针产出结果文件）；
> 「吸顶表头」那条是**完整** `gate.mjs`（不跳探针）跑出来的实测输出，用于验证整条流水线确实接通。

**⑥ 脏状态 guard 与日志轮询（独立探针，14/14）**

```
✅ 日志页自动刷新默认开启 — aria-checked=true
✅ 可见时轮询在跑 — 2 → 4
✅ 切后台后停止轮询 — 4 → 4
✅ 恢复可见后立即刷新 — 4 → 5
✅ 设置页改端口后切页 → 出现确认框 — 「设置」有未保存的改动，离开将丢失。 | 留在此页 | 放弃改动并离开
✅ 「留在此页」后仍在设置页
✅ 「放弃」后真的切到日志页
✅ ProvidersPage / McpPage / SkillsPage / SettingsPage 均已接入 useDirtyGuard
```

**⑦ 可移植性**

```
$ grep -rn "deepseek-harness" tools/ scripts/ ui/src/
tools/visual-regression/_env.mjs:4:  # 仅一行**历史注释**
$ PLAYWRIGHT_PATH=<旧位置> node -e "…_env.mjs…"   → PLAYWRIGHT_DIR 指向旧位置（覆盖生效）
$ node -e "…_env.mjs…"                            → /…/JAI/ui/node_modules/playwright-core（仓库内回落）
```

**⑧ 既有回归探针（证明本轮没回退已验收的行为）**

`mcp-switches` 全部通过 · `gateway-endpoints` **17/17** · `sync-intervals` 全部通过 ·
`probe-sticky` / `probe-fade` / `probe-models-cols` 正常出结果。
`cargo fmt --check` / `cargo clippy -D warnings` / `cargo test --workspace` 全绿
（234 单测 + 各集成文件，0 失败）。

**⑨ 两条**判据被修订**（按「判据本身不合理时必须先说明并给替代判据」的口径，在此留档）**

- **`partialCut`（折叠线切半）由「失败」改为「信息」**。原文要求
  「`partialCut` 里不得出现 input/button/textarea/select」，但长页面在固定视口下**必然**
  有控件跨在视口底边上（设置页内容 1848px / 可视 564px；模型页 1259/564），
  而且「底边露出半个控件」本身是**正常的滚动提示**（告诉用户下面还有内容），不是缺陷 ——
  该判据会稳定误报，且只能靠改正常布局来「修」，正是「为绿而改」的反面。
  **替代判据**：① **主操作**必须完整可见（不能跨在折叠线上，已实现为 `straddles` 检查，
  比原来更严）；② 跨线控件必须能靠滚动完整看到（`run.mjs` 的 `unreachable` 覆盖）；
  ③ 表格不得溢出容器（P1-5 的实质，已单列为判据）。`partialCut` 仍逐页打印，只是不再当失败。
- **`smallTargets` 从 `audit.mjs` 移除**。命中区判据曾有两个归属且结论互相矛盾
  （audit 报 42 处 / probe-hits 复核后大多合格）。现由 `probe-hits.mjs` 唯一归属，
  `audit.mjs` 内写了注释说明为什么它不再给这个判定。
- 另：`probe-hits.mjs` 自身修掉一个**测量 bug** —— 只判「元素是否在视口内」会漏掉
  「元素被可滚动祖先裁掉」的情况（抽屉正文底部），在模型页抽屉的「启用」开关上
  稳定误报「有效 0×0」。现同时判视口与祖先裁切，两者都不可达才标 `offscreen` 并排除出判定。

**⑩ 本轮的两条环境备注（不是本轮引入）**

- `scripts/release_check.sh` 的**第 1 步（工作区干净）在本轮无法通过**（这是**边界条件**，不是缺陷）：
  本轮改动按约定不提交、不打 tag，所以它必然停在 `FAIL: 工作区存在未提交/未暂存变更`（本轮实测确认）。
  其余步骤各自单独验证过（`node v24.19.0`）：`regression.sh` 的 `cargo fmt --check` **通过**、
  `cargo clippy -D warnings` **通过**、前端 `pnpm build`（含 `tsc --noEmit`）**通过**、
  新增的第 6 步 UI 门禁 **通过**（`gate.mjs` 退出码 0）。
- **另有两个与本轮无关、但同样会让 `release_check.sh` 红的前置条件**（实测确认，未处置）：
  ① 第 4 步会失败 —— `src-tauri/tauri.conf.json` 的版本仍是 `0.2.12`，而 tag `v0.2.12` 已存在
  （`git describe` = `v0.2.12-3-g78abf89`），脚本要求「先升级版本号」；
  ② `cargo test --workspace` 当前**有 1 个失败**：`codec::responses::tests::finish_safety_block_maps_to_content_filter`
  （`crates/gateway-core/src/codec/responses.rs:2764` 断言 `response.incomplete`）——
  这属于**工作区里另一个并行进行的工作流**（Responses API 的 `stop_reason` 落地：
  `crates/gateway-core/src/{codec/responses.rs,server/proxy.rs,store/logs.rs}` +
  `tests/m6_responses_inbound.rs`，diff 中出现 71 处 `stop_reason`），
  其文件 mtime 落在本会话时段内。**本 UI 专题未触碰任何 `crates/`、`src-tauri/` 文件**，
  故这一失败既不是本轮引入、也不应由本轮修（见边界「不改动 Rust 侧行为」）。
- `regression.sh` 里的 `pnpm build` 在本机默认 `node v18.9.0` 下会失败，报
  `corepack: TypeError: URL.canParse is not a function` —— corepack 需要 node ≥ 18.17。
  用仓库自己的 `node v24.19.0` 跑则全绿。**与本轮改动无关**，但会让「本机跑
  release_check.sh」在默认 PATH 下必然红，建议后续把 node 版本写进文档或加 `engines`。

**⑪ 本轮末段修掉的两个「门禁自己不可信」的问题**

- **「声明 sticky ≠ 真的吸顶」，且这条判据一开始是恒真的（假绿）。** 把「吸顶表头」写成可测量判据
  （真的滚 150px 看 `thead` 位移）后，实测发现**日志页的吸顶表头此前从未真正生效**；
  随后又发现新写的判据**跑不到**（被误插进 `if (cutPrimary.length) {…}` 内部，只有主操作被切半时
  才顺带检查），`stickyHeads` 恒缺 → 又一次「恒真的断言」。已移到正确层级并做负控制（⑤ 第 4 行）。
- **`fold.mjs` 崩溃而 `gate.mjs` 报「✓ 全部通过」。** 根因与对策见 4.2。
  修后实测：`tables` 字段恢复（模型页 900×600 表 674/容器 724、溢出 -50），
  且**探针崩溃时门禁必红**（判据「探针可运行」）。

## 5. 最小窗口尺寸：实测下限 + macOS 上的静默失效（2026-09-22）

> 触发来源：用户提出「估测一下最小窗口需要多少，把最小大小限制一下 —— 缩得太小 UI 会错乱/难看，
> 所有 UI 都要能正常展示」。
> 结论：**最小 900×600 不变**（这次是实测出来的下限，不是拍的），但**它在主力平台 macOS 上从未生效**，
> 本轮把这条限制真正落地，并把「不许再静默丢」变成一条门禁。

### 5.1 下限是怎么量出来的（不是估的）

判据用现有门禁的 `audit.mjs`（`truncated` = **被截断且没有 `title` 兜底**的文本，即信息真的丢了）。
固定高度 600、只改宽度：

| 宽度 | 1180 | **900** | 890 | 880 | 860 | 820 |
|---|---|---|---|---|---|---|
| `truncated` | 0 | **0** | 7 | 9 | 16 | 18 |
| `hScroll` | 0 | **0** | 0 | 0 | 0 | 0 |

**900 是「零信息丢失」的硬边界，且余量为 0px**（890 立刻掉 7 处）。低于 900 布局不会「崩」
（表格是流式的，`hScroll` 一直为 0），但会开始**静默截断**：`http://127.0.0.1:1314/v1/chat/completion`
这类长 URL 与技能描述被切掉且没有 tooltip 兜底 —— 正是用户说的「比较难看 / 信息看不到」。

高度方向宽松得多（弹窗自带 `max-h` + 内滚，自适应），固定宽度 900 只改高度：

| 高度 | 600 | 560 | 520 | 480 |
|---|---|---|---|---|
| `truncated` / `hScroll` / 弹窗越界 / 弹窗控件滚不到 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 |

⇒ 高度 600 不是「技术下限」而是**可用性下限**（首屏要放得下主操作条 + 双列卡片），与
§3 第 12 条（默认 1180×800、最小 900×600）的历史结论一致，故**不改这个数**。

### 5.2 真问题：macOS 上这个限制根本没生效（根因）

`tauri.macos.conf.json` 用 `app.windows` 覆盖窗口配置，而 Tauri 的平台配置合并走
**JSON Merge Patch（RFC 7396）**：**数组是整体替换，不是按下标逐字段合并**。
于是基础配置 `tauri.conf.json` 里那个窗口对象的 `label/title/width/height/minWidth/minHeight`
在 macOS 上被**全部丢弃**：

```rust
// 实测：tauri_utils::config::parse::read_from(Target::MacOS, "src-tauri")  —— 改动前
app.windows = [ { decorations, hiddenTitle, titleBarStyle, transparent, windowEffects } ]
//              ↑ 没有 minWidth/minHeight ⇒ macOS 上没有任何最小尺寸限制，可以拖到极小
//              ↑ 也没有 width/height ⇒ 窗口退回 Tauri 默认 800×600（而非设计值 1180×800）
```

为什么一直没被发现：§4 的双尺寸验收是在**浏览器探针**里按视口尺寸跑的（`gate.mjs --size=900x600`），
它验证的是「900×600 时 UI 正常」，**从来不是**「真实窗口拖不到 900×600 以下」。
即 §4 自己写的教训 —— **「最小窗口 900×600 从没被当成一等公民验收过」** —— 的又一处体现，
只不过这次漏的是「限制本身是否生效」。

### 5.3 处置

1. **`src-tauri/tauri.macos.conf.json`**：把被数组替换吃掉的 6 个键**显式补齐**
   （`label/title/width/height/minWidth/minHeight`），值取基础配置的设计值
   （1180×800 / 最小 900×600）。平台专有的 `decorations/titleBarStyle/hiddenTitle/transparent/windowEffects` 保持不变。
2. **新增 `scripts/tauri_window_check.mjs`（零依赖门禁）**，两条判据：
   - ① **键集完整性**：平台配置里的窗口对象必须重新声明基础配置里的**每一个**键
     （值可不同 —— `decorations`/`windowEffects` 正是故意按平台不同的）。以后谁在基础配置加一个窗口键、
     忘了同步平台文件，门禁立刻红，而不是在某个平台上静默丢。
   - ② **下限不低于 UI 验收尺寸**：解析后的 `minWidth/minHeight` ≥ UI 验收尺寸。
     **单一来源**：不硬编码 900×600，而是从 `gate.mjs` 读它的 `--sizes` 默认值取最小一组 ——
     改验收尺寸只需改 `gate.mjs` 一处；**正则失配时判据直接报错**（不允许静默变绿，见 §4 的教训）。
   已接进 `tools/visual-regression/gate.mjs` 的「1. 静态规范（零依赖）」段，即
   `release_check.sh` 第 6 步自动覆盖。

### 5.4 验证证据

**① 解析结果（`tauri_utils::config::parse::read_from` + 反序列化成 tauri `Config`，`deny_unknown_fields`）**

```
=== MacOS (files: ["tauri.conf.json", "tauri.macos.conf.json"])
  反序列化 OK: label="main" title="JAI Gateway" size=1180x800 min=Some(900.0)xSome(600.0) decorations=true
=== Windows (files: ["tauri.conf.json"])
  反序列化 OK: label="main" title="JAI Gateway" size=1180x800 min=Some(900.0)xSome(600.0) decorations=false
```

**② 门禁本身的正/负控制（证明它真的能红，不是装饰）**

| 场景 | 结果 |
|---|---|
| 修复后（正控制） | `✓ 最小窗口尺寸门禁通过（… 均为 900×600，默认 1180×800；UI 验收下限 900×600）` 退出码 0 |
| 回退成改动前那份 macOS 配置（负控制 1） | 红：`漏了基础配置里的 label, title, width, height, minWidth, minHeight` + `minWidth=undefined minHeight=undefined`，退出码 1 |
| 键齐全但 `minWidth/minHeight = 700×480`（负控制 2） | 红：`700×480 < 验收下限 900×600`，退出码 1 |
| 恢复后 | 绿，退出码 0 |

**③ 全量 UI 门禁**：`node tools/visual-regression/gate.mjs` → `✓ 全部通过`
（新增的静态判据出现在输出里：`── scripts/tauri_window_check.mjs（最小窗口尺寸）`）。

**④ 编译期校验**：`cargo check`（`generate_context!` 会在编译期解析平台配置）通过。

> 备注（与 §4 ⑩ 同口径）：本轮**只改配置与门禁**，未触碰 `crates/` 与 `src-tauri/src/` 的任何 Rust 行为；
> 工作区里 `crates/gateway-core/**` 的改动来自另一个并行工作流，与本轮无关。


## 6. 按钮反馈的落点：从「都堆在页面顶部」到「就近 / 吸顶」（2026-09-24）

> 触发来源：用户提出「按钮的 tips，现在有些反馈都在最顶部，窗口上下有滚动条。还得滚到上面或者下面去看提示。很恶心」。
> 结论：**toast 本身没问题**（实测视口固定，见 6.1）；真问题是页内 `msg/err` 的锚点写死在
> `PageHeader` 之后，而触发它的按钮常在列表下方。本轮把落点规则收敛成两条，并按**触发按钮的层级**
> 自动选边，同时把「错误不自动消失」变成一条可断言的规矩。

### 6.1 先排除误诊：toast 是视口固定，不是它的错

`[data-sonner-toaster]` 实测（1180×800）：

```
position = fixed                 li rect = 723→776（视口高 800，完全在内）
transform 祖先 = []（逐个祖先的 transform / filter / perspective / contain:paint / willChange 全查过）
```

⇒ 用户看到的「要滚上去看」**不是 toast**，而是**页内那条横幅**：它锚在页面顶部，触发它的按钮
在列表下方，所以点完必须滚回顶部。

### 6.2 真问题：5 个页面把反馈锚在 `PageHeader` 之后（实测距离）

同类问题在 5 个页面同时存在（网关 / 同步 / MCP / 技能 / 供应商）。把页面源码回退到整改前
（`git stash` 前后各测一次）、1180×800、**MCP 夹具扩到 10 个 server 让列表真的滚动**：

| 场景 | 点击时滚动位置 | 反馈出现位置 | 结论 |
|---|---|---|---|
| MCP 页点**最后一行**的「列出工具」 | 404px（可滚 404） | 页面顶部（非吸顶）`top=-236` | **需滚回 272px** |
| 同步页点「推送」（WebDAV 操作条） | 458px（可滚 969） | 页面顶部（非吸顶）`top=-330` | **需滚回 366px** |

同一场景整改后：

| 场景 | 点击时滚动位置 | 反馈出现位置 | 结论 |
|---|---|---|---|
| MCP 页点最后一行「列出工具」 | 310px | **该行内** | 不用滚 |
| 同步页点「推送」 | 356px | **吸顶条里**（`top=125`，视口 36~800） | 不用滚 |

> 说明：`ProvidersPage` 一直把反馈放在**产生它的那张卡片**里，落点本来就是对的 —— 这次只是把它
> 推广到其余 4 个页面，并把「行级按钮」单独拆出来。

### 6.3 规则：按触发按钮的层级选边 + 「每页只留一个 sticky」

1. **行级按钮**（列表某一行的「列出工具」等）→ 反馈落在**该行内**（`FeedbackLine` 渲在该行卡片里），
   key 用行 id。**不许**再汇总到页面顶部 —— 那正是被投诉的行为。
2. **页级 / 卡片级按钮** → 反馈**吸顶**（`sticky top-0`）或并入页面已有的吸顶操作条，跟着滚动条走。
3. **错误 `role="alert"` 一律不自动消失**（可手动关），成功 / 确认类 2.4s 自动收。
4. **每个页面只保留一个 `sticky top-0` 区域**：两个 `top-0` 的 sticky 会叠在一起，DOM 靠后的那个
   把前面那个盖住。同步页的 WebDAV 操作条因此从卡片内**上移**到页面顶部，与反馈条合成同一个容器；
   同时**把 WebDAV 卡移到最前** —— 操作条作用的就是这张卡的字段，它上移后必须紧邻被作用的对象，
   否则首屏看到的是「保存配置」孤零零挂在一张无关的卡上面（这条也是实测出来的：第一版上移后
   首屏顺序是「操作条 → 导入导出 JSON 卡 → WebDAV 卡」，按钮与字段被整整一张卡隔开）。

### 6.4 处置

- 新增 `ui/src/components/common/PageFeedback.tsx`：`FeedbackLine`（行内 / 吸顶条共用，带
  `aria-label="关闭提示"`、`data-testid`、`data-kind`、关闭钮真实盒子 24×24）、`StickyFeedback`
  （**只给页面上没有别的吸顶条的页面**：MCP / 技能 / 供应商）、`useFeedback()`（一个 map：页级 key
  `__page__`、行级 key = 行 id；`OK_TTL_MS = 2400` 只作用于成功类；卸载清定时器）。
- `ui/src/lib/toast.ts`：错误改 `sonnerToast.error(msg, { duration: Infinity, closeButton: true })`。
  `duration: Infinity` 是 sonner 明确支持的写法（它内部专门判 `toast.duration === Infinity` 并跳过
  `setTimeout` —— 因为 `setTimeout(fn, Infinity)` 会因延迟溢出被当成 0，即立刻触发）。
- `ui/src/index.css`：`[data-sonner-toast] [data-close-button] { pointer-events: auto; }` ——
  `Toaster` 整体是 `pointer-events: none`（防「toast 吞掉底下控件的点击」，见 §1 的 P0-3 修复），
  只恢复这个小圆钮。
- 5 个页面的 `msg/err` 状态全部换成 `fb`（`pageOk` / `pageErr` / `rowOk` / `rowErr` / `dismiss`）。
- 新增探针 `tools/visual-regression/probe-feedback.mjs`，判据集中写在 `gate.mjs`。

### 6.5 判据与变异验证（负控制）

判据 5 条（都读 computed style 或实测滚动 / 时长，不靠类名猜）：

| # | 判据 | 怎么量 |
|---|---|---|
| ① | 页级反馈**吸顶** | 反馈元素往上找最近的 sticky 祖先，读它的 `position` |
| ② | **滚到页面最底仍在视口内** | `main.scrollTop = scrollHeight` 后重读 rect，比 `clientHeight` |
| ③ | 错误**不自动消失**且可手动关 | 等 3s 仍在 → 点「关闭提示」→ 消失 |
| ④ | 错误 **toast 不自动消失** + 关闭钮可点 | 等 4.6s 仍在、有 `[data-close-button]`、`pointer-events === "auto"`、点它后消失 |
| ⑤ | 行级反馈落在**该行内**、页级条为空 | 按 `[data-testid=mcp-row]` 的下标核对反馈所在行 = 被点的那一行 |

**5 处分别改坏，门禁全部变红**（单尺寸单主题跑 `gate.mjs`）：

| 改坏什么 | 红的判据 |
|---|---|
| 页级吸顶容器去掉 `sticky top-0` | ① `页级反馈吸顶`、② `页级反馈滚到底仍可见` |
| 把 `OK_TTL_MS` 也套到错误上（`kind === "ok" \|\| kind === "err"`） | ③ `错误反馈不自动消失`、`错误反馈可手动关闭` |
| 错误 toast 去掉 `duration: Infinity` | ④ `错误 toast 不自动消失`、`错误 toast 有关闭按钮` |
| MCP「列出工具」结果改回 `fb.pageOk` | ⑤ `行级反馈落在该行内`、`行级反馈不再汇总到页面顶部` |
| `index.css` 删掉 `[data-close-button]{pointer-events:auto}` | ④ `错误 toast 的关闭按钮可点`（`pointer-events=none`） |

> 顺带发现并修掉一个**判据自身的不严谨**：错误 toast 那条原先只等 3s，而 `duration: Infinity`
> 一旦被去掉，兜底链是 `Toaster` 的 2400ms → sonner 自家的 **4000ms**。只等 3s 时，**同时**去掉
> 这两处就会假绿（4000 > 3600）。改成等 4.6s（长于兜底默认值），判据才真正成立。

### 6.6 顺带修掉的三处（2026-09-23 遗留 + 本轮发现）

| # | 问题 | 根因 | 验证 |
|---|---|---|---|
| 1 | `mcp-switches.mjs` / `sync-intervals.mjs` 自 D9-T6a 起**一直崩**（切页时「等侧边栏按钮超时」） | 探针自带极简 mock 的 `default: return null` → 新增的 `gateway_key_list` 返回 `null` → `setKeys(null)` 渲染抛错 → 整棵 React 树卸载 | 补上 `gateway_key_list` / `gateway_key_rules_get` 两个 case 后，两个探针恢复通过 |
| 2 | 密钥规则弹窗「拒绝」选中态在浅色主题**对比度 3.87 < 4.5** | `text-destructive` on `bg-destructive/10` | 改实心底 + 白字，audit 通过 |
| 3 | 密钥前缀按钮（94×16）与规则 chip（56×22）**真实盒子 < 24px** | 只有文字撑高 | 加 `inline-flex min-h-6 items-center`，`probe-hits` 通过 |
