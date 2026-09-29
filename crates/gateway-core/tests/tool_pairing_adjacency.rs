//! 工具调用配对**紧邻性**回归：`assistant.tool_calls` 之后必须**立即**是覆盖其全部
//! `tool_call_id` 的 `role=tool` 消息。
//!
//! 上游原文（LiteLLM / OpenAI）：
//! `An assistant message with 'tool_calls' must be followed by tool messages responding
//!  to each 'tool_call_id'. (insufficient tool messages following tool_calls message)`
//!
//! 旧缺陷：`openai::encode_request` 处理「工具结果内嵌图片」时，把降级提升的 user 消息
//! **插在了同一轮的两条 tool 消息之间**（tool(A) → user(提升图) → tool(B)），
//! 于是上游只看到 A 被回应、B 悬空 → 400「insufficient tool messages」。
//! 触发条件是 dsh 的常态：一步里**并行调用多个工具**（如 `read_image` + `bash`），
//! 且图片结果**不是最后一个**。

use gateway_core::codec::anthropic as anthropic_codec;
use gateway_core::codec::ir::CanonicalRequest;
use gateway_core::codec::openai as openai_codec;
use gateway_core::codec::responses as responses_codec;
use serde_json::{json, Value};

const JPEG_B64: &str = "aGVsbG8tdmlzaW9u";

/// 严格判据：每个带 `tool_calls` 的 assistant 消息后面**紧邻**的 tool 消息块
/// 必须恰好覆盖它的全部 id（数量与集合都相等）。任何 user/system/assistant 消息
/// 夹在中间都算违约 —— 这正是上游 400 的判据。
fn assert_strict_pairing(msgs: &[Value], tag: &str) {
    for (i, m) in msgs.iter().enumerate() {
        if m["role"] != "assistant" {
            continue;
        }
        let Some(tcs) = m.get("tool_calls").and_then(Value::as_array) else {
            continue;
        };
        let want: Vec<String> = tcs
            .iter()
            .filter_map(|t| t["id"].as_str().map(String::from))
            .collect();
        let mut got: Vec<String> = Vec::new();
        let mut j = i + 1;
        while j < msgs.len() && msgs[j]["role"] == "tool" {
            got.push(msgs[j]["tool_call_id"].as_str().unwrap_or("").into());
            j += 1;
        }
        let covered = got.len() == want.len() && want.iter().all(|w| got.contains(w));
        assert!(
            covered,
            "[{tag}] assistant[{i}] tool_calls={want:?} 之后紧邻的 tool 消息 = {got:?}\n\
             夹在中间的首条消息 role = {:?}\n完整 messages = {}",
            msgs.get(i + 1).map(|m| m["role"].clone()),
            serde_json::to_string(msgs).unwrap()
        );
    }
}

fn encode(req: &CanonicalRequest) -> Vec<Value> {
    let v = openai_codec::encode_request(req).unwrap();
    v["messages"].as_array().unwrap().clone()
}

/// 生产现场形状（dsh 实测 2026-09-29 16:08）：一步内并行两个工具，
/// **带图的那个在前**，两个结果都回传。
#[test]
fn parallel_tool_calls_with_image_in_first_result_keeps_adjacency() {
    let body = json!({
        "model": "基元律动/deepseek-flash",
        "input": [
            {"role": "user", "content": [{"type": "input_text", "text": "看下截图，再跑个命令"}]},
            {"type": "function_call", "call_id": "call_00_read", "name": "read_image",
             "arguments": "{\"path\":\"/tmp/a.png\"}"},
            {"type": "function_call", "call_id": "call_01_bash", "name": "bash",
             "arguments": "{\"command\":\"ls\"}"},
            {"type": "function_call_output", "call_id": "call_00_read", "output": [
                {"type": "input_text", "text": "截图如下"},
                {"type": "input_image", "image_url": format!("data:image/jpeg;base64,{JPEG_B64}")}
            ]},
            {"type": "function_call_output", "call_id": "call_01_bash", "output": "a.png\n"}
        ]
    });
    let req = responses_codec::decode_request(&serde_json::to_vec(&body).unwrap()).unwrap();
    let msgs = encode(&req);
    assert_strict_pairing(&msgs, "并行工具 + 首个结果带图");
    // 图片不得因为重排而丢失
    assert!(
        msgs.iter()
            .any(|m| serde_json::to_string(m).unwrap().contains("image_url")),
        "提升的图片不能在重排中丢失: {msgs:?}"
    );
}

/// 同一 user 消息里 tool_result 之后跟文本（Anthropic 合法形态，Claude Code 常见）。
#[test]
fn tool_result_and_text_in_one_user_message_keeps_adjacency() {
    let body = json!({
        "model": "claude-sonnet-4",
        "messages": [
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_1", "name": "screenshot", "input": {}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": [
                    {"type": "text", "text": "截图如下"},
                    {"type": "image",
                     "source": {"type": "base64", "media_type": "image/jpeg", "data": JPEG_B64}}
                ]},
                {"type": "text", "text": "顺便看看右上角"}
            ]}
        ]
    });
    let req = anthropic_codec::decode_request(&serde_json::to_vec(&body).unwrap()).unwrap();
    let msgs = encode(&req);
    assert_strict_pairing(&msgs, "tool_result + 文本同消息");
}

/// Responses 入站：工具结果之前夹了一条 user 文本（回合中插话）。
#[test]
fn user_text_between_call_and_output_keeps_adjacency() {
    let body = json!({
        "model": "dsh-model",
        "input": [
            {"type": "function_call", "call_id": "call_1", "name": "bash", "arguments": "{}"},
            {"role": "user", "content": [{"type": "input_text", "text": "补充说明"}]},
            {"type": "function_call_output", "call_id": "call_1", "output": "done"}
        ]
    });
    let req = responses_codec::decode_request(&serde_json::to_vec(&body).unwrap()).unwrap();
    let msgs = encode(&req);
    assert_strict_pairing(&msgs, "call 与 output 之间插了 user 文本");
}

/// 已经合法的序列不得被修复逻辑改动（幂等 / 不误伤）。
#[test]
fn already_valid_sequence_is_untouched() {
    let body = json!({
        "model": "dsh-model",
        "input": [
            {"role": "user", "content": [{"type": "input_text", "text": "读文件"}]},
            {"type": "function_call", "call_id": "call_1", "name": "read", "arguments": "{}"},
            {"type": "function_call", "call_id": "call_2", "name": "read", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "call_1", "output": "a"},
            {"type": "function_call_output", "call_id": "call_2", "output": "b"},
            {"role": "user", "content": [{"type": "input_text", "text": "继续"}]}
        ]
    });
    let req = responses_codec::decode_request(&serde_json::to_vec(&body).unwrap()).unwrap();
    let msgs = encode(&req);
    assert_strict_pairing(&msgs, "已合法");
    let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(
        roles,
        vec!["user", "assistant", "tool", "tool", "user"],
        "合法序列不应被改动: {msgs:?}"
    );
}
