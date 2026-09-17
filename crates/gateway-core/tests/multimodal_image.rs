//! 多模态（Vision）跨族图片链路组合测试 —— docs/design/multimodal-support.md 配套。
//!
//! 不起网络：直接组合各 codec 纯函数，验证同一张 base64 jpeg 图片
//! 在 入站解码 → IR → 各族出站编码 的转换中 media_type 与载荷不失真。
//!
//! 链路：
//! 1. OpenAI chat 入站（data URL jpeg）→ Anthropic 出站（base64 source）
//! 2. OpenAI chat 入站 → Gemini 出站（inlineData mimeType）
//! 3. OpenAI chat 入站 → OpenAI 出站（还原 data URL，media_type 保留）
//! 4. Anthropic 入站（Claude Code 图片）→ OpenAI 出站
//! 5. Anthropic 入站 → Gemini 出站

use gateway_core::codec::anthropic as anthropic_codec;
use gateway_core::codec::gemini as gemini_codec;
use gateway_core::codec::ir::{Block, CanonicalRequest};
use gateway_core::codec::openai as openai_codec;
use gateway_core::codec::responses as responses_codec;
use serde_json::{json, Value};

const JPEG_B64: &str = "aGVsbG8tdmlzaW9u"; // "hello-vision"

fn openai_inbound() -> CanonicalRequest {
    let body = json!({
        "model": "gpt-4o",
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": "这张图里有什么？"},
                {"type": "image_url",
                 "image_url": {"url": format!("data:image/jpeg;base64,{JPEG_B64}")}}
            ]
        }]
    });
    let bytes = serde_json::to_vec(&body).unwrap();
    openai_codec::decode_request(&bytes).unwrap()
}

fn first_image_of(blocks: &[Block]) -> &Block {
    blocks
        .iter()
        .find(|b| matches!(b, Block::Image { .. }))
        .expect("应含 Image 块")
}

#[test]
fn openai_inbound_to_anthropic_outbound_keeps_media_type() {
    let req = openai_inbound();
    let v = anthropic_codec::encode_request(&req).unwrap();
    let content = &v["messages"][0]["content"];
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "这张图里有什么？");
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["source"]["type"], "base64");
    assert_eq!(content[1]["source"]["media_type"], "image/jpeg");
    assert_eq!(content[1]["source"]["data"], JPEG_B64);
}

#[test]
fn openai_inbound_to_gemini_outbound_keeps_media_type() {
    let req = openai_inbound();
    let v = gemini_codec::encode_request(&req).unwrap();
    let parts = &v["contents"][0]["parts"];
    assert_eq!(parts[0]["text"], "这张图里有什么？");
    assert_eq!(parts[1]["inlineData"]["mimeType"], "image/jpeg");
    assert_eq!(parts[1]["inlineData"]["data"], JPEG_B64);
}

#[test]
fn openai_inbound_to_openai_outbound_restores_data_url() {
    let req = openai_inbound();
    let v = openai_codec::encode_request(&req).unwrap();
    let msgs = v["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1, "同轮 text+image 应合为一条消息");
    let content = msgs[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[1]["type"], "image_url");
    assert_eq!(
        content[1]["image_url"]["url"],
        format!("data:image/jpeg;base64,{JPEG_B64}"),
        "media_type 应原样还原，不得退化为 png"
    );
}

#[test]
fn anthropic_inbound_image_to_openai_and_gemini_outbound() {
    // Claude Code 形状入站：base64 图 + http url 图
    let body = json!({
        "model": "claude-sonnet-4",
        "messages": [{
            "role": "user",
            "content": [
                {"type": "image",
                 "source": {"type": "base64", "media_type": "image/webp", "data": JPEG_B64}},
                {"type": "image",
                 "source": {"type": "url", "url": "https://x.com/a.png"}}
            ]
        }]
    });
    let bytes = serde_json::to_vec(&body).unwrap();
    let req = anthropic_codec::decode_request(&bytes).unwrap();
    let blocks = &req.messages[0].blocks;
    match first_image_of(blocks) {
        Block::Image {
            media_type,
            data_base64,
            ..
        } => {
            assert_eq!(media_type, "image/webp");
            assert_eq!(data_base64.as_deref(), Some(JPEG_B64));
        }
        other => panic!("期望 Image 块: {other:?}"),
    }

    // → OpenAI 出站：base64 块还原为带真实 media_type 的 data URL
    let v = openai_codec::encode_request(&req).unwrap();
    let content = v["messages"][0]["content"].as_array().unwrap();
    assert_eq!(
        content[0]["image_url"]["url"],
        format!("data:image/webp;base64,{JPEG_B64}")
    );
    // → Gemini 出站：inlineData mimeType 保留（url 块在 proxy 层先转 base64，
    //   纯编码场景 data_base64 缺省不产出 inlineData，不臆造）
    let v = gemini_codec::encode_request(&req).unwrap();
    let parts = v["contents"][0]["parts"].as_array().unwrap();
    let inline: Vec<&Value> = parts
        .iter()
        .filter(|p| p.get("inlineData").is_some())
        .collect();
    assert_eq!(inline.len(), 1, "仅 base64 块产出 inlineData");
    assert_eq!(inline[0]["inlineData"]["mimeType"], "image/webp");
    assert_eq!(inline[0]["inlineData"]["data"], JPEG_B64);
}

#[test]
fn anthropic_inbound_to_anthropic_outbound_image_roundtrip() {
    // Anthropic 入站 → Anthropic 出站（同族转换路径）：media_type 与载荷不丢
    let body = json!({
        "model": "claude-sonnet-4",
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": "hi"},
                {"type": "image",
                 "source": {"type": "base64", "media_type": "image/png", "data": JPEG_B64}}
            ]
        }]
    });
    let bytes = serde_json::to_vec(&body).unwrap();
    let req = anthropic_codec::decode_request(&bytes).unwrap();
    let v = anthropic_codec::encode_request(&req).unwrap();
    let content = v["messages"][0]["content"].as_array().unwrap();
    assert_eq!(content[1]["source"]["media_type"], "image/png");
    assert_eq!(content[1]["source"]["data"], JPEG_B64);
}

// ================================================================
// 工具结果内嵌图片（tool_result / function_call_output）
//
// 与上面的「消息级」图片是两条独立链路：agent 类客户端（Claude Code、
// dsh、Codex）的截图/图表类工具会把图片放进**工具结果**里，而不是用户消息。
// 各族的编解码器都必须保住它——旧实现只取文本字段（`filter_map(as_text)`），
// 图片会被无声吃掉，客户端表现为「工具返回了图，模型却说没看到」。

/// 取出唯一 ToolResult 块的 content。
fn tool_result_content(req: &CanonicalRequest) -> &[Block] {
    req.messages
        .iter()
        .flat_map(|m| m.blocks.iter())
        .find_map(|b| match b {
            Block::ToolResult { content, .. } => Some(content.as_slice()),
            _ => None,
        })
        .expect("应含 ToolResult 块")
}

/// 断言若干块中有一块是「指定 media_type + 载荷不丢」的图片。
fn assert_image_kept(blocks: &[Block], media_type: &str) {
    let hit = blocks.iter().any(|b| {
        matches!(
            b,
            Block::Image {
                media_type: m,
                data_base64: Some(d),
                ..
            } if m == media_type && d == JPEG_B64
        )
    });
    assert!(
        hit,
        "应含 media_type={media_type} 且载荷完整的 Image 块，实际: {blocks:?}"
    );
}

#[test]
fn anthropic_inbound_tool_result_keeps_image() {
    // Claude Code 形状：工具结果内容块数组里含图片
    let body = json!({
        "model": "claude-sonnet-4",
        "messages": [{
            "role": "user",
            "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": [
                    {"type": "text", "text": "截图如下"},
                    {"type": "image",
                     "source": {"type": "base64", "media_type": "image/jpeg", "data": JPEG_B64}}
                ]}
            ]
        }]
    });
    let bytes = serde_json::to_vec(&body).unwrap();
    let req = anthropic_codec::decode_request(&bytes).unwrap();
    let content = tool_result_content(&req);
    assert!(
        content.iter().any(|b| b.as_text() == Some("截图如下")),
        "文本块不应丢"
    );
    assert_image_kept(content, "image/jpeg");
}

#[test]
fn openai_inbound_tool_message_array_content_keeps_image() {
    // 部分客户端/中转把 role=tool 的 content 发成内容数组（可含图片）
    let body = json!({
        "model": "gpt-4o",
        "messages": [
            {"role": "assistant", "content": null, "tool_calls": [
                {"id": "call_1", "type": "function",
                 "function": {"name": "screenshot", "arguments": "{}"}}
            ]},
            {"role": "tool", "tool_call_id": "call_1", "content": [
                {"type": "text", "text": "截图如下"},
                {"type": "image_url",
                 "image_url": {"url": format!("data:image/jpeg;base64,{JPEG_B64}")}}
            ]}
        ]
    });
    let bytes = serde_json::to_vec(&body).unwrap();
    let req = openai_codec::decode_request(&bytes).unwrap();
    let content = tool_result_content(&req);
    assert!(
        content.iter().any(|b| b.as_text() == Some("截图如下")),
        "数组形状不应整条被抹成空串，实际: {content:?}"
    );
    assert_image_kept(content, "image/jpeg");
}

#[test]
fn responses_inbound_function_call_output_array_keeps_image() {
    // dsh/Codex 形状：function_call_output.output 为内容项数组，含 input_image
    let body = json!({
        "model": "gpt-5",
        "input": [
            {"type": "function_call_output", "call_id": "call_1", "output": [
                {"type": "input_text", "text": "截图如下"},
                {"type": "input_image",
                 "image_url": format!("data:image/jpeg;base64,{JPEG_B64}")}
            ]}
        ]
    });
    let bytes = serde_json::to_vec(&body).unwrap();
    let req = responses_codec::decode_request(&bytes).unwrap();
    let content = tool_result_content(&req);
    assert!(
        content.iter().any(|b| b.as_text() == Some("截图如下")),
        "文本项不应丢，实际: {content:?}"
    );
    assert_image_kept(content, "image/jpeg");
}

#[test]
fn responses_inbound_input_image_data_url_parses_media_type() {
    // 回归：data URL 必须拆成 media_type + 载荷，不得整串塞进 url
    // （否则跨族出站会把它当远程 URL 发给上游，Anthropic/Gemini 会拒）
    let body = json!({
        "model": "gpt-5",
        "input": [{
            "type": "message",
            "role": "user",
            "content": [
                {"type": "input_text", "text": "这是什么？"},
                {"type": "input_image",
                 "image_url": format!("data:image/webp;base64,{JPEG_B64}")}
            ]
        }]
    });
    let bytes = serde_json::to_vec(&body).unwrap();
    let req = responses_codec::decode_request(&bytes).unwrap();
    let blocks = &req.messages[0].blocks;
    assert_image_kept(blocks, "image/webp");
    assert!(
        !blocks.iter().any(|b| matches!(
            b,
            Block::Image { url: Some(u), .. } if u.starts_with("data:")
        )),
        "data URL 不得被当成远程 URL 透传，实际: {blocks:?}"
    );
}

// ---------------- 出站：工具结果内嵌图片不得被丢弃 ----------------

/// Anthropic 入站（工具结果含图）→ IR，供出站侧复用。
fn anthropic_inbound_with_tool_result_image() -> CanonicalRequest {
    let body = json!({
        "model": "claude-sonnet-4",
        "messages": [{
            "role": "user",
            "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": [
                    {"type": "text", "text": "截图如下"},
                    {"type": "image",
                     "source": {"type": "base64", "media_type": "image/jpeg", "data": JPEG_B64}}
                ]}
            ]
        }]
    });
    let bytes = serde_json::to_vec(&body).unwrap();
    anthropic_codec::decode_request(&bytes).unwrap()
}

#[test]
fn anthropic_outbound_tool_result_keeps_image() {
    // 同族转换路径必须与字节级直通等价：tool_result.content 里保留 image 块
    let req = anthropic_inbound_with_tool_result_image();
    let v = anthropic_codec::encode_request(&req).unwrap();
    let content = v["messages"][0]["content"].as_array().unwrap();
    let tr = &content[0];
    assert_eq!(tr["type"], "tool_result");
    let inner = tr["content"].as_array().unwrap();
    assert_eq!(inner[0]["type"], "text");
    assert_eq!(inner[0]["text"], "截图如下");
    assert_eq!(inner[1]["type"], "image", "工具结果内的图片不得被丢弃");
    assert_eq!(inner[1]["source"]["type"], "base64");
    assert_eq!(inner[1]["source"]["media_type"], "image/jpeg");
    assert_eq!(inner[1]["source"]["data"], JPEG_B64);
}

#[test]
fn responses_outbound_tool_result_keeps_image() {
    // Responses 出站：output 需改为内容项数组才能承载图片
    let req = anthropic_inbound_with_tool_result_image();
    let v = responses_codec::encode_request(&req).unwrap();
    let fco = v["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["type"] == "function_call_output")
        .expect("应产出 function_call_output");
    let output = fco["output"]
        .as_array()
        .expect("含图片时 output 应为内容项数组");
    assert_eq!(output[0]["type"], "input_text");
    assert_eq!(output[0]["text"], "截图如下");
    assert_eq!(output[1]["type"], "input_image");
    assert_eq!(
        output[1]["image_url"],
        format!("data:image/jpeg;base64,{JPEG_B64}"),
        "media_type 应原样还原，不得退化为 png"
    );
}

#[test]
fn responses_outbound_text_only_tool_result_stays_string() {
    // 无图片时保持既有的字符串形状，避免无谓改变上游请求体口径
    let body = json!({
        "model": "claude-sonnet-4",
        "messages": [{
            "role": "user",
            "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "sunny"}
            ]
        }]
    });
    let bytes = serde_json::to_vec(&body).unwrap();
    let req = anthropic_codec::decode_request(&bytes).unwrap();
    let v = responses_codec::encode_request(&req).unwrap();
    let fco = v["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["type"] == "function_call_output")
        .expect("应产出 function_call_output");
    assert_eq!(fco["output"], json!("sunny"), "纯文本应仍为字符串形状");
}

// ---------------- 出站：无法原生承载的族 → 降级提升为相邻 user 消息 ----------------

/// 带真实工具调用轮的 IR：assistant tool_use + user tool_result（含图）。
fn anthropic_inbound_with_tool_call_and_image_result() -> CanonicalRequest {
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
                ]}
            ]}
        ]
    });
    let bytes = serde_json::to_vec(&body).unwrap();
    anthropic_codec::decode_request(&bytes).unwrap()
}

#[test]
fn openai_outbound_tool_result_image_is_hoisted_to_user_message() {
    // OpenAI chat 规范：tool 消息只支持 text part → 图片必须提升为紧随其后的 user 消息
    let req = anthropic_inbound_with_tool_call_and_image_result();
    let v = openai_codec::encode_request(&req).unwrap();
    let msgs = v["messages"].as_array().unwrap();
    let idx = msgs
        .iter()
        .position(|m| m["role"] == "tool")
        .expect("应产出 role=tool 消息");
    assert!(
        msgs[idx]["content"].as_str().unwrap().contains("截图如下"),
        "tool 消息应保留文本"
    );
    let next = &msgs[idx + 1];
    assert_eq!(next["role"], "user", "图片应提升为紧随其后的 user 消息");
    let img = next["content"]
        .as_array()
        .expect("提升消息应为多模态数组")
        .iter()
        .find(|c| c["type"] == "image_url")
        .expect("应含提升的图片 part");
    assert_eq!(
        img["image_url"]["url"],
        format!("data:image/jpeg;base64,{JPEG_B64}"),
        "media_type 应原样还原"
    );
}

#[test]
fn gemini_outbound_tool_result_keeps_image_in_function_response_parts() {
    // Gemini v1beta 的 functionResponse.parts 可承载 inlineData（JAI 出站固定打 v1beta）
    let req = anthropic_inbound_with_tool_call_and_image_result();
    let v = gemini_codec::encode_request(&req).unwrap();
    let fr = v["contents"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["parts"].as_array().unwrap())
        .find(|p| p.get("functionResponse").is_some())
        .expect("应产出 functionResponse");
    assert_eq!(fr["functionResponse"]["name"], "screenshot");
    assert_eq!(fr["functionResponse"]["response"]["result"], "截图如下");
    assert_eq!(
        fr["functionResponse"]["parts"][0]["inlineData"]["mimeType"],
        "image/jpeg"
    );
    assert_eq!(
        fr["functionResponse"]["parts"][0]["inlineData"]["data"],
        JPEG_B64
    );
}

#[test]
fn capability_warns_only_for_families_without_native_tool_result_images() {
    // 降级必须"可见"：装不下图的那一族要有 CapabilityWarn，能原生承载的族不得误报
    use gateway_core::codec::capability::{caps_of, plan_compatibility};
    use gateway_core::codec::Family;

    for (family, expect_warn) in [
        (Family::OpenAiCompat, true),
        (Family::Anthropic, false),
        (Family::OpenAiResponses, false),
        (Family::Gemini, false),
    ] {
        let mut req = anthropic_inbound_with_tool_call_and_image_result();
        let outcome = plan_compatibility(&req, caps_of(family)).resolve(&mut req);
        assert_eq!(
            !outcome.warnings.is_empty(),
            expect_warn,
            "{family:?} 的降级告警与能力面不符，warnings={:?}",
            outcome.warnings
        );
    }
}

// ── bug 7：Responses 出站此前丢弃「消息级」图片（只修了工具结果内嵌图片）──

/// 取出 Responses 出站 body 里第一条 message 的 content 数组。
fn responses_first_message_content(v: &Value) -> &Vec<Value> {
    v["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["type"] == "message")
        .expect("应产出 message")["content"]
        .as_array()
        .expect("content 应为数组")
}

#[test]
fn responses_outbound_user_message_keeps_image_in_block_order() {
    // OpenAI chat 入站（text + data URL 图）→ Responses 出站：
    // 图片必须原生承载为 `input_image`，且块序与入站一致（文本在前）。
    let req = openai_inbound();
    let v = responses_codec::encode_request(&req).unwrap();
    let content = responses_first_message_content(&v);

    assert_eq!(
        content.len(),
        2,
        "text + image 应各占一个内容项: {content:?}"
    );
    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(content[0]["text"], "这张图里有什么？");
    assert_eq!(
        content[1]["type"], "input_image",
        "用户消息里的图片不得被丢弃（bug 7）"
    );
    assert_eq!(
        content[1]["image_url"],
        format!("data:image/jpeg;base64,{JPEG_B64}"),
        "media_type 应原样还原，不得退化为 png"
    );
}

#[test]
fn responses_outbound_image_only_user_message_is_not_dropped() {
    // 只有图片、没有文本的用户消息：旧实现连 message 都不产出（整轮图丢失）。
    // 同时覆盖 http url 形式的图片块（不臆造 base64）。
    let body = json!({
        "model": "claude-sonnet-4",
        "messages": [{
            "role": "user",
            "content": [
                {"type": "image",
                 "source": {"type": "base64", "media_type": "image/webp", "data": JPEG_B64}},
                {"type": "image",
                 "source": {"type": "url", "url": "https://x.com/a.png"}}
            ]
        }]
    });
    let bytes = serde_json::to_vec(&body).unwrap();
    let req = anthropic_codec::decode_request(&bytes).unwrap();
    let v = responses_codec::encode_request(&req).unwrap();
    let content = responses_first_message_content(&v);

    assert_eq!(
        content.len(),
        2,
        "两张图应产出两个 input_image: {content:?}"
    );
    assert_eq!(content[0]["type"], "input_image");
    assert_eq!(
        content[0]["image_url"],
        format!("data:image/webp;base64,{JPEG_B64}")
    );
    assert_eq!(content[1]["type"], "input_image");
    assert_eq!(content[1]["image_url"], "https://x.com/a.png");
}

#[test]
fn responses_outbound_text_only_user_message_shape_unchanged() {
    // 无图片时保持既有形状（一段文本一个 input_text 项），避免无谓改变上游请求体口径
    let body = json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "只问一句"}]
    });
    let bytes = serde_json::to_vec(&body).unwrap();
    let req = openai_codec::decode_request(&bytes).unwrap();
    let v = responses_codec::encode_request(&req).unwrap();
    let content = responses_first_message_content(&v);

    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "input_text");
    assert_eq!(content[0]["text"], "只问一句");
}
