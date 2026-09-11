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
