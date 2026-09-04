//! 协议中间表示（IR）—— protocol-ir.md §2 的 Rust 落地。
//!
//! 三条转换链路的公共载体：入站协议解码 → IR → 上游协议编码。
//! 类型定义与文档 §2 对齐；结构合规变换（§4-C）与护栏常量（§M4）
//! 也在本层提供纯函数，便于三协议 codec 复用。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

// ================================================================ 护栏常量

/// 单条消息最大内容块数（roadmap M4 护栏，越界 400）。
/// 按 Anthropic 单消息 content block 上限语义，按单条消息校验、不跨消息累计——
/// agent 客户端（dsh 等）每轮请求全量重放历史，跨消息累计会把正常长会话误伤。
pub const MAX_BLOCKS_PER_REQUEST: usize = 64;
/// 工具参数累计上限（roadmap M4 护栏，越界 400）
pub const MAX_TOTAL_TOOL_ARGS_BYTES: usize = 256 * 1024;

// ================================================================ 角色与请求

/// 消息角色。ToolResult 不作为顶层角色（承载于 Block），
/// 出站渲染器负责按目标协议放置（OpenAI: 独立 role=tool / 其他: user）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

/// 内容块（对应 Anthropic 的 block 原语）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    Image {
        media_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data_base64: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        call_id: String,
        content: Vec<Block>,
        is_error: bool,
    },
    /// v1 占位存储不转换（protocol-ir §10-3）
    Thinking {
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        text: String,
    },
    /// 家族特有块逃生舱：跨族按 §7 丢弃并 WARN
    FamilyRaw {
        family: String,
        value: Value,
    },
}

impl Block {
    pub fn is_text(&self) -> bool {
        matches!(self, Block::Text { .. })
    }

    /// 文本块的文本（非文本块 -> None）
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Block::Text { text } => Some(text),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema（OpenAPI 子集）
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
    Required,
    Specific(String),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SampleParams {
    pub max_output_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,
    #[serde(default)]
    pub stop_sequences: Vec<String>,
    pub frequency_penalty: Option<f32>,
    pub presence_penalty: Option<f32>,
    pub seed: Option<i64>,
    /// reasoning effort（OpenAI 系档位：none/minimal/low/medium/high；值原样透传，
    /// 族差异由 capability 规划层声明 + 各 encoder 映射，见 capability.rs）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

/// 单条消息：role + 内容块。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanonMessage {
    pub role: Role,
    pub blocks: Vec<Block>,
}

impl CanonMessage {
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            blocks: vec![Block::Text { text: text.into() }],
        }
    }
}

/// 规范化请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CanonicalRequest {
    pub model: String,
    /// 多条 system 按序合并，渲染时以 \n\n 连接
    #[serde(default)]
    pub system: Vec<String>,
    #[serde(default)]
    pub messages: Vec<CanonMessage>,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    #[serde(default)]
    pub tool_choice: ToolChoice,
    #[serde(default)]
    pub params: SampleParams,
    #[serde(default)]
    pub stream: bool,
    /// 未建模字段（§7 Lenient 丢弃 + WARN）
    #[serde(default, skip_serializing)]
    pub extensions: Map<String, Value>,
}

// ================================================================ 响应与流式事件

/// 统一结束原因（映射唯一权威见 protocol-ir §5-C）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    ToolUse,
    SafetyBlock,
    Other(String),
}

impl StopReason {
    /// 用于日志的枚举名
    pub fn as_log_str(&self) -> &'static str {
        match self {
            StopReason::EndTurn => "end_turn",
            StopReason::MaxTokens => "max_tokens",
            StopReason::ToolUse => "tool_use",
            StopReason::SafetyBlock => "safety",
            StopReason::Other(_) => "other",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
}

/// 规范化响应（非流式）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanonicalResponse {
    pub id: String,
    pub model: String,
    pub output: Vec<Block>,
    pub stop_reason: StopReason,
    pub usage: Usage,
}

/// 流式事件 IR。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "evt", rename_all = "snake_case")]
pub enum StreamEvent {
    Start {
        model: String,
    },
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolCallStart {
        index: usize,
        id: String,
        name: String,
    },
    ToolCallArgsDelta {
        index: usize,
        args_fragment: String,
    },
    ToolCallEnd {
        index: usize,
    },
    Finish {
        stop_reason: StopReason,
        usage: Usage,
    },
}

// ================================================================ 结构合规变换（§4-C）

/// 请求级结构合规：合并相邻同角色消息；system 提升已在 CanonicalRequest 独立字段。
///
/// `→A` / `→G` 强制 user/assistant 交替，OpenAI 无此约束但合并无害且省 token。
/// 返回新请求（内容不变，仅合并文本块相邻消息）。
pub fn merge_adjacent_same_role(req: &CanonicalRequest) -> CanonicalRequest {
    let mut messages: Vec<CanonMessage> = Vec::with_capacity(req.messages.len());
    for m in &req.messages {
        if let Some(last) = messages.last_mut() {
            if last.role == m.role {
                last.blocks.extend(m.blocks.clone());
                continue;
            }
        }
        messages.push(m.clone());
    }
    CanonicalRequest {
        model: req.model.clone(),
        system: req.system.clone(),
        messages,
        tools: req.tools.clone(),
        tool_choice: req.tool_choice.clone(),
        params: req.params.clone(),
        stream: req.stream,
        extensions: Map::new(),
    }
}

/// 汇总一条 WARN 用的扩展字段清单（§7 Lenient 策略）。
pub fn extension_warn_note(req: &CanonicalRequest) -> Option<String> {
    if req.extensions.is_empty() {
        return None;
    }
    let names: Vec<&str> = req.extensions.keys().map(String::as_str).collect();
    Some(format!("未知字段已按 Lenient 丢弃：{}", names.join(", ")))
}

// ================================================================ tool id 可逆编码（§5-B）

/// 出站侧 tool_use id 确定性内嵌编码：`toolu_` + base58("jai1." + inbound_id)。
/// 客户端历史回传时反解复原；Anthropic 上限 64 字符，超长回落 tool_id_map（M5 验收 2，
/// 存储接线在 store::tool_id_map；当前纯函数不感知 DB，超长时仍返回确定性编码，后续 M8 接映射）。
const TOOL_ID_MAGIC: &str = "jai1.";
const BASE58_ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// 出站侧 tool_use id 确定性内嵌编码：`toolu_` + base58("jai1." + inbound_id)。
/// 客户端历史回传时反解复原；Anthropic 原生 `toolu_*` 幂等直通。
pub fn canonical_to_anthropic_id(inbound: &str) -> String {
    if inbound.starts_with("toolu_") {
        return inbound.to_string(); // 已编码过 / Anthropic 原生 id，幂等
    }
    let encoded = base58_encode(format!("{TOOL_ID_MAGIC}{inbound}").as_bytes());
    format!("toolu_{encoded}")
}

/// 反解：`toolu_xxx` → 原始入站 id。
///
/// 优先识别本网关的 `toolu_` + base58("jai1." + id) 格式并还原；
/// 其他 `toolu_*`（Anthropic 原生或旧版简单前缀）按原后缀返回，保证多轮中
/// 未经过本网关编码的 id 也能保持可关联。
pub fn decode_anthropic_tool_id(anthropic_id: &str) -> Option<String> {
    let encoded = anthropic_id.strip_prefix("toolu_")?;
    if let Some(bytes) = base58_decode(encoded) {
        if let Ok(s) = String::from_utf8(bytes) {
            if let Some(orig) = s.strip_prefix(TOOL_ID_MAGIC) {
                return Some(orig.to_string());
            }
        }
    }
    Some(encoded.to_string())
}

fn base58_encode(input: &[u8]) -> String {
    let mut zeros = 0;
    while zeros < input.len() && input[zeros] == 0 {
        zeros += 1;
    }
    let mut data = input.to_vec();
    let mut first = zeros;
    let mut encoded: Vec<u8> = Vec::new();
    while first < data.len() {
        let mut rem = 0usize;
        let mut i = first;
        while i < data.len() {
            let acc = rem * 256 + data[i] as usize;
            data[i] = (acc / 58) as u8;
            rem = acc % 58;
            i += 1;
        }
        encoded.push(BASE58_ALPHABET[rem]);
        while first < data.len() && data[first] == 0 {
            first += 1;
        }
    }
    let mut out = String::with_capacity(zeros + encoded.len());
    for _ in 0..zeros {
        out.push('1');
    }
    for &b in encoded.iter().rev() {
        out.push(b as char);
    }
    out
}

fn base58_decode(s: &str) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    let mut leading_ones = 0usize;
    for c in s.bytes() {
        if out.is_empty() && c == b'1' {
            leading_ones += 1;
            continue;
        }
        let val = BASE58_ALPHABET.iter().position(|&a| a == c)? as u32;
        let mut carry = val;
        for b in out.iter_mut().rev() {
            let acc = *b as u32 * 58 + carry;
            *b = (acc & 0xff) as u8;
            carry = acc >> 8;
        }
        while carry > 0 {
            out.insert(0, (carry & 0xff) as u8);
            carry >>= 8;
        }
    }
    while out.len() > 1 && out[0] == 0 {
        out.remove(0);
    }
    let mut result = vec![0u8; leading_ones];
    result.extend(out);
    Some(result)
}

// ================================================================ 护栏校验

/// M4 护栏：单条消息 blocks ≤ 64（Anthropic 单消息上限语义）且工具参数累计 ≤ 256KB。
/// 返回 Err(描述)。
pub fn validate_guards(req: &CanonicalRequest) -> Result<(), String> {
    let mut tool_args_bytes = 0usize;
    for m in &req.messages {
        if m.blocks.len() > MAX_BLOCKS_PER_REQUEST {
            return Err(format!(
                "单条消息内容块超过 {} 个上限（护栏）",
                MAX_BLOCKS_PER_REQUEST
            ));
        }
        for b in &m.blocks {
            if let Block::ToolUse { input, .. } = b {
                tool_args_bytes += serde_json::to_string(input).unwrap_or_default().len();
                if tool_args_bytes > MAX_TOTAL_TOOL_ARGS_BYTES {
                    return Err(format!(
                        "工具参数累计超过 {} 字节上限（护栏）",
                        MAX_TOTAL_TOOL_ARGS_BYTES
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req_with_messages() -> CanonicalRequest {
        CanonicalRequest {
            model: "m".into(),
            system: vec![],
            messages: vec![
                CanonMessage::text(Role::User, "a"),
                CanonMessage::text(Role::User, "b"),
                CanonMessage::text(Role::Assistant, "c"),
                CanonMessage::text(Role::Assistant, "d"),
            ],
            tools: vec![],
            tool_choice: ToolChoice::Auto,
            params: SampleParams::default(),
            stream: false,
            extensions: Map::new(),
        }
    }

    #[test]
    fn merge_adjacent_keeps_order_and_roles() {
        let req = merge_adjacent_same_role(&req_with_messages());
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[0].role, Role::User);
        assert_eq!(req.messages[0].blocks.len(), 2);
        assert_eq!(req.messages[1].role, Role::Assistant);
        assert_eq!(req.messages[1].blocks.len(), 2);
    }

    #[test]
    fn guards_pass_and_fail() {
        let ok = req_with_messages();
        assert!(validate_guards(&ok).is_ok());

        // blocks 超限
        let mut too_many = ok.clone();
        too_many.messages[0].blocks =
            vec![Block::Text { text: "x".into() }; MAX_BLOCKS_PER_REQUEST + 1];
        assert!(validate_guards(&too_many).is_err());

        // 工具参数超限
        let mut too_big = req_with_messages();
        too_big.messages[0].blocks = vec![Block::ToolUse {
            id: "t1".into(),
            name: "f".into(),
            input: json!({"big": "x".repeat(MAX_TOTAL_TOOL_ARGS_BYTES + 1)}),
        }];
        assert!(validate_guards(&too_big).is_err());
    }

    #[test]
    fn guards_count_per_message_not_request() {
        // agent 长会话历史重放：多条消息各 40 块、累计远超 64，不应被全局累计误伤
        let mut req = req_with_messages();
        for m in &mut req.messages {
            m.blocks = vec![Block::Text { text: "x".into() }; MAX_BLOCKS_PER_REQUEST - 24];
        }
        assert_eq!(req.messages[0].blocks.len(), 40);
        assert_eq!(req.messages.len(), 4);
        assert!(validate_guards(&req).is_ok(), "跨消息累计不应触发护栏");

        // 单条消息仍受 64 上限约束
        req.messages[0].blocks = vec![Block::Text { text: "x".into() }; MAX_BLOCKS_PER_REQUEST + 1];
        assert!(validate_guards(&req).is_err());
    }

    #[test]
    fn extension_note_lists_fields() {
        let mut req = req_with_messages();
        req.extensions.insert("foo".into(), Value::Null);
        let note = extension_warn_note(&req).expect("有扩展字段");
        assert!(note.contains("foo"));
        assert!(note.contains("Lenient"));
    }

    #[test]
    fn anthropic_tool_id_base58_roundtrip() {
        for id in [
            "call_abc123",
            "call-9",
            "fn_12345678901234567890",
            "奇怪_id-符号",
        ] {
            let encoded = canonical_to_anthropic_id(id);
            assert!(encoded.starts_with("toolu_"), "编码应带前缀: {encoded}");
            assert!(
                encoded.len() <= 64,
                "典型 id 不应超过 Anthropic 64 上限: {encoded}"
            );
            assert_eq!(
                decode_anthropic_tool_id(&encoded).as_deref(),
                Some(id),
                "base58 反解应还原原始 id"
            );
            assert_eq!(
                canonical_to_anthropic_id(&encoded),
                encoded,
                "已编码 id 应幂等"
            );
        }
    }

    #[test]
    fn anthropic_tool_id_native_prefix_fallback() {
        // Anthropic 原生 id 不带 jai1 magic：反解保持后缀，不破坏关联
        assert_eq!(
            decode_anthropic_tool_id("toolu_01ABCxYz").as_deref(),
            Some("01ABCxYz")
        );
        assert_eq!(
            canonical_to_anthropic_id("toolu_01ABCxYz"),
            "toolu_01ABCxYz"
        );
    }
}
