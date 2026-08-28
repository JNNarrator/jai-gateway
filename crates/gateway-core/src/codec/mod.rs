//! 协议编解码层。
//!
//! 设计权威：docs/design/protocol-ir.md（v1 已定稿）。
//! 布局：
//! - `ir`       ：协议中间表示类型 + 结构合规变换 + 护栏（§2/§4-C）
//! - `openai`   ：OpenAI 旁路工具（M1）+ InboundCodec::OpenAI（M4）
//! - `anthropic`：Anthropic 直通助手（M3）+ UpstreamCodec::Anthropic（M4）
//! - `gemini`   ：UpstreamCodec::Gemini（M4）
//!
//! 实现节奏：M1 直通 → M3 Anthropic 直通 → M4/M5 跨族 Codec → M6 Responses 入站。

use serde::{Deserialize, Serialize};

pub mod anthropic;
pub mod gemini;
pub mod ir;
pub mod openai;
pub mod responses;

/// 协议家族。同族请求/上游之间允许字节级直通（见 IR 文档 §1 原则一）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    OpenAiCompat,
    OpenAiResponses,
    Anthropic,
    Gemini,
}

impl Family {
    /// 存储层的字符串表示（providers.family CHECK 约束同款拼写）。
    pub fn as_db_str(&self) -> &'static str {
        match self {
            Family::OpenAiCompat => "openai_compat",
            Family::OpenAiResponses => "openai_responses",
            Family::Anthropic => "anthropic",
            Family::Gemini => "gemini",
        }
    }

    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "openai_compat" => Some(Family::OpenAiCompat),
            "openai_responses" => Some(Family::OpenAiResponses),
            "anthropic" => Some(Family::Anthropic),
            "gemini" => Some(Family::Gemini),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_roundtrip_db_str() {
        for f in [
            Family::OpenAiCompat,
            Family::OpenAiResponses,
            Family::Anthropic,
            Family::Gemini,
        ] {
            assert_eq!(Family::from_db_str(f.as_db_str()), Some(f));
        }
        assert_eq!(Family::from_db_str("bogus"), None);
    }
}
