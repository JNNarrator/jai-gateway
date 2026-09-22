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
    /// 客户端声明的输出预算（`max_tokens` / `max_completion_tokens` / Responses 的
    /// `max_output_tokens`）。
    ///
    /// 网关**不**改写它（既不兜底、也不按模型配置归一）：输出预算是客户端自己规划上下文的
    /// 一部分，替它决定等于从远端插手 agent 循环。这里只是读出来给诊断用 —— 直通流式的
    /// 「零可见输出的截断轮」诊断要说清「这一轮允许多少输出」，否则运维看不出预算已经被
    /// 压到几十个 token（真机 2026-09-22：基元律动/deepseek-flash，completion_tokens=16）。
    pub max_output_tokens: Option<u32>,
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
        // 三种入站线的字段名都收：OpenAI/Anthropic 用 `max_tokens`、OpenAI 新式与
        // o 系用 `max_completion_tokens`、Responses 用 `max_output_tokens`。
        max_output_tokens: v
            .get("max_tokens")
            .or_else(|| v.get("max_completion_tokens"))
            .or_else(|| v.get("max_output_tokens"))
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok()),
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
        .or_else(|| u.pointer("/input_tokens_details/cached_tokens"))
        .or_else(|| u.get("cache_read_input_tokens"))
        .and_then(num);
    let cache_write = u
        .pointer("/prompt_tokens_details/cache_write_tokens")
        .or_else(|| u.pointer("/input_tokens_details/cache_write_tokens"))
        .or_else(|| u.get("cache_creation_input_tokens"))
        .and_then(num);
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

    // 能力面字段（n>1 / logprobs 等）统一收集进 extensions，由 capability 规划层
    // 决策拒绝（decode 层只做结构解析，见 capability.rs plan_extension_fields）
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
                // `developer` 是 OpenAI 新版给 system 的继任角色（o 系 / gpt-5 系用它
                // 承载指令）。此前它落到下面的 `other` 分支被**整条丢弃** ⇒ 客户端用
                // developer 传 system prompt 时指令静默消失。两者归一到 system。
                "system" | "developer" => match content.and_then(Value::as_str) {
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
                    // 历史里回传的推理（三种线上拼写都要认）：跨族转换要把它带下去，
                    // 否则 thinking 上游在后续轮次校验缺 reasoning_content → 400。
                    // 放在最前，与 DeepSeek 流式的「reasoning 先于 content」顺序一致。
                    for field in REASONING_FIELDS {
                        let Some(rc) = m.get(field).and_then(Value::as_str) else {
                            continue;
                        };
                        // 空串也保留：客户端**显式发过**这个字段就说明它期望该字段存在
                        // （官方 DeepSeek 接受 `""`；严格中继另有要求，见下方 encoder 注释）。
                        // 只认「字段存在」，不臆断内容。
                        blocks.insert(
                            0,
                            Block::Thinking {
                                signature: Some(field.to_string()),
                                text: rc.to_string(),
                            },
                        );
                        break;
                    }
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
                    let result_blocks = decode_tool_content_blocks(content);
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
        reasoning_effort: v
            .get("reasoning_effort")
            .and_then(Value::as_str)
            .map(str::to_string),
    };

    // 未建模字段（§7 Lenient 收集）
    for k in [
        "response_format",
        "stream_options",
        "user",
        "logit_bias",
        "repetition_penalty",
        "n",
        "logprobs",
        "top_logprobs",
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

/// OpenAI 兼容线上「推理内容」字段的三种已知拼写。
///
/// DeepSeek 系用 `reasoning_content`；部分中继 / 自建网关用 `reasoning_text` 或
/// `reasoning`（PI-Desktop 的 `COMPLETIONS_REASONING_SIGNATURES` 是同一份清单）。
/// IR 的 `Block::Thinking.signature` 就用来记**实际命中的名字**，编码侧原样回传
/// 同一个名字 —— 否则换名字回传会被严格中继当成「没有回传推理」而 400。
pub(crate) const REASONING_FIELDS: [&str; 3] = ["reasoning_content", "reasoning_text", "reasoning"];

/// `Block::Thinking.signature` → 已知的线上字段名；不是白名单内的名字则 `None`。
///
/// 必须白名单校验：Anthropic 入站的 thinking 块也会填 `signature`，那是加密签名串。
fn normalize_reasoning_field(signature: Option<&str>) -> Option<&'static str> {
    let sig = signature?;
    REASONING_FIELDS.iter().find(|f| **f == sig).copied()
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
                        blocks.push(crate::codec::image::image_block_from_url(img));
                    }
                    _ => {}
                }
            }
            Ok(blocks)
        }
        _ => Ok(vec![]),
    }
}

/// 解析 `role=tool` 消息的 content → ToolResult 内容块。
///
/// OpenAI chat 规范里 tool 消息的 content 是字符串，但部分客户端与中转会
/// 发送内容数组（可含 `image_url`，即"工具返回图片"）。旧实现只取
/// `Value::as_str()`，遇到数组会得到空串——整条工具结果被静默抹掉。
/// 此处对两种形状都接受：字符串 → Text；数组 → 逐 part 解析。
fn decode_tool_content_blocks(content: Option<&Value>) -> Vec<Block> {
    match content {
        Some(Value::String(s)) if !s.is_empty() => vec![Block::Text { text: s.clone() }],
        Some(Value::Array(parts)) => parts.iter().filter_map(decode_tool_content_part).collect(),
        _ => vec![],
    }
}

/// 解析 tool 消息内容数组里的单个 part（text / image_url，缺 type 时按 text 兜底）。
fn decode_tool_content_part(p: &Value) -> Option<Block> {
    match p.get("type").and_then(Value::as_str) {
        Some("text") => p.get("text").and_then(Value::as_str).map(|t| Block::Text {
            text: t.to_string(),
        }),
        Some("image_url") => {
            let url = p
                .get("image_url")
                .and_then(|i| i.get("url"))
                .and_then(Value::as_str)?;
            if url.is_empty() {
                None
            } else {
                Some(crate::codec::image::image_block_from_url(url))
            }
        }
        // 兼容省略 type 的裸 {"text": "..."}
        _ => p.get("text").and_then(Value::as_str).map(|t| Block::Text {
            text: t.to_string(),
        }),
    }
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
    // 客户端（dsh 等）从 prompt_tokens_details.cached_tokens / cache_write_tokens 读取
    // 缓存命中与写入细分；即使无缓存细分也保持 prompt_tokens 为主占用口径。
    if u.cache_read_tokens.is_some() || u.cache_write_tokens.is_some() {
        let mut details = serde_json::Map::new();
        if let Some(cr) = u.cache_read_tokens {
            details.insert("cached_tokens".into(), json!(cr));
        }
        if let Some(cw) = u.cache_write_tokens {
            details.insert("cache_write_tokens".into(), json!(cw));
        }
        o["prompt_tokens_details"] = Value::Object(details);
    }
    o
}

// ================================================================ M5：UpstreamCodec::OpenAI

// Anthropic 入站 × OpenAI 上游 时使用（Claude Code × GPT 模型）。

/// 渲染一个 OpenAI 图片 part（消息级与「工具结果提升」共用）。
///
/// url 优先；否则把 base64 载荷包成 data URL，`media_type` 缺失/为空才回落 png。
/// 既无 url 也无载荷 → `None`（不产出空图片 part）。
fn render_image_part(
    media_type: &str,
    data_base64: &Option<String>,
    url: &Option<String>,
) -> Option<Value> {
    let url_val = if let Some(u) = url {
        Value::String(u.clone())
    } else if let Some(b64) = data_base64 {
        let mime = if media_type.is_empty() {
            "image/png"
        } else {
            media_type
        };
        Value::String(format!("data:{mime};base64,{b64}"))
    } else {
        return None;
    };
    Some(json!({
        "type":"image_url",
        "image_url":{"url": url_val}
    }))
}

/// 编码请求：IR → OpenAI chat completions body。
pub fn encode_request(req: &crate::codec::ir::CanonicalRequest) -> Result<Value, String> {
    let mut body = json!({
        "model": req.model,
        "messages": [],
    });

    // 渠道是否需要「非空推理回放」（`crate::codec::replay`）：由规划层经 extensions 传入。
    // 走 extensions 而非函数签名，四个 encoder 的签名保持不变。
    let replay_required = req
        .extensions
        .get(crate::codec::replay::EXT_KEY)
        .and_then(Value::as_bool)
        .unwrap_or(false);

    // message 数组（system 作为第一条 system 消息还原，§4-C）
    let mut messages: Vec<Value> = Vec::new();
    if !req.system.is_empty() {
        messages.push(json!({"role":"system","content": req.system.join("\n\n")}));
    }
    for m in &req.messages {
        match m.role {
            Role::User => {
                // 文本 + 图片同轮时合成一个多模态 content 数组（保块序）；
                // 纯文本保持 string content（与既有夹具兼容）；tool 结果 → 独立 role=tool 消息
                let mut text = String::new();
                let mut parts: Vec<Value> = Vec::new();
                let mut has_image = false;
                let mut tool_msgs: Vec<Value> = Vec::new();
                // 工具结果内嵌图片的降级落点（tool 消息装不下图 → 提升为紧随其后的 user 消息）
                let mut hoisted_parts: Vec<Value> = Vec::new();
                for b in &m.blocks {
                    match b {
                        Block::Text { text: t } => {
                            if has_image {
                                // 已进入图片模式：文本进 parts，保序
                                parts.push(json!({"type":"text","text":t}));
                            } else {
                                text.push_str(t);
                            }
                        }
                        Block::Image {
                            media_type,
                            data_base64,
                            url,
                        } => {
                            // 首图出现时把已积累文本转进 parts（保序），此后走多模态数组
                            if !has_image {
                                has_image = true;
                                if !text.is_empty() {
                                    parts.push(json!({"type":"text","text":text}));
                                    text = String::new();
                                }
                            }
                            // OpenAI 可接受 url 或 base64 data url；url 优先，
                            // base64 的 data URL 用 IR 真实 media_type（缺失回落 png）
                            let Some(part) = render_image_part(media_type, data_base64, url) else {
                                continue;
                            };
                            parts.push(part);
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
                            // 工具结果内嵌图片：OpenAI chat 规范明确「tool 消息只支持 text
                            // part」，装不下图片 → 降级提升为紧随其后的一条 user 消息
                            //（capability 面 tool_result_images=false，规划层已记 CapabilityWarn）。
                            // 旧实现只渲染文本块，图片被无声丢弃。
                            for c in content {
                                if let Block::Image {
                                    media_type,
                                    data_base64,
                                    url,
                                } = c
                                {
                                    if let Some(p) = render_image_part(media_type, data_base64, url)
                                    {
                                        if hoisted_parts.is_empty() {
                                            hoisted_parts.push(json!({
                                                "type": "text",
                                                "text": "[工具结果内嵌图片，已降级提升为紧随其后的 user 消息]"
                                            }));
                                        }
                                        hoisted_parts.push(p);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if has_image {
                    if !text.is_empty() {
                        parts.push(json!({"type":"text","text":text}));
                    }
                    if !parts.is_empty() {
                        messages.push(json!({"role":"user","content": Value::Array(parts)}));
                    }
                } else if !text.is_empty() {
                    messages.push(json!({"role":"user","content":text}));
                }
                messages.extend(tool_msgs);
                if !hoisted_parts.is_empty() {
                    messages.push(json!({
                        "role":"user",
                        "content": Value::Array(hoisted_parts)
                    }));
                }
            }
            Role::Assistant => {
                let mut content = String::new();
                let mut tool_calls: Vec<Value> = Vec::new();
                let mut reasoning = String::new();
                // 回传时用的线上字段名：默认 reasoning_content，被 Block::Thinking.signature
                // 命中已知拼写时改写（见 normalize_reasoning_field）。
                let mut reasoning_field: &str = REASONING_FIELDS[0];
                // 客户端/上游**显式带过**具名字段的推理时置位：此时即使文本为空也要把
                // 字段发出去（保留「该字段存在」这一事实，不擅自补内容）。
                let mut reasoning_named = false;
                for b in &m.blocks {
                    match b {
                        Block::Text { text } => content.push_str(text),
                        Block::Thinking { text, signature } => {
                            // signature 记的是上游用的线上字段名；不是已知名字
                            // （例如 Anthropic 入站填的是加密签名串）时退回默认。
                            if let Some(known) = normalize_reasoning_field(signature.as_deref()) {
                                reasoning_field = known;
                                reasoning_named = true;
                            }
                            reasoning.push_str(text);
                        }
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
                // 有具名字段的推理时即使为空也发（保 presence）；无具名来源且文本为空
                // 则默认完全不发该字段 —— 不发明模型没产生过的内容。
                //
                // 例外：渠道声明/学习到「要求非空推理回放」（`crate::codec::replay`，
                // 由规划层经 extensions 传入）时，没有推理的轮次也要**补一个非空占位** ——
                // 严格中继会拒绝缺字段与空串，这是唯一能让它接受历史的办法。
                // 该标记默认关闭，且只在被上游 400 验证过（或模型名明确指向 DeepSeek）时打开。
                if !reasoning.is_empty() || reasoning_named {
                    msg[reasoning_field] = Value::String(reasoning);
                } else if replay_required {
                    msg[reasoning_field] =
                        Value::String(crate::codec::replay::PLACEHOLDER.to_string());
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
    // 结构化输出（capability 规划层已裁决：Supported 原样 / Degraded 已覆写为 json_object）
    if let Some(format) = req.extensions.get("response_format") {
        body["response_format"] = format.clone();
    }
    // reasoning effort（Native 档）：原样透传
    if let Some(effort) = &p.reasoning_effort {
        body["reasoning_effort"] = json!(effort);
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
                // 线上字段名有三种拼写：DeepSeek 系 `reasoning_content`、部分中继的
                // `reasoning_text` / `reasoning`。把**实际命中的名字**记进 `signature`，
                // 编码侧据此原样回传同一个名字（口径同 PI-Desktop 的 thinkingSignature）。
                for field in REASONING_FIELDS {
                    let Some(rc) = msg.get(field).and_then(Value::as_str) else {
                        continue;
                    };
                    if !rc.is_empty() {
                        output.push(Block::Thinking {
                            signature: Some(field.to_string()),
                            text: rc.to_string(),
                        });
                        break;
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
    let cache_write = u
        .and_then(|v| v.pointer("/prompt_tokens_details/cache_write_tokens"))
        .and_then(Value::as_u64);
    Usage {
        input_tokens: get("prompt_tokens"),
        output_tokens: get("completion_tokens"),
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
    }
}

/// 解析 OpenAI SSE 一个 `data: {...}` 包 → StreamEvent 列表。
pub fn parse_stream_event(raw: &[u8]) -> Result<Vec<crate::codec::ir::StreamEvent>, String> {
    let v: Value =
        serde_json::from_slice(raw).map_err(|e| format!("OpenAI SSE JSON 解析失败: {e}"))?;

    let mut out = Vec::new();
    // usage 的采集与 choices 的形状无关。OpenAI 官方末帧是 `"choices":[] + usage`，
    // 但 openai_compat 阵营里同样合法的写法还有 `"choices":[{"index":0,"delta":{}}] + usage`
    // （scnet/超算 gateway）以及 usage 与 content 增量同帧。此前只有 choices 为空/缺失
    // 才认 usage 帧，非空末帧被当普通 chunk 整帧丢弃 → IR 只收到 finish_reason 那帧的
    // 零值 Finish → 出站 usage 恒 0，dsh-tui 的上下文占比统计不出来。
    let frame_usage = v
        .get("usage")
        .filter(|u| !u.is_null())
        .map(|u| parse_usage(Some(u)));
    let mut saw_finish = false;

    if let Some(choices) = v.get("choices").and_then(Value::as_array) {
        if let Some(c) = choices.first() {
            if let Some(delta) = c.get("delta") {
                // thinking 模型（deepseek 等）：reasoning_content 增量必须保留进 IR，
                // 否则跨族出站丢思考、客户端历史缺 reasoning_content、下轮回传上游 400。
                // 与 DeepSeek 流式顺序一致：reasoning_content 先于 content。
                if let Some(rc) = delta.get("reasoning_content").and_then(Value::as_str) {
                    if !rc.is_empty() {
                        out.push(StreamEvent::ThinkingDelta {
                            text: rc.to_string(),
                        });
                    }
                }
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
                let usage = frame_usage
                    .clone()
                    .unwrap_or_else(|| parse_usage(v.get("usage")));
                saw_finish = true;
                out.push(StreamEvent::Finish { stop_reason, usage });
            }
        }
    }
    // 本帧带 usage 却没有 finish_reason：供应商把两者拆成两帧的写法（含 choices 非空
    // 的末帧）。这里必须补出 Finish，否则真实 usage 永远进不了 IR，客户端只会看到 0。
    if let Some(usage) = frame_usage {
        if !saw_finish {
            out.push(StreamEvent::Finish {
                stop_reason: StopReason::EndTurn,
                usage,
            });
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

    /// 复现排查（2026-09-03）：responses 直通流 输入/输出恒 0 的猜测验证。
    /// Chat Completions 形状：choices 中途 frames + 末尾 usage 帧。
    #[test]
    fn repro_responses_passthrough_chatcompletions_shape() {
        let frames = [
            r#"data: {"id":"c1","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant"},"index":0}]}"#,
            r#"data: {"id":"c1","object":"chat.completion.chunk","choices":[{"delta":{"content":"你好"},"index":0}]}"#,
            r#"data: {"id":"c1","object":"chat.completion.chunk","choices":[{"delta":{"content":"世界"},"index":0}]}"#,
            r#"data: {"id":"c1","object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":11333,"completion_tokens":64,"total_tokens":11397}}"#,
            r#"data: [DONE]"#,
        ];
        // 整段一次性喂入（模拟整块）
        let mut once = UsageScanner::new();
        let joined = frames.join("\n\n") + "\n\n";
        once.feed(joined.as_bytes());
        let v = once.finish();
        match &v {
            Some(v) => println!(
                "[chatcompletions once]: prompt={} completion={}",
                v["prompt_tokens"], v["completion_tokens"]
            ),
            None => println!("[chatcompletions once]: NONE"),
        }
        // 逐帧喂入（模拟真实网络分批）
        let mut chunked = UsageScanner::new();
        for f in frames {
            chunked.feed((f.to_string() + "\n\n").as_bytes());
        }
        let v2 = chunked.finish();
        match &v2 {
            Some(v) => println!(
                "[chatcompletions chunked]: prompt={} completion={}",
                v["prompt_tokens"], v["completion_tokens"]
            ),
            None => println!("[chatcompletions chunked]: NONE"),
        }
        assert!(v.is_some(), "ChatCompletions 形状整段喂入应捕获 usage");
        assert!(v2.is_some(), "ChatCompletions 形状逐帧喂入应捕获 usage");
    }

    /// Responses 协议形状：response.completed 事件内嵌 usage。
    #[test]
    fn repro_responses_passthrough_responses_shape() {
        let completed = r#"data: {"type":"response.completed","response":{"id":"resp_x","object":"response","status":"completed","model":"deepseek-v4-flash-0731","output":[{"type":"message","role":"assistant","content":[]}],"usage":{"input_tokens":11333,"input_tokens_details":{"cached_tokens":0},"output_tokens":64,"total_tokens":11397}}}"#;
        let frames = [
            r#"data: {"type":"response.created","response":{"id":"resp_x","object":"response","status":"in_progress","model":"deepseek-v4-flash-0731","output":[]}}"#,
            r#"data: {"type":"response.output_text.delta","item_id":"msg_x","output_index":0,"delta":"你好"}"#,
            completed,
        ];
        let mut once = UsageScanner::new();
        let joined = frames.join("\n\n") + "\n\n";
        once.feed(joined.as_bytes());
        let v = once.finish();
        match &v {
            Some(v) => println!(
                "[responses once]: input={} output={}",
                v["input_tokens"], v["output_tokens"]
            ),
            None => println!("[responses once]: NONE"),
        }
        // 模拟真实网络分批：把 completed 帧切成多个小块喂入
        let mut chunked = UsageScanner::new();
        for f in [&frames[0], &frames[1]] {
            chunked.feed((f.to_string() + "\n\n").as_bytes());
        }
        for byte_slice in completed.as_bytes().chunks(48) {
            chunked.feed(byte_slice);
        }
        chunked.feed(b"\n\n");
        let v2 = chunked.finish();
        match &v2 {
            Some(v) => println!(
                "[responses chunked]: input={} output={}",
                v["input_tokens"], v["output_tokens"]
            ),
            None => println!("[responses chunked]: NONE"),
        }
        assert!(v.is_some(), "Responses 形状整段喂入应捕获 usage");
        assert!(v2.is_some(), "Responses 形状分批喂入应捕获 usage");
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
        // decode 层只做结构解析：n>1 收集进 extensions，由能力规划层拒绝（capability.rs）
        let req = super::decode_request(
            br#"{"model":"m","n":2,"messages":[{"role":"user","content":"x"}]}"#,
        )
        .expect("decode 不应直接拒绝 n>1");
        assert!(req.extensions.contains_key("n"));

        // anthropic 族（不支持多候选）→ 规划层 Rejected
        let plan = crate::codec::capability::plan_compatibility(
            &req,
            crate::codec::capability::caps_of(crate::codec::Family::Anthropic),
        );
        let mut req2 = req.clone();
        let outcome = plan.resolve(&mut req2);
        let (msg, _code) = outcome.rejection.expect("n>1 应被规划层拒绝");
        assert!(msg.contains("n>1"));

        // n=1 不拒绝
        let ok = super::decode_request(
            br#"{"model":"m","n":1,"messages":[{"role":"user","content":"x"}]}"#,
        )
        .expect("n=1 正常");
        assert!(ok.extensions.contains_key("n"));
    }

    #[test]
    fn encode_response_format_external() {
        // 规划层裁决后，response_format 从 extensions 外传给 openai_compat 上游
        let mut req = CanonicalRequest {
            model: "gpt-4o".into(),
            ..Default::default()
        };
        req.extensions.insert(
            "response_format".into(),
            serde_json::json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "weather",
                    "schema": {"type": "object", "properties": {"temp": {"type": "number"}}},
                    "strict": true
                }
            }),
        );
        let v = super::encode_request(&req).unwrap();
        let rf = v.get("response_format").expect("response_format 应外传");
        assert_eq!(rf.get("type").and_then(Value::as_str), Some("json_schema"));
        assert_eq!(
            rf.get("json_schema")
                .and_then(|s| s.get("name"))
                .and_then(Value::as_str),
            Some("weather")
        );

        // 降级路径（json_object）：原样外传
        let mut req2 = CanonicalRequest {
            model: "gpt-4o".into(),
            ..Default::default()
        };
        req2.extensions.insert(
            "response_format".into(),
            serde_json::json!({"type": "json_object"}),
        );
        let v2 = super::encode_request(&req2).unwrap();
        assert_eq!(
            v2.get("response_format")
                .and_then(|f| f.get("type"))
                .and_then(Value::as_str),
            Some("json_object")
        );
    }

    #[test]
    fn encode_reasoning_effort_passthrough() {
        let mut req = CanonicalRequest {
            model: "gpt-4o".into(),
            ..Default::default()
        };
        req.params.reasoning_effort = Some("high".into());
        let v = super::encode_request(&req).unwrap();
        assert_eq!(v["reasoning_effort"], "high");
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
    fn parse_stream_event_usage_frame_with_empty_choices() {
        // OpenAI 流式 include_usage 的标准末帧形状："choices":[] + usage。
        // 此前该帧被当普通 chunk 丢弃，跨族转换路径 usage 恒 0（dsh ctx 恒 0 根因）。
        let evs = super::parse_stream_event(
            br#"{"id":"c1","object":"chat.completion.chunk","choices":[],
                "usage":{"prompt_tokens":11333,"completion_tokens":64,"total_tokens":11397}}"#,
        )
        .unwrap();
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            crate::codec::ir::StreamEvent::Finish { usage, .. } => {
                assert_eq!(usage.input_tokens, 11333);
                assert_eq!(usage.output_tokens, 64);
            }
            other => panic!("应识别为 Finish，得到 {other:?}"),
        }
    }

    #[test]
    fn parse_stream_event_empty_choices_without_usage_noop() {
        // 空 choices 但无 usage 的帧不应产生任何事件（防御空转）
        let evs = super::parse_stream_event(
            br#"{"id":"c1","object":"chat.completion.chunk","choices":[]}"#,
        )
        .unwrap();
        assert!(evs.is_empty());
    }

    fn finish_usage(evs: &[crate::codec::ir::StreamEvent]) -> crate::codec::ir::Usage {
        use crate::codec::ir::StreamEvent as Ev;
        evs.iter()
            .find_map(|e| match e {
                Ev::Finish { usage, .. } => Some(usage.clone()),
                _ => None,
            })
            .expect("应产出 Finish 事件")
    }

    #[test]
    fn parse_stream_event_usage_frame_with_nonempty_choices() {
        // scnet/超算 gateway 的末帧形状：choices 非空（只有一个空 delta）但带 usage。
        // 此前按 "choices 为空" 判定 usage 帧，整帧被丢弃 → 出站 usage 恒 0，
        // dsh-tui 的上下文占比统计不出来（2026-09-15 实测根因）。
        let evs = super::parse_stream_event(
            br#"{"id":"c1","object":"chat.completion.chunk",
                "choices":[{"index":0,"delta":{}}],
                "usage":{"prompt_tokens":259132,"completion_tokens":1805,"total_tokens":260937,
                          "prompt_tokens_details":{"cached_tokens":258944}}}"#,
        )
        .unwrap();
        let u = finish_usage(&evs);
        assert_eq!(u.input_tokens, 259132);
        assert_eq!(u.output_tokens, 1805);
        assert_eq!(u.cache_read_tokens, Some(258944));
    }

    #[test]
    fn parse_stream_event_usage_split_after_finish_reason_frame() {
        // 供应商拆两帧：先 finish_reason（无 usage）再 usage 末帧。
        // 两帧各产出一个 Finish，且 usage 帧必须带真实数字，供上层择优保留。
        let first = super::parse_stream_event(
            br#"{"id":"c1","object":"chat.completion.chunk",
                "choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        )
        .unwrap();
        assert_eq!(finish_usage(&first).input_tokens, 0);
        let second = super::parse_stream_event(
            br#"{"id":"c1","object":"chat.completion.chunk",
                "choices":[{"index":0,"delta":{}}],
                "usage":{"prompt_tokens":66,"completion_tokens":26,"total_tokens":92}}"#,
        )
        .unwrap();
        assert_eq!(finish_usage(&second).input_tokens, 66);
        assert_eq!(finish_usage(&second).output_tokens, 26);
    }

    #[test]
    fn parse_stream_event_usage_same_frame_as_content_delta() {
        // usage 与 content 增量同帧：正文不能因为补 Finish 而被吞掉。
        use crate::codec::ir::StreamEvent as Ev;
        let evs = super::parse_stream_event(
            br#"{"id":"c1","object":"chat.completion.chunk",
                "choices":[{"index":0,"delta":{"content":"OK"}}],
                "usage":{"prompt_tokens":10,"completion_tokens":1,"total_tokens":11}}"#,
        )
        .unwrap();
        assert!(matches!(&evs[0], Ev::TextDelta { text } if text == "OK"));
        assert_eq!(finish_usage(&evs).input_tokens, 10);
    }

    #[test]
    fn parse_stream_event_null_usage_is_noop() {
        // `"usage":null` 的中间帧（部分供应商每帧都带 null usage）不应产出 Finish
        let evs = super::parse_stream_event(
            br#"{"id":"c1","object":"chat.completion.chunk",
                "choices":[{"index":0,"delta":{"content":"hi"}}],"usage":null}"#,
        )
        .unwrap();
        assert_eq!(evs.len(), 1);
    }

    #[test]
    fn parse_stream_event_reasoning_content_delta() {
        // thinking 模型（deepseek 等）流式增量带 reasoning_content；
        // 必须转成 ThinkingDelta 保留进 IR，否则跨族出站丢思考、
        // 客户端历史缺 reasoning_content、下轮回传上游 400。
        use crate::codec::ir::StreamEvent as Ev;
        let evs = super::parse_stream_event(
            br#"{"id":"c1","object":"chat.completion.chunk",
                "choices":[{"index":0,"delta":{"reasoning_content":"step one"}}]}"#,
        )
        .unwrap();
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            Ev::ThinkingDelta { text } => assert_eq!(text, "step one"),
            other => panic!("应识别为 ThinkingDelta，得到 {other:?}"),
        }

        // 同时带 content 与 reasoning_content 时两者都产出
        let evs = super::parse_stream_event(
            br#"{"id":"c1","object":"chat.completion.chunk",
                "choices":[{"index":0,"delta":{"reasoning_content":"think","content":"answer"}}]}"#,
        )
        .unwrap();
        let kinds: Vec<String> = evs
            .iter()
            .map(|e| match e {
                Ev::ThinkingDelta { .. } => "thinking".into(),
                Ev::TextDelta { .. } => "text".into(),
                _ => "other".into(),
            })
            .collect();
        assert_eq!(kinds, vec!["thinking", "text"]);
    }

    #[test]
    fn usage_json_emits_cache_details() {
        // dsh 客户端从 prompt_tokens_details.cached_tokens / cache_write_tokens
        // 读取缓存细分；网关 Chat 出站必须带上（此前 krate_has_cache 恒 false 整体丢弃）。
        let u = Usage {
            input_tokens: 90,
            output_tokens: 6,
            cache_read_tokens: Some(70),
            cache_write_tokens: Some(12),
        };
        let v = usage_json(&u);
        assert_eq!(v["prompt_tokens"], 90);
        assert_eq!(v["prompt_tokens_details"]["cached_tokens"], 70);
        assert_eq!(v["prompt_tokens_details"]["cache_write_tokens"], 12);

        // 仅 cache_write 存在时也建 details
        let u2 = Usage {
            input_tokens: 10,
            output_tokens: 1,
            cache_read_tokens: None,
            cache_write_tokens: Some(5),
        };
        let v2 = usage_json(&u2);
        assert_eq!(v2["prompt_tokens_details"]["cache_write_tokens"], 5);

        // 无缓存细分时不产出空 details
        let v3 = usage_json(&Usage::default());
        assert!(v3.get("prompt_tokens_details").is_none());
    }

    #[test]
    fn parse_usage_reads_cache_write_tokens() {
        let raw = serde_json::json!({
            "prompt_tokens": 80,
            "completion_tokens": 4,
            "total_tokens": 84,
            "prompt_tokens_details": {
                "cached_tokens": 50,
                "cache_write_tokens": 20
            }
        });
        let u = parse_usage(Some(&raw));
        assert_eq!(u.input_tokens, 80);
        assert_eq!(u.cache_read_tokens, Some(50));
        assert_eq!(u.cache_write_tokens, Some(20));
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

    #[test]
    fn decode_image_parts_data_url_and_url() {
        // 入站图片：data URL 拆 base64+media_type；http url 透传占位 png
        let req = super::decode_request(
            br#"{"model":"gpt-4o","messages":[{"role":"user","content":[
                {"type":"text","text":"look"},
                {"type":"image_url","image_url":{"url":"data:image/jpeg;base64,aGVsbG8="}},
                {"type":"image_url","image_url":{"url":"https://x.com/a.png"}}
            ]}]}"#,
        )
        .unwrap();
        let blocks = &req.messages[0].blocks;
        assert_eq!(blocks.len(), 3);
        match &blocks[1] {
            IrBlock::Image {
                media_type,
                data_base64,
                url,
            } => {
                assert_eq!(media_type, "image/jpeg");
                assert_eq!(data_base64.as_deref(), Some("aGVsbG8="));
                assert!(url.is_none());
            }
            other => panic!("期望 Image，得到 {other:?}"),
        }
        match &blocks[2] {
            IrBlock::Image { url, .. } => {
                assert_eq!(url.as_deref(), Some("https://x.com/a.png"));
            }
            other => panic!("期望 Image，得到 {other:?}"),
        }
    }

    #[test]
    fn encode_text_and_image_single_message_preserves_media_type() {
        use crate::codec::ir::{CanonMessage, Role as IrRole, SampleParams, ToolChoice};
        // 同轮 text+image → 单条 user 消息多模态数组，保块序；
        // base64 的 data URL 用 IR 真实 media_type（jpeg 不再被硬编码成 png）
        let req = crate::codec::ir::CanonicalRequest {
            model: "gpt-4o".into(),
            system: vec![],
            messages: vec![CanonMessage {
                role: IrRole::User,
                blocks: vec![
                    IrBlock::Text {
                        text: "before".into(),
                    },
                    IrBlock::Image {
                        media_type: "image/jpeg".into(),
                        data_base64: Some("aGVsbG8=".into()),
                        url: None,
                    },
                    IrBlock::Text {
                        text: "after".into(),
                    },
                ],
            }],
            tools: vec![],
            tool_choice: ToolChoice::Auto,
            params: SampleParams::default(),
            stream: false,
            extensions: Default::default(),
        };
        let v = super::encode_request(&req).unwrap();
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1, "文本+图片应合为一条 user 消息");
        assert_eq!(msgs[0]["role"], "user");
        let content = msgs[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 3, "块序应保留 text→image→text");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "before");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(
            content[1]["image_url"]["url"],
            "data:image/jpeg;base64,aGVsbG8="
        );
        assert_eq!(content[2]["type"], "text");
        assert_eq!(content[2]["text"], "after");
    }

    #[test]
    fn encode_image_url_priority_and_plain_text_stays_string() {
        use crate::codec::ir::{CanonMessage, Role as IrRole, SampleParams, ToolChoice};
        // url 优先于 base64；纯文本消息仍是 string content（兼容旧夹具）
        let req = crate::codec::ir::CanonicalRequest {
            model: "gpt-4o".into(),
            system: vec![],
            messages: vec![
                CanonMessage {
                    role: IrRole::User,
                    blocks: vec![IrBlock::Image {
                        media_type: "image/jpeg".into(),
                        data_base64: Some("aGk=".into()),
                        url: Some("https://x.com/a.png".into()),
                    }],
                },
                CanonMessage::text(IrRole::User, "plain"),
            ],
            tools: vec![],
            tool_choice: ToolChoice::Auto,
            params: SampleParams::default(),
            stream: false,
            extensions: Default::default(),
        };
        let v = super::encode_request(&req).unwrap();
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(
            msgs[0]["content"][0]["image_url"]["url"], "https://x.com/a.png",
            "url 优先于 base64"
        );
        assert_eq!(msgs[1]["content"], "plain");
    }

    /// 2026-09-22（PI-Desktop 源码对账）：客户端历史里的推理必须**双向**保真。
    ///
    /// 三个缺口一起守：
    /// 1. 入站历史里的 `reasoning_content` 此前**完全没读**（`grep reasoning` 在
    ///    request 解码区 0 命中）⇒ 跨族到 thinking 上游时历史推理丢失 → 上游 400；
    /// 2. 线上字段名有三种拼写（`reasoning_content` / `reasoning_text` / `reasoning`），
    ///    必须**原样回传命中的那个名字**，换名字会被严格中继当成没回传；
    /// 3. 客户端**显式发过**该字段时（哪怕是空串）要保留「字段存在」这一事实
    ///    （官方 DeepSeek 接受 `""`），但不发明模型没产生过的内容。
    #[test]
    fn client_history_reasoning_replays_with_original_field_name() {
        // 1) reasoning_content：读出 + 同名回传
        let req = super::decode_request(
            br#"{"model":"deepseek-v4-flash","messages":[
                {"role":"user","content":"hi"},
                {"role":"assistant","reasoning_content":"prior think","content":"ok"}
            ]}"#,
        )
        .unwrap();
        let asst = &req.messages[1];
        assert!(matches!(
            &asst.blocks[0],
            Block::Thinking { text, signature }
                if text == "prior think" && signature.as_deref() == Some("reasoning_content")
        ));
        let body = super::encode_request(&req).unwrap();
        assert_eq!(body["messages"][1]["reasoning_content"], "prior think");

        // 2) 换拼写：`reasoning_text` 进、`reasoning_text` 出（不能改叫 reasoning_content）
        let req2 = super::decode_request(
            br#"{"model":"m","messages":[
                {"role":"user","content":"hi"},
                {"role":"assistant","reasoning_text":"alt think","content":"ok"}
            ]}"#,
        )
        .unwrap();
        assert!(matches!(
            &req2.messages[1].blocks[0],
            Block::Thinking { signature, .. } if signature.as_deref() == Some("reasoning_text")
        ));
        let body2 = super::encode_request(&req2).unwrap();
        assert_eq!(body2["messages"][1]["reasoning_text"], "alt think");
        assert!(
            body2["messages"][1].get("reasoning_content").is_none(),
            "不能把客户端用的字段名换掉"
        );

        // 3) 显式空串：保 presence（官方 DeepSeek 接受 ""）
        let req3 = super::decode_request(
            br#"{"model":"m","messages":[
                {"role":"user","content":"hi"},
                {"role":"assistant","reasoning_content":"","content":"ok"}
            ]}"#,
        )
        .unwrap();
        let body3 = super::encode_request(&req3).unwrap();
        assert_eq!(
            body3["messages"][1]["reasoning_content"], "",
            "客户端显式发过的字段应保留存在性"
        );

        // 4) 客户端没发过 → 不发明（保持既有行为）
        let req4 = super::decode_request(
            br#"{"model":"m","messages":[
                {"role":"user","content":"hi"},
                {"role":"assistant","content":"ok"}
            ]}"#,
        )
        .unwrap();
        let body4 = super::encode_request(&req4).unwrap();
        assert!(body4["messages"][1].get("reasoning_content").is_none());
    }

    /// 2026-09-22：`developer` 角色此前落到 `other` 分支被**整条丢弃**
    /// ⇒ 客户端用 developer 传 system prompt 时指令静默消失（只剩一条 CapabilityWarn）。
    #[test]
    fn developer_role_maps_to_system_not_dropped() {
        let req = super::decode_request(
            br#"{"model":"gpt-5","messages":[
                {"role":"developer","content":"be terse"},
                {"role":"user","content":"hi"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(
            req.system,
            vec!["be terse".to_string()],
            "developer 应归一到 system 段，而不是被丢弃"
        );
        assert_eq!(req.messages.len(), 1, "developer 不该再产生一条用户消息");
    }
}
