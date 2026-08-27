# 协议中间表示（IR）设计 — v1

> 状态：已评审定稿（2025-08）。本文档是协议转换层（`gateway-core::codec`）的开发对照权威。
> 上游需求：见《JAI — 桌面 AI API 网关.md》「架构核心」一节。

## 1. 总原则

### 原则一：双轨制 —— 同族直通，跨族才转换

| 场景 | 路径 |
| --- | --- |
| 入站 OpenAI 族 → 出站 OpenAI 族供应商 | **字节级直通**：仅改写上游 URL 与鉴权头，body 不动 |
| 入站 Anthropic → 出站 Anthropic（Claude Code → Claude 官方） | 同上 |
| 跨族（OpenAI ↔ Anthropic ↔ Gemini） | 解码 → IR → 编码 |

直通的收益：零损耗（cache_control、citations 等 IR 未建模特性原样保留）、性能最优、bug 面最小。
日志采集：直通路径上做**旁路轻量扫描**（SSE 尾部抓 usage 数字），不做语义级解析。

### 原则二：先归一化三大结构性差异，再建类型

三个协议不是「字段名不同」，而是「结构位置不同」：

| 差异点 | OpenAI | Anthropic | Gemini |
| --- | --- | --- | --- |
| system 提示词位置 | 消息数组内的 role:`system` | 请求顶层 `system` 参数 | 顶层 `systemInstruction` |
| tool 结果载体 | 独立消息 role:`tool` | 后续 user 消息内的 `tool_result` 块 | user 轮次的 `functionResponse` part |
| 结束原因 | `finish_reason` | `stop_reason`（枚举值另一套） | `finishReason`（第三套） |

因此 IR 中：system 独立于消息列表；tool 结果是一种内容块；stop reason 是统一枚举。

## 2. 核心 Rust 类型

```rust
/// 协议家族：同族才允许直通
enum Family { OpenAiCompat, Anthropic, Gemini }

// ============ 请求 IR ============
struct CanonicalRequest {
    model: String,
    system: Vec<String>,          // 多条 system 按序合并，渲染时以 \n\n 连接
    messages: Vec<CanonMessage>,
    tools: Vec<ToolSpec>,
    tool_choice: ToolChoice,
    params: SampleParams,
    stream: bool,
    extensions: Extensions,       // 未建模字段的兜底容器，见 §7
}

struct CanonMessage {
    // 注意：没有顶层 role=ToolResult —— tool 结果由 Block::ToolResult 承载，
    // 其宿主消息的角色（OpenAI: 独立 role=tool / 其他: user）由出站渲染器决定
    role: Role,                   // User | Assistant
    blocks: Vec<Block>,
}

/// 内容块采用 type-tag 枚举，对齐 Anthropic 的 block 原语
/// （三者中表达力最强，作 IR 载体损耗最小）
#[serde(tag = "type", rename_all = "snake_case")]
enum Block {
    Text       { text: String },
    Image      { media_type: String,
                 data_base64: Option<String>,   // 二选一必填（MVP 承诺 base64 场景）
                 url:         Option<String> }, // 跨族渲染规则见 §6-D
    ToolUse    { id: String, name: String, input: serde_json::Value },
    ToolResult { call_id: String, content: Vec<Block>, is_error: bool },
    /// 占位，v1 不产出；思考内容跨族语义不等价，暂不做转换承诺
    Thinking   { signature: Option<String>, text: String },
    /// 家族特有块的保底逃生舱：跨族转换时按 §7 策略丢弃并 WARN
    FamilyRaw  { family: Family, value: serde_json::Value },
}

struct ToolSpec { name: String, description: Option<String>, input_schema: serde_json::Value }

enum ToolChoice { Auto, None, Required, Specific(String) }

struct SampleParams {
    max_output_tokens: Option<u32>,   // Anthropic 出站为必填，缺省由 encoder 填模型配置值
    temperature:       Option<f32>,
    top_p:             Option<f32>,
    top_k:             Option<u32>,   // OpenAI 系出站静默忽略（WARN 一次）
    stop_sequences:    Vec<String>,
    frequency_penalty: Option<f32>,   // 非 OpenAI 出站忽略
    presence_penalty:  Option<f32>,
    seed:              Option<i64>,
}

// ============ 响应 IR ============
struct CanonicalResponse {
    id: String,
    model: String,
    output: Vec<Block>,               // 通常为 [Text] 或 [Text.., ToolUse..]
    stop_reason: StopReason,
    usage: Usage,
}

/// 统一结束原因（映射唯一权威见 §5-C）
enum StopReason { EndTurn, MaxTokens, ToolUse, SafetyBlock, Other(String) }

struct Usage {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens:  Option<u64>,  // 日志展示用，缺协议则 None
    cache_write_tokens: Option<u64>,
}

// ============ 流式事件 IR ============
enum StreamEvent {
    Start             { model: String },
    TextDelta         { text: String },
    ThinkingDelta     { text: String },
    ToolCallStart     { index: usize, id: String, name: String },
    ToolCallArgsDelta { index: usize, args_fragment: String },  // JSON 文本片段
    ToolCallEnd       { index: usize },
    Finish            { stop_reason: StopReason, usage: Usage },
}
```

### 流式排序约束（实现时的重点坑位）

- **Anthropic 序列严格嵌套顺序**：block i 必须 Start → deltas → Stop 完结后才开始 i+1；
- **OpenAI 分片可交错**：多个 tool call 的参数增量可穿插到达；
- 因此 IR 事件只按 `index` 寻址，**不施加嵌套纪律**；校验/重排责任在「渲染至 Anthropic」方向的 Codec（攒缓冲后按序吐出）。
- **Gemini 的 functionCall 是整对象到达**（不分片）：上游解码时切成单次 `ArgsDelta`；目标为 Gemini 时需聚合成整对象一次发出。两端均在 Codec 层消化，IR 不感知。

## 3. 能力支持面声明（超出即拒绝）

以下入站字段 MVP **不支持**，返回明确的 400 错误（错误中列出受支持的子集）：

| 入站字段 | 原因 |
| --- | --- |
| `n > 1`（多候选） | 三方均需流式多路复用，复杂度不值 |
| `logprobs` / `top_logprobs` | 仅 OpenAI 有；直通路径不受影响，跨族拒绝 |
| `response_format`(json_schema)，目标为跨族时 | 用户已拍板：报错而非指令注入（OpenAI 族目标仍透传） |
| `reasoning_effort`、`verbosity` 等 o 系列参数 | 随 Thinking 支持一起后置 |
| 音频输入/输出、视频输入 | 超出本期范围 |

## 4. 请求参数逐字段映射表

约定列名：O = OpenAI Chat Completions，A = Anthropic Messages，G = Gemini `generateContent`。

### A. 采样与控制参数

| IR 字段 | O | A | G |
| --- | --- | --- | --- |
| `model` | body `model`（必填） | body `model`（必填） | URL 路径 `/v1beta/models/{model}:generateContent`（`models/` 前缀剥除） |
| `max_output_tokens` | `max_completion_tokens`（兼容回落 `max_tokens`） | `max_tokens` **必填** | `generationConfig.maxOutputTokens` |
| `temperature` | [0, 2] | [0, 1]，越界截断并 WARN | [0, 2] |
| `top_p` | `top_p` | `top_p` | `generationConfig.topP` |
| `top_k` | ✗ 忽略 | `top_k` | `generationConfig.topK` |
| `stop_sequences` | `stop`（≤4 条） | `stop_sequences` | `generationConfig.stopSequences` |
| `frequency_penalty` | 支持 | ✗ 忽略 | ✗ 忽略 |
| `presence_penalty` | 支持 | ✗ 忽略 | ✗ 忽略 |
| `seed` | 支持 | ✗ 忽略 | `generationConfig.seed` |
| `stream` | true ⇒ SSE | true ⇒ SSE | `alt=sse` 查询参数 |
| usage 注入 | 出站注入 `stream_options:{include_usage:true}`，回传前剥除（客户端未要求时） | usage 随 `message_delta` 天然携带 | `usageMetadata` 天然携带 |

### B. 工具调用

| 语义 | O | A | G |
| --- | --- | --- | --- |
| 工具定义 | `tools:[{type:"function",function:{name,description,parameters}}]` | `tools:[{name,description,input_schema}]` | `tools:[{functionDeclarations:[{name,description,parameters}]}]`，parameters 仅接受 OpenAPI 子集（encoder 需做 JSON Schema 子集校验/裁剪） |
| choice:auto | `"auto"` | `{type:"auto"}` | `toolConfig.functionCallingConfig.mode:"AUTO"` |
| choice:none | `"none"` | 省略 `tools` 字段整段 | mode:`NONE` |
| choice:required | `"required"` | `{type:"any"}` | mode:`ANY` |
| choice:specific(fn) | `{type:"function",function:{name:X}}` | `{type:"tool",name:X}` | mode:`ANY` + `allowedFunctionNames:[X]` |
| 发起调用（IR ToolUse） | assistant 消息 + `tool_calls:[{id,type,function:{name,arguments:<字符串化 JSON>}}]` | assistant 块 `{type:"tool_use",id,name,input:<JSON 对象>}` | model 轮 part `{functionCall:{name,args:<对象>}}` ⚠️ **无 id** |
| 返回结果（IR ToolResult） | 独立消息 `role:"tool"` + `tool_call_id` + content | 后续 user 消息块 `{type:"tool_result",tool_use_id,content,is_error}` | user 轮 part `{functionResponse:{name,response}}` ⚠️ **按 name 关联** |
| 并行调用 | 单消息多个 tool_calls（arguments 各自分片流式到达） | 单 assistant 消息多个 tool_use 块 | 单 model 轮多个 functionCall parts |

**Gemini 无调用 id 的对策**：上游解码时由 JAI 合成 id（`fn名` 冲突时追加 `_k{n}`），下游渲染时丢弃 id 用 name 关联。该合成逻辑隔离在 Gemini Codec 内部。

**ID 跨轮次无状态映射**：出站侧生成的 tool_use id 采用确定性内嵌编码 `"toolu_" + base58("jai1." + inbound_id)`，客户端历史回传时反解复原；超长（Anthropic 上限 64 字符）时落入 SQLite `tool_id_map` 表兜底。

### C. 结构合规变换（各 Encoder 的义务）

| 规则 | 适用方向 |
| --- | --- |
| 相邻同角色消息必须合并（文本块以 `\n\n` 衔接） | →A、→G（二者强制 user/assistant 交替；O 无此约束） |
| 多条 `role:tool` 结果必须聚合为同一条后续 user 消息/A 的连续 tool_result 块/G 的同轮连续 parts | →A、→G |
| tool_result 与后续纯文本同属一轮时，保持 result 块在前 | →A（Anthropic 要求 tool_result 先于其他块） |
| system 多条合并 | →A、→G 收敛为单一顶层字段；←O 时还原为首条消息 |

## 5. 响应侧映射表

### C. StopReason

| IR | O `finish_reason` | A `stop_reason` | G `finishReason` |
| --- | --- | --- | --- |
| EndTurn | `stop` | `end_turn` | `STOP` |
| MaxTokens | `length` | `max_tokens` | `MAX_TOKENS` |
| ToolUse | `tool_calls` | `tool_use` | `STOP`（输出含 functionCall 时**推断升级**为 ToolUse） |
| SafetyBlock | `content_filter` | `refusal` | `SAFETY` / `PROHIBITED_CONTENT` / `RECITATION` |
| 其他值 | 记入 `Other(raw)`，渲染为最近似类别并 WARN | 同左 | 同左 |

### D. Usage

| IR | O | A | G |
| --- | --- | --- | --- |
| input | `usage.prompt_tokens` | `usage.input_tokens` | `usageMetadata.promptTokenCount` |
| output | `usage.completion_tokens` | `usage.output_tokens` | `usageMetadata.candidatesTokenCount (+thoughtsTokenCount)` |
| cache read | `prompt_tokens_details.cached_tokens` | `cache_read_input_tokens` | — |
| cache write | — | `cache_creation_input_tokens` | — |

### E. SSE 事件对应

| IR StreamEvent | O chunk | A event | G（alt=sse 包） |
| --- | --- | --- | --- |
| Start | 首 chunk（id/model，delta.role=assistant） | `message_start` | 首包（常带 prompt 计数的 usageMetadata） |
| TextDelta | `delta.content` | `content_block_delta(text_delta)` | `candidates[0].content.parts[i].text` |
| ToolCallStart+Args | `delta.tool_calls[]{index,id?,function{name?,arguments"<片段>"}}`（id/name 仅首片） | `content_block_start(tool_use)` + `input_json_delta` | parts 中的整体 `functionCall` ⇒ 解码为 Start+完整 Args+End |
| ToolCallEnd | 无显式事件（下一 index 隐式收尾） | `content_block_stop` | — |
| Finish | 末 chunk `finish_reason`，随后 `[DONE]` | `message_delta`(stop_reason+usage) → `message_stop` | 尾包 `finishReason` + 最终 usageMetadata |
| 错误 | SSE `data:{"error":…}` | `event: error` | 包体 `{"error":{code,message,status}}` |

## 6. 错误 IR 与渲染

内部统一错误：`{ kind, provider_status, provider_body_excerpt }`，kind ∈ `GatewayAuth | UpstreamAuth | RateLimit | InvalidRequest | ContextTooLong | Overloaded | ProviderOther`。

| kind | HTTP(→O 客户端) | O error.type/code | HTTP(→A 客户端) | A error.type |
| --- | --- | --- | --- | --- |
| GatewayAuth（sk-jai 校验失败） | 401 | invalid_request_error / invalid_api_key | 401 | authentication_error |
| UpstreamAuth | 401* | apiproxy 侧如实转发语义 | 401* | authentication_error* |
| RateLimit | 429 | rate_limit_error，附 Retry-After | 429 | rate_limit_error |
| InvalidRequest | 400 | invalid_request_error | 400 | invalid_request_error |
| ContextTooLong | 400 | invalid_request_error / context_length_exceeded | 400 | invalid_request_error（message 注明上下文超限） |
| Overloaded | 503 | api_error | **529** | overloaded_error（保留 Anthropic 特有状态码） |

\* 跨族时将供应商错误**转译为入站方言**再回传；HTTP 状态码按本表对齐。直通模式不经过此处。

## 7. 未知字段策略（Extensions）

- 所有解码阶段无法归类进 IR 的字段收集进 `extensions`（serde flatten JsonMap，家族标签注明来源 family）。
- **默认 Lenient**：跨族转换时静默丢弃，每请求汇总一条 WARN 日志（列出被降级字段名清单）。
- 设置页可切 **Strict**：存在无法建模字段即 400，message 明确指出字段名与目标协议不兼容。
- 回传方向同理：上游响应中 IR 未覆盖的字段，在同族直通不受影响；跨族时丢弃。

## 8. Codec 接口形状

```rust
trait InboundCodec {   // 每 Family 一个实现
    fn decode_request(&self, body: Bytes) -> Result<CanonicalRequest>;
    fn render_response(&self, r: &CanonicalResponse) -> Result<Bytes>;
    fn render_stream_event(&self, e: &StreamEvent, st: &mut RenderState) -> Option<SseChunk>;
}

trait UpstreamCodec {  // 每 (Family × target_model_capabilities) 一个实现
    fn encode_request(&self, r: &CanonicalRequest, ep: &Endpoint) -> Result<HttpTemplate>;
    fn parse_response(&self, body: Bytes) -> Result<CanonicalResponse>;
    fn parse_stream_chunk(&self, c: &SseChunk) -> Result<Vec<StreamEvent>>;
}

enum RouteDecision {
    Passthrough,                      // 同族：字节代理 + 旁路 usage 抓取
    Convert(Box<dyn UpstreamCodec>),  // 跨族：走 IR
}
```

鉴权头写法：O=`Authorization: Bearer …`；A=`x-api-key` + `anthropic-version`（直通时原样透传版本头）；G=优先 `x-goog-api-key` 头（不用 query `key=`，避免落入代理访问日志）。OpenRouter 类兼容站的扩展头（HTTP-Referer/X-Title 等）经 FamilyRaw/供应商级 headers 配置透传。

## 9. 测试策略

1. **黄金夹具矩阵**：夹具集 = { 纯文本单轮、带 system、带图(base64)、tool 定义+单调用、多工具结果多轮、错误(429/529/context超限)、长流式交错 tool calls }；跑满 5 条转换链路，断言语义等价 + 关键字节形状。
2. **性质测试**：`decode(encode(ir)) == ir` 往返不变性；`render(parse(x))` 在同族对上幂等。
3. **直通回归**：直通路径断言「body 未被触碰」（哈希对比）。
4. 本地各协议 mock server（写入 fixtures 的重放脚本），CI 不打真实供应商 API。

## 10. 已知边界与未决项

| # | 事项 | 当前决策/说明 |
| --- | --- | --- |
| 1 | `POST /v1/messages/count_tokens`（Claude Code 会调用） | MVP 返回粗估（chars/4 级别），避免 CC 降级报错；精确实现随用量统计一起做 |
| 2 | http(s) 图片 → Gemini | Gemini 不接受任意外链（fileData 仅限 GCS URI），Codec 需拉取转 inlineData/base64；拉取失败明确报错 |
| 3 | thinking/signature 跨族 | v1 仅占位存储不转换；直通不受影响 |
| 4 | Anthropic prompt caching 标记（cache_control） | 仅同族直通有效；跨族丢弃并 WARN |
