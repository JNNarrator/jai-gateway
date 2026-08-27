//! Anthropic 族编解码助手（M3 范围：直通路径旁路 + count_tokens + 错误形状）。
//!
//! 与 `openai` 模块对称：直通模式 body 字节不改动，因此只做三件事：
//! - [`peek`]：轻量解析请求关键字段（model/stream）用于路由与日志
//! - [`count_tokens`]：粗估 input token 数（roadmap M3 验收 3：
//!   `ceil(chars/4)` + system/tools 开销常量，避免 Claude Code 降级）
//! - [`error_body`]：Anthropic 错误响应形状

use serde_json::{Value, json};

/// Anthropic API 默认版本头（缺省注入）。
pub const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";

/// 从 Anthropic 请求体提取路由所需最小字段。
#[derive(Debug, Clone)]
pub struct PeekRequest {
    pub model: String,
    pub stream: bool,
}

pub fn peek(body: &[u8]) -> Result<PeekRequest, String> {
    let v: Value =
        serde_json::from_slice(body).map_err(|e| format!("请求体不是合法 JSON: {e}"))?;
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
    let v: Value =
        serde_json::from_slice(body).map_err(|e| format!("请求体不是合法 JSON: {e}"))?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peek_parses_model_and_stream() {
        let p = peek(
            br#"{"model":"claude-sonnet-4","stream":true,"max_tokens":1024,"messages":[]}"#,
        )
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
}