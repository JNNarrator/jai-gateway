//! UpstreamCodec::Gemini —— M4 转换：CanonicalRequest → Gemini generateContent；
//! Gemini 响应 → CanonicalResponse / StreamEvents。
//!
//! 依据 protocol-ir §4/§5：
//! - 模型名入 URL 路径（`/v1beta/models/{model}:generateContent`，剥 `models/` 前缀）
//! - system → `systemInstruction`；消息 → `contents[].parts[]`
//! - tools → `tools[].functionDeclarations[]`（parameters 仅 OpenAPI 子集，直接透传）
//! - tool_choice → `toolConfig.functionCallingConfig.mode`
//! - 响应：`candidates[0].content.parts[]`、`finishReason`、`usageMetadata`
//! - 流式（alt=sse 包）：`content.parts[].text` / 整对象 `functionCall`
//! - Gemini functionCall 无 id：由 JAI 合成（向上游/客户端方向由调用方决定）；
//!   本层解析时生成 `_k{n}` 后缀 id（冲突由调用方处理）
//! - usage：`usageMetadata.promptTokenCount / candidatesTokenCount`

use serde_json::{Value, json};

use crate::codec::ir::{
    Block, CanonicalRequest, CanonicalResponse, Role, StopReason, StreamEvent, ToolChoice, Usage,
};

/// 请求端点路径（含 model）。
pub fn model_url(model: &str) -> String {
    format!(
        "/v1beta/models/{}:generateContent",
        model.trim_start_matches("models/")
    )
}

/// 编码请求：IR → Gemini body。
pub fn encode_request(req: &CanonicalRequest) -> Result<Value, String> {
    let mut body = json!({"contents": []});

    // systemInstruction（多条合并为一段，§4-C）
    if !req.system.is_empty() {
        let joined = req.system.join("\n\n");
        body["systemInstruction"] = json!({
            "parts": [{"text": joined}]
        });
    }

    // 消息 → contents[].parts[]
    let msgs = crate::codec::ir::merge_adjacent_same_role(req).messages;
    let mut contents: Vec<Value> = Vec::new();
    // 当前正在累积的 user 轮（tool 结果与文本同轮聚合成 parts）
    let mut cur: Option<Value> = None;
    let flush = |cur: &mut Option<Value>, contents: &mut Vec<Value>| {
        if let Some(c) = cur.take() {
            contents.push(c);
        }
    };
    // 用局部可变借用避免闭包捕获问题：直接展开循环
    for m in &msgs {
        let mut parts: Vec<Value> = Vec::new();
        for b in &m.blocks {
            match b {
                Block::Text { text } => parts.push(json!({"text": text})),
                Block::Image { data_base64, .. } => {
                    // Gemini 需 inlineData（http 图片 URL 在 proxy 层已转 base64）
                    if let Some(b64) = data_base64.as_ref() {
                        parts.push(json!({
                            "inlineData": {"mimeType": "image/png", "data": b64}
                        }));
                    }
                }
                Block::ToolUse { name, input, .. } => {
                    parts.push(json!({"functionCall": {"name": name, "args": input}}));
                }
                Block::ToolResult { call_id: _, content, is_error } => {
                    // Gemini functionResponse 按 name 关联；调用方负责把 call_id 映射为 name
                    let text = content
                        .iter()
                        .filter_map(|c| c.as_text())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let response = if *is_error {
                        json!({"error": text})
                    } else {
                        json!({"result": text})
                    };
                    parts.push(json!({"functionResponse": {"name": "", "response": response}}));
                }
                _ => {}
            }
        }
        if parts.is_empty() {
            continue;
        }
        let role = match m.role {
            Role::User => "user",
            Role::Assistant => "model",
        };
        if let Some(c) = cur.as_mut() {
            if c["role"] == role {
                // 合并同角色轮次（user 的 tool 结果 + 后续文本）
                if let Some(arr) = c.get_mut("parts").and_then(Value::as_array_mut) {
                    arr.extend(parts);
                }
                continue;
            } else {
                flush(&mut cur, &mut contents);
            }
        }
        cur = Some(json!({"role": role, "parts": parts}));
    }
    flush(&mut cur, &mut contents);
    body["contents"] = Value::Array(contents);

    // tools → functionDeclarations
    if !req.tools.is_empty() {
        body["tools"] = json!([{
            "functionDeclarations": req.tools.iter().map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description.as_deref().unwrap_or(""),
                    "parameters": t.input_schema,
                })
            }).collect::<Vec<_>>()
        }]);
    }

    // tool_choice
    if !req.tools.is_empty() {
        let mode = match &req.tool_choice {
            ToolChoice::Auto => "AUTO",
            ToolChoice::None => "NONE",
            ToolChoice::Required => "ANY",
            ToolChoice::Specific(_) => "ANY",
        };
        body["toolConfig"] = json!({
            "functionCallingConfig": {
                "mode": mode,
                "allowedFunctionNames": match &req.tool_choice {
                    ToolChoice::Specific(n) => json!([n]),
                    _ => json!(null),
                },
            }
        });
    }

    // generationConfig
    let p = &req.params;
    let mut gen = serde_json::Map::new();
    if let Some(m) = p.max_output_tokens {
        gen.insert("maxOutputTokens".into(), json!(m));
    }
    if let Some(t) = p.temperature {
        gen.insert("temperature".into(), json!(super::anthropic::round_f32(t)));
    }
    if let Some(v) = p.top_p {
        gen.insert("topP".into(), json!(super::anthropic::round_f32(v)));
    }
    if let Some(k) = p.top_k {
        gen.insert("topK".into(), json!(k));
    }
    if !p.stop_sequences.is_empty() {
        gen.insert(
            "stopSequences".into(),
            json!(p.stop_sequences),
        );
    }
    if let Some(seed) = p.seed {
        gen.insert("seed".into(), json!(seed));
    }
    if !gen.is_empty() {
        body["generationConfig"] = Value::Object(gen);
    }

    Ok(body)
}

// ================================================================ 响应解析

/// 解析 Gemini 非流式响应 → CanonicalResponse。
pub fn parse_response(body: &[u8]) -> Result<CanonicalResponse, String> {
    let v: Value =
        serde_json::from_slice(body).map_err(|e| format!("Gemini 响应 JSON 解析失败: {e}"))?;
    // 错误形状穿透
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Gemini 上游错误");
        return Err(format!("Gemini 上游错误: {msg}"));
    }

    let mut output: Vec<Block> = Vec::new();
    if let Some(cands) = v.get("candidates").and_then(Value::as_array) {
        if let Some(c) = cands.first() {
            if let Some(content) = c.get("content").and_then(|x| x.get("parts")).and_then(Value::as_array)
            {
                for p in content {
                    if let Some(t) = p.get("text").and_then(Value::as_str) {
                        output.push(Block::Text { text: t.to_string() });
                    } else if let Some(fc) = p.get("functionCall") {
                        // Gemini 无 id → 合成 id（fn 名 + _k0）；冲突后缀由上层兜底
                        let name = fc.get("name").and_then(Value::as_str).unwrap_or_default();
                        let args = fc.get("args").cloned().unwrap_or_else(|| json!({}));
                        output.push(Block::ToolUse {
                            id: format!("{name}_k0"),
                            name: name.to_string(),
                            input: args,
                        });
                    }
                }
            }
        }
    }

    let stop_reason = match v
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("finishReason"))
        .and_then(Value::as_str)
    {
        Some("STOP") => {
            if output.iter().any(|b| matches!(b, Block::ToolUse { .. })) {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            }
        }
        Some("MAX_TOKENS") => StopReason::MaxTokens,
        Some("SAFETY") | Some("PROHIBITED_CONTENT") | Some("RECITATION") => {
            StopReason::SafetyBlock
        }
        _ => StopReason::EndTurn,
    };

    let usage = parse_usage(v.get("usageMetadata"));

    Ok(CanonicalResponse {
        id: format!("gemini-{}", crate::store::now_ms()),
        model: String::new(),
        output,
        stop_reason,
        usage,
    })
}

fn parse_usage(u: Option<&Value>) -> Usage {
    let get = |k: &str| u.and_then(|v| v.get(k)).and_then(Value::as_u64).unwrap_or(0);
    Usage {
        input_tokens: get("promptTokenCount"),
        output_tokens: get("candidatesTokenCount"),
        cache_read_tokens: None,
        cache_write_tokens: None,
    }
}

// ================================================================ 流式解析

/// 解析 Gemini `alt=sse` 流中的一个 `data: {...}` 包 → StreamEvent 列表。
pub fn parse_stream_event(raw: &[u8]) -> Result<Vec<StreamEvent>, String> {
    let v: Value =
        serde_json::from_slice(raw).map_err(|e| format!("Gemini SSE JSON 解析失败: {e}"))?;

    let mut out = Vec::new();

    // usageMetadata 先到（部分实现首包即带）
    let usage = parse_usage(v.get("usageMetadata"));

    if let Some(cands) = v.get("candidates").and_then(Value::as_array) {
        if let Some(c) = cands.first() {
            // 文本增量
            if let Some(parts) = c
                .get("content")
                .and_then(|x| x.get("parts"))
                .and_then(Value::as_array)
            {
                for p in parts {
                    if let Some(t) = p.get("text").and_then(Value::as_str) {
                        if !t.is_empty() {
                            out.push(StreamEvent::TextDelta { text: t.to_string() });
                        }
                    } else if let Some(fc) = p.get("functionCall") {
                        let name = fc
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let args = serde_json::to_string(
                            &fc.get("args").cloned().unwrap_or_else(|| json!({})),
                        )
                        .unwrap_or_default();
                        // 整对象到达 → Start + 完整 Args + End
                        out.push(StreamEvent::ToolCallStart {
                            index: 0,
                            id: format!("{name}_k0"),
                            name,
                        });
                        out.push(StreamEvent::ToolCallArgsDelta {
                            index: 0,
                            args_fragment: args,
                        });
                        out.push(StreamEvent::ToolCallEnd { index: 0 });
                    }
                }
            }
            // 终态
            if let Some(fr) = c.get("finishReason").and_then(Value::as_str) {
                let stop_reason = match fr {
                    "STOP" => StopReason::EndTurn,
                    "MAX_TOKENS" => StopReason::MaxTokens,
                    "SAFETY" | "PROHIBITED_CONTENT" | "RECITATION" => StopReason::SafetyBlock,
                    _ => StopReason::EndTurn,
                };
                let has_tool = out
                    .iter()
                    .any(|e| matches!(e, StreamEvent::ToolCallStart { .. }));
                let sr = if has_tool && stop_reason == StopReason::EndTurn {
                    StopReason::ToolUse
                } else {
                    stop_reason
                };
                out.push(StreamEvent::Finish {
                    stop_reason: sr,
                    usage: usage.clone(),
                });
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::ir::{CanonMessage, SampleParams, ToolSpec};

    fn basic_req() -> CanonicalRequest {
        CanonicalRequest {
            model: "gemini-2.0-flash".into(),
            system: vec!["Be concise.".into()],
            messages: vec![
                CanonMessage::text(Role::User, "hi"),
                CanonMessage::text(Role::Assistant, "hello"),
            ],
            tools: vec![ToolSpec {
                name: "get_weather".into(),
                description: Some("w".into()),
                input_schema: json!({"type":"object","properties":{"city":{"type":"string"}}}),
            }],
            tool_choice: ToolChoice::Auto,
            params: SampleParams {
                max_output_tokens: Some(2048),
                temperature: Some(0.7),
                ..Default::default()
            },
            stream: false,
            extensions: Default::default(),
        }
    }

    #[test]
    fn model_url_build() {
        assert_eq!(
            model_url("models/gemini-2.0-flash"),
            "/v1beta/models/gemini-2.0-flash:generateContent"
        );
    }

    #[test]
    fn encode_system_messages_tools() {
        let v = encode_request(&basic_req()).unwrap();
        assert_eq!(v["systemInstruction"]["parts"][0]["text"], "Be concise.");
        assert_eq!(v["contents"][0]["role"], "user");
        assert_eq!(v["contents"][0]["parts"][0]["text"], "hi");
        assert_eq!(v["contents"][1]["role"], "model");
        assert_eq!(v["tools"][0]["functionDeclarations"][0]["name"], "get_weather");
        assert_eq!(v["generationConfig"]["maxOutputTokens"], 2048);
    }

    #[test]
    fn encode_merges_same_role() {
        let mut req = basic_req();
        req.messages = vec![
            CanonMessage::text(Role::User, "a"),
            CanonMessage::text(Role::User, "b"),
        ];
        let v = encode_request(&req).unwrap();
        let contents = v["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 1, "相邻同角色应合并");
        assert_eq!(contents[0]["parts"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn encode_tool_result_as_function_response() {
        let mut req = basic_req();
        req.messages = vec![CanonMessage {
            role: Role::User,
            blocks: vec![Block::ToolResult {
                call_id: "call_1".into(),
                content: vec![Block::Text { text: "sunny".into() }],
                is_error: false,
            }],
        }];
        let v = encode_request(&req).unwrap();
        let part = &v["contents"][0]["parts"][0];
        assert!(part.get("functionResponse").is_some(), "tool 结果应为 functionResponse");
    }

    #[test]
    fn parse_response_text_and_tool() {
        let body = br#"{
            "candidates": [{
                "content": {"parts": [
                    {"text": "Let me check."},
                    {"functionCall": {"name": "get_weather", "args": {"city": "beijing"}}}
                ]},
                "finishReason": "STOP"
            }],
            "usageMetadata": {"promptTokenCount": 15, "candidatesTokenCount": 8}
        }"#;
        let r = parse_response(body).unwrap();
        assert_eq!(r.output.len(), 2);
        assert_eq!(r.stop_reason, StopReason::ToolUse, "含 functionCall 推断升级");
        match &r.output[1] {
            Block::ToolUse { id, name, input } => {
                assert_eq!(id, "get_weather_k0");
                assert_eq!(name, "get_weather");
                assert_eq!(input["city"], "beijing");
            }
            other => panic!("期望 ToolUse: {other:?}"),
        }
        assert_eq!(r.usage.input_tokens, 15);
        assert_eq!(r.usage.output_tokens, 8);
    }

    #[test]
    fn parse_stream_text_and_finish() {
        let evts = parse_stream_event(
            br#"{"candidates":[{"content":{"parts":[{"text":"ni"}]}}]}"#,
        )
        .unwrap();
        assert!(matches!(&evts[0], StreamEvent::TextDelta { text } if text == "ni"));

        let fin = parse_stream_event(
            br#"{"candidates":[{"finishReason":"STOP"}],
                "usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":1}}"#,
        )
        .unwrap();
        assert!(matches!(&fin[0], StreamEvent::Finish { stop_reason: StopReason::EndTurn, .. }));
    }

    #[test]
    fn parse_stream_function_call_whole_object() {
        let evts = parse_stream_event(
            br#"{"candidates":[{"content":{"parts":[
                {"functionCall":{"name":"f","args":{"a":1}}}
            ]}}]}"#,
        )
        .unwrap();
        assert_eq!(evts.len(), 3);
        assert!(matches!(&evts[0], StreamEvent::ToolCallStart { id, name, .. }
            if id == "f_k0" && name == "f"));
        assert!(matches!(&evts[1], StreamEvent::ToolCallArgsDelta { .. }));
        assert!(matches!(&evts[2], StreamEvent::ToolCallEnd { .. }));
    }
}