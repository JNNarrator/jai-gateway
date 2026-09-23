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

/// 渠道健康判定（冷却窗口见 [`HEALTH_COOLDOWN_MS`]）。
///
/// 对外可见：`proxy.rs` 在兑现「限定名 `供应商/模型` 优先」时也要用它 ——
/// 指定渠道已知不健康时不该去抢健康备渠道的位置。
pub fn is_healthy(c: &StoreRouteCandidate, now_ms: i64) -> bool {
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

// D9-T3：原先这里有一个 `first_byte_verdict(err_is_connect)`，用来判定「响应头已收到、
// 首字节未到手」阶段的失败是否允许切换。它**从未被生产代码调用**（只有自己的单测），
// 因为真正的判定内联在 `streaming_response` / `convert_streaming_response` 的三个失败
// 分支里（stream 错误 → ProviderOther、空流 → ProviderOther、首字节超时 → Overloaded）。
// 本次把那些分支的语义**落地**为 `Attempt::Failed`（可 failover），死代码随之删除。

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
        // D9-T3：405 / 501 = 「这个端点不接受该方法 / 没实现」→ 换个渠道有意义
        405 | 501 => AttemptVerdict::Failover {
            kind: "ProviderOther",
        },
        404 | 408 => AttemptVerdict::Failover {
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

// ---------------------------------------------------------------- 退避（D9-T2）

/// 单次等待上限：上游给的 `Retry-After` 再大也不等超过这个值。
///
/// 没有上限的话，一个返回 `Retry-After: 3600` 的坏上游就能把下游客户端挂住一小时。
pub const RETRY_AFTER_CAP_MS: u64 = 5_000;

/// 抖动幅度（百分比）。
///
/// 多个客户端、多个候选同拍重试会把限流窗口二次打满，所以实际等待在
/// `±RETRY_AFTER_JITTER_PCT%` 内随机。
pub const RETRY_AFTER_JITTER_PCT: u32 = 20;

/// 一次请求内的累计等待上限（跨候选累加）。
///
/// 只封顶单次等待不够：6 个候选各等 5s 就是 30s，客户端会先超时。
pub const MAX_TOTAL_BACKOFF_MS: u64 = 10_000;

/// 该失败类型是否值得等待退避。
///
/// 只有上游限速 / 过载（`RateLimit` = 429、`Overloaded` = 5xx/529）值得等；
/// 认证错（401/403）与请求错（4xx）等多久都不会变好。
///
/// T3 引入 `FailureClass` 后本函数由 `FailureClass::retryable_with_backoff()` 取代。
pub fn kind_waits_for_backoff(kind: &str) -> bool {
    matches!(kind, "RateLimit" | "Overloaded")
}

/// 解析上游 `Retry-After`（RFC 7231 §7.1.3），返回毫秒。
///
/// 两种形式：
/// - `delta-seconds`（主路径，绝大多数上游）：`"120"`；宽松接受小数 `"0.5"`
/// - `HTTP-date`（IMF-fixdate）：`"Wed, 21 Oct 2026 07:28:00 GMT"`
///
/// 非法 / 负数 / 已过期 / 非有限值 → `None`（调用方回退到指数退避）。
///
/// `now_unix`（秒）由调用方注入，便于单测确定化。
pub fn parse_retry_after(value: &str, now_unix: i64) -> Option<u64> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    // delta-seconds。RFC 要求非负整数，但确有上游给小数，宽松接受。
    // f64 → u64 是饱和转换，超大值（"1e30"）饱和到 u64::MAX，
    // 由调用方的 RETRY_AFTER_CAP_MS 截断；这里不会 panic。
    if let Ok(secs) = v.parse::<f64>() {
        if !secs.is_finite() || secs < 0.0 {
            return None;
        }
        return Some((secs * 1000.0).round() as u64);
    }
    // HTTP-date：用 httpdate 解析，绝不手写日期解析。
    let target = httpdate::parse_http_date(v).ok()?;
    let target_secs = target.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    if target_secs <= now_unix {
        return None;
    }
    Some((target_secs - now_unix) as u64 * 1000)
}

/// 计算切换下一候选前的实际等待（毫秒）。
///
/// - `retry_after_ms` 有值：以它为准
/// - 无值：按 `attempt` 次数的指数退避（1s / 2s / 4s / 8s）。
///   注意：`dispatch` 目前**不会**走这一支（上游没说就不等，理由见 proxy 里的注释）；
///   它是为 T3 的分组重试准备的 —— 同一组内会回到同一个上游，等待才有意义。
/// - 两者都先封顶 [`RETRY_AFTER_CAP_MS`]，再叠加 ±`jitter_pct`% 抖动
///
/// 抖动是加在封顶之后的，所以返回值可能略超 `RETRY_AFTER_CAP_MS`
/// （上限 +20%）；累计封顶由调用方的 [`MAX_TOTAL_BACKOFF_MS`] 负责。
pub fn backoff_delay_ms(retry_after_ms: Option<u64>, attempt: usize, jitter_pct: u32) -> u64 {
    use rand::Rng as _;

    let base = match retry_after_ms {
        Some(ms) => ms.min(RETRY_AFTER_CAP_MS),
        None => (1000u64 << attempt.min(3) as u32).min(RETRY_AFTER_CAP_MS),
    };
    if jitter_pct == 0 {
        return base;
    }
    let pct = jitter_pct.min(100) as i64;
    let delta = (base as i64) * pct / 100;
    if delta <= 0 {
        return base;
    }
    let lo = (base as i64 - delta).max(0) as u64;
    let hi = (base as i64 + delta) as u64;
    rand::thread_rng().gen_range(lo..=hi)
}

// ---------------------------------------------------------------- 失败分类与重试预算（D9-T3）

/// 失败分类：决定「换不换候选」「能不能跨组」「值不值得退避」。
///
/// 与 [`AttemptVerdict`] 的关系：后者保留原有的 Stop / Failover 二元判定
/// （`classify_status` 的边界与既有测试不变），本枚举是它的收敛上层 ——
/// 多出「跨不跨组」这一维。落库用的 `error_kind` 字符串由 [`Failure::kind`] 原样携带，
/// 不经过本枚举，所以 `request_logs.error_kind` 的语义逐字不变。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// 调用方自己的错（400/422 等）：换候选也不会好 → 立即返回
    CallerTerminal,
    /// 渠道凭据类终态（401/403）：只在本组内换候选，绝不跨组。
    /// 跨协议重试会让权限 / 语义错误被另一个协议的成功掩盖。
    ChannelAuthTerminal,
    /// 端点不接受这个方法 / 没实现（405/501）：可跨组（换个协议族也许支持）
    EndpointUnsupported,
    /// 限速 / 过载 / 上游 5xx / 模型不存在（404/408/429/529/5xx）：可跨组
    Retryable,
    /// 上游协议层错误（响应无法解析）：可跨组
    UpstreamProtocolError,
    /// 已向下游 commit 之后断流：无法 failover（字节已经出去了）
    CommittedStreamError,
}

impl FailureClass {
    /// 是否允许跨组（从同族组跳到转换组）。
    pub fn degradable(self) -> bool {
        matches!(
            self,
            FailureClass::EndpointUnsupported
                | FailureClass::Retryable
                | FailureClass::UpstreamProtocolError
        )
    }

    /// 是否值得等待上游给的 `Retry-After`（只有限速 / 过载值得）。
    pub fn waits_for_backoff(self) -> bool {
        matches!(self, FailureClass::Retryable)
    }

    /// 把落库用的 `error_kind` 字符串还原成行为分类。
    ///
    /// 只用于「手上只有一个 kind 字符串」的失败路径（建连失败、流建立失败等）。
    /// 注意 `ProviderOther` 是**复合** kind：404 / 405 / 501 / 建连失败都用它，
    /// 这里一律按最宽松的 `Retryable` 还原（组内换候选 + 允许跨组）。
    /// 手上还有 HTTP 状态码时请走 [`classify_failure`] —— 那里能分辨 405/501。
    pub fn from_kind(kind: &str) -> Self {
        match kind {
            "UpstreamAuth" => FailureClass::ChannelAuthTerminal,
            "InvalidRequest" | "ContextTooLong" => FailureClass::CallerTerminal,
            _ => FailureClass::Retryable,
        }
    }
}

/// 一次失败的完整描述：行为分类 + 落库字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failure {
    pub class: FailureClass,
    /// 落库 `request_logs.error_kind` 的字符串 —— 与 `classify_status` 的 kind 完全一致
    pub kind: &'static str,
}

impl Failure {
    pub fn new(class: FailureClass, kind: &'static str) -> Self {
        Self { class, kind }
    }

    /// 落库用的 `error_kind` 字符串（与 [`classify_status`] 的 kind 逐字一致）。
    pub fn as_kind(self) -> &'static str {
        self.kind
    }
}

/// 依据上游 HTTP 状态码分类（比 [`classify_status`] 多一维「能不能跨组」）。
pub fn classify_failure(status: u16, body_excerpt: &str) -> Failure {
    let verdict = classify_status(status, body_excerpt);
    match verdict {
        AttemptVerdict::Success => Failure::new(FailureClass::Retryable, "ProviderOther"),
        AttemptVerdict::Stop { kind } => Failure::new(FailureClass::CallerTerminal, kind),
        AttemptVerdict::Failover { kind } => {
            let class = match status {
                401 | 403 => FailureClass::ChannelAuthTerminal,
                405 | 501 => FailureClass::EndpointUnsupported,
                _ => FailureClass::Retryable,
            };
            Failure::new(class, kind)
        }
    }
}

/// 候选分组：原生协议组（同族，字节直通）与转换组（跨族，走 IR）。
///
/// 分组的实际意义是预算与跨组规则 —— 原生组永不因优先级被转换组跳级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupTier {
    Native,
    Conversion,
}

/// 按入站协议族把候选切成两组，各自保持传入顺序（调用方已做过 priority / weight 排序）。
pub fn group_candidates(
    cands: Vec<StoreRouteCandidate>,
    inbound_family: &str,
) -> (Vec<StoreRouteCandidate>, Vec<StoreRouteCandidate>) {
    cands.into_iter().partition(|c| c.family == inbound_family)
}

/// 同组内最多尝试几个候选。
pub const DEFAULT_MAX_ATTEMPTS_PER_GROUP: usize = 3;
/// 一次请求最多尝试几个候选（跨组累计）。
pub const DEFAULT_MAX_ATTEMPTS_TOTAL: usize = 6;

/// 重试预算：组内上限 + 全局上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryBudget {
    pub per_group: usize,
    pub total: usize,
}

impl Default for RetryBudget {
    fn default() -> Self {
        Self {
            per_group: DEFAULT_MAX_ATTEMPTS_PER_GROUP,
            total: DEFAULT_MAX_ATTEMPTS_TOTAL,
        }
    }
}

/// 从 meta 读重试预算。缺失 / 非法 / 小于 1 一律回退默认值。
///
/// 设为 `(1, 1)` 等价于「关闭重试」= 改造前的行为。
pub fn retry_budget_from_meta(get: impl Fn(&str) -> Option<String>) -> RetryBudget {
    let read = |key: &str, fallback: usize| -> usize {
        get(key)
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|n| *n >= 1)
            .unwrap_or(fallback)
    };
    RetryBudget {
        per_group: read("retry_max_per_group", DEFAULT_MAX_ATTEMPTS_PER_GROUP),
        total: read("retry_max_total", DEFAULT_MAX_ATTEMPTS_TOTAL),
    }
}

/// [`AttemptFlow::next`] 的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowStep {
    /// 同组内换下一个候选
    NextInGroup,
    /// 组内预算耗尽，进转换组
    NextGroup,
    /// 不再尝试
    Stop,
}

/// 候选遍历状态机：组内预算 → 跨组 → 全局预算。
#[derive(Debug, Clone)]
pub struct AttemptFlow {
    tier: GroupTier,
    per_group_used: usize,
    total_used: usize,
    budget: RetryBudget,
}

impl AttemptFlow {
    /// `non_idempotent` 为真时预算强制压到 `(1, 1)` —— 重发会产生副作用
    /// （Responses 带 `store` / `background`），不重试。
    pub fn new(budget: RetryBudget, non_idempotent: bool) -> Self {
        let budget = if non_idempotent {
            RetryBudget {
                per_group: 1,
                total: 1,
            }
        } else {
            budget
        };
        Self {
            tier: GroupTier::Native,
            per_group_used: 0,
            total_used: 0,
            budget,
        }
    }

    pub fn tier(&self) -> GroupTier {
        self.tier
    }

    pub fn budget(&self) -> RetryBudget {
        self.budget
    }

    pub fn total_used(&self) -> usize {
        self.total_used
    }

    /// 记一次已发出的尝试。
    pub fn record_attempt(&mut self) {
        self.per_group_used += 1;
        self.total_used += 1;
    }

    /// 一次失败后决定下一步。
    ///
    /// `group_exhausted`：**调用方**已知当前组里没有下一个候选了（候选列表走完）。
    /// 它必须参与判定，否则「同组只有 1 个候选」时 `per_group_used` 永远小于上限，
    /// 401 之类不可降级的失败会被误判成 `NextInGroup` 而顺着列表跨到转换组 ——
    /// 那就等于绕过了「凭据类错误不跨组」这条硬约束。
    pub fn next(&mut self, class: FailureClass, group_exhausted: bool) -> FlowStep {
        // 确定性错误与「已 commit 后断流」都不该再试
        if matches!(
            class,
            FailureClass::CallerTerminal | FailureClass::CommittedStreamError
        ) {
            return FlowStep::Stop;
        }
        if self.total_used >= self.budget.total {
            return FlowStep::Stop;
        }
        if !group_exhausted && self.per_group_used < self.budget.per_group {
            return FlowStep::NextInGroup;
        }
        // 组内没得试了（候选走完 / 组内预算耗尽）：只有可降级的失败才允许跨组
        if class.degradable() && self.tier == GroupTier::Native {
            self.tier = GroupTier::Conversion;
            self.per_group_used = 0;
            return FlowStep::NextGroup;
        }
        FlowStep::Stop
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
            reasoning_effort_levels: None,
            max_tools: None,
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

    // ------------------------------------------------------------ 退避（D9-T2）

    #[test]
    fn retry_after_delta_seconds() {
        // 主路径：整数秒
        assert_eq!(parse_retry_after("120", 0), Some(120_000));
        assert_eq!(parse_retry_after("0", 0), Some(0));
        // 前后空白要容忍（上游偶尔带空格）
        assert_eq!(parse_retry_after("  30  ", 0), Some(30_000));
        // 小数：非 RFC 但确有上游这么发
        assert_eq!(parse_retry_after("0.5", 0), Some(500));
        assert_eq!(parse_retry_after("1.5", 0), Some(1_500));
    }

    #[test]
    fn retry_after_rejects_garbage_and_negative() {
        assert_eq!(parse_retry_after("", 0), None);
        assert_eq!(parse_retry_after("   ", 0), None);
        assert_eq!(parse_retry_after("-1", 0), None);
        assert_eq!(parse_retry_after("abc", 0), None);
        assert_eq!(parse_retry_after("120abc", 0), None);
        // 非有限值必须被拒（否则会污染后续的封顶计算）
        assert_eq!(parse_retry_after("inf", 0), None);
        assert_eq!(parse_retry_after("NaN", 0), None);
    }

    #[test]
    fn retry_after_huge_value_saturates_without_panic() {
        // 超大值不能 panic；饱和到 u64::MAX 后由调用方按 CAP 截断
        let got = parse_retry_after("1e30", 0).expect("超大 delta-seconds 应能解析");
        assert_eq!(got, u64::MAX);
        assert_eq!(
            backoff_delay_ms(Some(got), 0, 0),
            RETRY_AFTER_CAP_MS,
            "封顶必须把超大值压回 RETRY_AFTER_CAP_MS"
        );
    }

    #[test]
    fn retry_after_http_date() {
        // 2026-10-21T07:28:00Z = 1792567680
        let now = 1_792_567_680 - 120;
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT", now),
            Some(120_000)
        );
        // 已过期的日期 → None（等一个过去的时间点没有意义）
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT", 1_792_567_680),
            None
        );
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT", 1_792_567_680 + 60),
            None
        );
    }

    #[test]
    fn backoff_caps_single_wait() {
        // 上游要 1 小时，我们只等 CAP
        assert_eq!(backoff_delay_ms(Some(3_600_000), 0, 0), RETRY_AFTER_CAP_MS);
        // 无 Retry-After 时的指数退避：1s / 2s / 4s，第 4 次起封顶
        assert_eq!(backoff_delay_ms(None, 0, 0), 1_000);
        assert_eq!(backoff_delay_ms(None, 1, 0), 2_000);
        assert_eq!(backoff_delay_ms(None, 2, 0), 4_000);
        assert_eq!(
            backoff_delay_ms(None, 3, 0),
            8_000_u64.min(RETRY_AFTER_CAP_MS)
        );
        assert_eq!(backoff_delay_ms(None, 99, 0), RETRY_AFTER_CAP_MS);
    }

    #[test]
    fn backoff_jitter_stays_within_band() {
        let base = 1_000u64;
        let lo = base - base * RETRY_AFTER_JITTER_PCT as u64 / 100;
        let hi = base + base * RETRY_AFTER_JITTER_PCT as u64 / 100;
        for _ in 0..200 {
            let d = backoff_delay_ms(Some(base), 0, RETRY_AFTER_JITTER_PCT);
            assert!((lo..=hi).contains(&d), "抖动后 {d} 应落在 [{lo}, {hi}] 内");
        }
        // jitter=0 时必须确定化（回归保护：不要让抖动变成无条件随机）
        assert_eq!(backoff_delay_ms(Some(base), 0, 0), base);
    }

    #[test]
    fn backoff_zero_base_is_exactly_zero() {
        // Retry-After: 0 → 不等待，且不能被抖动放大成非零
        assert_eq!(backoff_delay_ms(Some(0), 0, RETRY_AFTER_JITTER_PCT), 0);
    }

    #[test]
    fn only_rate_limit_and_overloaded_wait() {
        assert!(kind_waits_for_backoff("RateLimit"));
        assert!(kind_waits_for_backoff("Overloaded"));
        // 认证错与请求错等多久都不会变好
        assert!(!kind_waits_for_backoff("UpstreamAuth"));
        assert!(!kind_waits_for_backoff("InvalidRequest"));
        assert!(!kind_waits_for_backoff("ContextTooLong"));
        assert!(!kind_waits_for_backoff("ProviderOther"));
    }

    // ------------------------------------------------- 失败分类与重试预算（D9-T3）

    /// 兼容护栏：`classify_failure` 的 kind 必须与 `classify_status` 逐字一致，
    /// 否则 `request_logs.error_kind` 的语义（以及前端日志页 / 统计）会静默漂移。
    #[test]
    fn classify_failure_kind_matches_classify_status_verbatim() {
        let cases: [(u16, &str); 10] = [
            (400, ""),
            (400, r#"{"error":{"code":"context_length_exceeded"}}"#),
            (401, ""),
            (403, ""),
            (404, ""),
            (405, ""),
            (408, ""),
            (429, ""),
            (501, ""),
            (503, ""),
        ];
        for (status, body) in cases {
            let expect = match classify_status(status, body) {
                AttemptVerdict::Stop { kind } | AttemptVerdict::Failover { kind } => kind,
                AttemptVerdict::Success => panic!("{status} 不应判 Success"),
            };
            assert_eq!(
                classify_failure(status, body).as_kind(),
                expect,
                "status {status} kind 漂移"
            );
        }
    }

    #[test]
    fn classify_failure_maps_status_to_class() {
        use FailureClass::*;
        for (status, body, want) in [
            (400u16, "", CallerTerminal),
            (422, "", CallerTerminal),
            (400, r#"{"context_length":1}"#, CallerTerminal),
            (401, "", ChannelAuthTerminal),
            (403, "", ChannelAuthTerminal),
            (405, "", EndpointUnsupported),
            (501, "", EndpointUnsupported),
            (404, "", Retryable),
            (408, "", Retryable),
            (429, "", Retryable),
            (529, "", Retryable),
            (500, "", Retryable),
        ] {
            assert_eq!(
                classify_failure(status, body).class,
                want,
                "status {status}"
            );
        }
    }

    #[test]
    fn from_kind_restores_class_from_error_kind() {
        assert_eq!(
            FailureClass::from_kind("UpstreamAuth"),
            FailureClass::ChannelAuthTerminal
        );
        assert_eq!(
            FailureClass::from_kind("InvalidRequest"),
            FailureClass::CallerTerminal
        );
        assert_eq!(
            FailureClass::from_kind("ContextTooLong"),
            FailureClass::CallerTerminal
        );
        // 复合 kind：一律按最宽松处理（组内换候选 + 允许跨组）
        for kind in ["ProviderOther", "RateLimit", "Overloaded"] {
            assert_eq!(FailureClass::from_kind(kind), FailureClass::Retryable);
        }
    }

    #[test]
    fn degradable_matrix() {
        assert!(!FailureClass::CallerTerminal.degradable());
        assert!(!FailureClass::ChannelAuthTerminal.degradable());
        assert!(!FailureClass::CommittedStreamError.degradable());
        assert!(FailureClass::EndpointUnsupported.degradable());
        assert!(FailureClass::Retryable.degradable());
        assert!(FailureClass::UpstreamProtocolError.degradable());
    }

    #[test]
    fn only_retryable_waits_for_backoff_class() {
        assert!(FailureClass::Retryable.waits_for_backoff());
        assert!(!FailureClass::ChannelAuthTerminal.waits_for_backoff());
        assert!(!FailureClass::CallerTerminal.waits_for_backoff());
        assert!(!FailureClass::EndpointUnsupported.waits_for_backoff());
    }

    /// `AttemptFlow::next` 真值表：6 个 FailureClass × 组内预算状态。
    ///
    /// 预算 `(3, 6)`（默认）：组内用过 1 次且组里还有候选 → 未耗尽；用满 3 次 → 已耗尽。
    #[test]
    fn attempt_flow_truth_table() {
        use FailureClass as C;
        use FlowStep::*;

        for (class, unspent, spent) in [
            (C::CallerTerminal, Stop, Stop),
            // 凭据类终态绝不跨组：组内还有名额就换，没了就停
            (C::ChannelAuthTerminal, NextInGroup, Stop),
            (C::EndpointUnsupported, NextInGroup, NextGroup),
            (C::Retryable, NextInGroup, NextGroup),
            (C::UpstreamProtocolError, NextInGroup, NextGroup),
            (C::CommittedStreamError, Stop, Stop),
        ] {
            let mut f = AttemptFlow::new(RetryBudget::default(), false);
            f.record_attempt();
            assert_eq!(
                f.next(class, false),
                unspent,
                "{class:?} 组内未超预算且组里还有候选"
            );

            let mut f = AttemptFlow::new(RetryBudget::default(), false);
            for _ in 0..DEFAULT_MAX_ATTEMPTS_PER_GROUP {
                f.record_attempt();
            }
            assert_eq!(f.next(class, true), spent, "{class:?} 组内已超预算");
        }
    }

    /// 「本组候选已经走完」必须和「组内预算耗尽」同等地触发跨组判定 ——
    /// 否则同组只有 1 个候选时，401 会顺着候选列表跨进转换组。
    #[test]
    fn attempt_flow_treats_group_exhausted_as_budget_exhausted() {
        // 组里还剩候选名额（per_group=3），但候选列表已经走完
        let mut f = AttemptFlow::new(RetryBudget::default(), false);
        f.record_attempt();
        assert_eq!(f.next(FailureClass::Retryable, true), FlowStep::NextGroup);

        // 同样情形下 401：不跨组
        let mut f = AttemptFlow::new(RetryBudget::default(), false);
        f.record_attempt();
        assert_eq!(
            f.next(FailureClass::ChannelAuthTerminal, true),
            FlowStep::Stop,
            "凭据类错误在组候选走完后必须停，不能跨组"
        );
    }

    /// 跨组只发生一次，且进组后组内计数归零（新组重新享受组内预算）。
    #[test]
    fn attempt_flow_switches_tier_once_and_then_next_in_group() {
        let mut f = AttemptFlow::new(RetryBudget::default(), false);
        assert_eq!(f.tier(), GroupTier::Native);
        for _ in 0..DEFAULT_MAX_ATTEMPTS_PER_GROUP {
            f.record_attempt();
        }
        assert_eq!(f.next(FailureClass::Retryable, true), FlowStep::NextGroup);
        assert_eq!(f.tier(), GroupTier::Conversion);
        f.record_attempt();
        assert_eq!(
            f.next(FailureClass::Retryable, false),
            FlowStep::NextInGroup
        );
    }

    /// 总预算用尽一律 Stop，哪怕组内还剩名额、失败本身可降级。
    #[test]
    fn attempt_flow_stops_at_total_budget() {
        let mut f = AttemptFlow::new(
            RetryBudget {
                per_group: 10,
                total: 2,
            },
            false,
        );
        f.record_attempt();
        assert_eq!(
            f.next(FailureClass::Retryable, false),
            FlowStep::NextInGroup
        );
        f.record_attempt();
        assert_eq!(f.total_used(), 2);
        assert_eq!(f.next(FailureClass::Retryable, false), FlowStep::Stop);
    }

    /// 非幂等请求：预算压到 (1,1)，一次失败就是终点。
    #[test]
    fn attempt_flow_non_idempotent_gets_single_attempt() {
        let mut f = AttemptFlow::new(RetryBudget::default(), true);
        assert_eq!(
            f.budget(),
            RetryBudget {
                per_group: 1,
                total: 1
            }
        );
        f.record_attempt();
        assert_eq!(f.next(FailureClass::Retryable, false), FlowStep::Stop);
    }

    #[test]
    fn retry_budget_from_meta_defaults_and_overrides() {
        // 缺失 → 默认
        assert_eq!(
            retry_budget_from_meta(|_| None),
            RetryBudget::default(),
            "meta 无值时用默认预算"
        );
        // 非法 / 0 → 各自回退默认（不能让一个写坏的 meta 把重试关掉）
        assert_eq!(
            retry_budget_from_meta(|_| Some("0".into())),
            RetryBudget::default()
        );
        assert_eq!(
            retry_budget_from_meta(|_| Some("abc".into())),
            RetryBudget::default()
        );
        // 显式覆盖（含前后空白容忍）
        let got = retry_budget_from_meta(|k| match k {
            "retry_max_per_group" => Some(" 2 ".into()),
            "retry_max_total" => Some("9".into()),
            _ => None,
        });
        assert_eq!(
            got,
            RetryBudget {
                per_group: 2,
                total: 9
            }
        );
        // (1,1) = 关闭重试，等价改造前行为
        let off = retry_budget_from_meta(|_| Some("1".into()));
        assert_eq!(
            off,
            RetryBudget {
                per_group: 1,
                total: 1
            }
        );
    }

    #[test]
    fn group_candidates_splits_by_inbound_family_keeping_order() {
        let mut a = cand("native-1", 1, 1, None, None);
        a.family = "openai_responses".into();
        let mut b = cand("other-1", 1, 1, None, None);
        b.family = "anthropic".into();
        let mut c = cand("native-2", 2, 1, None, None);
        c.family = "openai_responses".into();

        let (native, conversion) = group_candidates(vec![a, b, c], "openai_responses");
        let ids =
            |v: &[StoreRouteCandidate]| v.iter().map(|c| c.provider_id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&native), vec!["native-1", "native-2"]);
        assert_eq!(ids(&conversion), vec!["other-1"]);
    }
}
