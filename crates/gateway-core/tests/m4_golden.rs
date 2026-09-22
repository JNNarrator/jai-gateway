//! M4 黄金夹具属性测试（roadmap M4 验收 1 的转化形态）：
//! `decode_request(OpenAI) → encode_request(Anthropic/Gemini) → 再渲染` 的关键不变性。
//!
//! 由于 Anthropic/Gemini 的 InboundCodec（请求解码）在 M5 才交付，
//! 完整的 decode(encode(ir))==ir 往返在 M5 集成层补全；本文件验证：
//! 1. OpenAI 解码出的 IR 与 Anthropic/Gemini 编码后的语义一致（工具/文本/采样参数）
//! 2. 多轮含 tool_result 的请求通过两族编码不丢消息
//! 3. system 多段在所有出站族都按 IR 契约合并（`\n\n` 连接）

use gateway_core::codec::anthropic as acodec;
use gateway_core::codec::gemini as gcodec;
use gateway_core::codec::ir::{
    Block, CanonMessage, CanonicalRequest, Role, SampleParams, ToolChoice, ToolSpec,
};
use gateway_core::codec::openai;
use serde_json::{json, Value};

/// 一个含 tools + tool_result 的标准多轮请求的 OpenAI 原始 body。
const GOLDEN_OPENAI_BODY: &[u8] = br#"{
    "model":"gpt-4o",
    "messages":[
        {"role":"system","content":"be helpful"},
        {"role":"user","content":"weather in beijing?"},
        {"role":"assistant","content":null,
         "tool_calls":[{"id":"call_1","type":"function",
             "function":{"name":"get_weather","arguments":"{\"city\":\"beijing\"}"}}]},
        {"role":"tool","tool_call_id":"call_1","content":"sunny"}
    ],
    "tools":[{"type":"function","function":{
        "name":"get_weather","description":"Get weather",
        "parameters":{"type":"object","properties":{"city":{"type":"string"}}}
    }}],
    "temperature":0.3,
    "max_tokens":512,
    "stream":false
}"#;

#[test]
fn golden_ir_survives_anthropic_encode() {
    let ir = openai::decode_request(GOLDEN_OPENAI_BODY).unwrap();

    // 工具调用 + 结果块均进入 IR
    assert!(ir.messages.iter().any(|m| m
        .blocks
        .iter()
        .any(|b| matches!(b, Block::ToolUse { name, .. } if name == "get_weather"))));
    assert!(ir.messages.iter().any(|m| m
        .blocks
        .iter()
        .any(|b| matches!(b, Block::ToolResult { call_id, .. } if call_id == "call_1"))));

    // Anthropic 编码
    let enc = acodec::encode_request(&ir).unwrap();
    assert_eq!(enc["model"], "gpt-4o");
    assert_eq!(enc["system"], "be helpful");
    assert_eq!(enc["max_tokens"], 512);
    assert!(enc["messages"].as_array().unwrap().len() >= 2);

    // tool_result 块在场（user 宿主）
    let has_tool_result = enc["messages"].as_array().unwrap().iter().any(|m| {
        m["content"]
            .as_array()
            .map(|a| {
                a.iter()
                    .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
            })
            .unwrap_or(false)
    });
    assert!(has_tool_result, "tool_result 应保留");
}

#[test]
fn golden_ir_survives_gemini_encode() {
    let ir = openai::decode_request(GOLDEN_OPENAI_BODY).unwrap();
    let enc = gcodec::encode_request(&ir).unwrap();

    // systemInstruction 合并
    assert_eq!(enc["systemInstruction"]["parts"][0]["text"], "be helpful");
    // functionDeclarations
    assert_eq!(
        enc["tools"][0]["functionDeclarations"][0]["name"],
        "get_weather"
    );
    // 消息轮次存在（user 含 functionCall + functionResponse 聚合）
    let contents = enc["contents"].as_array().unwrap();
    assert!(!contents.is_empty());
}

#[test]
fn multi_system_segments_merge_in_every_outbound_family() {
    // 2026-09-22：四个 encoder 统一按 IR 契约「多条 system 按序合并，以 \n\n 连接」
    // （ir.rs 的 CanonicalRequest.system 注释）。Anthropic 此前是唯一对 >1 段直接 Err
    // 的族 → 把上游完全能跑的请求在网关侧判成 400「system 段数为 N」。
    // 多段 system 的常规来源是 openai 入站：每条 role=system 消息（以及 system 的
    // 多段 content 数组）都会各成一段。HTTP 级回归用例见
    // tests/m5_anthropic_inbound.rs 的 openai_chat_multi_system_merges_for_anthropic_upstream。
    let ir = CanonicalRequest {
        model: "m".into(),
        system: vec!["sys1".into(), "sys2".into()],
        messages: vec![CanonMessage::text(Role::User, "x")],
        tools: vec![],
        tool_choice: ToolChoice::Auto,
        params: SampleParams::default(),
        stream: false,
        extensions: Default::default(),
    };

    let a = acodec::encode_request(&ir).unwrap();
    assert_eq!(a["system"], "sys1\n\nsys2");

    let g = gcodec::encode_request(&ir).unwrap();
    assert_eq!(g["systemInstruction"]["parts"][0]["text"], "sys1\n\nsys2");

    let o = openai::encode_request(&ir).unwrap();
    assert_eq!(o["messages"][0]["role"], "system");
    assert_eq!(o["messages"][0]["content"], "sys1\n\nsys2");

    let r = gateway_core::codec::responses::encode_request(&ir).unwrap();
    assert_eq!(r["instructions"], "sys1\n\nsys2");

    // 单段保持原样、无 system 时不下发该字段
    let mut one = ir.clone();
    one.system = vec!["only".into()];
    assert_eq!(acodec::encode_request(&one).unwrap()["system"], "only");
    one.system = vec![];
    assert!(acodec::encode_request(&one)
        .unwrap()
        .get("system")
        .is_none());
}

#[test]
fn param_mapping_across_families() {
    let ir = openai::decode_request(
        br#"{"model":"m","messages":[{"role":"user","content":"x"}],
            "temperature":0.9,"top_p":0.8,"stop":["END","STOP"],
            "frequency_penalty":0.5,"seed":42}"#,
    )
    .unwrap();

    let a = acodec::encode_request(&ir).unwrap();
    assert_eq!(a["temperature"], 0.9);
    assert_eq!(a["top_p"], 0.8);
    assert_eq!(a["stop_sequences"], json!(["END", "STOP"]));

    let g = gcodec::encode_request(&ir).unwrap();
    assert_eq!(g["generationConfig"]["temperature"], 0.9);
    assert_eq!(g["generationConfig"]["topP"], 0.8);
    assert_eq!(
        g["generationConfig"]["stopSequences"],
        json!(["END", "STOP"])
    );
    assert_eq!(g["generationConfig"]["seed"], 42);
    // frequency_penalty Gemini 不支持 → 不出现在输出
    assert!(g["generationConfig"].get("frequencyPenalty").is_none());
}

#[test]
fn tool_choice_maps() {
    let mut ir = CanonicalRequest {
        model: "m".into(),
        system: vec![],
        messages: vec![CanonMessage::text(Role::User, "x")],
        tools: vec![ToolSpec {
            name: "f".into(),
            description: None,
            input_schema: json!({"type":"object"}),
        }],
        tool_choice: ToolChoice::Specific("f".into()),
        params: SampleParams::default(),
        stream: false,
        extensions: Default::default(),
    };
    let a = acodec::encode_request(&ir).unwrap();
    assert_eq!(a["tool_choice"]["type"], "tool");
    assert_eq!(a["tool_choice"]["name"], "f");

    let g = gcodec::encode_request(&ir).unwrap();
    assert_eq!(g["toolConfig"]["functionCallingConfig"]["mode"], "ANY");
    assert_eq!(
        g["toolConfig"]["functionCallingConfig"]["allowedFunctionNames"],
        json!(["f"])
    );

    ir.tool_choice = ToolChoice::None;
    let g2 = gcodec::encode_request(&ir).unwrap();
    assert_eq!(g2["toolConfig"]["functionCallingConfig"]["mode"], "NONE");
}
