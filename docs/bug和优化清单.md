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

- [ ] 6. `mcp_pool` 集成测试负载敏感 + **失败级联**（会污染 `scripts/regression.sh` 门禁）
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

- [ ] 7. Responses **出站**丢弃消息级图片（`Block::Image` 在 user 消息里）
  - 位置：`crates/gateway-core/src/codec/responses.rs` 的 `encode_request`，注释写「Responses 上游 v1 不支持
    图片内联转换，Lenient 丢弃」——**该注释与官方 schema 存疑**：Responses 的 message content 支持
    `input_image`（依据见 `docs/design/tool-result-image-protocol-factcheck.md`）。
  - 与「工具结果内嵌图片」（本次已修）属同一类静默丢失，但**本次未动**：改动会让原本「静默丢图但请求成功」
    的请求变成「带图请求」，若某些中转上游不接受可能由 200 变 4xx，需单独评估。
  - 建议：先确认目标上游对 `input_image` 的接受度，再按与本次相同的「原生承载 / 显式降级 + CapabilityWarn」口径处理。

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



## 2. 优化清单

- [x] 1. 创建供应商弹框应该有按钮可以测试能不能获取到模型。
- [x] 2. skill 添加应该支持 zip 导入添加。
- [x] 3. UI/UX 优化：按 `docs/ui优化.md` 的 66 项建议逐项实施并勾选。
- [x] 4. 多供应商时，dsh 模型列表显示“供应商名/模型名”，请求支持按该限定 ID 路由。
- [x] 5. MCP 支持粘贴标准 `mcpServers` JSON 导入（Claude Code 格式）
  - 说明：`{"mcpServers":{name:{command,args,env,url,type}}}` 粘贴即导，同名更新、不合法条目跳过并报告；
    新增迁移 0005（`mcp_servers.env` 列），stdio 启动子进程时注入 env；导出客户端配置同步带 env。
  - 验证：131 测试全绿 + 前端构建通过。

## 3. 视觉回归（默认窗口 980×640）

> 详见 [`docs/视觉回归整改plan.md`](视觉回归整改plan.md)（含 303 步动态点击遍历、715 张截图、对比度/命中区/折叠线量化数据）。
> 完成一项就把 `[ ]` 改成 `[x]`。

- [x] 1. P0 供应商「添加/编辑」弹窗主按钮打开即不可见：弹窗内容 620–780px 挤进 542px 可滚区，`创建/保存/测试连接/取消` 在可视框外（`ProvidersPage.tsx:508` 把 `max-h-[85vh] overflow-y-auto` 加在整个 DialogContent 上）
- [x] 2. P0 弹窗基座 `dialog.tsx` 缺 `max-h`/可滚动正文结构：内容一长（长技能正文、MCP env 行、大 JSON 导入）就溢出视口且无法滚动（760×520 下 MCP 添加弹窗 h=528 > 520，上下各被切 4px）
- [x] 3. P0 toast 遮挡底部控件：`bottom-center` toaster（z=999999999，rect x312–668/y565–619）在遍历中造成 150 次「被遮挡点不到」，含弹窗内输入框与 `检查更新`/`测试连接`；建议改 `bottom-right` + z-index 降到 40
- [~] 4. P1 主操作在首屏之外（同步页已修：操作条吸顶；设置页/网关页待做）：同步页首屏曾仅 38%（`保存配置/测试连接/预览变更/推送/拉取` 全在 y=761）、日志 21%、设置 34%、模型 36%、网关 55%；760×520 下同步 28%、供应商 39%
- [~] 5. P1 模型表：已确认由 Table 基座横向内滚 + 加窄窗口提示；列降级待评估。原：760 宽窗口溢出 217px，行内输入被折叠线切 9–14px
- [x] 6. P1 日志页 2931px 长表：`加载更多` 在 y=2870，表头无 sticky
- [x] 7. P2 暗色主题主按钮对比度 2.59:1（`--primary` #00A6F4 + `--primary-foreground` #FAFAFA），AA 要求 4.5:1；浅色主题同按钮 5.03:1 合格
- [~] 8. P2 浅色主题小字/状态色：600→700 级 + 日志状态码 700/800 + 提示条/toast 改前景色；79 类 → 剩余约 2 类（设置页主按钮 4.27 待复核）。原 79 类：badge「缺少凭据」3.2、日志状态码「429/499」3.38、同步「成功」3.55、设置说明 3.65、toast 文案 4.26
- [~] 9. P2 命中区：Switch 已用伪元素扩到约 44×36、弹窗关闭 16→24；行内复制/勾选/官网链接待做。原文：Switch 32×18、弹窗关闭 16×16、行内复制 16×16、批量勾选 16×16、弹窗内协议 `select` 1×1（几乎不可点）
- [~] 10. P2 字号：2 处 10px → 11px；URL 截断待加 title。原文：2 处 10px 文本；MCP 注册 URL 截 22px、供应商 base URL 截 21px
- [ ] 11. P3 折叠线硬切与顶部无渐隐：卡片被切半（`客户端接入` 398–660、WebDAV 348–1617）、内容滚动时被标题栏切开
- [ ] 12. P3 默认窗口偏小：内容普遍需要 1100–1900px 高，建议默认 1080×740、`minHeight` 520→560
