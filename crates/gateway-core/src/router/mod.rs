//! 路由执行层占位。
//!
//! 设计权威：docs/design/storage-schema.md §3（路由 SQL）、roadmap M2。
//! M2 交付：按 `priority, rowid` 序逐渠道尝试、故障转移规则集、
//! 「SSE 首字节下发后禁止切换」。M0 仅声明路由决策类型骨架。

use crate::codec::Family;

/// 单个候选渠道路由结果（对应一次 JOIN 行）。
#[derive(Debug, Clone)]
pub struct RouteCandidate {
    pub provider_id: String,
    pub base_url: String,
    pub family: Family,
    /// 上游真实模型 id；None 表示与请求同名。
    pub upstream_model_id: Option<String>,
    pub max_output_tokens: i64,
    pub context_window: Option<i64>,
}
