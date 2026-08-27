//! 路由执行层（roadmap M2）。
//!
//! 职责：
//! - 按 `priority, rowid` 序逐渠道尝试（候选列表来自 store::route_candidates）
//! - 故障转移规则集：{连接拒绝、超时、UpstreamAuth、RateLimit、Overloaded、上游 5xx}
//!   → 下一渠道；InvalidRequest / ContextTooLong → 即刻返回不切换
//! - 首字节下发下游后禁止切换（该纪律在 proxy 层的流式管道兑现）
//!
//! 本模块保持纯函数化：分类逻辑可单测，不依赖 axum/reqwest 具体类型。

use crate::codec::Family;

/// 单渠道尝试的结果分类 —— 决定「切换下一渠道」还是「停在这里返回给客户端」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptVerdict {
    /// 确定性错误：立即返回，不切换（InvalidRequest / ContextTooLong / 上游确认的格式错误）
    Stop {
        /// storage 日志的 error_kind 枚举名（protocol-ir §6）
        kind: &'static str,
    },
    /// 建议切换到下一个候选渠道
    Failover {
        kind: &'static str,
    },
    /// 请求成功，进入响应阶段
    Success,
}

/// 断言「响应头已收到但首字节未到手」阶段的失败是否允许切换。
/// 上游已返回响应头、但首个数据字节到手前的失败（连接中断/超时/EOF）
/// 一律允许切换 —— 此时客户端尚未收到任何字节。
pub fn first_byte_verdict(err_is_connect: bool) -> AttemptVerdict {
    if err_is_connect {
        AttemptVerdict::Failover {
            kind: "ProviderOther",
        }
    } else {
        AttemptVerdict::Failover {
            kind: "Overloaded",
        }
    }
}

/// 依据上游 HTTP 状态码分类 + 错误体特征决定行为。
///
/// `body_excerpt`：上游错误响应体的摘要（用于 ContextTooLong 判定），
/// 可为空串。
pub fn classify_status(status: u16, body_excerpt: &str) -> AttemptVerdict {
    match status {
        // 401/403 为密钥类错误：可用另一渠道的凭据碰运气 → 切换
        401 | 403 => AttemptVerdict::Failover {
            kind: "UpstreamAuth",
        },
        404 => AttemptVerdict::Failover {
            kind: "ProviderOther",
        },
        408 => AttemptVerdict::Failover {
            kind: "ProviderOther",
        },
        429 => AttemptVerdict::Failover {
            kind: "RateLimit",
        },
        529 => AttemptVerdict::Failover {
            kind: "Overloaded",
        },
        // 5xx 一律可切换；其他 4xx 视为客户端请求错误，停止
        400 => {
            if body_excerpt.contains("context_length")
                || body_excerpt.contains("context_window")
            {
                AttemptVerdict::Stop {
                    kind: "ContextTooLong",
                }
            } else {
                AttemptVerdict::Stop {
                    kind: "InvalidRequest",
                }
            }
        }
        s if (500..600).contains(&s) => AttemptVerdict::Failover {
            kind: "Overloaded",
        },
        s if (400..500).contains(&s) => AttemptVerdict::Stop {
            kind: "InvalidRequest",
        },
        // 2xx 一律成功
        s if (200..300).contains(&s) => AttemptVerdict::Success,
        _ => AttemptVerdict::Stop {
            kind: "ProviderOther",
        },
    }
}

/// 当下游不可达（客户端断开）时使用。
pub fn client_lost() -> AttemptVerdict {
    AttemptVerdict::Stop {
        kind: "InvalidRequest",
    }
}

/// 单个候选渠道（store 层 JOIN 结果的行视图，剔除 DB 细节）。
#[derive(Debug, Clone)]
pub struct RouteCandidate {
    pub provider_id: String,
    pub provider_name: String,
    pub base_url: String,
    pub family: Family,
    pub extra_headers: Option<String>,
    pub keyring_ref: String,
    pub upstream_model_id: Option<String>,
    pub max_output_tokens: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_4xx_never_failover() {
        assert_eq!(
            classify_status(400, ""),
            AttemptVerdict::Stop {
                kind: "InvalidRequest"
            }
        );
        assert_eq!(
            classify_status(422, ""),
            AttemptVerdict::Stop {
                kind: "InvalidRequest"
            }
        );
    }

    #[test]
    fn context_too_long_detected_from_body() {
        assert_eq!(
            classify_status(400, r#"{"error":{"code":"context_length_exceeded"}}"#),
            AttemptVerdict::Stop {
                kind: "ContextTooLong"
            }
        );
        // 同一 400 无 context 关键词 → 普通 InvalidRequest
        assert_eq!(
            classify_status(400, r#"{"error":{"message":"bad request"}}"#),
            AttemptVerdict::Stop {
                kind: "InvalidRequest"
            }
        );
    }

    #[test]
    fn failover_bucket_statuses() {
        for (st, kind) in [
            (401u16, "UpstreamAuth"),
            (403, "UpstreamAuth"),
            (429, "RateLimit"),
            (529, "Overloaded"),
            (500, "Overloaded"),
            (503, "Overloaded"),
            (404, "ProviderOther"),
            (408, "ProviderOther"),
        ] {
            assert_eq!(
                classify_status(st, ""),
                AttemptVerdict::Failover { kind },
                "status {st}"
            );
        }
    }

    #[test]
    fn success_is_success() {
        assert_eq!(classify_status(200, ""), AttemptVerdict::Success);
        assert_eq!(classify_status(201, ""), AttemptVerdict::Success);
    }

    #[test]
    fn first_byte_failures_are_failover_candidates() {
        assert_eq!(
            first_byte_verdict(true),
            AttemptVerdict::Failover {
                kind: "ProviderOther"
            }
        );
        assert_eq!(
            first_byte_verdict(false),
            AttemptVerdict::Failover {
                kind: "Overloaded"
            }
        );
    }
}