//! 图片块构造与 data URL 解析的共享工具。
//!
//! 各族协议都用 `data:` URL 或裸 URL 表达图片，解析口径必须一致：
//! 同一张图若在某族被当成"远程 URL"透传（把整个 `data:image/jpeg;base64,...`
//! 塞进 `url` 字段），上游会直接拒收或静默变空——这是跨族图片链路的
//! 保真底线，因此集中在此，供入站解码与出站编码共用。

use crate::codec::ir::{Block, CanonicalRequest};

/// 解析 `data:image/{type};base64,{payload}` → `(media_type, base64 载荷)`。
pub fn parse_data_url(s: &str) -> Option<(String, String)> {
    let rest = s.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let media_type = meta.strip_suffix(";base64")?.to_string();
    Some((media_type, payload.to_string()))
}

/// 由一个 url 字段构造 Image 块：data URL 拆成 `media_type` + base64 载荷，
/// 否则按远程 URL 透传（media_type 未知时占位 png，与既有口径一致）。
pub fn image_block_from_url(url: &str) -> Block {
    match parse_data_url(url) {
        Some((media_type, data_base64)) => Block::Image {
            media_type,
            data_base64: Some(data_base64),
            url: None,
        },
        None => Block::Image {
            media_type: "image/png".into(),
            data_base64: None,
            url: Some(url.to_string()),
        },
    }
}

/// 提取一批 IR 块中「工具结果内嵌的图片」。
///
/// agent 类客户端（Claude Code / dsh / Codex）的截图、图表类工具会把图片放进
/// **工具结果**里而非用户消息——这是一条与消息级图片独立的链路。出站族若不支持
/// 原生承载（见 `capability.tool_result_images`），调用方需显式降级而非静默丢弃。
pub fn tool_result_images(blocks: &[Block]) -> Vec<&Block> {
    blocks
        .iter()
        .flat_map(|b| match b {
            Block::ToolResult { content, .. } => content
                .iter()
                .filter(|c| matches!(c, Block::Image { .. }))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect()
}

/// 请求中任一工具结果是否带图片（规划层判定降级用）。
pub fn request_has_tool_result_image(req: &CanonicalRequest) -> bool {
    req.messages
        .iter()
        .any(|m| !tool_result_images(&m.blocks).is_empty())
}
