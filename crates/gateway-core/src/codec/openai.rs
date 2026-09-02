//! OpenAI 族编解码助手（M1 范围：直通路径的旁路工具）。
//!
//! 直通模式下 body 字节不改动（roadmap M1 验收 1），因此本模块只做三件事：
//! - [`peek`]：轻量解析请求关键字段（model/stream）用于路由与日志
//! - [`UsageScanner`]：流式响应中增量抽取 usage 对象，供日志落库
//! - URL 拼接与错误形状构造

use serde_json::{json, Value};

/// 从请求体提取路由所需最小字段。解析失败返回 Err（调用方按 400 处理）。
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
    Ok(PeekRequest {
        model,
        stream: v.get("stream").and_then(Value::as_bool).unwrap_or(false),
    })
}

// ================================================================ usage 扫描

/// 流式字节中的 usage 抽取器。
///
/// 策略：滚动缓冲区搜索 `"usage"` 关键字 → 判定其后的值（对象/`null`/标量）→
/// 对象则括号配对（感知字符串）截取完整对象 → 首次命中即锁定。
/// 判定可跨 feed 悬挂（`"usage"` 在上一块末尾、值在下一块），上限
/// [`KEY_WAIT_LIMIT`] 字节内仍无判定则放弃该关键字。
/// 非流式响应可整体 feed 后调 [`Self::finish`]。
#[derive(Default)]
pub struct UsageScanner {
    buf: Vec<u8>,
    scan_from: usize,
    collect_start: Option<usize>, // Some(i): 正在收集，i 为 '{' 在 buf 内位置
    /// Some(key_end): 已命中 `"usage"` 关键字、等待值判定（key_end = 关键字末尾偏移）
    pending_key: Option<usize>,
    depth: usize,
    in_string: bool,
    escaped: bool,
    captured: Option<String>,
}

const WINDOW_KEEP: usize = 4096;
const CAPTURE_CAP: usize = 8192;
/// `"usage"` 后值判定窗口：64 字节内既无 `{` 也无 `null` 则放弃（合法 JSON 中
/// `"usage"` 与其值之间只有 `:` 和空白，64 字节足够宽裕）。
const KEY_WAIT_LIMIT: usize = 64;

/// 关键字后值判定的三种去向。
enum Decision {
    /// 值是对象，`usize` = '{' 在 buf 内的位置，开始配对收集
    Collect(usize),
    /// 值是 `null`/标量/数组 → 跳过该关键字继续扫描
    Skip,
    /// 窗口未满但缓冲耗尽 → 保持悬挂等下个 feed
    Wait,
}

impl UsageScanner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);

        loop {
            // 收集中：推进括号配对
            if let Some(start) = self.collect_start {
                while self.scan_from < self.buf.len() {
                    let b = self.buf[self.scan_from];
                    self.scan_from += 1;
                    if self.in_string {
                        if self.escaped {
                            self.escaped = false;
                        } else if b == b'\\' {
                            self.escaped = true;
                        } else if b == b'"' {
                            self.in_string = false;
                        }
                    } else {
                        match b {
                            b'"' => self.in_string = true,
                            b'{' => self.depth += 1,
                            b'}' => {
                                self.depth -= 1;
                                if self.depth == 0 {
                                    let obj = self.buf[start..self.scan_from].to_vec();
                                    self.captured =
                                        Some(String::from_utf8_lossy(&obj).into_owned());
                                    self.collect_start = None;
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                if self.collect_start.is_some() && self.buf.len() > CAPTURE_CAP * 2 {
                    // 异常保护：目标对象过大，放弃本次捕获
                    self.reset_collection();
                }
                self.compact();
                return;
            }

            // 悬挂判定：此前命中了 "usage" 关键字，现在继续判定其后的值
            if let Some(key_end) = self.pending_key {
                match self.decide_after_key(key_end) {
                    Decision::Collect(brace_at) => {
                        self.pending_key = None;
                        self.collect_start = Some(brace_at);
                        self.scan_from = brace_at;
                        self.depth = 0;
                        self.in_string = false;
                        self.escaped = false;
                        continue; // 同一 feed 内立即开始收集
                    }
                    Decision::Skip => {
                        // "usage":null（或标量）——显式跳过该关键字，继续找下一个
                        self.pending_key = None;
                        continue;
                    }
                    Decision::Wait => {
                        // 跨 feed 悬挂：线索保持，下个 feed 从关键字后继续判定
                        self.compact();
                        return;
                    }
                }
            }

            // 寻找下一个 "usage" 关键字
            if let Some(pos) = find_subslice(
                &self.buf[self.scan_from.min(self.buf.len())..],
                b"\"usage\"",
            ) {
                let key_at = self.scan_from + pos;
                let key_end = key_at + b"\"usage\"".len();
                self.scan_from = key_end;
                self.pending_key = Some(key_end);
                continue; // 立即尝试判定
            }
            // 未找到：保留窗口尾部以处理跨块分割的关键字
            self.scan_from = self.buf.len().saturating_sub(24);
            self.compact();
            return;
        }
    }

    /// 判定 `"usage"` 关键字后的值：对象 → Collect；`null`/标量 → Skip；
    /// 缓冲耗尽 → Wait；窗口上限耗尽仍无判定 → Skip（病态输入保护）。
    fn decide_after_key(&self, key_end: usize) -> Decision {
        let start = key_end.min(self.buf.len());
        let limit = start + KEY_WAIT_LIMIT;
        let mut i = start;
        while i < self.buf.len() {
            if i >= limit {
                return Decision::Skip;
            }
            let b = self.buf[i];
            match b {
                // 值前的分隔符：`:` 与空白
                b' ' | b'\t' | b'\n' | b'\r' | b':' => i += 1,
                b'{' => return Decision::Collect(i),
                b'n' if self.buf[i..].starts_with(b"null") => return Decision::Skip,
                // 数字/引号/数组等：OpenAI usage 恒为对象，非对象即误报
                _ => return Decision::Skip,
            }
        }
        Decision::Wait
    }

    /// 全部输入结束后取结果并尝试解析为 JSON。
    pub fn finish(&self) -> Option<Value> {
        let raw = self.captured.as_deref()?;
        serde_json::from_str(raw).ok()
    }

    fn reset_collection(&mut self) {
        self.collect_start = None;
        self.depth = 0;
        self.in_string = false;
        self.escaped = false;
        self.captured = None;
    }

    /// 裁剪缓冲：保留足够的回看窗口，重定位游标。
    fn compact(&mut self) {
        if self.buf.len() <= WINDOW_KEEP {
            return;
        }
        let cut = self.buf.len() - WINDOW_KEEP;
        self.buf.drain(0..cut);
        self.scan_from = self.scan_from.saturating_sub(cut);
        if let Some(s) = self.collect_start.as_mut() {
            *s = s.saturating_sub(cut);
        }
        if let Some(p) = self.pending_key.as_mut() {
            *p = p.saturating_sub(cut);
        }
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// 从 usage JSON 提取 IR Usage 四元组（protocol-ir §5-D）。
/// 返回 (input, output, cache_read, cache_write)。
pub fn extract_usage(u: &Value) -> (Option<i64>, Option<i64>, Option<i64>, Option<i64>) {
    let num = |v: &Value| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64));
    let input = u
        .get("prompt_tokens")
        .or_else(|| u.get("input_tokens"))
        .and_then(num);
    let output = u
        .get("completion_tokens")
        .or_else(|| u.get("output_tokens"))
        .and_then(num);
    let cache_read = u
        .pointer("/prompt_tokens_details/cached_tokens")
        .or_else(|| u.get("cache_read_input_tokens"))
        .and_then(num);
    let cache_write = u.get("cache_creation_input_tokens").and_then(num);
    (input, output, cache_read, cache_write)
}

// ================================================================ URL 与错误

/// base 尾部斜杠归一后拼接路径。
pub fn url_join(base: &str, suffix: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        suffix.trim_start_matches('/')
    )
}

/// OpenAI 错误响应体。
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

// ================================================================ M4：InboundCodec::OpenAI

// 转换路径入口：OpenAI body（请求）→ CanonicalRequest。
// 依据 protocol-ir §4 映射表；tool calling 三段（定义/发起/结果）在此归一。

use crate::codec::ir::{
    Block, CanonMessage, CanonicalRequest, CanonicalResponse, Role, SampleParams, StopReason,
    StreamEvent, ToolChoice, ToolSpec, Usage,
};
use serde_json::Map;

/// 解码 OpenAI chat completions 请求体。
/// 失败返回 Err(客户端可读消息) —— 调用方按 400 处理。
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

    // 拒绝跨族不支持的能力面（protocol-ir §3）
    if let Some(n) = v.get("n").and_then(Value::as_u64) {
        if n > 1 {
            return Err("n>1（多候选）跨协议转换不支持；请使用 n=1".into());
        }
    }
    if v.get("logprobs")
        .or_else(|| v.get("top_logprobs"))
        .is_some()
    {
        return Err("logprobs/top_logprobs 跨协议转换不支持".into());
    }
    if let Some(stream) = v.get("stream").and_then(Value::as_bool) {
        if !stream && v.get("stream_options").is_some() {
            let _ = stream; // 忽略 stream_options（非流式场景无意义）
        }
    }

    let mut system: Vec<String> = Vec::new();
    let mut messages: Vec<CanonMessage> = Vec::new();
    let mut extensions = Map::new();

    if let Some(arr) = v.get("messages").and_then(Value::as_array) {
        for m in arr {
            let role = m.get("role").and_then(Value::as_str).unwrap_or_default();
            let content = m.get("content");
            match role {
                "system" => match content.and_then(Value::as_str) {
                    Some(s) => system.push(s.to_string()),
                    // 多段 system（content 为数组）仅取文本部分
                    None => {
                        if let Some(parts) = content.and_then(Value::as_array) {
                            for p in parts {
                                if let Some(t) = p.get("text").and_then(Value::as_str) {
                                    system.push(t.to_string());
                                }
                            }
                        }
                    }
                },
                "user" => {
                    messages.push(CanonMessage {
                        role: Role::User,
                        blocks: content_blocks(content)?,
                    });
                }
                "assistant" => {
                    let mut blocks = content_blocks(content)?;
                    // tool_calls → ToolUse 块
                    if let Some(tcs) = m.get("tool_calls").and_then(Value::as_array) {
                        for tc in tcs {
                            if let (Some(id), Some(fn_obj)) =
                                (tc.get("id").and_then(Value::as_str), tc.get("function"))
                            {
                                let name = fn_obj
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default();
                                let input: Value = fn_obj
                                    .get("arguments")
                                    .and_then(Value::as_str)
                                    .and_then(|s| serde_json::from_str(s).ok())
                                    .unwrap_or_else(|| json!({}));
                                blocks.push(Block::ToolUse {
                                    id: id.to_string(),
                                    name: name.to_string(),
                                    input,
                                });
                            }
                        }
                    }
                    // 仅当有实际内容或工具调用才入列
                    if !blocks.is_empty() {
                        messages.push(CanonMessage {
                            role: Role::Assistant,
                            blocks,
                        });
                    }
                }
                "tool" => {
                    // role=tool 消息：独立 ToolResult 块（宿主角色由渲染器决定，
                    // IR 层用 User 宿主 + ToolResult 块，见 protocol-ir §2 注释）
                    let call_id = m
                        .get("tool_call_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let is_error = m.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                    let text = content
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_default();
                    let result_blocks = if text.is_empty() {
                        vec![]
                    } else {
                        vec![Block::Text { text }]
                    };
                    messages.push(CanonMessage {
                        role: Role::User,
                        blocks: vec![Block::ToolResult {
                            call_id,
                            content: result_blocks,
                            is_error,
                        }],
                    });
                }
                other => {
                    extensions
                        .entry(format!("message_role:{other}"))
                        .or_insert(Value::Null);
                }
            }
        }
    }

    // tools 定义
    let mut tools: Vec<ToolSpec> = Vec::new();
    if let Some(arr) = v.get("tools").and_then(Value::as_array) {
        for t in arr {
            if let Some(fn_obj) = t.get("function") {
                tools.push(ToolSpec {
                    name: fn_obj
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    description: fn_obj
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    input_schema: fn_obj
                        .get("parameters")
                        .cloned()
                        .unwrap_or_else(|| json!({"type":"object","properties":{}})),
                });
            }
        }
    }

    // tool_choice
    let tool_choice = match v.get("tool_choice") {
        Some(Value::String(s)) if s == "none" => ToolChoice::None,
        Some(Value::String(s)) if s == "required" => ToolChoice::Required,
        Some(Value::String(_)) => ToolChoice::Auto,
        Some(Value::Object(o)) => {
            if let Some(fn_name) = o
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
            {
                ToolChoice::Specific(fn_name.to_string())
            } else {
                ToolChoice::Auto
            }
        }
        _ => ToolChoice::Auto,
    };

    // 采样参数
    let params = SampleParams {
        max_output_tokens: v
            .get("max_tokens")
            .or_else(|| v.get("max_completion_tokens"))
            .and_then(Value::as_u64)
            .map(|n| n as u32),
        temperature: v
            .get("temperature")
            .and_then(Value::as_f64)
            .map(|f| f as f32),
        top_p: v.get("top_p").and_then(Value::as_f64).map(|f| f as f32),
        top_k: None, // OpenAI 入站无 top_k
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

    // 未建模字段（§7 Lenient 收集）
    for k in [
        "response_format",
        "stream_options",
        "user",
        "logit_bias",
        "repetition_penalty",
    ] {
        if v.get(k).is_some() {
            let _ = extensions
                .entry(k.to_string())
                .or_insert_with(|| v.get(k).cloned().unwrap_or(Value::Null));
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

/// 解析 content 字段为块列表（文本字符串 | 内容块数组）。
fn content_blocks(content: Option<&Value>) -> Result<Vec<Block>, String> {
    match content {
        None => Ok(vec![]),
        Some(Value::String(s)) => Ok(vec![Block::Text { text: s.clone() }]),
        Some(Value::Array(parts)) => {
            let mut blocks = Vec::new();
            for p in parts {
                match p.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = p.get("text").and_then(Value::as_str) {
                            blocks.push(Block::Text {
                                text: t.to_string(),
                            });
                        }
                    }
                    Some("image_url") => {
                        let img = p
                            .get("image_url")
                            .and_then(|i| i.get("url"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        // data:image/png;base64,xxxx 内联，否则当 URL
                        if let Some((meta, b64)) = parse_data_url(img) {
                            blocks.push(Block::Image {
                                media_type: meta,
                                data_base64: Some(b64),
                                url: None,
                            });
                        } else {
                            blocks.push(Block::Image {
                                media_type: "image/png".into(),
                                data_base64: None,
                                url: Some(img.to_string()),
                            });
                        }
                    }
                    _ => {}
                }
            }
            Ok(blocks)
        }
        _ => Ok(vec![]),
    }
}

/// 解析 `data:image/{type};base64,{payload}`。
fn parse_data_url(s: &str) -> Option<(String, String)> {
    let rest = s.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let media_type = meta.strip_suffix(";base64")?.to_string();
    Some((media_type, payload.to_string()))
}

/// 渲染非流式响应：CanonicalResponse → OpenAI chat.completion JSON。
pub fn render_response(r: &crate::codec::ir::CanonicalResponse) -> Value {
    let mut message = json!({"role": "assistant", "content": None::<String>});
    let mut tool_calls: Vec<Value> = Vec::new();

    for b in &r.output {
        match b {
            Block::Text { text } => {
                let cur = message["content"].as_str().unwrap_or_default().to_string();
                message["content"] = Value::String(format!("{cur}{text}"));
            }
            Block::ToolUse { id, name, input } => {
                tool_calls.push(json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": serde_json::to_string(input).unwrap_or_else(|_| "{}".into()),
                    }
                }));
            }
            Block::Thinking { .. } => { /* 不渲染（v1 不产出） */ }
            _ => {}
        }
    }
    if !tool_calls.is_empty() {
        message["content"] = message["content"].take();
        let msg = message.as_object_mut().expect("object");
        msg.insert("tool_calls".into(), Value::Array(tool_calls));
    }

    let finish_reason = match &r.stop_reason {
        StopReason::EndTurn => "stop",
        StopReason::MaxTokens => "length",
        StopReason::ToolUse => "tool_calls",
        StopReason::SafetyBlock => "content_filter",
        StopReason::Other(_) => "stop",
    };

    json!({
        "id": r.id,
        "object": "chat.completion",
        "created": crate::store::now_ms() / 1000,
        "model": r.model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason,
            "logprobs": null,
        }],
        "usage": usage_json(&r.usage),
    })
}

fn usage_json(u: &Usage) -> Value {
    let mut o = json!({
        "prompt_tokens": u.input_tokens,
        "completion_tokens": u.output_tokens,
        "total_tokens": u.input_tokens + u.output_tokens,
    });
    if krate_has_cache() {
        if let Some(cr) = u.cache_read_tokens {
            o["prompt_tokens_details"] = json!({"cached_tokens": cr});
        }
    }
    o
}

/// (无实际依赖，恒 false；预留 cache 细节)
fn krate_has_cache() -> bool {
    false
}

// ================================================================ M5：UpstreamCodec::OpenAI

// Anthropic 入站 × OpenAI 上游 时使用（Claude Code × GPT 模型）。

/// 编码请求：IR → OpenAI chat completions body。
pub fn encode_request(req: &crate::codec::ir::CanonicalRequest) -> Result<Value, String> {
    let mut body = json!({
        "model": req.model,
        "messages": [],
    });

    // message 数组（system 作为第一条 system 消息还原，§4-C）
    let mut messages: Vec<Value> = Vec::new();
    if !req.system.is_empty() {
        messages.push(json!({"role":"system","content": req.system.join("\n\n")}));
    }
    for m in &req.messages {
        match m.role {
            Role::User => {
                // 普通文本（串接）+ tool 结果 → 独立 role=tool 消息
                let mut text = String::new();
                let mut tool_msgs: Vec<Value> = Vec::new();
                for b in &m.blocks {
                    match b {
                        Block::Text { text: t } => text.push_str(t),
                        Block::Image {
                            data_base64, url, ..
                        } => {
                            // OpenAI 可接受 url 或 base64 data url；目前传 url 优先
                            if let Some(u) = url {
                                messages.push(json!({
                                    "role":"user",
                                    "content":[{"type":"image_url","image_url":{"url":u}}]
                                }));
                            } else if let Some(b64) = data_base64 {
                                messages.push(json!({
                                    "role":"user",
                                    "content":[{"type":"image_url",
                                        "image_url":{"url": format!("data:image/png;base64,{b64}")}}]
                                }));
                            }
                        }
                        Block::ToolResult {
                            call_id,
                            content,
                            is_error,
                        } => {
                            let content_text = content
                                .iter()
                                .filter_map(|c| c.as_text())
                                .collect::<Vec<_>>()
                                .join("\n");
                            let content_val = if *is_error {
                                json!({"error": content_text})
                            } else {
                                Value::String(content_text)
                            };
                            tool_msgs.push(json!({
                                "role":"tool",
                                "tool_call_id": call_id,
                                "content": content_val,
                            }));
                        }
                        _ => {}
                    }
                }
                if !text.is_empty() {
                    messages.push(json!({"role":"user","content":text}));
                }
                messages.extend(tool_msgs);
            }
            Role::Assistant => {
                let mut content = String::new();
                let mut tool_calls: Vec<Value> = Vec::new();
                let mut reasoning = String::new();
                for b in &m.blocks {
                    match b {
                        Block::Text { text } => content.push_str(text),
                        Block::Thinking { text, .. } => reasoning.push_str(text),
                        Block::ToolUse { id, name, input } => {
                            tool_calls.push(json!({
                                "id": id,
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": serde_json::to_string(input)
                                        .unwrap_or_else(|_| "{}".into()),
                                },
                            }));
                        }
                        _ => {}
                    }
                }
                let mut msg = json!({
                    "role":"assistant",
                    "content": if content.is_empty() { Value::Null } else { Value::String(content) },
                });
                if !reasoning.is_empty() {
                    msg["reasoning_content"] = Value::String(reasoning);
                }
                if !tool_calls.is_empty() {
                    msg["tool_calls"] = Value::Array(tool_calls);
                }
                messages.push(msg);
            }
        }
    }
    body["messages"] = Value::Array(messages);

    // tools
    if !req.tools.is_empty() {
        body["tools"] = Value::Array(
            req.tools
                .iter()
                .map(|t| {
                    json!({
                        "type":"function",
                        "function":{
                            "name": t.name,
                            "description": t.description.as_deref().unwrap_or(""),
                            "parameters": t.input_schema,
                        },
                    })
                })
                .collect(),
        );
        let choice = match &req.tool_choice {
            ToolChoice::Auto => None,
            ToolChoice::None => Some(json!("none")),
            ToolChoice::Required => Some(json!("required")),
            ToolChoice::Specific(name) => Some(json!({
                "type":"function","function":{"name": name}
            })),
        };
        if let Some(c) = choice {
            body["tool_choice"] = c;
        }
    }

    // 采样参数
    let p = &req.params;
    if let Some(m) = p.max_output_tokens {
        body["max_completion_tokens"] = json!(m);
    }
    if let Some(t) = p.temperature {
        body["temperature"] = json!(super::anthropic::round_f32(t));
    }
    if let Some(v) = p.top_p {
        body["top_p"] = json!(super::anthropic::round_f32(v));
    }
    if !p.stop_sequences.is_empty() {
        body["stop"] = json!(p.stop_sequences);
    }
    if let Some(f) = p.frequency_penalty {
        body["frequency_penalty"] = json!(super::anthropic::round_f32(f));
    }
    if let Some(pr) = p.presence_penalty {
        body["presence_penalty"] = json!(super::anthropic::round_f32(pr));
    }
    if let Some(seed) = p.seed {
        body["seed"] = json!(seed);
    }
    if req.stream {
        body["stream"] = json!(true);
        // 注入 usage 采集（§4-A：出站注入 include_usage，回传前剥除）
        body["stream_options"] = json!({"include_usage": true});
    }

    Ok(body)
}

/// 解析 OpenAI 非流式响应 → CanonicalResponse。
pub fn parse_response(body: &[u8]) -> Result<crate::codec::ir::CanonicalResponse, String> {
    let v: Value =
        serde_json::from_slice(body).map_err(|e| format!("OpenAI 响应 JSON 解析失败: {e}"))?;
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
    if let Some(choices) = v.get("choices").and_then(Value::as_array) {
        if let Some(c) = choices.first() {
            if let Some(msg) = c.get("message") {
                // thinking 模型（如 deepseek 系列）：reasoning_content 必须原样回传，
                // 否则上游在后续轮次校验失败（400）。
                if let Some(rc) = msg.get("reasoning_content").and_then(Value::as_str) {
                    if !rc.is_empty() {
                        output.push(Block::Thinking {
                            signature: None,
                            text: rc.to_string(),
                        });
                    }
                }
                if let Some(content) = msg.get("content").and_then(Value::as_str) {
                    if !content.is_empty() {
                        output.push(Block::Text {
                            text: content.to_string(),
                        });
                    }
                }
                if let Some(tcs) = msg.get("tool_calls").and_then(Value::as_array) {
                    for tc in tcs {
                        if let Some(fn_obj) = tc.get("function") {
                            output.push(Block::ToolUse {
                                id: tc
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                name: fn_obj
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                input: fn_obj
                                    .get("arguments")
                                    .and_then(Value::as_str)
                                    .and_then(|s| serde_json::from_str(s).ok())
                                    .unwrap_or_else(|| json!({})),
                            });
                        }
                    }
                }
            }
        }
    }
    let stop_reason = match v
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("finish_reason"))
        .and_then(Value::as_str)
    {
        Some("stop") => StopReason::EndTurn,
        Some("length") => StopReason::MaxTokens,
        Some("tool_calls") => StopReason::ToolUse,
        Some("content_filter") => StopReason::SafetyBlock,
        _ => StopReason::EndTurn,
    };
    let usage = parse_usage(v.get("usage"));

    Ok(CanonicalResponse {
        id,
        model,
        output,
        stop_reason,
        usage,
    })
}

fn parse_usage(u: Option<&Value>) -> Usage {
    let get = |k: &str| {
        u.and_then(|v| v.get(k))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let cache_read = u
        .and_then(|v| v.pointer("/prompt_tokens_details/cached_tokens"))
        .and_then(Value::as_u64);
    Usage {
        input_tokens: get("prompt_tokens"),
        output_tokens: get("completion_tokens"),
        cache_read_tokens: cache_read,
        cache_write_tokens: None,
    }
}

/// 解析 OpenAI SSE 一个 `data: {...}` 包 → StreamEvent 列表。
pub fn parse_stream_event(raw: &[u8]) -> Result<Vec<crate::codec::ir::StreamEvent>, String> {
    let v: Value =
        serde_json::from_slice(raw).map_err(|e| format!("OpenAI SSE JSON 解析失败: {e}"))?;

    let mut out = Vec::new();
    // usage chunk（choices 为空但有 usage）
    if v.get("choices").map(Value::is_array) == Some(false) || v.get("choices").is_none() {
        if let Some(u) = v.get("usage") {
            let usage = parse_usage(Some(u));
            out.push(StreamEvent::Finish {
                stop_reason: StopReason::EndTurn,
                usage,
            });
            return Ok(out);
        }
    }

    if let Some(choices) = v.get("choices").and_then(Value::as_array) {
        if let Some(c) = choices.first() {
            if let Some(delta) = c.get("delta") {
                if let Some(content) = delta.get("content").and_then(Value::as_str) {
                    if !content.is_empty() {
                        out.push(StreamEvent::TextDelta {
                            text: content.to_string(),
                        });
                    }
                }
                if let Some(tcs) = delta.get("tool_calls").and_then(Value::as_array) {
                    for tc in tcs {
                        let idx = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                        if let Some(id) = tc.get("id").and_then(Value::as_str) {
                            let name = tc
                                .pointer("/function/name")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            out.push(StreamEvent::ToolCallStart {
                                index: idx,
                                id: id.to_string(),
                                name: name.to_string(),
                            });
                        }
                        if let Some(args) =
                            tc.pointer("/function/arguments").and_then(Value::as_str)
                        {
                            if !args.is_empty() {
                                out.push(StreamEvent::ToolCallArgsDelta {
                                    index: idx,
                                    args_fragment: args.to_string(),
                                });
                            }
                        }
                    }
                }
            }
            if let Some(fr) = c.get("finish_reason").and_then(Value::as_str) {
                let stop_reason = match fr {
                    "stop" => StopReason::EndTurn,
                    "length" => StopReason::MaxTokens,
                    "tool_calls" => StopReason::ToolUse,
                    "content_filter" => StopReason::SafetyBlock,
                    _ => StopReason::EndTurn,
                };
                let usage = parse_usage(v.get("usage"));
                out.push(StreamEvent::Finish { stop_reason, usage });
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- 流式渲染

/// OpenAI SSE 渲染状态。
#[derive(Debug, Clone, Default)]
pub struct RenderState {
    pub id: String,
    pub model: String,
    pub started: bool,
}

/// 渲染单条 IR 流事件为 OpenAI SSE 行（不含 `data: [DONE]`）。
/// Finish 返回最后一条 chunk；调用方在流结束时自行补 `data: [DONE]`。
pub fn render_stream_event(
    e: &crate::codec::ir::StreamEvent,
    st: &mut RenderState,
) -> Option<String> {
    use crate::codec::ir::StreamEvent as Ev;
    let id = st.id.clone();
    let model = st.model.clone();
    let chunk_base = |delta: Value| {
        json!({
            "id": id, "object": "chat.completion.chunk",
            "created": crate::store::now_ms() / 1000, "model": model,
            "choices": [{"index": 0, "delta": delta, "finish_reason": null, "logprobs": null}],
        })
    };

    match e {
        Ev::Start { .. } => {
            st.started = true;
            Some(chunk_base(json!({"role": "assistant", "content": ""})).to_string())
        }
        Ev::TextDelta { text } => {
            if text.is_empty() {
                return None;
            }
            Some(chunk_base(json!({"content": text})).to_string())
        }
        Ev::ThinkingDelta { .. } => None, // OpenAI 无 thinking 增量（v1 不产出）
        Ev::ToolCallStart { index, id, name } => Some(
            chunk_base(json!({
                "tool_calls": [{
                    "index": index,
                    "id": id,
                    "type": "function",
                    "function": {"name": name, "arguments": ""},
                }],
            }))
            .to_string(),
        ),
        Ev::ToolCallArgsDelta {
            index,
            args_fragment,
        } => Some(
            chunk_base(json!({
                "tool_calls": [{
                    "index": index,
                    "function": {"arguments": args_fragment},
                }],
            }))
            .to_string(),
        ),
        Ev::ToolCallEnd { .. } => None, // OpenAI 无显式结束事件
        Ev::Finish { stop_reason, usage } => {
            let fr = match stop_reason {
                StopReason::EndTurn => "stop",
                StopReason::MaxTokens => "length",
                StopReason::ToolUse => "tool_calls",
                StopReason::SafetyBlock => "content_filter",
                StopReason::Other(_) => "stop",
            };
            Some(
                json!({
                    "id": id, "object": "chat.completion.chunk",
                    "created": crate::store::now_ms() / 1000, "model": model,
                    "choices": [{
                        "index": 0, "delta": {}, "finish_reason": fr, "logprobs": null,
                    }],
                    "usage": usage_json(usage),
                })
                .to_string(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peek_parses_minimal_fields() {
        let p = peek(br#"{"model":"deepseek-chat","stream":true,"messages":[]}"#).unwrap();
        assert_eq!(p.model, "deepseek-chat");
        assert!(p.stream);

        assert!(peek(b"not json").is_err());
        assert!(peek(br#"{"messages":[]}"#).is_err());
    }

    #[test]
    fn scanner_handles_split_across_chunks_and_strings() {
        let mut s = UsageScanner::new();
        // 跨块分割的 usage 对象 + 内容里带大括号与转义引号的字符串
        s.feed(br#"data: {"choices":[{"delta":{"content":"a {b} \" quo"}}"#);
        s.feed(b"\n\ndata: {\"usage\": {\"prompt_tokens\": 12,\"comp");
        s.feed("letion_tokens\": 34}}\n\ndata: [DONE]\n\n".as_bytes());
        let v = s.finish().expect("应捕获 usage");
        assert_eq!(v["prompt_tokens"], 12);
        assert_eq!(v["completion_tokens"], 34);
    }

    #[test]
    fn scanner_full_body_shortcut() {
        let mut s = UsageScanner::new();
        let body = br#"{"id":"x","usage":{"input_tokens":7,"output_tokens":9}}"#;
        s.feed(body);
        let v = s.finish().unwrap();
        assert_eq!(v["output_tokens"], 9);
    }

    /// 回归（2026-09-02 实测）：glm-5.3-flash 中转流含 17 个 `"usage":null` +
    /// 末尾完整 usage 帧（prompt 13 / completion 16），旧状态机会在 null 处
    /// 误触发/丢线索导致恒空。整段 feed 必须取出末尾 usage。
    #[test]
    fn scanner_glm_stream_with_null_usages() {
        let null_frame = |content: &str| {
            format!(
                "data: {{\"id\":\"c1\",\"choices\":[{{\"delta\":{{\"content\":\"{content}\"}},\"index\":0}}],\"usage\":null}}\n\n"
            )
        };
        let mut s = UsageScanner::new();
        s.feed(b"data: {\"id\":\"c1\",\"choices\":[{\"delta\":{\"role\":\"assistant\"}}],\"usage\":null}\n\n");
        for i in 0..17 {
            s.feed(null_frame(&format!("字{i}")).as_bytes());
        }
        s.feed(b"data: {\"id\":\"c1\",\"choices\":[],\"usage\":{\"prompt_tokens\":13,\"completion_tokens\":16,\"total_tokens\":29}}\n\n");
        s.feed(b"data: [DONE]\n\n");
        let v = s.finish().expect("null 群后末尾 usage 应被捕获");
        assert_eq!(v["prompt_tokens"], 13);
        assert_eq!(v["completion_tokens"], 16);
    }

    /// 同一流按 1 字节粒度逐块喂入（最恶劣切分），跨 feed 悬挂判定必须保持线索。
    #[test]
    fn scanner_glm_stream_byte_by_byte() {
        let null_frame = "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}],\"usage\":null}\n\n";
        let mut body = String::new();
        for _ in 0..17 {
            body.push_str(null_frame);
        }
        body.push_str(
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":13,\"completion_tokens\":16}}\n\n",
        );
        body.push_str("data: [DONE]\n\n");

        let mut s = UsageScanner::new();
        for b in body.as_bytes() {
            s.feed(std::slice::from_ref(b));
        }
        let v = s.finish().expect("逐字节喂入也应捕获 usage");
        assert_eq!(v["prompt_tokens"], 13);
        assert_eq!(v["completion_tokens"], 16);
    }

    /// 关键字在块末尾截断、值在下块开头（`"usage"` 跨 feed 分割 + 值跨 feed 分割）。
    #[test]
    fn scanner_usage_key_and_value_split_across_feeds() {
        let mut s = UsageScanner::new();
        s.feed(br#"data: {"id":"x","usage"#);
        s.feed(br#"":null,"usage""#);
        s.feed(br#": {"prompt_to"#);
        s.feed(br#"kens":5,"completion_tokens":6}}"#);
        s.feed(b"\n\ndata: [DONE]\n\n");
        let v = s.finish().expect("跨 feed 悬挂应保持线索直至取值");
        assert_eq!(v["prompt_tokens"], 5);
        assert_eq!(v["completion_tokens"], 6);
    }

    /// usage 值紧跟关键字在同块但窗口极限（59 字节空白）也能判定；
    /// 超过 KEY_WAIT_LIMIT 仍无值则放弃该关键字（病态输入保护）。
    #[test]
    fn scanner_key_wait_limit_protection() {
        // 窗口内可判定：59 个空白 + '{'（64 上限内）
        let mut s = UsageScanner::new();
        s.feed(
            format!(
                "data: {{\"usage\":{}{{\"prompt_tokens\":1,\"completion_tokens\":2}}}}",
                " ".repeat(59)
            )
            .as_bytes(),
        );
        let v = s.finish().expect("上限内空白应仍可判定");
        assert_eq!(v["prompt_tokens"], 1);

        // 超限：70 个空白后才是 '{' —— 该关键字被放弃，不误收集
        let mut s2 = UsageScanner::new();
        s2.feed(format!("data: {{\"usage\":{}{{\"evil\":1}}", " ".repeat(70)).as_bytes());
        assert!(s2.finish().is_none(), "超窗口的病态输入应被放弃");
    }

    #[test]
    fn extract_maps_openai_and_anthropic_shapes() {
        let oai = serde_json::json!({"prompt_tokens":10,"completion_tokens":5,
            "prompt_tokens_details":{"cached_tokens":4}});
        assert_eq!(extract_usage(&oai), (Some(10), Some(5), Some(4), None));

        let ant = serde_json::json!({"input_tokens":8,"output_tokens":3,
            "cache_read_input_tokens":2,"cache_creation_input_tokens":6});
        assert_eq!(extract_usage(&ant), (Some(8), Some(3), Some(2), Some(6)));
    }

    #[test]
    fn url_join_normalizes_slashes() {
        assert_eq!(
            url_join("https://api.x.com/v1/", "chat/completions"),
            "https://api.x.com/v1/chat/completions"
        );
        assert_eq!(
            url_join("https://g.cn", "/v1beta/models"),
            "https://g.cn/v1beta/models"
        );
    }

    // ---------------- M4: decode / render ----------------

    use crate::codec::ir::{
        Block as IrBlock, CanonicalResponse as IrResp, StreamEvent, Usage as IrUsage,
    };

    #[test]
    fn decode_basic_text_and_system() {
        let req = super::decode_request(
            br#"{"model":"gpt-4o","messages":[
                {"role":"system","content":"be nice"},
                {"role":"user","content":"hi"},
                {"role":"assistant","content":"hello"}
            ],"temperature":0.5,"stream":true}"#,
        )
        .unwrap();
        assert_eq!(req.model, "gpt-4o");
        assert_eq!(req.system, vec!["be nice"]);
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role, super::Role::User);
        assert_eq!(req.messages[0].blocks[0].as_text(), Some("hi"));
        assert!(req.stream);
        assert_eq!(req.params.temperature, Some(0.5));
    }

    #[test]
    fn decode_tool_calls_and_results() {
        let req = super::decode_request(
            br#"{"model":"gpt-4o","messages":[
                {"role":"user","content":"weather?"},
                {"role":"assistant","content":null,
                 "tool_calls":[{"id":"call_1","type":"function",
                    "function":{"name":"get_weather","arguments":"{\"city\":\"beijing\"}"}}]},
                {"role":"tool","tool_call_id":"call_1","content":"sunny"}
            ],"tools":[{"type":"function","function":{"name":"get_weather",
                "description":"w","parameters":{"type":"object"}}}]}"#,
        )
        .unwrap();
        assert_eq!(req.tools.len(), 1);
        assert_eq!(req.tools[0].name, "get_weather");
        assert_eq!(req.messages.len(), 3);
        let m1 = &req.messages[1];
        assert_eq!(m1.role, super::Role::Assistant);
        match &m1.blocks[0] {
            IrBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "call_1");
                assert_eq!(name, "get_weather");
                assert_eq!(input["city"], "beijing");
            }
            other => panic!("期望 ToolUse，得到 {other:?}"),
        }
        let m2 = &req.messages[2];
        match &m2.blocks[0] {
            IrBlock::ToolResult {
                call_id,
                content,
                is_error,
            } => {
                assert_eq!(call_id, "call_1");
                assert_eq!(content[0].as_text(), Some("sunny"));
                assert!(!is_error);
            }
            other => panic!("期望 ToolResult，得到 {other:?}"),
        }
    }

    #[test]
    fn decode_rejects_cross_family_unsupported() {
        assert!(super::decode_request(
            br#"{"model":"m","n":2,"messages":[{"role":"user","content":"x"}]}"#
        )
        .is_err());
    }

    #[test]
    fn render_response_roundtrips_text_and_tools() {
        let resp = IrResp {
            id: "chatcmpl-1".into(),
            model: "gpt-4o".into(),
            output: vec![
                IrBlock::Text {
                    text: "请稍候".into(),
                },
                IrBlock::ToolUse {
                    id: "call_9".into(),
                    name: "get_weather".into(),
                    input: json!({"city":"shanghai"}),
                },
            ],
            stop_reason: super::StopReason::ToolUse,
            usage: IrUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
        };
        let v = super::render_response(&resp);
        assert_eq!(v["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(v["choices"][0]["message"]["content"], "请稍候");
        assert_eq!(v["choices"][0]["message"]["tool_calls"][0]["id"], "call_9");
        assert_eq!(v["usage"]["prompt_tokens"], 10);
    }

    #[test]
    fn render_stream_emits_start_delta_finish() {
        let mut st = RenderState {
            id: "chunk-id".into(),
            model: "gpt-4o".into(),
            started: false,
        };
        let s1 = super::render_stream_event(
            &StreamEvent::Start {
                model: "gpt-4o".into(),
            },
            &mut st,
        )
        .unwrap();
        assert!(s1.contains("\"role\":\"assistant\""));

        let s2 = super::render_stream_event(&StreamEvent::TextDelta { text: "你".into() }, &mut st)
            .unwrap();
        assert!(s2.contains("\"content\":\"你\""));

        let s3 = super::render_stream_event(
            &StreamEvent::Finish {
                stop_reason: super::StopReason::EndTurn,
                usage: IrUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
            },
            &mut st,
        )
        .unwrap();
        assert!(s3.contains("\"finish_reason\":\"stop\""));
        assert!(s3.contains("\"usage\""));
    }

    #[test]
    fn render_stream_tool_call_parts() {
        let mut st = RenderState {
            id: "c".into(),
            model: "m".into(),
            started: false,
        };
        let a = super::render_stream_event(
            &StreamEvent::ToolCallStart {
                index: 0,
                id: "t1".into(),
                name: "f".into(),
            },
            &mut st,
        )
        .unwrap();
        assert!(a.contains("\"id\":\"t1\""));
        let b = super::render_stream_event(
            &StreamEvent::ToolCallArgsDelta {
                index: 0,
                args_fragment: "{\"a\":1}".into(),
            },
            &mut st,
        )
        .unwrap();
        assert!(b.contains("\"arguments\":\"{\\\"a\\\":1}\""));
    }

    #[test]
    fn reasoning_content_roundtrip() {
        // thinking 模型（deepseek 等）：上游返回 reasoning_content，
        // 解析进 IR Thinking 块，编码回传时必须原样保留（否则上游 400）。
        let body = br#"{
            "id":"resp_1",
            "model":"deepseek-v4-flash",
            "choices":[{
                "index":0,
                "message":{
                    "role":"assistant",
                    "reasoning_content":"thinking hard",
                    "content":"answer",
                    "tool_calls":[{
                        "id":"call_1",
                        "type":"function",
                        "function":{"name":"f","arguments":"{}"}
                    }]
                },
                "finish_reason":"tool_calls"
            }],
            "usage":{"prompt_tokens":10,"completion_tokens":5}
        }"#;
        let resp = super::parse_response(body).unwrap();
        assert!(matches!(
            resp.output[0],
            Block::Thinking { ref text, .. } if text == "thinking hard"
        ));
        assert!(matches!(resp.output[1], Block::Text { ref text, .. } if text == "answer"));
        assert!(matches!(resp.output[2], Block::ToolUse { .. }));

        // 编码回传：assistant 消息带 reasoning_content + tool_calls
        let req = crate::codec::ir::CanonicalRequest {
            model: "deepseek-v4-flash".into(),
            system: vec![],
            messages: vec![crate::codec::ir::CanonMessage {
                role: crate::codec::ir::Role::Assistant,
                blocks: resp.output,
            }],
            tools: vec![],
            tool_choice: crate::codec::ir::ToolChoice::Auto,
            params: crate::codec::ir::SampleParams::default(),
            stream: false,
            extensions: Default::default(),
        };
        let body = super::encode_request(&req).unwrap();
        let m = body["messages"][0].as_object().unwrap();
        assert_eq!(m["role"], "assistant");
        assert_eq!(m["reasoning_content"], "thinking hard");
        assert_eq!(m["content"], "answer");
        assert!(m.get("tool_calls").is_some());
    }
}
