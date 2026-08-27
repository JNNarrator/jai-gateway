//! 协议编解码层占位。
//!
//! 设计权威：docs/design/protocol-ir.md（v1 已定稿）。
//! 实现节奏：M1 落地 InboundCodec::OpenAI 的直通路径；M4/M5 填充跨族 Codec；
//! M6 增加 Responses API 入站适配。本模块 M0 仅固化 `Family` 枚举，
//! 因为存储层（providers.family）与路由层都依赖它。

use serde::{Deserialize, Serialize};

pub mod anthropic;
pub mod openai;

/// 协议家族。同族请求/上游之间允许字节级直通（见 IR 文档 §1 原则一）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    OpenAiCompat,
    Anthropic,
    Gemini,
}

impl Family {
    /// 存储层的字符串表示（providers.family CHECK 约束同款拼写）。
    pub fn as_db_str(&self) -> &'static str {
        match self {
            Family::OpenAiCompat => "openai_compat",
            Family::Anthropic => "anthropic",
            Family::Gemini => "gemini",
        }
    }

    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "openai_compat" => Some(Family::OpenAiCompat),
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
        for f in [Family::OpenAiCompat, Family::Anthropic, Family::Gemini] {
            assert_eq!(Family::from_db_str(f.as_db_str()), Some(f));
        }
        assert_eq!(Family::from_db_str("bogus"), None);
    }
}
