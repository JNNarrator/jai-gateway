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

/// 从 Responses reasoning item 提取推理文本，兼容三种形状：
/// - OpenAI 标准：`summary` / `content` 数组（summary_text / reasoning_text 元素）
/// - 旧式：`reasoning` 数组（含 `text` 字段）或字符串
///
/// 只认推理类 part（reasoning_text/summary_text/无 type 的 text），
/// 不抓 output_text/input_text 正文（否则 message 正文会被误当推理）。
fn extract_reasoning_text(item: &Value) -> String {
    let parts = |key: &str| -> String {
        item.get(key)
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter(|p| {
                        // 只收集推理类 part：明确带 type 的必须是 reasoning_text/
                        // summary_text；不带 type 的按 text 容错（旧式形状）。
                        match p.get("type").and_then(Value::as_str) {
                            Some("reasoning_text" | "summary_text") => true,
                            Some(_) => false,
                            None => true,
                        }
                    })
                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    };
    // 空数组也要视为「无内容」继续尝试下一形状，不能阻断 fallthrough
    for key in ["content", "summary", "reasoning"] {
        let t = parts(key);
        if !t.is_empty() {
            return t;
        }
    }
    item.get("reasoning")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// 把当前累积的 assistant 轮次 flush 成一条消息。
/// dsh 等 agent 把一轮 assistant 输出拆成独立 item（reasoning/message/function_call），
/// 必须合并回同一条 IR 消息，出站才会是带 reasoning_content + tool_calls 的
/// 同一条 chat 消息（thinking 上游要求，否则 400）。
/// `pending` 是该轮前置的独立 reasoning 文本，作为 Thinking 块放在最前。
fn flush_assistant_round(
    messages: &mut Vec<CanonMessage>,
    cur: &mut Vec<Block>,
    pending: &mut String,
) {
    if cur.is_empty() {
        // 无宿主 assistant 消息，独立的 reasoning 自然丢弃
        pending.clear();
        return;
    }
    let mut blocks = Vec::with_capacity(cur.len() + 1);
    if !pending.is_empty() {
        blocks.push(Block::Thinking {
            signature: None,
            text: std::mem::take(pending),
        });
    }
    blocks.append(cur);
    messages.push(CanonMessage {
        role: Role::Assistant,
        blocks,
    });
}

/// 解码 `/v1/responses` 请求体 → CanonicalRequest。
pub fn decode_request(body: &[u8]) -> Result<CanonicalRequest, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("请求体不是合法 JSON: {e}"))?;

    // 临时诊断：JAI_DEBUG_RESPONSES_INPUT=1 时打印 input 数组结构（定位 reasoning 丢失）
    if std::env::var("JAI_DEBUG_RESPONSES_INPUT").is_ok() {
        if let Some(input) = v.get("input") {
            eprintln!(
                "[debug] responses input:\n{}",
                serde_json::to_string_pretty(input).unwrap_or_default()
            );
        }
    }

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
                // OpenAI Responses 里一轮 assistant 输出可能拆成多个独立 item：
                //   reasoning → message(output_text) → function_call
                // （dsh 等 agent 把流式 output_item 原样存历史）。这三者属于同一轮，
                // 必须合并成一条 IR assistant 消息（Thinking+Text+ToolUse），出站
                // chat 才会是带 reasoning_content + tool_calls 的同一条消息——
                // deepseek 等 thinking 模型要求 tool_calls 消息原样回传
                // reasoning_content，否则上游 400。
                let mut pending_reasoning = String::new();
                // 当前 assistant 轮次的累积块（尚未 flush）
                let mut cur_asst: Vec<Block> = Vec::new();
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
                            if role == "assistant" {
                                // 消息内嵌 reasoning 字段（OpenAI 标准形状）或内嵌
                                // function_call（推理模型的真实历史格式）都并入本轮。
                                let rtext = extract_reasoning_text(item);
                                if !rtext.is_empty() {
                                    blocks.push(Block::Thinking {
                                        signature: None,
                                        text: rtext,
                                    });
                                }
                                if let Some(fc) = item.get("function_call") {
                                    // function_call 可能是单对象（内嵌形态），也可能
                                    // 是数组（多工具调用的容错形态）；数组逐个展开。
                                    let fcs: Vec<&Value> = match fc {
                                        Value::Array(arr) => arr.iter().collect(),
                                        _ => vec![fc],
                                    };
                                    for f in fcs {
                                        let id = f
                                            .get("call_id")
                                            .or_else(|| f.get("id"))
                                            .and_then(Value::as_str)
                                            .unwrap_or_default()
                                            .to_string();
                                        let name = f
                                            .get("name")
                                            .and_then(Value::as_str)
                                            .unwrap_or_default()
                                            .to_string();
                                        let input: Value = f
                                            .get("arguments")
                                            .and_then(Value::as_str)
                                            .and_then(|s| serde_json::from_str(s).ok())
                                            .unwrap_or_else(|| json!({}));
                                        blocks.push(Block::ToolUse { id, name, input });
                                    }
                                }
                                cur_asst.extend(blocks);
                            } else {
                                // user 消息：先 flush 上一轮 assistant 累积
                                flush_assistant_round(&mut messages, &mut cur_asst, &mut pending_reasoning);
                                if let Some(fco) = item.get("function_call_output") {
                                    let call_id = fco
                                        .get("call_id")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string();
                                    let output = fco
                                        .get("output")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string();
                                    blocks.push(Block::ToolResult {
                                        call_id,
                                        content: if output.is_empty() {
                                            vec![]
                                        } else {
                                            vec![Block::Text { text: output }]
                                        },
                                        is_error: false,
                                    });
                                }
                                if !blocks.is_empty() {
                                    messages.push(CanonMessage {
                                        role: Role::User,
                                        blocks,
                                    });
                                }
                            }
                        }
                        "function_call" => {
                            // 独立 function_call item：属于当前 assistant 轮次，
                            // 累积进 cur_asst（与前置 reasoning/文本合并）
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
                            cur_asst.push(Block::ToolUse { id, name, input });
                        }
                        "function_call_output" => {
                            // tool 结果：flush 当前 assistant 轮次（含 reasoning），
                            // 再作为 user ToolResult 落一条
                            flush_assistant_round(&mut messages, &mut cur_asst, &mut pending_reasoning);
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
                            // 独立 reasoning item：缓存文本，随本轮 assistant flush
                            // 时作为 Thinking 块放在最前（thinking 模型要求原样回传
                            // reasoning_content，否则上游 400）。
                            let text = extract_reasoning_text(item);
                            if !text.is_empty() {
                                pending_reasoning = text;
                            }
                        }
                        other => {
                            extensions
                                .entry(format!("input_item:{other}"))
                                .or_insert(Value::Null);
                        }
                    }
                }
                // 收尾：flush 最后累积的 assistant 轮次
                flush_assistant_round(&mut messages, &mut cur_asst, &mut pending_reasoning);
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
            Block::Thinking { text, .. } => {
                // 推理内容以独立 reasoning item 下发，客户端（dsh 等）存进历史，
                // 下轮回传原样带回（thinking 模型要求，否则上游 400）
                output.push(json!({
                    "type": "reasoning",
                    "id": format!("rs_{}", r.id),
                    "role": "assistant",
                    "summary": [],
                    "content": [{"type": "reasoning_text", "text": text, "annotations": []}],
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
    let usage = build_usage(&r.usage);
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

/// 把 IR Usage 渲染成 Responses 协议 usage 对象。
///
/// 客户端（dsh-tui 等 agent）从完成事件的 usage 里读取上下文占用：
/// input_tokens 为占用主体，input_tokens_details.cached_tokens /
/// cache_write_tokens 分别为缓存命中与写入。跨协议转换时若无缓存细分，
/// 也不应收敛 input_tokens（OpenAI 语义里 input_tokens 已含缓存部分）。
fn build_usage(u: &Usage) -> Value {
    let mut usage = json!({
        "input_tokens": u.input_tokens,
        "output_tokens": u.output_tokens,
        "total_tokens": u.input_tokens + u.output_tokens,
    });
    if u.cache_read_tokens.is_some() || u.cache_write_tokens.is_some() {
        let mut details = serde_json::Map::new();
        if let Some(cr) = u.cache_read_tokens {
            details.insert("cached_tokens".into(), json!(cr));
        }
        if let Some(cw) = u.cache_write_tokens {
            details.insert("cache_write_tokens".into(), json!(cw));
        }
        usage["input_tokens_details"] = Value::Object(details);
    }
    usage
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
    if let Some(d) = usage
        .get("input_tokens_details")
        .and_then(|d| d.get("cache_write_tokens"))
        .and_then(Value::as_u64)
    {
        parsed_usage.cache_write_tokens = Some(d);
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
            Block::Thinking { text, .. } => {
                // 推理内容独立 reasoning item（含 added/delta/done 生命周期）
                let item_id = format!("rs_{}", resp.id);
                let item = json!({
                    "type":"reasoning",
                    "id": item_id,
                    "role":"assistant",
                    "summary":[],
                    "content":[],
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
                    "response.reasoning_text.delta",
                    json!({
                        "type":"response.reasoning_text.delta",
                        "item_id": item_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "delta": text,
                    }),
                );
                let done_item = json!({
                    "type":"reasoning",
                    "id": item_id,
                    "role":"assistant",
                    "summary":[],
                    "content":[{"type":"reasoning_text","text":text,"annotations":[]}],
                });
                push(
                    &mut out,
                    "response.output_item.done",
                    json!({
                        "type":"response.output_item.done",
                        "output_index": output_index,
                        "item": done_item,
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
    /// 当前 reasoning item 是否已开（thinking 增量流）
    pub reasoning_started: bool,
    /// 当前 reasoning item 已累积的推理文本（用于 output_item.done 回填）
    pub current_reasoning: String,
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
    // 非 thinking 事件到来前，若 reasoning item 仍开着（上游 thinking 总是先于
    // text/tool delta 到达），先关闭它并推进 output_index，避免与后续 item 撞号。
    fn close_reasoning(st: &mut RenderState, out: &mut Vec<String>) {
        if !st.reasoning_started {
            return;
        }
        st.reasoning_started = false;
        let text = std::mem::take(&mut st.current_reasoning);
        let part = json!({"type": "reasoning_text", "text": text, "annotations": []});
        out.push(
            json!({
                "type": "response.output_item.done",
                "output_index": st.output_index,
                "item": {
                    "type": "reasoning",
                    "id": format!("rs_{}", st.response_id),
                    "role": "assistant",
                    "summary": [],
                    "content": [part],
                },
            })
            .to_string(),
        );
        st.output_index += 1;
    }
    match e {
        Ev::Start { model: _ } => {
            st.started = true;
            st.output_index = 0;
            st.item_started = false;
            st.current_text.clear();
            st.reasoning_started = false;
            st.current_reasoning.clear();
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
            close_reasoning(st, &mut out);
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
        Ev::ThinkingDelta { text } => {
            if text.is_empty() {
                return Vec::new();
            }
            let mut out = Vec::new();
            if !st.reasoning_started {
                st.reasoning_started = true;
                st.current_reasoning.clear();
                out.push(
                    json!({
                        "type": "response.output_item.added",
                        "output_index": st.output_index,
                        "item": {
                            "type": "reasoning",
                            "id": format!("rs_{}", st.response_id),
                            "role": "assistant",
                            "summary": [],
                            "content": [],
                        },
                    })
                    .to_string(),
                );
            }
            st.current_reasoning.push_str(text);
            out.push(
                json!({
                    "type": "response.reasoning_text.delta",
                    "item_id": format!("rs_{}", st.response_id),
                    "output_index": st.output_index,
                    "content_index": 0,
                    "delta": text,
                })
                .to_string(),
            );
            out
        }
        Ev::ToolCallStart { index, id, name } => {
            let mut out = Vec::new();
            close_reasoning(st, &mut out);
            let item_id = format!("fc_{index}_{id}");
            st.item_started = true;
            out.push(
                json!({
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
                .to_string(),
            );
            out
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
            // 纯 thinking 响应（无 text/tool）：先关闭 reasoning item 再收尾
            close_reasoning(st, &mut out);
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
            let usage_json = build_usage(usage);
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
    fn decode_assistant_embedded_function_call() {
        // OpenAI Responses 多轮历史：assistant 消息内嵌 function_call 顶层字段
        //（reasoning 模型历史），user 消息可内嵌 function_call_output。
        let req = decode_request(
            br#"{
                "model":"gpt-4o",
                "input":[
                    {"role":"user","content":[{"type":"input_text","text":"weather?"}]},
                    {"role":"assistant","content":[],
                     "reasoning":[{"type":"reasoning_text","text":"think"}],
                     "function_call":{"call_id":"call_e1","name":"get_weather","arguments":"{\"city\":\"bj\"}"}},
                    {"role":"user","content":[],
                     "function_call_output":{"call_id":"call_e1","output":"sunny"}}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(req.messages.len(), 3);
        assert!(matches!(
            &req.messages[1].blocks[0],
            Block::Thinking { text, .. } if text == "think"
        ));
        assert!(matches!(
            &req.messages[1].blocks[1],
            Block::ToolUse { id, name, .. } if id == "call_e1" && name == "get_weather"
        ));
        assert!(matches!(
            &req.messages[2].blocks[0],
            Block::ToolResult { call_id, content, .. }
                if call_id == "call_e1" && content[0].as_text() == Some("sunny")
        ));
    }

    #[test]
    fn decode_assistant_function_call_array_expands_multiple() {
        let req = decode_request(
            r#"{
                "model":"gpt-4o",
                "input":[
                    {"role":"assistant","content":[],
                     "function_call":[
                        {"call_id":"call_a1","name":"bash","arguments":"{\"command\":\"a\"}"},
                        {"call_id":"call_b2","name":"bash","arguments":"{\"command\":\"b\"}"}
                     ]}
                ]
            }"#
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(req.messages.len(), 1);
        let blocks = &req.messages[0].blocks;
        assert_eq!(blocks.len(), 2, "数组形态应展开为 2 个 ToolUse");
        assert!(matches!(
            &blocks[0],
            Block::ToolUse { id, name, input } if id == "call_a1" && name == "bash" && input["command"] == "a"
        ));
        assert!(matches!(
            &blocks[1],
            Block::ToolUse { id, name, input } if id == "call_b2" && name == "bash" && input["command"] == "b"
        ));
    }

    #[test]
    fn decode_standalone_reasoning_merges_into_next_assistant() {
        // dsh 等 agent 的真实历史：独立 reasoning item 先于其所属 assistant message，
        // 推理内容必须并入该 assistant 一并回传（thinking 模型要求原样带
        // reasoning_content，否则上游 400）。旧实现把它当未知字段丢弃。
        let req = decode_request(
            br#"{
                "model":"gpt-4o",
                "input":[
                    {"role":"user","content":[{"type":"input_text","text":"hi"}]},
                    {"type":"reasoning","id":"rs_e1",
                     "reasoning":[{"type":"reasoning_text","text":"thinking step one"}]},
                    {"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]},
                    {"role":"user","content":[{"type":"input_text","text":"continue"}]}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(req.messages.len(), 3);
        let asst = &req.messages[1];
        assert_eq!(asst.role, Role::Assistant);
        // Thinking 块并入 assistant 顶部，text/工具块在其后
        assert!(matches!(
            &asst.blocks[0],
            Block::Thinking { text, .. } if text == "thinking step one"
        ));
        assert!(matches!(&asst.blocks[1], Block::Text { text } if text == "ok"));

        // 独立 reasoning 与 function_call 组合：Thinking、ToolUse 都并入同一条 assistant
        let req2 = decode_request(
            br#"{
                "model":"gpt-4o",
                "input":[
                    {"type":"reasoning","id":"rs_e2",
                     "reasoning":[{"type":"reasoning_text","text":"think step"}]},
                    {"type":"message","role":"assistant","content":[],
                     "function_call":[{"call_id":"call_e2","name":"f","arguments":"{}"}]}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(req2.messages.len(), 1);
        assert!(matches!(
            &req2.messages[0].blocks[0],
            Block::Thinking { text, .. } if text == "think step"
        ));
        assert!(matches!(
            &req2.messages[0].blocks[1],
            Block::ToolUse { id, name, .. } if id == "call_e2" && name == "f"
        ));

        // 跨族出站（Responses→openai_compat）：assistant 推理内容还原为 reasoning_content，
        // 与 tool_calls 同消息，供 thinking 上游校验回传（否则上游 400）
        let encoded = crate::codec::openai::encode_request(&req2).unwrap();
        let msgs = encoded["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["reasoning_content"], "think step");
        assert!(msgs[0]["tool_calls"].is_array());
    }

    #[test]
    fn decode_openai_standard_reasoning_item_shapes() {
        // OpenAI Responses 官方 output 里 reasoning item 用 summary/content 数组
        // （summary_text / reasoning_text 元素）；dsh（pi-ai）回传历史时按
        // output_item.done 的 item 原样放回，故入站需兼容这两种形状。
        let req = decode_request(
            br#"{
                "model":"gpt-4o",
                "input":[
                    {"role":"user","content":[{"type":"input_text","text":"hi"}]},
                    {"type":"reasoning","id":"rs_s1",
                     "summary":[],
                     "content":[{"type":"reasoning_text","text":"think via content"}]},
                    {"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}
                ]
            }"#,
        )
        .unwrap();
        assert!(matches!(
            &req.messages[1].blocks[0],
            Block::Thinking { text, .. } if text == "think via content"
        ));

        let req2 = decode_request(
            br#"{
                "model":"gpt-4o",
                "input":[
                    {"type":"reasoning","id":"rs_s2",
                     "summary":[{"type":"summary_text","text":"brief"},{"type":"summary_text","text":"more"}],
                     "content":[]},
                    {"type":"message","role":"assistant","content":[],
                     "function_call":{"call_id":"call_s2","name":"f","arguments":"{}"}}
                ]
            }"#,
        )
        .unwrap();
        assert!(matches!(
            &req2.messages[0].blocks[0],
            Block::Thinking { text, .. } if text == "brief\nmore"
        ));
    }

    #[test]
    fn decode_standalone_function_call_keeps_pending_reasoning() {
        // dsh（pi-ai）的真实历史形状：reasoning 与 function_call 是两个独立
        // output item（流式 output_item.added/done 原样存回）。旧实现 function_call
        // 分支不消费 pending_reasoning → IR assistant 消息只有 ToolUse 无 Thinking →
        // 跨族出站缺 reasoning_content → thinking 上游校验 400（76cb5093 实测）。
        let req = decode_request(
            br#"{
                "model":"gpt-4o",
                "input":[
                    {"role":"user","content":[{"type":"input_text","text":"hi"}]},
                    {"type":"reasoning","id":"rs_f1",
                     "summary":[],
                     "content":[{"type":"reasoning_text","text":"think before call"}]},
                    {"type":"function_call","call_id":"call_f1","name":"lookup","arguments":"{\"k\":\"v\"}"}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(req.messages.len(), 2);
        let asst = &req.messages[1];
        assert_eq!(asst.role, Role::Assistant);
        // Thinking 必须并入 function_call 同一条 assistant 消息（在 ToolUse 前）
        assert!(matches!(
            &asst.blocks[0],
            Block::Thinking { text, .. } if text == "think before call"
        ), "pending_reasoning 丢失: {asst:?}");
        assert!(matches!(
            &asst.blocks[1],
            Block::ToolUse { id, name, .. } if id == "call_f1" && name == "lookup"
        ));

        // 跨族出站（Responses→openai_compat）：tool_calls 消息必须带 reasoning_content
        let encoded = crate::codec::openai::encode_request(&req).unwrap();
        let msgs = encoded["messages"].as_array().unwrap();
        let last = &msgs[msgs.len() - 1];
        assert_eq!(last["reasoning_content"], "think before call");
        assert!(last["tool_calls"].is_array());
    }

    #[test]
    fn decode_dsh_split_round_merges_reasoning_text_tool() {
        // dsh 76cb5093 实测真实形状：同一轮 assistant 输出被拆成三个独立 item：
        //   reasoning → message(output_text) → function_call → function_call_output
        // 旧实现把 reasoning 并入 message(文本)，导致带 tool_calls 的 assistant 消息
        // 缺 reasoning_content，上游 litellm 400；且 extract_reasoning_text 把
        // output_text 正文误当 reasoning，产生重复 Thinking。
        let req = decode_request(
            br#"{
                "model":"gpt-4o",
                "input":[
                    {"role":"user","content":[{"type":"input_text","text":"hi"}]},
                    {"type":"reasoning","id":"rs_d1","role":"assistant","summary":[],
                     "content":[{"type":"reasoning_text","text":"think step","annotations":[]}]},
                    {"type":"message","id":"msg_d1","role":"assistant","status":"completed",
                     "content":[{"type":"output_text","text":"let me search","annotations":[]}]},
                    {"type":"function_call","id":"fc_d1","call_id":"call_d1","name":"bash",
                     "arguments":"{\"command\":\"ls\"}"},
                    {"type":"function_call_output","call_id":"call_d1","output":"(no output)"}
                ]
            }"#,
        )
        .unwrap();
        // reasoning + message + function_call 合并成一条 assistant，后跟 tool 结果
        assert_eq!(req.messages.len(), 3);
        let asst = &req.messages[1];
        assert_eq!(asst.role, Role::Assistant);
        let kinds: Vec<&str> = asst
            .blocks
            .iter()
            .map(|b| match b {
                Block::Thinking { .. } => "thinking",
                Block::Text { .. } => "text",
                Block::ToolUse { .. } => "tooluse",
                _ => "other",
            })
            .collect();
        // 思考在前、正文次之、工具调用同消息（不重复、不丢失）
        assert_eq!(kinds, vec!["thinking", "text", "tooluse"], "blocks: {:?}", asst.blocks);
        assert!(matches!(&asst.blocks[0], Block::Thinking { text, .. } if text == "think step"));

        // 跨族出站：一条 assistant 消息同时带 reasoning_content + content + tool_calls
        let encoded = crate::codec::openai::encode_request(&req).unwrap();
        let msgs = encoded["messages"].as_array().unwrap();
        let asst = &msgs[1];
        assert_eq!(asst["reasoning_content"], "think step");
        assert_eq!(asst["content"], "let me search");
        assert!(asst["tool_calls"].is_array(), "tool_calls 缺失: {asst}");
    }

    #[test]
    fn render_response_includes_reasoning_item() {
        // 非流式出站：Thinking 块 → reasoning item（含 content 全文），
        // 供客户端存历史、下轮回传（thinking 上游要求原样带 reasoning_content）
        let r = CanonicalResponse {
            id: "resp_think".into(),
            model: "deepseek".into(),
            output: vec![
                Block::Thinking {
                    signature: None,
                    text: "deep thinking".into(),
                },
                Block::Text {
                    text: "answer".into(),
                },
            ],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        };
        let v = render_response(&r);
        let out = v["output"].as_array().unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["type"], "reasoning");
        assert_eq!(out[0]["content"][0]["type"], "reasoning_text");
        assert_eq!(out[0]["content"][0]["text"], "deep thinking");
        assert_eq!(out[1]["type"], "message");
        assert_eq!(out[1]["content"][0]["text"], "answer");
    }

    #[test]
    fn render_stream_thinking_events() {
        // 流式出站：ThinkingDelta → reasoning item 的 added + reasoning_text.delta
        // 事件；后续 text 到来前先 output_item.done 关闭 reasoning item。
        use crate::codec::ir::StreamEvent as Ev;
        let mut st = RenderState {
            response_id: "resp_t".into(),
            model: "deepseek".into(),
            current_text: String::new(),
            ..Default::default()
        };
        let _ = render_stream_event(
            &Ev::Start {
                model: "deepseek".into(),
            },
            &mut st,
        );

        let first = render_stream_event(
            &Ev::ThinkingDelta {
                text: "think step one".into(),
            },
            &mut st,
        );
        assert!(first[0].contains("response.output_item.added"));
        assert!(first[0].contains("\"type\":\"reasoning\""));
        assert!(first[0].contains("rs_resp_t"));
        assert!(first[1].contains("response.reasoning_text.delta"));
        assert!(first[1].contains("think step one"));

        let second = render_stream_event(
            &Ev::ThinkingDelta {
                text: " step two".into(),
            },
            &mut st,
        );
        assert!(second[0].contains("response.reasoning_text.delta"));
        assert!(second[0].contains("step two"));

        // text 到来：先关 reasoning（done 含累计全文），再开文本 item
        let txt = render_stream_event(&Ev::TextDelta { text: "hi".into() }, &mut st);
        assert!(txt[0].contains("response.output_item.done"));
        assert!(txt[0].contains("think step one step two"));
        assert!(txt[0].contains("\"type\":\"reasoning\""));
        assert!(txt.iter().any(|s| s.contains("response.output_text.delta")));
        // reasoning item 已关闭：output_index 前进到 1（text item 用）
        assert_eq!(st.output_index, 1);
        assert!(!st.reasoning_started);

        // 纯 thinking 响应（无 text）在 Finish 时也要关闭 reasoning item
        let mut st2 = RenderState {
            response_id: "resp_t2".into(),
            model: "deepseek".into(),
            ..Default::default()
        };
        let _ = render_stream_event(
            &Ev::Start {
                model: "deepseek".into(),
            },
            &mut st2,
        );
        let _ = render_stream_event(
            &Ev::ThinkingDelta {
                text: "only think".into(),
            },
            &mut st2,
        );
        let fin = render_stream_event(
            &Ev::Finish {
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
            &mut st2,
        );
        assert!(fin[0].contains("response.output_item.done"));
        assert!(fin[0].contains("only think"));
        assert!(fin.iter().any(|s| s.contains("response.completed")));
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
    fn parse_response_reads_cache_write_tokens() {
        // dsh 客户端从 input_tokens_details.cache_write_tokens 读缓存写入细分；
        // 网关入站转换必须把它带回 IR Usage，出站渲染才能继续透传。
        let body = br#"{
            "id":"resp_cw","object":"response","status":"completed","model":"gpt-4o",
            "output":[{"type":"message","id":"msg_1","role":"assistant","status":"completed","content":[{"type":"output_text","text":"hi","annotations":[]}]}],
            "usage":{"input_tokens":100,"output_tokens":7,"total_tokens":107,
              "input_tokens_details":{"cached_tokens":60,"cache_write_tokens":25}}
        }"#;
        let parsed = parse_response(body).expect("应解析成功");
        assert_eq!(parsed.usage.input_tokens, 100);
        assert_eq!(parsed.usage.cache_read_tokens, Some(60));
        assert_eq!(parsed.usage.cache_write_tokens, Some(25));

        // 出站渲染保留两个细分字段
        let rendered = render_response(&parsed);
        assert_eq!(
            rendered["usage"]["input_tokens_details"]["cached_tokens"],
            60
        );
        assert_eq!(
            rendered["usage"]["input_tokens_details"]["cache_write_tokens"],
            25
        );
    }

    #[test]
    fn build_usage_merges_cache_read_and_write() {
        let u = Usage {
            input_tokens: 50,
            output_tokens: 9,
            cache_read_tokens: Some(8),
            cache_write_tokens: Some(3),
        };
        let v = build_usage(&u);
        assert_eq!(v["input_tokens"], 50);
        assert_eq!(v["input_tokens_details"]["cached_tokens"], 8);
        assert_eq!(v["input_tokens_details"]["cache_write_tokens"], 3);

        // cache_write 单独存在时也要建嵌套对象（OpenAI 语义可能只报写入不报命中）
        let u2 = Usage {
            input_tokens: 10,
            output_tokens: 1,
            cache_read_tokens: None,
            cache_write_tokens: Some(4),
        };
        let v2 = build_usage(&u2);
        assert_eq!(v2["input_tokens_details"]["cache_write_tokens"], 4);
        assert!(v2["input_tokens_details"].get("cached_tokens").is_none());

        // 全无缓存细分时不产出空 details
        let v3 = build_usage(&Usage::default());
        assert!(v3.get("input_tokens_details").is_none());
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
