//! 诊断工具（默认 `#[ignore]`，不进回归）：**Responses 入站 → chat/completions 出站**
//! 的转换结果肉眼可查。
//!
//! 用途：客户端（Reasonix / Codex / dsh）报「答非所问」「上下文被插了东西」时，
//! 先在这里把同一份 `input` 走一遍 `decode_request` + `openai::encode_request`，
//! 直接看**发给上游的 messages 数组**长什么样 —— 不必改任何线上配置。
//!
//! 跑法：
//! ```bash
//! # 内置的「Reasonix 形状」样例
//! cargo test --test diag_responses_conversion -- --ignored --nocapture
//! # 或喂真实抓到的请求体
//! JAI_DIAG_PAYLOAD=/tmp/real.json cargo test --test diag_responses_conversion -- --ignored --nocapture
//! ```
//!
//! 检查要点（每条都会打印）：
//! 1. **最后一条消息是不是用户真正的问题**（模型只回答最后一条；错位就会答非所问）
//! 2. system 是否来自 `instructions`
//! 3. 每条 assistant 的 `reasoning_content` 与 `content` 是否分离（混在一起会污染上下文）
//! 4. 工具调用与工具结果是否配对且顺序正确
//! 5. 有没有**多出来的**消息（例如图片降级插入的 user 消息）

use serde_json::{json, Value};

/// 尽量贴近 Reasonix 真实形状的多轮输入：
/// session-context 快照 → reasoning-language 指令 → 用户问题 → 推理/回答/工具 → 追问。
fn sample_reasonix_payload() -> Value {
    json!({
        "model": "基元律动/deepseek-flash",
        "stream": true,
        "instructions": "You are Reasonix, a coding agent. 使用工具完成任务。",
        "input": [
            {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "<session-context version=\"1\">This host-generated snapshot supersedes every earlier message. Workspace: /tmp/demo</session-context>"}
            ]},
            {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "<reasoning-language>必须使用简体中文书写全部可见思考/推理文本。</reasoning-language>"}
            ]},
            {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "第一问：src/main.rs 里的 bug 是什么？"}
            ]},
            {"type": "reasoning", "summary": [
                {"type": "summary_text", "text": "我需要先读文件才能判断。"}
            ]},
            {"type": "message", "role": "assistant", "content": [
                {"type": "output_text", "text": "好的，我先读一下文件。"}
            ]},
            {"type": "function_call", "call_id": "call_1", "name": "read_file",
             "arguments": "{\"path\":\"src/main.rs\"}"},
            {"type": "function_call_output", "call_id": "call_1",
             "output": "fn main() { let x = 1; println!(\"{}\", y); }"},
            {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "第二问：请给出修复补丁。"}
            ]}
        ],
        "tools": [
            {"type": "function", "name": "read_file", "description": "读文件",
             "parameters": {"type": "object",
                            "properties": {"path": {"type": "string"}},
                            "required": ["path"]}}
        ]
    })
}

fn preview(s: &str) -> String {
    let one_line: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    let t: String = one_line.chars().take(70).collect();
    if one_line.chars().count() > 70 {
        format!("{t}…")
    } else {
        t
    }
}

#[test]
#[ignore = "诊断工具，手动运行；见文件头说明"]
fn dump_responses_to_chat_conversion() {
    let payload: Value = match std::env::var("JAI_DIAG_PAYLOAD") {
        Ok(p) => {
            let raw = std::fs::read_to_string(&p).expect("读取 JAI_DIAG_PAYLOAD 失败");
            eprintln!("[diag] 使用外部载荷: {p}");
            serde_json::from_str(&raw).expect("外部载荷不是合法 JSON")
        }
        Err(_) => {
            eprintln!("[diag] 使用内置「Reasonix 形状」样例");
            sample_reasonix_payload()
        }
    };

    let body = serde_json::to_vec(&payload).unwrap();

    let req = gateway_core::codec::responses::decode_request(&body).expect("decode_request 失败");
    let out = gateway_core::codec::openai::encode_request(&req).expect("encode_request 失败");

    let msgs = out["messages"].as_array().expect("出站没有 messages");
    eprintln!();
    eprintln!(
        "================ 发给上游的 messages（共 {} 条）================",
        msgs.len()
    );
    for (i, m) in msgs.iter().enumerate() {
        let role = m["role"].as_str().unwrap_or("?");
        // 内容可能是字符串或数组
        let content_txt = match m.get("content") {
            Some(Value::String(s)) => preview(s),
            Some(Value::Array(parts)) => {
                let joined: Vec<String> = parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(Value::as_str).map(preview))
                    .collect();
                joined.join(" | ")
            }
            _ => String::new(),
        };
        // 推理与工具是**分开**的字段，必须单独看，避免「推理被塞进 content」
        let reasoning = m
            .get("reasoning_content")
            .and_then(Value::as_str)
            .map(preview)
            .unwrap_or_else(|| "—".into());
        let tools = m
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|t| t["function"]["name"].as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_else(|| "—".into());
        let tool_call_id = m.get("tool_call_id").and_then(Value::as_str).unwrap_or("—");

        eprintln!("[{i:02}] role={role}");
        eprintln!("      content        : {content_txt}");
        eprintln!("      reasoning_content: {reasoning}");
        eprintln!("      tool_calls     : {tools}");
        eprintln!("      tool_call_id   : {tool_call_id}");
    }
    eprintln!("==============================================================");
    eprintln!();

    // ---- 自动化检查（打印结论，不 panic，便于人工判断）----
    let last = msgs.last().expect("messages 为空");
    let last_role = last["role"].as_str().unwrap_or("?");
    let last_text = match last.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    };
    let tail: String = {
        let n = last_text.chars().count();
        last_text.chars().skip(n.saturating_sub(30)).collect()
    };

    eprintln!("检查 1 · 最后一条是否为「用户真正的问题」");
    eprintln!("   role = {last_role}");
    eprintln!("   尾部 = …{tail}");
    if last_role == "user" && last_text.contains("第二问") {
        eprintln!("   ✅ 最后一条是用户的追问（模型会回答它）");
    } else {
        eprintln!("   ❌ 最后一条**不是**用户的追问 —— 模型会答错内容（答非所问）");
    }

    eprintln!();
    eprintln!("检查 2 · system 是否来自 instructions");
    let first_role = msgs[0]["role"].as_str().unwrap_or("?");
    if first_role == "system" {
        eprintln!("   ✅ 首条是 system");
    } else {
        eprintln!("   ❌ 首条不是 system，而是 {first_role}");
    }

    eprintln!();
    eprintln!("检查 3 · 推理与可见内容是否分离");
    let mixed = msgs.iter().any(|m| {
        m["role"].as_str() == Some("assistant")
            && m.get("reasoning_content").is_none()
            && m.get("content")
                .and_then(Value::as_str)
                .map(|s| s.contains("我需要先读文件"))
                .unwrap_or(false)
    });
    if mixed {
        eprintln!("   ❌ 推理文本落进了 content（会把内心独白当成上一句回答）");
    } else {
        eprintln!("   ✅ 推理在 reasoning_content 里，未混入 content");
    }

    eprintln!();
    eprintln!("检查 4 · 工具调用与结果是否配对");
    let calls: Vec<String> = msgs
        .iter()
        .filter_map(|m| m.get("tool_calls").and_then(Value::as_array))
        .flat_map(|a| {
            a.iter()
                .filter_map(|t| t["id"].as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .collect();
    let results: Vec<String> = msgs
        .iter()
        .filter_map(|m| {
            m.get("tool_call_id")
                .and_then(Value::as_str)
                .map(String::from)
        })
        .collect();
    eprintln!("   调用 id  = {calls:?}");
    eprintln!("   结果 id  = {results:?}");
    let ok = calls.len() == results.len() && calls.iter().all(|c| results.iter().any(|r| r == c));
    eprintln!(
        "   {}",
        if ok {
            "✅ 一一配对"
        } else {
            "❌ 不配对（上游可能 400，或模型看到悬空的工具结果）"
        }
    );

    eprintln!();
    eprintln!("检查 5 · 消息条数（有无多出来的插入消息）");
    eprintln!(
        "   入站 input 条目数 = {}",
        payload["input"].as_array().map(|a| a.len()).unwrap_or(0)
    );
    eprintln!("   出站 messages 条数 = {}", msgs.len());
    eprintln!(
        "   （期望：3 条前置 user + 1 轮 assistant + 1 条工具结果 + 1 条追问 + 1 条 system）"
    );
    eprintln!();
}

/// 并行多工具调用（Reasonix 一轮实测 `tool_calls=4`）：最容易出错的地方。
/// 关注：多个 function_call 是否合并进**同一条** assistant、多个 function_call_output
/// 是否各自成为独立的 tool 消息、以及 id 是否一一对应。
fn sample_parallel_tools_payload() -> Value {
    json!({
        "model": "基元律动/deepseek-flash",
        "stream": true,
        "instructions": "You are Reasonix, a coding agent.",
        "input": [
            {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "请同时看这几个文件并给出结论"}
            ]},
            {"type": "reasoning", "summary": [
                {"type": "summary_text", "text": "并行读三个文件。"}
            ]},
            {"type": "function_call", "call_id": "call_a", "name": "read_file",
             "arguments": "{\"path\":\"a.rs\"}"},
            {"type": "function_call", "call_id": "call_b", "name": "read_file",
             "arguments": "{\"path\":\"b.rs\"}"},
            {"type": "function_call", "call_id": "call_c", "name": "bash",
             "arguments": "{\"command\":\"ls\"}"},
            {"type": "function_call_output", "call_id": "call_a", "output": "内容A"},
            {"type": "function_call_output", "call_id": "call_b", "output": "内容B"},
            {"type": "function_call_output", "call_id": "call_c", "output": "内容C"},
            {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "现在总结一下"}
            ]}
        ]
    })
}

#[test]
#[ignore = "诊断工具，手动运行；见文件头说明"]
fn dump_parallel_tool_calls_conversion() {
    let payload = sample_parallel_tools_payload();
    let body = serde_json::to_vec(&payload).unwrap();
    let req = gateway_core::codec::responses::decode_request(&body).expect("decode_request 失败");
    let out = gateway_core::codec::openai::encode_request(&req).expect("encode_request 失败");
    let msgs = out["messages"].as_array().expect("出站没有 messages");

    eprintln!();
    eprintln!(
        "========== 并行多工具调用 → messages（共 {} 条）==========",
        msgs.len()
    );
    for (i, m) in msgs.iter().enumerate() {
        let role = m["role"].as_str().unwrap_or("?");
        let ids: Vec<String> = m
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|t| {
                        format!(
                            "{}:{}",
                            t["id"].as_str().unwrap_or("?"),
                            t["function"]["name"].as_str().unwrap_or("?")
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let tcid = m.get("tool_call_id").and_then(Value::as_str).unwrap_or("—");
        let txt = match m.get("content") {
            Some(Value::String(s)) => preview(s),
            Some(Value::Array(p)) => p
                .iter()
                .filter_map(|x| x.get("text").and_then(Value::as_str).map(preview))
                .collect::<Vec<_>>()
                .join(" | "),
            _ => String::new(),
        };
        eprintln!("[{i:02}] {role:<9} tool_calls={ids:?} tool_call_id={tcid} content={txt}");
    }
    eprintln!("=========================================================");

    // 配对检查
    let calls: Vec<String> = msgs
        .iter()
        .filter_map(|m| m.get("tool_calls").and_then(Value::as_array))
        .flat_map(|a| {
            a.iter()
                .filter_map(|t| t["id"].as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .collect();
    let results: Vec<String> = msgs
        .iter()
        .filter_map(|m| {
            m.get("tool_call_id")
                .and_then(Value::as_str)
                .map(String::from)
        })
        .collect();
    eprintln!("调用 id = {calls:?}");
    eprintln!("结果 id = {results:?}");
    let all_paired = calls.len() == results.len()
        && calls.iter().all(|c| results.contains(c))
        && results.iter().all(|r| calls.contains(r));
    eprintln!(
        "{}",
        if all_paired {
            "✅ 三个调用与三个结果一一配对"
        } else {
            "❌ 调用与结果不配对 —— 上游会 400，或模型看到悬空工具结果（典型「答非所问」诱因）"
        }
    );
    let last = msgs.last().unwrap();
    eprintln!(
        "最后一条 role = {}，内容 = {}",
        last["role"].as_str().unwrap_or("?"),
        preview(
            last.get("content")
                .and_then(Value::as_str)
                .unwrap_or("<非字符串>")
        )
    );
    eprintln!();
}
