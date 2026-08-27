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
    Ok(PeekRequest {
        model,
        stream: v.get("stream").and_then(Value::as_bool).unwrap_or(false),
    })
}

// ================================================================ usage 扫描

/// 流式字节中的 usage 抽取器。
///
/// 策略：滚动缓冲区搜索 `"usage"` 关键字 → 定位首个 `{` → 括号配对（感知字符串）
/// 截取完整对象 → 首次命中即锁定。非流式响应可整体 feed 后调 [`Self::finish`]。
#[derive(Default)]
pub struct UsageScanner {
    buf: Vec<u8>,
    scan_from: usize,
    collect_start: Option<usize>, // Some(i): 正在收集，i 为 '{' 在 buf 内位置
    depth: usize,
    in_string: bool,
    escaped: bool,
    captured: Option<String>,
}

const WINDOW_KEEP: usize = 4096;
const CAPTURE_CAP: usize = 8192;

impl UsageScanner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);

        loop {
            // 收集中：推进括号配对
            if let Some(start) = self.collect_start {
                let had_progress = self.scan_from < self.buf.len();
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
                if self.collect_start.is_some() && !had_progress {
                    // 本次没有新增可消费字节且仍未结束：等待下一个 chunk
                }
                self.compact();
                return;
            }

            // 寻找下一个 "usage" 关键字
            if let Some(pos) =
                find_subslice(&self.buf[self.scan_from.min(self.buf.len())..], b"\"usage\"")
            {
                let key_at = self.scan_from + pos;
                self.scan_from = key_at + b"\"usage\"".len();
                if let Some(rel_brace) = self.buf[self.scan_from..]
                    .iter()
                    .take(32)
                    .position(|&b| b == b'{')
                {
                    let brace_at = self.scan_from + rel_brace;
                    self.collect_start = Some(brace_at);
                    self.scan_from = brace_at;
                    self.depth = 0;
                    self.in_string = false;
                    self.escaped = false;
                    continue; // 同一 feed 内立即开始收集
                }
            } else {
                // 未找到：保留窗口尾部以处理跨块分割的关键字
                self.scan_from = self.buf.len().saturating_sub(24);
            }
            self.compact();
            return;
        }
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
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
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
        assert_eq!(url_join("https://g.cn", "/v1beta/models"), "https://g.cn/v1beta/models");
    }
}
