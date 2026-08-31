//! InboundCodec::Responses —— M6：OpenAI Responses API 入站（Codex 原生线）。
//!
//! 映射权威：roadmap M6 + protocol-ir §2/§4/§5。
//! 本模块把 `/v1/responses` 请求解码为 CanonicalRequest，并把 CanonicalResponse /
//! StreamEvent 渲染回 Responses 对象 / SSE 事件流。
//!
//! 请求结构（Codex 实际触达子集）：
//! - `instructions` → system
//! - `input`：message / function_call / function_call_output 条目 → IR 消息块
//! - `tools` / `tool_choice` / 采样参数 → ToolSpec / ToolChoice / SampleParams
//!
//! 响应结构：
//! - 非流式：`response` 对象（output[] 含 message / function_call）
//! - 流式：`response.created`、`response.output_item.added`、
//!   `response.output_text.delta`、`response.function_call_arguments.delta`、
//!   `response.completed` 等 SSE 帧

use serde_json::{json, Map, Value};

use crate::codec::ir::{
    Block, CanonMessage, CanonicalRequest, CanonicalResponse, Role, SampleParams, StopReason,
    StreamEvent, ToolChoice, ToolSpec, Usage,
};

/// OpenAI Responses 错误形状（protocol-ir §6 的 Responses 方言）。
pub fn error_body(message: &str, err_type: &str, code: Option<&str>) -> Value {
    json!({
        "error": {
            "message": message,
            "type": err_type,
            "param": null,
            "code": code,
        }
    })
}

/// 解码 `/v1/responses` 请求体 → CanonicalRequest。
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
    match v.get("instructions") {
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
    let mut extensions = Map::new();

    if let Some(input) = v.get("input") {
        match input {
            Value::String(s) => messages.push(CanonMessage::text(Role::User, s)),
            Value::Array(items) => {
                for item in items {
                    let typ = item.get("type").and_then(Value::as_str).unwrap_or_default();
                    // OpenAI 官方 input item 常省略顶层 type（直接 role+content），
                    // 此时按 message 处理；function_call(_output) 仍显式带 type。
                    let is_message = typ == "message"
                        || (typ.is_empty() && item.get("role").and_then(Value::as_str).is_some());
                    match typ {
                        _ if is_message => {
                            let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
                            let mut blocks = Vec::new();
                            if let Some(content) = item.get("content").and_then(Value::as_array) {
                                for part in content {
                                    let ptype = part
                                        .get("type")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default();
                                    match ptype {
                                        "input_text" | "output_text" | "text" => {
                                            if let Some(t) =
                                                part.get("text").and_then(Value::as_str)
                                            {
                                                blocks.push(Block::Text {
                                                    text: t.to_string(),
                                                });
                                            }
                                        }
                                        "input_image" => {
                                            if let Some(image_url) =
                                                part.get("image_url").and_then(Value::as_str)
                                            {
                                                blocks.push(Block::Image {
                                                    media_type: "image/png".into(),
                                                    data_base64: None,
                                                    url: Some(image_url.to_string()),
                                                });
                                            } else if let Some(detail) =
                                                part.get("image").and_then(Value::as_str)
                                            {
                                                // 兼容 data URL 内联
                                                blocks.push(Block::Image {
                                                    media_type: "image/png".into(),
                                                    data_base64: Some(detail.to_string()),
                                                    url: None,
                                                });
                                            }
                                        }
                                        _ => {
                                            extensions
                                                .entry(format!("input_part:{ptype}"))
                                                .or_insert(Value::Null);
                                        }
                                    }
                                }
                            }
                            if !blocks.is_empty() {
                                let r = if role == "assistant" {
                                    Role::Assistant
                                } else {
                                    Role::User
                                };
                                messages.push(CanonMessage { role: r, blocks });
                            }
                        }
                        "function_call" => {
                            let id = item
                                .get("call_id")
                                .or_else(|| item.get("id"))
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            let name = item
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            let input: Value = item
                                .get("arguments")
                                .and_then(Value::as_str)
                                .and_then(|s| serde_json::from_str(s).ok())
                                .unwrap_or_else(|| json!({}));
                            messages.push(CanonMessage {
                                role: Role::Assistant,
                                blocks: vec![Block::ToolUse { id, name, input }],
                            });
                        }
                        "function_call_output" => {
                            let call_id = item
                                .get("call_id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            let output = item
                                .get("output")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            messages.push(CanonMessage {
                                role: Role::User,
                                blocks: vec![Block::ToolResult {
                                    call_id,
                                    content: if output.is_empty() {
                                        vec![]
                                    } else {
                                        vec![Block::Text { text: output }]
                                    },
                                    is_error: false,
                                }],
                            });
                        }
                        "reasoning" => {
                            // Lenient：推理内容跨协议不转换
                            extensions
                                .entry("input_item:reasoning".to_string())
                                .or_insert(Value::Null);
                        }
                        other => {
                            extensions
                                .entry(format!("input_item:{other}"))
                                .or_insert(Value::Null);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let mut tools: Vec<ToolSpec> = Vec::new();
    if let Some(arr) = v.get("tools").and_then(Value::as_array) {
        for t in arr {
            let name = t.get("name").and_then(Value::as_str).unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            tools.push(ToolSpec {
                name: name.to_string(),
                description: t
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                input_schema: t
                    .get("parameters")
                    .cloned()
                    .unwrap_or_else(|| json!({"type":"object"})),
            });
        }
    }

    let tool_choice = match v.get("tool_choice") {
        Some(Value::String(s)) if s == "none" => ToolChoice::None,
        Some(Value::String(s)) if s == "required" => ToolChoice::Required,
        Some(Value::String(_)) => ToolChoice::Auto,
        Some(Value::Object(o)) => {
            if let Some(name) = o.get("name").and_then(Value::as_str) {
                ToolChoice::Specific(name.to_string())
            } else {
                ToolChoice::Auto
            }
        }
        _ => ToolChoice::Auto,
    };

    let params = SampleParams {
        max_output_tokens: v
            .get("max_output_tokens")
            .and_then(Value::as_u64)
            .map(|n| n as u32),
        temperature: v
            .get("temperature")
            .and_then(Value::as_f64)
            .map(|f| f as f32),
        top_p: v.get("top_p").and_then(Value::as_f64).map(|f| f as f32),
        top_k: None,
        stop_sequences: v
            .get("stop")
            .and_then(|s| match s {
                Value::String(x) => Some(vec![x.clone()]),
                Value::Array(a) => Some(
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect(),
                ),
                _ => None,
            })
            .unwrap_or_default(),
        frequency_penalty: v
            .get("frequency_penalty")
            .and_then(Value::as_f64)
            .map(|f| f as f32),
        presence_penalty: v
            .get("presence_penalty")
            .and_then(Value::as_f64)
            .map(|f| f as f32),
        seed: v.get("seed").and_then(Value::as_i64),
    };

    // 未建模字段 Lenient 收集
    for k in [
        "parallel_tool_calls",
        "stream_options",
        "store",
        "metadata",
        "user",
        "reasoning",
        "text",
        "include",
        "previous_response_id",
    ] {
        if v.get(k).is_some() {
            extensions.entry(k.to_string()).or_insert(Value::Null);
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

/// 渲染非流式响应：CanonicalResponse → Responses response 对象。
pub fn render_response(r: &CanonicalResponse) -> Value {
    let mut output = Vec::new();
    for b in &r.output {
        match b {
            Block::Text { text } => {
                output.push(json!({
                    "type": "message",
                    "id": format!("msg_{}", r.id),
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "output_text", "text": text, "annotations": []}],
                }));
            }
            Block::ToolUse { id, name, input } => {
                output.push(json!({
                    "type": "function_call",
                    "id": format!("fc_{id}"),
                    "call_id": id,
                    "name": name,
                    "arguments": serde_json::to_string(input).unwrap_or_else(|_| "{}".into()),
                }));
            }
            _ => {}
        }
    }
    let mut usage = json!({
        "input_tokens": r.usage.input_tokens,
        "output_tokens": r.usage.output_tokens,
        "total_tokens": r.usage.input_tokens + r.usage.output_tokens,
    });
    if let Some(cr) = r.usage.cache_read_tokens {
        usage["input_tokens_details"] = json!({"cached_tokens": cr});
    }
    json!({
        "id": r.id,
        "object": "response",
        "created_at": crate::store::now_ms() / 1000,
        "status": "completed",
        "model": r.model,
        "output": output,
        "usage": usage,
    })
}

/// 编码 IR → OpenAI Responses API 请求体（上游侧）。
///
/// 与 [`decode_request`] 保持对称；主要用于 MCP 工具自动合并时
/// Responses 入站 → Responses 同族上游的转换路径。
pub fn encode_request(req: &CanonicalRequest) -> Result<Value, String> {
    let mut body = json!({
        "model": req.model,
        "input": [],
    });

    if !req.system.is_empty() {
        body["instructions"] = Value::String(req.system.join("\n\n"));
    }

    let mut input: Vec<Value> = Vec::new();
    for m in &req.messages {
        match m.role {
            Role::User => {
                let mut text_parts: Vec<String> = Vec::new();
                let mut tool_results: Vec<Value> = Vec::new();
                for b in &m.blocks {
                    match b {
                        Block::Text { text } => text_parts.push(text.clone()),
                        Block::Image { .. } => {
                            // Responses 上游 v1 不支持图片内联转换，Lenient 丢弃
                        }
                        Block::ToolResult {
                            call_id,
                            content,
                            is_error,
                        } => {
                            let text = content
                                .iter()
                                .filter_map(|c| c.as_text())
                                .collect::<Vec<_>>()
                                .join("\n");
                            let output = if *is_error {
                                format!("[error] {text}")
                            } else {
                                text
                            };
                            tool_results.push(json!({
                                "type": "function_call_output",
                                "call_id": call_id,
                                "output": output,
                            }));
                        }
                        _ => {}
                    }
                }
                if !text_parts.is_empty() {
                    input.push(json!({
                        "type": "message",
                        "role": "user",
                        "content": text_parts.iter().map(|t| json!({
                            "type": "input_text",
                            "text": t,
                        })).collect::<Vec<_>>(),
                    }));
                }
                input.extend(tool_results);
            }
            Role::Assistant => {
                let mut text_parts: Vec<String> = Vec::new();
                let mut calls: Vec<Value> = Vec::new();
                for b in &m.blocks {
                    match b {
                        Block::Text { text } => text_parts.push(text.clone()),
                        Block::ToolUse { id, name, input } => {
                            calls.push(json!({
                                "type": "function_call",
                                "call_id": id,
                                "name": name,
                                "arguments": serde_json::to_string(input).unwrap_or_else(|_| "{}".into()),
                            }));
                        }
                        _ => {}
                    }
                }
                if !text_parts.is_empty() {
                    input.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": text_parts.iter().map(|t| json!({
                            "type": "output_text",
                            "text": t,
                        })).collect::<Vec<_>>(),
                    }));
                }
                input.extend(calls);
            }
        }
    }
    body["input"] = Value::Array(input);

    if !req.tools.is_empty() {
        body["tools"] = Value::Array(
            req.tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "name": t.name,
                        "description": t.description.as_deref().unwrap_or(""),
                        "parameters": t.input_schema,
                    })
                })
                .collect(),
        );
        let choice = match &req.tool_choice {
            ToolChoice::Auto => None,
            ToolChoice::None => Some(json!("none")),
            ToolChoice::Required => Some(json!("required")),
            ToolChoice::Specific(name) => Some(json!({"type": "function", "name": name})),
        };
        if let Some(c) = choice {
            body["tool_choice"] = c;
        }
    }

    let p = &req.params;
    if let Some(m) = p.max_output_tokens {
        body["max_output_tokens"] = json!(m);
    }
    if let Some(t) = p.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(v) = p.top_p {
        body["top_p"] = json!(v);
    }
    if !p.stop_sequences.is_empty() {
        body["stop"] = Value::Array(
            p.stop_sequences
                .iter()
                .map(|s| Value::String(s.clone()))
                .collect(),
        );
    }
    if req.stream {
        body["stream"] = json!(true);
    }

    Ok(body)
}

/// 解析 OpenAI Responses 非流式响应 → CanonicalResponse（上游侧）。
pub fn parse_response(body: &[u8]) -> Result<CanonicalResponse, String> {
    let v: Value =
        serde_json::from_slice(body).map_err(|e| format!("Responses 响应 JSON 解析失败: {e}"))?;
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("上游返回错误")
            .to_string();
        return Err(format!("Responses 上游错误: {msg}"));
    }

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
    let mut saw_tool_use = false;
    if let Some(items) = v.get("output").and_then(Value::as_array) {
        for item in items {
            let typ = item.get("type").and_then(Value::as_str).unwrap_or_default();
            match typ {
                "message" => {
                    if let Some(content) = item.get("content").and_then(Value::as_array) {
                        for part in content {
                            if let Some(t) = part.get("text").and_then(Value::as_str) {
                                if !t.is_empty() {
                                    output.push(Block::Text {
                                        text: t.to_string(),
                                    });
                                }
                            }
                        }
                    }
                }
                "function_call" => {
                    let call_id = item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let input: Value = item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_else(|| json!({}));
                    output.push(Block::ToolUse {
                        id: call_id,
                        name,
                        input,
                    });
                    saw_tool_use = true;
                }
                _ => {}
            }
        }
    }

    let usage = v.get("usage").cloned().unwrap_or_else(|| json!({}));
    let mut parsed_usage = Usage::default();
    if let Some(n) = usage.get("input_tokens").and_then(Value::as_u64) {
        parsed_usage.input_tokens = n;
    }
    if let Some(n) = usage.get("output_tokens").and_then(Value::as_u64) {
        parsed_usage.output_tokens = n;
    }
    if let Some(d) = usage
        .get("input_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(Value::as_u64)
    {
        parsed_usage.cache_read_tokens = Some(d);
    }

    let status = v
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("completed");
    let stop_reason = if saw_tool_use {
        StopReason::ToolUse
    } else if status == "incomplete" {
        StopReason::MaxTokens
    } else {
        StopReason::EndTurn
    };

    Ok(CanonicalResponse {
        id,
        model,
        output,
        stop_reason,
        usage: parsed_usage,
    })
}

/// 把非流式 CanonicalResponse 渲染为符合 OpenAI Responses 真实 SSE 形状的完整帧。
///
/// 用于 MCP 自动循环等需要“最终响应合成流式”的场景；包含
/// `response.created` / `output_item.added` / `output_text.delta` /
/// `output_text.done` / `content_part.done` / `output_item.done` /
/// `response.completed` 与结尾 `[DONE]`，并带 `event:` 行与 `sequence_number`。
pub fn render_response_sse(resp: &CanonicalResponse) -> Vec<String> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<String>, event: &str, data: Value| {
        out.push(format!(
            "event: {event}
data: {data}

"
        ));
    };

    let in_progress = json!({
        "id": resp.id,
        "object": "response",
        "created_at": crate::store::now_ms() / 1000,
        "status": "in_progress",
        "model": resp.model,
        "output": [],
    });
    push(
        &mut out,
        "response.created",
        json!({
            "type": "response.created",
            "response": in_progress,
        }),
    );
    push(
        &mut out,
        "response.in_progress",
        json!({
            "type": "response.in_progress",
            "response": in_progress,
        }),
    );

    for (output_index, block) in resp.output.iter().enumerate() {
        match block {
            Block::Text { text } => {
                let item_id = format!("msg_{}", resp.id);
                let empty_part = json!({"type":"output_text","text":"","annotations":[]});
                let item = json!({
                    "type":"message",
                    "id": item_id,
                    "role":"assistant",
                    "status":"in_progress",
                    "content":[empty_part],
                });
                push(
                    &mut out,
                    "response.output_item.added",
                    json!({
                        "type":"response.output_item.added",
                        "output_index": output_index,
                        "item": item,
                    }),
                );
                push(
                    &mut out,
                    "response.content_part.added",
                    json!({
                        "type":"response.content_part.added",
                        "item_id": item_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "part": empty_part,
                    }),
                );
                push(
                    &mut out,
                    "response.output_text.delta",
                    json!({
                        "type":"response.output_text.delta",
                        "item_id": item_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "delta": text,
                    }),
                );
                push(
                    &mut out,
                    "response.output_text.done",
                    json!({
                        "type":"response.output_text.done",
                        "item_id": item_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "text": text,
                    }),
                );
                let done_part = json!({"type":"output_text","text":text,"annotations":[]});
                push(
                    &mut out,
                    "response.content_part.done",
                    json!({
                        "type":"response.content_part.done",
                        "item_id": item_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "part": done_part,
                    }),
                );
                push(
                    &mut out,
                    "response.output_item.done",
                    json!({
                        "type":"response.output_item.done",
                        "output_index": output_index,
                        "item": {
                            "type":"message",
                            "id": item_id,
                            "role":"assistant",
                            "status":"completed",
                            "content":[done_part],
                        },
                    }),
                );
            }
            Block::ToolUse { id, name, input } => {
                let item_id = format!("fc_{id}");
                let arguments = serde_json::to_string(input).unwrap_or_else(|_| "{}".into());
                let item = json!({
                    "type":"function_call",
                    "id": item_id,
                    "call_id": id,
                    "name": name,
                    "arguments": "",
                });
                push(
                    &mut out,
                    "response.output_item.added",
                    json!({
                        "type":"response.output_item.added",
                        "output_index": output_index,
                        "item": item,
                    }),
                );
                push(
                    &mut out,
                    "response.function_call_arguments.delta",
                    json!({
                        "type":"response.function_call_arguments.delta",
                        "item_id": item_id,
                        "output_index": output_index,
                        "delta": arguments,
                    }),
                );
                push(
                    &mut out,
                    "response.function_call_arguments.done",
                    json!({
                        "type":"response.function_call_arguments.done",
                        "item_id": item_id,
                        "output_index": output_index,
                        "arguments": arguments,
                    }),
                );
                push(
                    &mut out,
                    "response.output_item.done",
                    json!({
                        "type":"response.output_item.done",
                        "output_index": output_index,
                        "item": {
                            "type":"function_call",
                            "id": item_id,
                            "call_id": id,
                            "name": name,
                            "arguments": arguments,
                        },
                    }),
                );
            }
            _ => {}
        }
    }

    let completed = render_response(resp);
    push(
        &mut out,
        "response.completed",
        json!({
            "type":"response.completed",
            "response": completed,
        }),
    );
    out.push("data: [DONE]\n\n".to_string());
    out
}

/// Responses SSE 渲染状态。
#[derive(Debug, Clone, Default)]
pub struct RenderState {
    pub response_id: String,
    pub model: String,
    pub started: bool,
    /// 当前 item 在 output[] 中的序号
    pub output_index: usize,
    /// 当前文本 item 是否已开
    pub item_started: bool,
    /// 当前文本 item 已累积的文本（用于 output_text.done / content_part.done）
    pub current_text: String,
}

impl RenderState {
    fn new_response(&self, status: &str) -> Value {
        json!({
            "id": self.response_id,
            "object": "response",
            "created_at": crate::store::now_ms() / 1000,
            "status": status,
            "model": self.model,
            "output": [],
        })
    }
}

/// 渲染单个 IR 流事件为 Responses SSE 的 `data: {...}` 帧。
/// 返回 0..N 个 JSON payload；调用方负责包成 `data: {payload}\n\n`。
pub fn render_stream_event(e: &StreamEvent, st: &mut RenderState) -> Vec<String> {
    use crate::codec::ir::StreamEvent as Ev;
    match e {
        Ev::Start { model: _ } => {
            st.started = true;
            st.output_index = 0;
            st.item_started = false;
            st.current_text.clear();
            vec![
                json!({
                    "type": "response.created",
                    "response": st.new_response("in_progress"),
                })
                .to_string(),
                json!({
                    "type": "response.in_progress",
                    "response": st.new_response("in_progress"),
                })
                .to_string(),
            ]
        }
        Ev::TextDelta { text } => {
            if text.is_empty() {
                return Vec::new();
            }
            let mut out = Vec::new();
            if !st.item_started {
                st.item_started = true;
                st.current_text.clear();
                out.push(
                    json!({
                        "type": "response.output_item.added",
                        "output_index": st.output_index,
                        "item": {
                            "type": "message",
                            "id": format!("msg_{}", st.response_id),
                            "role": "assistant",
                            "status": "in_progress",
                            "content": [],
                        },
                    })
                    .to_string(),
                );
                out.push(
                    json!({
                        "type": "response.content_part.added",
                        "item_id": format!("msg_{}", st.response_id),
                        "output_index": st.output_index,
                        "content_index": 0,
                        "part": {"type": "output_text", "text": "", "annotations": []},
                    })
                    .to_string(),
                );
            }
            st.current_text.push_str(text);
            out.push(
                json!({
                    "type": "response.output_text.delta",
                    "item_id": format!("msg_{}", st.response_id),
                    "output_index": st.output_index,
                    "content_index": 0,
                    "delta": text,
                })
                .to_string(),
            );
            out
        }
        Ev::ThinkingDelta { .. } => Vec::new(),
        Ev::ToolCallStart { index, id, name } => {
            let item_id = format!("fc_{index}_{id}");
            st.item_started = true;
            vec![json!({
                "type": "response.output_item.added",
                "output_index": st.output_index,
                "item": {
                    "type": "function_call",
                    "id": item_id,
                    "call_id": id,
                    "name": name,
                    "arguments": "",
                },
            })
            .to_string()]
        }
        Ev::ToolCallArgsDelta {
            index,
            args_fragment,
            ..
        } => {
            vec![json!({
                "type": "response.function_call_arguments.delta",
                "item_id": format!("fc_{index}_{}", "pending"),
                "output_index": st.output_index,
                "delta": args_fragment,
            })
            .to_string()]
        }
        Ev::ToolCallEnd { index } => {
            let mut out = vec![json!({
                "type": "response.function_call_arguments.done",
                "item_id": format!("fc_{index}_{}", "pending"),
                "output_index": st.output_index,
            })
            .to_string()];
            out.push(
                json!({
                    "type": "response.output_item.done",
                    "output_index": st.output_index,
                    "item": {
                        "type": "function_call",
                        "id": format!("fc_{index}_{}", "pending"),
                        "call_id": format!("call_{index}"),
                        "name": "",
                        "arguments": "",
                    },
                })
                .to_string(),
            );
            st.output_index += 1;
            st.item_started = false;
            out
        }
        Ev::Finish { stop_reason, usage } => {
            let mut out = Vec::new();
            if st.item_started {
                let item_id = format!("msg_{}", st.response_id);
                let text = std::mem::take(&mut st.current_text);
                out.push(
                    json!({
                        "type": "response.output_text.done",
                        "item_id": item_id,
                        "output_index": st.output_index,
                        "content_index": 0,
                        "text": text,
                    })
                    .to_string(),
                );
                let part = json!({"type": "output_text", "text": text, "annotations": []});
                out.push(
                    json!({
                        "type": "response.content_part.done",
                        "item_id": item_id,
                        "output_index": st.output_index,
                        "content_index": 0,
                        "part": part,
                    })
                    .to_string(),
                );
                out.push(
                    json!({
                        "type": "response.output_item.done",
                        "output_index": st.output_index,
                        "item": {
                            "type": "message",
                            "id": item_id,
                            "role": "assistant",
                            "status": "completed",
                            "content": [part],
                        },
                    })
                    .to_string(),
                );
                st.item_started = false;
            }
            let status = match stop_reason {
                StopReason::MaxTokens | StopReason::SafetyBlock => "incomplete",
                _ => "completed",
            };
            let mut usage_json = json!({
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
                "total_tokens": usage.input_tokens + usage.output_tokens,
            });
            if let Some(cr) = usage.cache_read_tokens {
                usage_json["input_tokens_details"] = json!({"cached_tokens": cr});
            }
            let mut response = st.new_response(status);
            response["usage"] = usage_json;
            out.push(
                json!({
                    "type": "response.completed",
                    "response": response,
                })
                .to_string(),
            );
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::ir::Usage;

    #[test]
    fn decode_basic_text_instructions() {
        let req = decode_request(
            br#"{
                "model":"gpt-4o",
                "instructions":"Be concise.",
                "input":"hello",
                "stream":true,
                "max_output_tokens":128
            }"#,
        )
        .unwrap();
        assert_eq!(req.model, "gpt-4o");
        assert_eq!(req.system, vec!["Be concise."]);
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].blocks[0].as_text(), Some("hello"));
        assert!(req.stream);
        assert_eq!(req.params.max_output_tokens, Some(128));
    }

    #[test]
    fn decode_message_and_function_items() {
        let req = decode_request(
            br#"{
                "model":"gpt-4o",
                "input":[
                    {"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]},
                    {"type":"function_call","call_id":"call_1","name":"get_weather","arguments":"{\"city\":\"bj\"}"},
                    {"type":"function_call_output","call_id":"call_1","output":"sunny"}
                ],
                "tools":[{"type":"function","name":"get_weather","description":"w","parameters":{"type":"object"}}],
                "tool_choice":"auto"
            }"#,
        )
        .unwrap();
        assert_eq!(req.messages.len(), 3);
        assert!(matches!(
            &req.messages[1].blocks[0],
            Block::ToolUse { id, name, .. } if id == "call_1" && name == "get_weather"
        ));
        assert!(matches!(
            &req.messages[2].blocks[0],
            Block::ToolResult { call_id, content, .. } if call_id == "call_1" && content[0].as_text() == Some("sunny")
        ));
        assert_eq!(req.tools.len(), 1);
        assert_eq!(req.tool_choice, ToolChoice::Auto);
    }

    #[test]
    fn decode_input_items_without_type_field() {
        // dsh 与 OpenAI 官方格式：input 数组项省略顶层 type（只有 role+content）。
        // 曾因 type 为空落入 _ 分支被丢弃，导致 messages 为空、上游 400 拒绝。
        let req = decode_request(
            br#"{
                "model":"gpt-4o",
                "instructions":"Be concise.",
                "input":[
                    {"role":"user","content":[{"type":"input_text","text":"hello"}]},
                    {"role":"assistant","content":[{"type":"output_text","text":"hi there"}]}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(req.system, vec!["Be concise."]);
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role, Role::User);
        assert_eq!(req.messages[0].blocks[0].as_text(), Some("hello"));
        assert_eq!(req.messages[1].role, Role::Assistant);
        assert_eq!(req.messages[1].blocks[0].as_text(), Some("hi there"));
        // 显式带 type 的 function_call 项仍按原名解析，不受影响
        let req2 = decode_request(
            br#"{
                "model":"gpt-4o",
                "input":[
                    {"type":"function_call","call_id":"call_1","name":"f","arguments":"{}"}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(req2.messages.len(), 1);
        assert!(matches!(
            &req2.messages[0].blocks[0],
            Block::ToolUse { name, .. } if name == "f"
        ));
    }

    #[test]
    fn render_response_shape() {
        let r = CanonicalResponse {
            id: "resp_test".into(),
            model: "gpt-4o".into(),
            output: vec![
                Block::Text {
                    text: "hello".into(),
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
                output_tokens: 5,
                cache_read_tokens: Some(3),
                cache_write_tokens: None,
            },
        };
        let v = render_response(&r);
        assert_eq!(v["object"], "response");
        assert_eq!(v["output"][0]["type"], "message");
        assert_eq!(v["output"][0]["content"][0]["text"], "hello");
        assert_eq!(v["output"][1]["type"], "function_call");
        assert_eq!(v["output"][1]["call_id"], "call_1");
        assert_eq!(v["usage"]["input_tokens"], 10);
    }

    #[test]
    fn encode_request_roundtrips_through_decode() {
        let req = CanonicalRequest {
            model: "gpt-4o".into(),
            system: vec!["Be concise.".into()],
            messages: vec![
                CanonMessage::text(Role::User, "hello"),
                CanonMessage {
                    role: Role::Assistant,
                    blocks: vec![Block::ToolUse {
                        id: "call_1".into(),
                        name: "get_weather".into(),
                        input: json!({"city":"bj"}),
                    }],
                },
                CanonMessage {
                    role: Role::User,
                    blocks: vec![Block::ToolResult {
                        call_id: "call_1".into(),
                        content: vec![Block::Text {
                            text: "sunny".into(),
                        }],
                        is_error: false,
                    }],
                },
            ],
            tools: vec![ToolSpec {
                name: "get_weather".into(),
                description: Some("weather".into()),
                input_schema: json!({"type":"object"}),
            }],
            tool_choice: ToolChoice::Auto,
            params: SampleParams {
                max_output_tokens: Some(128),
                ..SampleParams::default()
            },
            stream: false,
            extensions: Map::new(),
        };
        let encoded = encode_request(&req).unwrap();
        let decoded = decode_request(&serde_json::to_vec(&encoded).unwrap()).unwrap();
        assert_eq!(decoded.model, req.model);
        assert_eq!(decoded.system, req.system);
        assert_eq!(decoded.messages.len(), req.messages.len());
        assert_eq!(decoded.tools.len(), req.tools.len());
        assert!(matches!(
            &decoded.messages[1].blocks[0],
            Block::ToolUse { name, .. } if name == "get_weather"
        ));
        assert!(matches!(
            &decoded.messages[2].blocks[0],
            Block::ToolResult { call_id, .. } if call_id == "call_1"
        ));
    }

    #[test]
    fn parse_response_reads_text_and_function_call() {
        let body = json!({
            "id": "resp_1",
            "object": "response",
            "status": "completed",
            "model": "gpt-4o",
            "output": [
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "ok"}]},
                {"type": "function_call", "call_id": "call_1", "name": "get_weather", "arguments": "{\"city\":\"bj\"}"}
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5, "input_tokens_details": {"cached_tokens": 3}}
        });
        let parsed = parse_response(&serde_json::to_vec(&body).unwrap()).unwrap();
        assert_eq!(parsed.id, "resp_1");
        assert_eq!(parsed.output.len(), 2);
        assert_eq!(parsed.output[0].as_text(), Some("ok"));
        assert!(matches!(
            &parsed.output[1],
            Block::ToolUse { id, name, input } if id == "call_1" && name == "get_weather" && input["city"] == "bj"
        ));
        assert_eq!(parsed.stop_reason, StopReason::ToolUse);
        assert_eq!(parsed.usage.input_tokens, 10);
        assert_eq!(parsed.usage.output_tokens, 5);
        assert_eq!(parsed.usage.cache_read_tokens, Some(3));
    }

    #[test]
    fn render_response_sse_has_event_lines_and_done() {
        let r = CanonicalResponse {
            id: "resp_sse".into(),
            model: "gpt-4o".into(),
            output: vec![Block::Text {
                text: "hello".into(),
            }],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        };
        let frames = render_response_sse(&r);
        assert!(frames[0].starts_with(
            "event: response.created
"
        ));
        assert!(frames.iter().any(|f| f.starts_with(
            "event: response.output_text.done
"
        )));
        assert!(frames.last().unwrap().contains("[DONE]"));
        assert!(frames.iter().any(|f| f.starts_with(
            "event: response.completed
"
        )));
    }

    #[test]
    fn render_response_sse_includes_function_call_events() {
        let r = CanonicalResponse {
            id: "resp_tool".into(),
            model: "gpt-4o".into(),
            output: vec![Block::ToolUse {
                id: "call_1".into(),
                name: "get_weather".into(),
                input: json!({ "city": "bj" }),
            }],
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
        };
        let frames = render_response_sse(&r);
        assert!(frames.iter().any(|f| f.starts_with(
            "event: response.output_item.added
"
        )));
        assert!(frames.iter().any(|f| f.starts_with(
            "event: response.function_call_arguments.done
"
        )));
        assert!(frames.iter().any(|f| f.contains("get_weather")));
        assert!(frames.last().unwrap().contains("[DONE]"));
    }

    #[test]
    fn render_stream_events() {
        let mut st = RenderState {
            response_id: "resp_1".into(),
            model: "gpt-4o".into(),
            current_text: String::new(),
            ..Default::default()
        };
        let start = render_stream_event(
            &StreamEvent::Start {
                model: "gpt-4o".into(),
            },
            &mut st,
        );
        assert_eq!(start.len(), 2);
        assert!(start[0].contains("response.created"));

        let txt = render_stream_event(&StreamEvent::TextDelta { text: "hi".into() }, &mut st);
        assert!(txt.iter().any(|s| s.contains("response.output_text.delta")));

        let tool_start = render_stream_event(
            &StreamEvent::ToolCallStart {
                index: 0,
                id: "call_1".into(),
                name: "get_weather".into(),
            },
            &mut st,
        );
        assert!(tool_start[0].contains("response.output_item.added"));

        let args = render_stream_event(
            &StreamEvent::ToolCallArgsDelta {
                index: 0,
                args_fragment: "{\"city\":\"bj\"}".into(),
            },
            &mut st,
        );
        assert!(args[0].contains("response.function_call_arguments.delta"));

        let fin = render_stream_event(
            &StreamEvent::Finish {
                stop_reason: StopReason::ToolUse,
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
            },
            &mut st,
        );
        assert!(fin.iter().any(|s| s.contains("response.completed")));
        assert!(
            fin.iter().any(|s| s.contains("response.output_text.done")),
            "Finish 应补齐 output_text.done"
        );
    }
}
