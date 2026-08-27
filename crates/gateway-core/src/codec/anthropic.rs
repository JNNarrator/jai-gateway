//! Anthropic 族编解码助手（M3 范围：直通路径旁路 + count_tokens + 错误形状）。
//!
//! 与 `openai` 模块对称：直通模式 body 字节不改动，因此只做三件事：
//! - [`peek`]：轻量解析请求关键字段（model/stream）用于路由与日志
//! - [`count_tokens`]：粗估 input token 数（roadmap M3 验收 3：
//!   `ceil(chars/4)` + system/tools 开销常量，避免 Claude Code 降级）
//! - [`error_body`]：Anthropic 错误响应形状

use serde_json::{json, Value};

/// Anthropic API 默认版本头（缺省注入）。
pub const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";

/// 从 Anthropic 请求体提取路由所需最小字段。
#[derive(Debug, Clone)]
pub struct PeekRequest {
    pub model: String,
    pub stream: bool,
}

pub fn peek(body: &[u8]) -> Result<PeekRequest, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("请求体不是合法 JSON: {e}"))?;
    let model = v
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if model.is_empty() {
        return Err("缺少 model 字段".into());
    }
    // Anthropic 的流式标志是 stream: true（非 SSE 形态时缺省 false）
    Ok(PeekRequest {
        model,
        stream: v.get("stream").and_then(Value::as_bool).unwrap_or(false),
    })
}

// ================================================================ count_tokens 粗估

/// 估算请求的 input token 数（roadmap M3：粗估即可，不承诺精度）。
///
/// 估算公式（对齐 CC 客户端预期量级）：
/// - 每个文本块 `ceil(chars / 4)`（英文约占 4-5 chars/token，中文略多但可接受）
/// - 每条消息固定开销 4 tokens
/// - `system` 与 `tools` 定义按同样的 chars/4 估算 + 每条固定开销
pub fn count_tokens(body: &[u8]) -> Result<serde_json::Value, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("请求体不是合法 JSON: {e}"))?;

    let mut total: u64 = 0;

    // 消息内容
    if let Some(messages) = v.get("messages").and_then(Value::as_array) {
        for m in messages {
            total += 4; // 每条消息固定开销
            total += block_tokens(m);
        }
    }

    // 顶层 system（字符串或块数组）
    if let Some(system) = v.get("system") {
        total += 4;
        total += block_tokens(system);
    }

    // tools 定义
    if let Some(tools) = v.get("tools").and_then(Value::as_array) {
        for t in tools {
            total += 8; // 每条工具定义固定开销（名字+结构）
            total += json_chars_tokens(t);
        }
    }

    Ok(json!({ "input_tokens": total }))
}

/// 统计单个块/消息的文本字符数并折算 token。
fn block_tokens(v: &Value) -> u64 {
    match v {
        // Anthropic 块：{"type":"text","text":"..."} / {"type":"tool_result","content":...}
        Value::Object(map) => {
            let mut n = 0u64;
            if let Some(t) = map.get("text").and_then(Value::as_str) {
                n += ceil_chars4(t);
            }
            if let Some(content) = map.get("content") {
                n += block_tokens(content);
            }
            n
        }
        // 字符串（system 直接给字符串的情况）
        Value::String(s) => ceil_chars4(s),
        // 数组（多块）
        Value::Array(arr) => arr.iter().map(block_tokens).sum(),
        // 其他结构（tool 输入等）按 JSON 序列化长度估算
        other => json_chars_tokens(other),
    }
}

/// 任意 JSON 值的紧凑序列化长度折算。
fn json_chars_tokens(v: &Value) -> u64 {
    let s = serde_json::to_string(v).unwrap_or_default();
    ceil_chars4(&s)
}

fn ceil_chars4(s: &str) -> u64 {
    let chars = s.chars().count() as u64;
    chars.div_ceil(4)
}

/// f32 采样参数精度归一：保留 6 位有效小数，避免 f32 存储噪声
/// （如 0.9 → 0.899999976）污染上游请求体。
pub(crate) fn round_f32(v: f32) -> f64 {
    ((f64::from(v) * 1e6).round()) / 1e6
}

// ================================================================ 错误形状

/// Anthropic 错误响应体（protocol-ir §6 / M3：错误 Anthropic 化）。
pub fn error_body(message: &str, err_type: &str) -> Value {
    json!({
        "type": "error",
        "error": {
            "type": err_type,
            "message": message,
        }
    })
}

// ================================================================ M4：UpstreamCodec::Anthropic

// 转换路径：CanonicalRequest → Anthropic body；Anthropic 响应 → CanonicalResponse / StreamEvents。
// 依据 protocol-ir §4/§5 映射表；结构合规在 ir::merge_adjacent_same_role 预处理后展开。

use crate::codec::ir::{
    Block, CanonMessage, CanonicalRequest, CanonicalResponse, Role, SampleParams, StopReason,
    StreamEvent, ToolChoice, ToolSpec, Usage,
};

/// 编码请求：IR → Anthropic Messages body。
/// `max_output_tokens` 由调用方保证（请求侧缺省时用模型配置值）。
pub fn encode_request(req: &CanonicalRequest) -> Result<Value, String> {
    if req.system.len() > 1 {
        return Err(format!(
            "system 段数为 {}，跨族转换仅支持单条合并 system",
            req.system.len()
        ));
    }

    let mut body = json!({
        "model": req.model,
        "messages": [],
    });

    if let Some(sys) = req.system.first() {
        body["system"] = Value::String(sys.clone());
    }
    if let Some(m) = req.params.max_output_tokens {
        body["max_tokens"] = json!(m);
    }
    let p = &req.params;
    if let Some(t) = p.temperature {
        let clamped = t.clamp(0.0, 1.0);
        if clamped != t {
            eprintln!("[anthropic] temperature 越界截断 {t} → {clamped}（WARN）");
        }
        body["temperature"] = json!(round_f32(clamped));
    }
    if let Some(v) = p.top_p {
        body["top_p"] = json!(round_f32(v));
    }
    if let Some(k) = p.top_k {
        body["top_k"] = json!(k);
    }
    if !p.stop_sequences.is_empty() {
        body["stop_sequences"] = Value::Array(
            p.stop_sequences
                .iter()
                .map(|s| Value::String(s.clone()))
                .collect(),
        );
    }

    // tools + tool_choice
    if !req.tools.is_empty() {
        body["tools"] = Value::Array(
            req.tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description.as_deref().unwrap_or(""),
                        "input_schema": t.input_schema,
                    })
                })
                .collect(),
        );
        match &req.tool_choice {
            ToolChoice::Auto => {}
            ToolChoice::None => { /* 省略 tools 字段整段（§4-B none） */ }
            ToolChoice::Required => {
                body["tool_choice"] = json!({"type": "any"});
            }
            ToolChoice::Specific(name) => {
                body["tool_choice"] = json!({"type": "tool", "name": name});
            }
        }
    }

    // 消息：合并相邻同角色（调用方也可先做）；system 已在顶层。
    let msgs: Vec<CanonMessage> = crate::codec::ir::merge_adjacent_same_role(req).messages;
    let mut out = Vec::new();
    for m in &msgs {
        match m.role {
            Role::User => {
                // user 消息中的 ToolResult 块 + 文本块
                let mut blocks: Vec<Value> = Vec::new();
                let mut has_tool_result = false;
                for b in &m.blocks {
                    match b {
                        Block::Text { text } => blocks.push(json!({"type":"text","text":text})),
                        Block::Image {
                            media_type,
                            data_base64,
                            url,
                        } => {
                            let src = if let Some(b64) = data_base64 {
                                json!({"type":"base64","media_type":media_type,"data":b64})
                            } else {
                                json!({"type":"url","url":url.as_deref().unwrap_or("")})
                            };
                            blocks.push(json!({"type":"image","source":src}));
                        }
                        Block::ToolResult {
                            call_id,
                            content,
                            is_error,
                        } => {
                            has_tool_result = true;
                            let content_json: Vec<Value> = content
                                .iter()
                                .filter_map(|c| c.as_text())
                                .map(|t| json!({"type":"text","text":t}))
                                .collect();
                            blocks.push(json!({
                                "type":"tool_result",
                                "tool_use_id": call_id,
                                "content": content_json,
                                "is_error": is_error,
                            }));
                        }
                        _ => {}
                    }
                }
                if has_tool_result && blocks.len() > 1 {
                    // §4-C：tool_result 与其他块同轮时，result 块在前
                    blocks.sort_by_key(|b| {
                        if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                            0
                        } else {
                            1
                        }
                    });
                }
                if !blocks.is_empty() {
                    out.push(json!({"role":"user","content":blocks}));
                }
            }
            Role::Assistant => {
                let mut blocks: Vec<Value> = Vec::new();
                for b in &m.blocks {
                    match b {
                        Block::Text { text } => blocks.push(json!({"type":"text","text":text})),
                        Block::ToolUse { id, name, input } => {
                            blocks.push(json!({
                                "type":"tool_use",
                                "id": id,
                                "name": name,
                                "input": input,
                            }));
                        }
                        Block::Thinking { .. } => { /* v1 不产出 */ }
                        _ => {}
                    }
                }
                if !blocks.is_empty() {
                    out.push(json!({"role":"assistant","content":blocks}));
                }
            }
        }
    }
    body["messages"] = Value::Array(out);

    if req.stream {
        body["stream"] = json!(true);
    }

    Ok(body)
}

// ---------------------------------------------------------------- 响应解析

/// 解析 Anthropic 非流式响应 → CanonicalResponse。
pub fn parse_response(body: &[u8]) -> Result<CanonicalResponse, String> {
    let v: Value =
        serde_json::from_slice(body).map_err(|e| format!("Anthropic 响应 JSON 解析失败: {e}"))?;
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let model = v
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let mut output: Vec<Block> = Vec::new();
    if let Some(content) = v.get("content").and_then(Value::as_array) {
        for c in content {
            output.extend(parse_content_block(c));
        }
    }

    // stop_reason
    let stop_reason = match v.get("stop_reason").and_then(Value::as_str) {
        Some("end_turn") => StopReason::EndTurn,
        Some("max_tokens") => StopReason::MaxTokens,
        Some("tool_use") => StopReason::ToolUse,
        Some("refusal") => StopReason::SafetyBlock,
        Some(other) => StopReason::Other(other.to_string()),
        None => StopReason::EndTurn,
    };

    // usage
    let usage = parse_usage(v.get("usage"));

    Ok(CanonicalResponse {
        id,
        model,
        output,
        stop_reason,
        usage,
    })
}

/// 单个 Anthropic 内容块 → IR 块列表。
fn parse_content_block(c: &Value) -> Vec<Block> {
    match c.get("type").and_then(Value::as_str) {
        Some("text") => c
            .get("text")
            .and_then(Value::as_str)
            .map(|t| {
                vec![Block::Text {
                    text: t.to_string(),
                }]
            })
            .unwrap_or_default(),
        Some("tool_use") => {
            let id = c.get("id").and_then(Value::as_str).unwrap_or_default();
            let name = c.get("name").and_then(Value::as_str).unwrap_or_default();
            let input = c.get("input").cloned().unwrap_or_else(|| json!({}));
            vec![Block::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input,
            }]
        }
        Some("tool_result") => {
            let call_id = c
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let is_error = c.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            let content = c
                .get("content")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.get("text").and_then(Value::as_str))
                        .map(|t| Block::Text {
                            text: t.to_string(),
                        })
                        .collect()
                })
                .or_else(|| {
                    c.get("content").and_then(Value::as_str).map(|t| {
                        vec![Block::Text {
                            text: t.to_string(),
                        }]
                    })
                })
                .unwrap_or_default();
            vec![Block::ToolResult {
                call_id: call_id.to_string(),
                content,
                is_error,
            }]
        }
        _ => vec![],
    }
}

fn parse_usage(u: Option<&Value>) -> Usage {
    let get = |k: &str| {
        u.and_then(|v| v.get(k))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    Usage {
        input_tokens: get("input_tokens"),
        output_tokens: get("output_tokens"),
        cache_read_tokens: u
            .and_then(|v| v.get("cache_read_input_tokens"))
            .and_then(Value::as_u64),
        cache_write_tokens: u
            .and_then(|v| v.get("cache_creation_input_tokens"))
            .and_then(Value::as_u64),
    }
}

// ---------------------------------------------------------------- 流式解析

/// 解析 Anthropic SSE `data: {...}` 事件 → StreamEvent 列表。
/// `raw` 为事件 payload 的原始字节（已剥掉 `data: ` 前缀与结尾换行）。
pub fn parse_stream_event(raw: &[u8]) -> Result<Vec<StreamEvent>, String> {
    let v: Value =
        serde_json::from_slice(raw).map_err(|e| format!("Anthropic SSE JSON 解析失败: {e}"))?;
    let evt_type = v.get("type").and_then(Value::as_str).unwrap_or_default();

    let mut out = Vec::new();
    match evt_type {
        "message_start" => {
            if let Some(msg) = v.get("message") {
                let model = msg.get("model").and_then(Value::as_str).unwrap_or_default();
                out.push(StreamEvent::Start {
                    model: model.to_string(),
                });
            }
        }
        "content_block_start" => {
            let idx = v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            if let Some(cb) = v.get("content_block") {
                match cb.get("type").and_then(Value::as_str) {
                    Some("text") => out.push(StreamEvent::TextDelta {
                        text: cb
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    }),
                    Some("tool_use") => out.push(StreamEvent::ToolCallStart {
                        index: idx,
                        id: cb
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        name: cb
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    }),
                    _ => {}
                }
            }
        }
        "content_block_delta" => {
            let idx = v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            if let Some(delta) = v.get("delta") {
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => out.push(StreamEvent::TextDelta {
                        text: delta
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    }),
                    Some("input_json_delta") => out.push(StreamEvent::ToolCallArgsDelta {
                        index: idx,
                        args_fragment: delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    }),
                    Some("thinking_delta") => out.push(StreamEvent::ThinkingDelta {
                        text: delta
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    }),
                    _ => {}
                }
            }
        }
        "content_block_stop" => {
            let idx = v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            out.push(StreamEvent::ToolCallEnd { index: idx });
        }
        "message_delta" => {
            let finish = v
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(Value::as_str);
            let stop_reason = match finish {
                Some("end_turn") => StopReason::EndTurn,
                Some("max_tokens") => StopReason::MaxTokens,
                Some("tool_use") => StopReason::ToolUse,
                Some("refusal") => StopReason::SafetyBlock,
                Some(o) => StopReason::Other(o.to_string()),
                None => StopReason::EndTurn,
            };
            let usage = parse_usage(v.get("usage"));
            out.push(StreamEvent::Finish { stop_reason, usage });
        }
        // message_stop / ping：无内容
        _ => {}
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::ir::{CanonMessage, SampleParams, ToolSpec};

    #[test]
    fn peek_parses_model_and_stream() {
        let p =
            peek(br#"{"model":"claude-sonnet-4","stream":true,"max_tokens":1024,"messages":[]}"#)
                .unwrap();
        assert_eq!(p.model, "claude-sonnet-4");
        assert!(p.stream);

        assert!(peek(b"not json").is_err());
        assert!(peek(br#"{"messages":[]}"#).is_err());
    }

    #[test]
    fn count_tokens_rough_estimate_is_positive() {
        let body = r#"{
            "model":"claude-sonnet-4",
            "max_tokens":1024,
            "system":"You are a helpful assistant.",
            "messages":[
                {"role":"user","content":"Hello world, this is a test message for token estimate."}
            ],
            "tools":[
                {"name":"get_weather","description":"Get weather","input_schema":{"type":"object"}}
            ]
        }"#;
        let v = count_tokens(body.as_bytes()).unwrap();
        let n = v["input_tokens"].as_u64().expect("正整数");
        // 粗估应明显大于 0 且在合理区间（不会把整段文本缩成 1 或爆成巨数）
        assert!(n > 0 && n < 1000, "估算值异常: {n}");
    }

    #[test]
    fn count_tokens_minimal_message() {
        let body = br#"{"model":"x","messages":[{"role":"user","content":"hi"}]}"#;
        let v = count_tokens(body).unwrap();
        assert!(v["input_tokens"].as_u64().unwrap() >= 5); // 消息开销 4 + "hi"/4
    }

    #[test]
    fn error_body_shape() {
        let v = error_body("上游过载", "overloaded_error");
        assert_eq!(v["type"], "error");
        assert_eq!(v["error"]["type"], "overloaded_error");
        assert_eq!(v["error"]["message"], "上游过载");
        assert!(v.get("error").is_some());
    }

    fn basic_req() -> CanonicalRequest {
        CanonicalRequest {
            model: "claude-sonnet-4".into(),
            system: vec!["You are helpful.".into()],
            messages: vec![
                CanonMessage::text(Role::User, "hi"),
                CanonMessage::text(Role::Assistant, "hello"),
            ],
            tools: vec![],
            tool_choice: ToolChoice::Auto,
            params: SampleParams {
                max_output_tokens: Some(1024),
                temperature: Some(0.5),
                ..Default::default()
            },
            stream: true,
            extensions: Default::default(),
        }
    }

    #[test]
    fn encode_basic_messages() {
        let v = encode_request(&basic_req()).unwrap();
        assert_eq!(v["model"], "claude-sonnet-4");
        assert_eq!(v["system"], "You are helpful.");
        assert_eq!(v["max_tokens"], 1024);
        assert_eq!(v["temperature"], 0.5);
        assert_eq!(v["messages"][0]["role"], "user");
        assert_eq!(v["messages"][0]["content"][0]["text"], "hi");
        assert_eq!(v["stream"], true);
    }

    #[test]
    fn encode_tool_spec_and_choice() {
        let mut req = basic_req();
        req.tools = vec![ToolSpec {
            name: "get_weather".into(),
            description: Some("w".into()),
            input_schema: json!({"type":"object","properties":{"city":{"type":"string"}}}),
        }];
        req.tool_choice = ToolChoice::Required;
        let v = encode_request(&req).unwrap();
        assert_eq!(v["tools"][0]["name"], "get_weather");
        assert_eq!(v["tool_choice"]["type"], "any");
    }

    #[test]
    fn encode_user_tool_result_block() {
        let mut req = basic_req();
        req.messages = vec![CanonMessage {
            role: Role::User,
            blocks: vec![Block::ToolResult {
                call_id: "call_1".into(),
                content: vec![Block::Text {
                    text: "sunny".into(),
                }],
                is_error: false,
            }],
        }];
        let v = encode_request(&req).unwrap();
        let block = &v["messages"][0]["content"][0];
        assert_eq!(block["type"], "tool_result");
        assert_eq!(block["tool_use_id"], "call_1");
    }

    #[test]
    fn parse_response_text_and_tool_use() {
        let body = br#"{"id":"msg_1","type":"message","role":"assistant",
            "model":"claude-sonnet-4",
            "content":[{"type":"text","text":"Let me check."},
                       {"type":"tool_use","id":"toolu_1","name":"get_weather",
                        "input":{"city":"beijing"}}],
            "stop_reason":"tool_use",
            "usage":{"input_tokens":10,"output_tokens":4,
                     "cache_read_input_tokens":3,"cache_creation_input_tokens":5}}"#;
        let r = parse_response(body).unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        assert_eq!(r.output.len(), 2);
        match &r.output[1] {
            Block::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_1");
                assert_eq!(name, "get_weather");
                assert_eq!(input["city"], "beijing");
            }
            other => panic!("期望 ToolUse: {other:?}"),
        }
        assert_eq!(r.usage.input_tokens, 10);
        assert_eq!(r.usage.cache_read_tokens, Some(3));
        assert_eq!(r.usage.cache_write_tokens, Some(5));
    }

    #[test]
    fn parse_stream_events_sequence() {
        let start = parse_stream_event(
            br#"{"type":"message_start","message":{"id":"m1","model":"claude-sonnet-4"}}"#,
        )
        .unwrap();
        assert!(matches!(&start[0], StreamEvent::Start { model } if model == "claude-sonnet-4"));

        let ts = parse_stream_event(
            br#"{"type":"content_block_start","index":0,
                "content_block":{"type":"tool_use","id":"toolu_9","name":"get_weather"}}"#,
        )
        .unwrap();
        assert!(
            matches!(&ts[0], StreamEvent::ToolCallStart { index: 0, id, name }
            if id == "toolu_9" && name == "get_weather")
        );

        let args = parse_stream_event(
            br#"{"type":"content_block_delta","index":0,
                "delta":{"type":"input_json_delta","partial_json":"{\"city\":"}}"#,
        )
        .unwrap();
        assert!(matches!(
            &args[0],
            StreamEvent::ToolCallArgsDelta { index: 0, .. }
        ));

        let fin = parse_stream_event(
            br#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},
                "usage":{"input_tokens":1,"output_tokens":1}}"#,
        )
        .unwrap();
        assert!(matches!(
            &fin[0],
            StreamEvent::Finish {
                stop_reason: StopReason::ToolUse,
                ..
            }
        ));
    }
}

// ================================================================ M5：InboundCodec::Anthropic

// Claude Code 入站（Anthropic Messages 请求）→ IR；IR → Anthropic 响应/SSE。
// roadmap M5：
// - 入站 thinking 字段：Lenient 丢弃 + WARN（Thinking 块仅存储不转换）
// - tool id 反解：客户端回传的上游 tool_use id 应能关联（M4 已合成 _k{n}，本层识别）
// - message_start 的 input_tokens 前置填 0、message_delta 终局补齐

use crate::codec::ir::{canonical_to_anthropic_id, decode_anthropic_tool_id};

/// 解码 Anthropic Messages 请求 → CanonicalRequest。
pub fn decode_request(body: &[u8]) -> Result<CanonicalRequest, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("请求体不是合法 JSON: {e}"))?;

    let model = v
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if model.is_empty() {
        return Err("缺少 model 字段".into());
    }

    let mut system: Vec<String> = Vec::new();
    // system 可能是字符串或块数组
    match v.get("system") {
        Some(Value::String(s)) => system.push(s.clone()),
        Some(Value::Array(arr)) => {
            for b in arr {
                if let Some(t) = b.get("text").and_then(Value::as_str) {
                    system.push(t.to_string());
                }
            }
        }
        _ => {}
    }

    let mut messages: Vec<CanonMessage> = Vec::new();
    let mut extensions = serde_json::Map::new();
    if let Some(arr) = v.get("messages").and_then(Value::as_array) {
        for m in arr {
            let role = m.get("role").and_then(Value::as_str).unwrap_or_default();
            // 入站 thinking：仅当 content 中有 thinking 块时 WARN（下面 content 处理捕获）
            let mut blocks: Vec<Block> = Vec::new();
            let mut saw_thinking = false;
            if let Some(content) = m.get("content") {
                match content {
                    Value::String(s) => blocks.push(Block::Text { text: s.clone() }),
                    Value::Array(parts) => {
                        for p in parts {
                            match p.get("type").and_then(Value::as_str) {
                                Some("text") => {
                                    if let Some(t) = p.get("text").and_then(Value::as_str) {
                                        blocks.push(Block::Text {
                                            text: t.to_string(),
                                        });
                                    }
                                }
                                Some("tool_use") => {
                                    // 历史工具调用 id 同样反解：与回传的 tool_result
                                    // 保持同一把键（转 OpenAI 上游时二者必须匹配）
                                    let raw_id =
                                        p.get("id").and_then(Value::as_str).unwrap_or_default();
                                    let resolved = decode_anthropic_tool_id(raw_id)
                                        .unwrap_or_else(|| raw_id.to_string());
                                    blocks.push(Block::ToolUse {
                                        id: resolved,
                                        name: p
                                            .get("name")
                                            .and_then(Value::as_str)
                                            .unwrap_or_default()
                                            .to_string(),
                                        input: p.get("input").cloned().unwrap_or_else(|| json!({})),
                                    });
                                }
                                Some("tool_result") => {
                                    let call_id = p
                                        .get("tool_use_id")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default();
                                    let is_error =
                                        p.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                                    let content: Vec<Block> = p
                                        .get("content")
                                        .and_then(|c| match c {
                                            Value::String(s) => {
                                                Some(vec![Block::Text { text: s.clone() }])
                                            }
                                            Value::Array(a) => Some(
                                                a.iter()
                                                    .filter_map(|x| {
                                                        x.get("text").and_then(Value::as_str).map(
                                                            |t| Block::Text {
                                                                text: t.to_string(),
                                                            },
                                                        )
                                                    })
                                                    .collect(),
                                            ),
                                            _ => None,
                                        })
                                        .unwrap_or_default();
                                    // tool id 反解：M4 上游合成的 _k{n} → 恢复为可关联 id
                                    let resolved = decode_anthropic_tool_id(call_id)
                                        .unwrap_or_else(|| call_id.to_string());
                                    blocks.push(Block::ToolResult {
                                        call_id: resolved,
                                        content,
                                        is_error,
                                    });
                                }
                                Some("thinking") => {
                                    saw_thinking = true;
                                    // 仅存储不转换（protocol-ir §10-3）；块丢弃
                                    let _ = p;
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            if saw_thinking {
                eprintln!("[anthropic-in] 入站 thinking 块已 Lenient 丢弃 + WARN（仅存储不转换）");
            }
            if !blocks.is_empty() {
                let r = match role {
                    "assistant" => Role::Assistant,
                    _ => Role::User,
                };
                messages.push(CanonMessage { role: r, blocks });
            }
        }
    }

    // tools
    let mut tools: Vec<ToolSpec> = Vec::new();
    if let Some(arr) = v.get("tools").and_then(Value::as_array) {
        for t in arr {
            tools.push(ToolSpec {
                name: t
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                description: t
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                input_schema: t
                    .get("input_schema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type":"object"})),
            });
        }
    }

    // tool_choice
    let tool_choice = match v.get("tool_choice") {
        Some(Value::Object(o)) => match o.get("type").and_then(Value::as_str) {
            Some("none") => ToolChoice::None,
            Some("any") => ToolChoice::Required,
            Some("tool") => o
                .get("name")
                .and_then(Value::as_str)
                .map(|n| ToolChoice::Specific(n.to_string()))
                .unwrap_or(ToolChoice::Required),
            _ => ToolChoice::Auto,
        },
        _ => ToolChoice::Auto,
    };

    // 采样参数
    let params = SampleParams {
        max_output_tokens: v
            .get("max_tokens")
            .and_then(Value::as_u64)
            .map(|n| n as u32),
        temperature: v
            .get("temperature")
            .and_then(Value::as_f64)
            .map(|f| f as f32),
        top_p: v.get("top_p").and_then(Value::as_f64).map(|f| f as f32),
        top_k: v.get("top_k").and_then(Value::as_u64).map(|n| n as u32),
        stop_sequences: v
            .get("stop_sequences")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        frequency_penalty: None,
        presence_penalty: None,
        seed: None,
    };

    // 未建模字段（§7 Lenient；stream 已建模不收集）
    for k in ["metadata", "service_tier", "thinking", "cache_control"] {
        if v.get(k).is_some() {
            let _ = extensions.entry(k.to_string()).or_insert(Value::Null);
        }
    }

    Ok(CanonicalRequest {
        model,
        system,
        messages,
        tools,
        tool_choice,
        params,
        stream: v.get("stream").and_then(Value::as_bool).unwrap_or(false),
        extensions,
    })
}

/// 渲染非流式响应：CanonicalResponse → Anthropic message。
pub fn render_response(r: &CanonicalResponse) -> Value {
    let mut content: Vec<Value> = Vec::new();
    for b in &r.output {
        match b {
            Block::Text { text } => content.push(json!({"type":"text","text":text})),
            Block::ToolUse { id, name, input } => {
                let kid = canonical_to_anthropic_id(id);
                content.push(json!({
                    "type":"tool_use",
                    "id": kid,
                    "name": name,
                    "input": input,
                }));
            }
            Block::Thinking { .. } => { /* 不转换 */ }
            _ => {}
        }
    }
    let stop_reason = match &r.stop_reason {
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTokens => "max_tokens",
        StopReason::ToolUse => "tool_use",
        StopReason::SafetyBlock => "refusal",
        StopReason::Other(_) => "end_turn",
    };
    let mut usage = json!({
        "input_tokens": r.usage.input_tokens,
        "output_tokens": r.usage.output_tokens,
    });
    if let Some(cr) = r.usage.cache_read_tokens {
        usage["cache_read_input_tokens"] = json!(cr);
    }
    if let Some(cw) = r.usage.cache_write_tokens {
        usage["cache_creation_input_tokens"] = json!(cw);
    }
    json!({
        "id": r.id,
        "type": "message",
        "role": "assistant",
        "model": r.model,
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": usage,
    })
}

/// Anthropic SSE 渲染状态。
#[derive(Debug, Clone, Default)]
pub struct AnthropicRenderState {
    pub message_id: String,
    pub model: String,
    /// 当前正在输出中的 Anthropic content block index
    pub active_block: Option<usize>,
    /// 文本块是否已开（Anthropic 要求先 content_block_start(text) 再 delta）
    pub text_started: bool,
    /// 当前打开的 OpenAI/Gemini tool_calls index（对应 active_block）
    pub active_tool_index: Option<usize>,
    /// 下一个可用的 Anthropic content block index
    pub next_block_index: usize,
    /// message_delta 是否已发出（终局 usage 填充用）
    pub finished: bool,
}

/// 渲染单个 IR 流事件为 Anthropic SSE 帧（`event:...` 行 + `data: {...}` 行）。
/// 返回 0..N 帧（一个 IR 事件可能展开为多个 SSE 事件，如文本首片需先 content_block_start）。
///
/// 嵌套顺序纪律（protocol-ir §2「渲染至 Anthropic」方向）：block 必须
/// Start → deltas → Stop 完结后才开始下一个。本函数在渲染器内部维护打开的块，
/// 遇到下一个块开始或 Finish 时先补齐当前块的 `content_block_stop`，从而兼容
/// OpenAI 系交错 tool_calls 分片。
pub fn render_stream_event(
    e: &StreamEvent,
    st: &mut AnthropicRenderState,
) -> Vec<(&'static str, String)> {
    use crate::codec::ir::StreamEvent as Ev;

    fn close_open_block(st: &mut AnthropicRenderState, out: &mut Vec<(&'static str, String)>) {
        if let Some(idx) = st.active_block {
            out.push((
                "content_block_stop",
                json!({"type":"content_block_stop","index":idx}).to_string(),
            ));
        }
        st.active_block = None;
        st.text_started = false;
        st.active_tool_index = None;
    }

    match e {
        Ev::Start { model } => {
            st.active_block = None;
            st.text_started = false;
            st.active_tool_index = None;
            st.next_block_index = 0;
            st.finished = false;
            // message_start：input_tokens 前置填 0（M5 验收）
            vec![(
                "message_start",
                json!({
                    "type":"message_start",
                    "message":{
                        "id": st.message_id,
                        "type":"message",
                        "role":"assistant",
                        "model": model,
                        "content": [],
                        "stop_reason": null,
                        "usage":{"input_tokens":0,"output_tokens":0},
                    }
                })
                .to_string(),
            )]
        }
        Ev::TextDelta { text } => {
            if text.is_empty() {
                return Vec::new();
            }
            let mut out = Vec::new();
            // 若当前打开的是 tool 块，先结束它再开文本块（保持严格嵌套）
            if st.active_block.is_some() && !st.text_started {
                close_open_block(st, &mut out);
            }
            if !st.text_started {
                let idx = st.next_block_index;
                st.next_block_index += 1;
                st.active_block = Some(idx);
                st.text_started = true;
                out.push((
                    "content_block_start",
                    json!({
                        "type":"content_block_start",
                        "index": idx,
                        "content_block": {"type":"text","text":""},
                    })
                    .to_string(),
                ));
            }
            let idx = st.active_block.unwrap_or(0);
            out.push((
                "content_block_delta",
                json!({
                    "type":"content_block_delta",
                    "index": idx,
                    "delta":{"type":"text_delta","text":text},
                })
                .to_string(),
            ));
            out
        }
        Ev::ThinkingDelta { .. } => Vec::new(),
        Ev::ToolCallStart { index, id, name } => {
            // 同一工具调用重复首片：忽略，避免重复开块
            if st.active_tool_index == Some(*index) && st.active_block.is_some() {
                return Vec::new();
            }
            let mut out = Vec::new();
            // 先结束当前文本/tool 块，再开新 tool 块
            if st.active_block.is_some() {
                close_open_block(st, &mut out);
            }
            let idx = st.next_block_index;
            st.next_block_index += 1;
            st.active_block = Some(idx);
            st.active_tool_index = Some(*index);
            st.text_started = false;
            out.push((
                "content_block_start",
                json!({
                    "type":"content_block_start",
                    "index": idx,
                    "content_block": {
                        "type":"tool_use",
                        "id": canonical_to_anthropic_id(id),
                        "name": name,
                        "input": {},
                    },
                })
                .to_string(),
            ));
            out
        }
        Ev::ToolCallArgsDelta {
            index,
            args_fragment,
        } => {
            if st.active_tool_index != Some(*index) {
                // 非当前打开工具的分片：按 IR 纪律不应在嵌套顺序之外透传，丢弃并等待重排
                return Vec::new();
            }
            let idx = st.active_block.unwrap_or(0);
            vec![(
                "content_block_delta",
                json!({
                    "type":"content_block_delta",
                    "index": idx,
                    "delta":{"type":"input_json_delta","partial_json":args_fragment},
                })
                .to_string(),
            )]
        }
        Ev::ToolCallEnd { index } => {
            let mut out = Vec::new();
            if st.active_tool_index == Some(*index) {
                close_open_block(st, &mut out);
            }
            out
        }
        Ev::Finish { stop_reason, usage } => {
            let mut out = Vec::new();
            // 终局前补齐所有打开块（文本或工具）的 content_block_stop
            if st.active_block.is_some() {
                close_open_block(st, &mut out);
            }
            st.finished = true;
            let sr = match stop_reason {
                StopReason::EndTurn => "end_turn",
                StopReason::MaxTokens => "max_tokens",
                StopReason::ToolUse => "tool_use",
                StopReason::SafetyBlock => "refusal",
                StopReason::Other(_) => "end_turn",
            };
            // message_delta：终局 usage 补齐
            let delta = json!({
                "type":"message_delta",
                "delta":{"stop_reason":sr,"stop_sequence":null},
                "usage":{
                    "input_tokens": usage.input_tokens,
                    "output_tokens": usage.output_tokens,
                    "cache_read_input_tokens": usage.cache_read_tokens,
                    "cache_creation_input_tokens": usage.cache_write_tokens,
                },
            });
            out.push(("message_delta", delta.to_string()));
            out
        }
    }
}

/// 结束帧：message_stop（配合 render_stream_event 使用）。
pub fn render_message_stop() -> &'static str {
    "{\"type\":\"message_stop\"}"
}

#[cfg(test)]
mod m5_tests {
    use super::*;
    use crate::codec::ir::Usage;

    #[test]
    fn decode_anthropic_request_full() {
        let body = br#"{
            "model":"claude-sonnet-4","max_tokens":1024,"stream":true,
            "system":"You are terse.",
            "tools":[{"name":"get_weather","description":"w",
                      "input_schema":{"type":"object","properties":{"city":{"type":"string"}}}}],
            "tool_choice":{"type":"tool","name":"get_weather"},
            "messages":[
                {"role":"user","content":"weather?"},
                {"role":"assistant","content":[
                    {"type":"text","text":"checking"},
                    {"type":"tool_use","id":"toolu_x","name":"get_weather","input":{"city":"bj"}}
                ]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_x",
                    "content":[{"type":"text","text":"sunny"}]}]}
            ]
        }"#;
        let req = decode_request(body).unwrap();
        assert_eq!(req.model, "claude-sonnet-4");
        assert_eq!(req.system, vec!["You are terse."]);
        assert_eq!(req.params.max_output_tokens, Some(1024));
        assert_eq!(req.tools.len(), 1);
        assert_eq!(req.tool_choice, ToolChoice::Specific("get_weather".into()));
        assert!(req.stream);
        assert_eq!(req.messages.len(), 3);
        // assistant tool_use id 反解为原始键
        assert!(matches!(
            &req.messages[1].blocks[1],
            Block::ToolUse { id, name, .. }
                if id == "x" && name == "get_weather"
        ));
        // tool_result 反解后与 assistant 消息同一把键
        assert!(matches!(
            &req.messages[2].blocks[0],
            Block::ToolResult { call_id, content, .. }
                if call_id == "x" && content[0].as_text() == Some("sunny")
        ));
    }

    #[test]
    fn decode_anthropic_thinking_lenient() {
        let body = br#"{
            "model":"m","messages":[{"role":"assistant","content":[
                {"type":"thinking","thinking":"hidden"},
                {"type":"text","text":"visible"}
            ]}]
        }"#;
        let req = decode_request(body).unwrap();
        assert_eq!(req.messages[0].blocks.len(), 1, "thinking 块应被丢弃");
        assert_eq!(req.messages[0].blocks[0].as_text(), Some("visible"));
    }

    #[test]
    fn render_anthropic_response_with_tool() {
        let r = canonical_response_with_tool();
        let v = render_response(&r);
        assert_eq!(v["type"], "message");
        assert_eq!(v["stop_reason"], "tool_use");
        assert_eq!(v["content"][1]["type"], "tool_use");
        assert!(v["content"][1]["id"]
            .as_str()
            .unwrap()
            .starts_with("toolu_"));
        assert_eq!(v["usage"]["input_tokens"], 10);
    }

    #[test]
    fn render_stream_sequence_with_start() {
        let mut st = AnthropicRenderState {
            message_id: "msg_jai".into(),
            model: "claude-sonnet-4".into(),
            active_block: None,
            text_started: false,
            active_tool_index: None,
            next_block_index: 0,
            finished: false,
        };
        let evs = render_stream_event(
            &StreamEvent::Start {
                model: "claude-sonnet-4".into(),
            },
            &mut st,
        );
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].0, "message_start");
        let v: Value = serde_json::from_str(&evs[0].1).unwrap();
        assert_eq!(v["message"]["usage"]["input_tokens"], 0, "前置填 0");

        let evs = render_stream_event(&StreamEvent::TextDelta { text: "你".into() }, &mut st);
        assert_eq!(evs.len(), 2, "文本首片应拆为 start+delta");
        assert_eq!(evs[0].0, "content_block_start");
        assert_eq!(evs[1].0, "content_block_delta");

        let evs = render_stream_event(
            &StreamEvent::ToolCallStart {
                index: 0,
                id: "call_1".into(),
                name: "f".into(),
            },
            &mut st,
        );
        assert_eq!(evs.len(), 2, "开始 tool 前应结束文本块");
        assert_eq!(evs[0].0, "content_block_stop");
        assert_eq!(evs[1].0, "content_block_start");
        let v: Value = serde_json::from_str(&evs[1].1).unwrap();
        assert_eq!(v["content_block"]["type"], "tool_use");

        let evs = render_stream_event(
            &StreamEvent::ToolCallArgsDelta {
                index: 0,
                args_fragment: "{\"a\":1}".into(),
            },
            &mut st,
        );
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].0, "content_block_delta");

        let evs = render_stream_event(&StreamEvent::ToolCallEnd { index: 0 }, &mut st);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].0, "content_block_stop");

        let evs = render_stream_event(
            &StreamEvent::Finish {
                stop_reason: StopReason::ToolUse,
                usage: Usage {
                    input_tokens: 9,
                    output_tokens: 3,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
            },
            &mut st,
        );
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].0, "message_delta");
        let v: Value = serde_json::from_str(&evs[0].1).unwrap();
        assert_eq!(v["usage"]["input_tokens"], 9, "终局补齐");
    }

    #[test]
    fn render_stream_interleaved_tool_calls_are_nested() {
        let mut st = AnthropicRenderState {
            message_id: "msg_jai".into(),
            model: "claude-sonnet-4".into(),
            active_block: None,
            text_started: false,
            active_tool_index: None,
            next_block_index: 0,
            finished: false,
        };
        let _ = render_stream_event(&StreamEvent::Start { model: "m".into() }, &mut st);

        let frames = render_stream_event(
            &StreamEvent::ToolCallStart {
                index: 0,
                id: "call_0".into(),
                name: "f0".into(),
            },
            &mut st,
        );
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0, "content_block_start");
        assert!(frames[0].1.contains("\"index\":0"));

        let frames = render_stream_event(
            &StreamEvent::ToolCallArgsDelta {
                index: 0,
                args_fragment: "{\"a\":1}".into(),
            },
            &mut st,
        );
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0, "content_block_delta");
        assert!(frames[0].1.contains("\"index\":0"));

        // OpenAI 交错：index 1 的 start 先于 index 0 的显式 end 到达
        let frames = render_stream_event(
            &StreamEvent::ToolCallStart {
                index: 1,
                id: "call_1".into(),
                name: "f1".into(),
            },
            &mut st,
        );
        assert_eq!(frames.len(), 2, "新 tool 开始前必须结束前一块");
        assert_eq!(frames[0].0, "content_block_stop");
        assert!(frames[0].1.contains("\"index\":0"));
        assert_eq!(frames[1].0, "content_block_start");
        assert!(frames[1].1.contains("\"index\":1"));

        let frames = render_stream_event(
            &StreamEvent::ToolCallArgsDelta {
                index: 1,
                args_fragment: "{\"b\":2}".into(),
            },
            &mut st,
        );
        assert!(frames[0].1.contains("\"index\":1"));

        let frames = render_stream_event(
            &StreamEvent::Finish {
                stop_reason: StopReason::ToolUse,
                usage: Usage {
                    input_tokens: 5,
                    output_tokens: 2,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
            },
            &mut st,
        );
        assert_eq!(frames.len(), 2, "终局应先 stop 再 message_delta");
        assert_eq!(frames[0].0, "content_block_stop");
        assert!(frames[0].1.contains("\"index\":1"));
        assert_eq!(frames[1].0, "message_delta");
    }

    fn canonical_response_with_tool() -> CanonicalResponse {
        CanonicalResponse {
            id: "msg_jai".into(),
            model: "claude-sonnet-4".into(),
            output: vec![
                Block::Text {
                    text: "checking".into(),
                },
                Block::ToolUse {
                    id: "call_1".into(),
                    name: "get_weather".into(),
                    input: json!({"city":"bj"}),
                },
            ],
            stop_reason: StopReason::ToolUse,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 4,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
        }
    }
}
