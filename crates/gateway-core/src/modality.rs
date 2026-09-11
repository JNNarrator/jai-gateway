//! 模型级「输入/输出模态集合」——多模态能力的真相源。
//!
//! 设计权威：docs/design/multimodal-support.md。取代 0009 的单一 vision 布尔：
//! - **存储**：TEXT 逗号串，规范序 `text,image,audio,video`；`NULL` = 未知/未标注；
//! - **派生**：`supports_multimodal` 由「输入是否含 image」推出（旧列仅作回落，见
//!   [`derive_supports_multimodal`]）；
//! - **不臆断**：空串/全非法 token 解析为 `None`（未知），也绝不按模型名猜能力。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 规范序：编码与展示顺序固定，同一集合恒等同一字符串（可 diff、可比较）。
pub const CANONICAL_ORDER: [Modality; 4] = [
    Modality::Text,
    Modality::Image,
    Modality::Audio,
    Modality::Video,
];

/// 单一模态。序列化为小写串（`"text"`/`"image"`/`"audio"`/`"video"`），
/// 前后端与出站 JSON 共用同一口径。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modality {
    Text,
    Image,
    Audio,
    Video,
}

impl Modality {
    pub fn as_str(self) -> &'static str {
        match self {
            Modality::Text => "text",
            Modality::Image => "image",
            Modality::Audio => "audio",
            Modality::Video => "video",
        }
    }

    /// token 归一：大小写与首尾空白不敏感（上游各家族写法不一）。
    pub fn from_token(s: &str) -> Option<Modality> {
        match s.trim().to_ascii_lowercase().as_str() {
            "text" => Some(Modality::Text),
            "image" => Some(Modality::Image),
            "audio" => Some(Modality::Audio),
            "video" => Some(Modality::Video),
            _ => None,
        }
    }
}

/// 去重 + 规范序。
fn canonicalize(list: &[Modality]) -> Vec<Modality> {
    CANONICAL_ORDER
        .iter()
        .copied()
        .filter(|m| list.contains(m))
        .collect()
}

/// 编码为规范序逗号串。空集合 → 空串；读回即「未知」
/// （不引入「已知为空」这一无意义状态）。
pub fn encode(list: &[Modality]) -> String {
    canonicalize(list)
        .iter()
        .map(|m| m.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// 解析逗号串：空白/大小写归一、去重、规范序。
/// 空串或全为非法 token → `None`（未知，不臆断）。
pub fn parse(s: &str) -> Option<Vec<Modality>> {
    let mut seen: Vec<Modality> = Vec::new();
    for tok in s.split(',') {
        if let Some(m) = Modality::from_token(tok) {
            if !seen.contains(&m) {
                seen.push(m);
            }
        }
    }
    if seen.is_empty() {
        None
    } else {
        Some(canonicalize(&seen))
    }
}

/// `Option<&str>` 形态的 [`parse`]（DB 列读取直用）。
pub fn parse_opt(s: Option<&str>) -> Option<Vec<Modality>> {
    s.and_then(parse)
}

/// JSON 数组（`["TEXT","IMAGE"]` / `["text"]` / `["text","image"]`）→ 模态集合。
/// 非数组、空数组、或无任何可识别 token → `None`（未知）。
pub fn from_json_array(v: &Value) -> Option<Vec<Modality>> {
    let mut seen: Vec<Modality> = Vec::new();
    for x in v.as_array()? {
        if let Some(m) = x.as_str().and_then(Modality::from_token) {
            if !seen.contains(&m) {
                seen.push(m);
            }
        }
    }
    if seen.is_empty() {
        None
    } else {
        Some(canonicalize(&seen))
    }
}

/// 输入模态是否含图像 —— 即 0009 意义上的「支持多模态 / vision」。
pub fn supports_image(list: &[Modality]) -> bool {
    list.contains(&Modality::Image)
}

/// `supports_multimodal` 的**派生规则**（取代 0009 的独立真相源）：
/// - 输入集合有值 → 是否含 image；
/// - 输入集合为 `NULL` → 回落旧 `supports_multimodal` 列（老数据可读）；
/// - 两者皆无 → `None`（未知）。
///
/// 注意：集合一旦存在就**压过**旧列 —— 否则 UI 清除标注后旧列的 `true`
/// 会变成「幽灵 true」。
pub fn derive_supports_multimodal(
    input: Option<&[Modality]>,
    legacy: Option<bool>,
) -> Option<bool> {
    match input {
        Some(list) => Some(supports_image(list)),
        None => legacy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_is_canonical_and_deduped() {
        use Modality::*;
        assert_eq!(encode(&[Image, Text]), "text,image");
        assert_eq!(
            encode(&[Video, Audio, Image, Text]),
            "text,image,audio,video"
        );
        assert_eq!(encode(&[]), "");
    }

    #[test]
    fn parse_never_guesses() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("bogus"), None);
        assert_eq!(parse_opt(None), None);
        assert_eq!(
            parse("TEXT, image"),
            Some(vec![Modality::Text, Modality::Image])
        );
    }
}
