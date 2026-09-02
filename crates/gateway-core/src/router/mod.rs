//! 路由执行层（roadmap M2）。
//!
//! 职责：
//! - 按 `priority, rowid` 序逐渠道尝试（候选列表来自 store::route_candidates）
//! - 故障转移规则集：{连接拒绝、超时、UpstreamAuth、RateLimit、Overloaded、上游 5xx}
//!   → 下一渠道；InvalidRequest / ContextTooLong → 即刻返回不切换
//! - 首字节下发下游后禁止切换（该纪律在 proxy 层的流式管道兑现）
//!
//! 本模块保持纯函数化：分类逻辑可单测，不依赖 axum/reqwest 具体类型。

use crate::store::RouteCandidate as StoreRouteCandidate;

/// 健康冷却窗口：最近失败后在该窗口内视为不健康，会被排到同优先级健康渠道之后。
pub const HEALTH_COOLDOWN_MS: i64 = 5 * 60 * 1000;

/// 高级路由候选排序：
/// 1. 按 priority 分组，小组内保持主备次序；
/// 2. 同 priority 内健康渠道排在不健康渠道之前（基于 last_ok_at / last_err_at）；
/// 3. 健康/不健康组内分别按 weight 加权随机打散，实现权重负载均衡。
pub fn order_candidates(
    candidates: Vec<StoreRouteCandidate>,
    now_ms: i64,
) -> Vec<StoreRouteCandidate> {
    use std::collections::HashMap;

    // 健康感知主备切换：健康渠道整体排在不健康渠道之前；
    // 这样最近失败的主渠道会被健康备渠道接管，而不是每次先撞一次失败。
    let (healthy_all, unhealthy_all): (Vec<StoreRouteCandidate>, Vec<StoreRouteCandidate>) =
        candidates.into_iter().partition(|c| is_healthy(c, now_ms));

    let mut rng = rand::thread_rng();
    let mut out = Vec::with_capacity(healthy_all.len() + unhealthy_all.len());
    for group in [healthy_all, unhealthy_all] {
        // 每个健康状态组内再按 priority 分组，保持主备大序。
        let mut groups: Vec<(i64, Vec<StoreRouteCandidate>)> = Vec::new();
        let mut index: HashMap<i64, usize> = HashMap::new();
        for c in group {
            match index.get(&c.priority) {
                Some(&i) => groups[i].1.push(c),
                None => {
                    index.insert(c.priority, groups.len());
                    groups.push((c.priority, vec![c]));
                }
            }
        }
        for (_prio, mut same_priority) in groups {
            weighted_shuffle(&mut same_priority, &mut rng);
            out.extend(same_priority);
        }
    }
    out
}

fn is_healthy(c: &StoreRouteCandidate, now_ms: i64) -> bool {
    match (c.last_ok_at, c.last_err_at) {
        (Some(ok), Some(err)) => ok >= err,
        (Some(_), None) => true,
        (None, Some(err)) => now_ms.saturating_sub(err) > HEALTH_COOLDOWN_MS,
        (None, None) => true,
    }
}

fn weighted_shuffle(items: &mut Vec<StoreRouteCandidate>, rng: &mut impl rand::Rng) {
    let mut out = Vec::with_capacity(items.len());
    while !items.is_empty() {
        let total: i64 = items.iter().map(|c| c.weight.max(1)).sum();
        let mut pick = rng.gen_range(0..total);
        let mut idx = 0usize;
        for (i, c) in items.iter().enumerate() {
            let w = c.weight.max(1);
            if pick < w {
                idx = i;
                break;
            }
            pick -= w;
        }
        out.push(items.remove(idx));
    }
    *items = out;
}

/// 单渠道尝试的结果分类 —— 决定「切换下一渠道」还是「停在这里返回给客户端」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptVerdict {
    /// 确定性错误：立即返回，不切换（InvalidRequest / ContextTooLong / 上游确认的格式错误）
    Stop {
        /// storage 日志的 error_kind 枚举名（protocol-ir §6）
        kind: &'static str,
    },
    /// 建议切换到下一个候选渠道
    Failover { kind: &'static str },
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
        AttemptVerdict::Failover { kind: "Overloaded" }
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
        429 => AttemptVerdict::Failover { kind: "RateLimit" },
        529 => AttemptVerdict::Failover { kind: "Overloaded" },
        // 5xx 一律可切换；其他 4xx 视为客户端请求错误，停止
        400 => {
            if body_excerpt.contains("context_length") || body_excerpt.contains("context_window") {
                AttemptVerdict::Stop {
                    kind: "ContextTooLong",
                }
            } else {
                AttemptVerdict::Stop {
                    kind: "InvalidRequest",
                }
            }
        }
        s if (500..600).contains(&s) => AttemptVerdict::Failover { kind: "Overloaded" },
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
            AttemptVerdict::Failover { kind: "Overloaded" }
        );
    }

    fn cand(
        id: &str,
        priority: i64,
        weight: i64,
        last_ok_at: Option<i64>,
        last_err_at: Option<i64>,
    ) -> StoreRouteCandidate {
        StoreRouteCandidate {
            provider_id: id.into(),
            provider_name: id.into(),
            priority,
            base_url: format!("http://{id}"),
            family: "openai_compat".into(),
            extra_headers: None,
            api_key: Some("sk-test".into()),
            website: None,
            upstream_model_id: None,
            max_output_tokens: 4096,
            weight,
            last_ok_at,
            last_err_at,
        }
    }

    #[test]
    fn order_keeps_priority_groups_and_healthy_first() {
        let now = 1_000_000;
        let list = vec![
            cand("unhealthy-p1", 1, 1, Some(now - 1000), Some(now - 10)),
            cand("healthy-p1", 1, 1, Some(now - 10), Some(now - 1000)),
            cand("healthy-p2", 2, 1, Some(now), None),
        ];
        let ordered = order_candidates(list, now);
        let names: Vec<&str> = ordered.iter().map(|c| c.provider_id.as_str()).collect();
        assert_eq!(names[0], "healthy-p1", "健康渠道应在前");
        assert_eq!(names[1], "healthy-p2", "健康备渠道应接管不健康主渠道");
        assert_eq!(names[2], "unhealthy-p1", "不健康渠道整体排最后");
    }

    #[test]
    fn order_is_permutation_and_handles_empty() {
        let now = 1_000_000;
        let list = vec![
            cand("a", 1, 10, None, None),
            cand("b", 1, 1, None, None),
            cand("c", 2, 1, Some(now), None),
        ];
        let ordered = order_candidates(list, now);
        let mut names: Vec<&str> = ordered.iter().map(|c| c.provider_id.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["a", "b", "c"]);
        assert!(order_candidates(Vec::new(), now).is_empty());
    }
}
