//! 渠道草稿探测（D9-T1）：保存前对每个候选端点发一次**真实的最小推理请求**。
//!
//! 为什么需要它：现有的「测试连接」只打 `/models`（列模型）。能列模型 ≠ 能推理 ——
//! 中转站常见三种假绿：`/models` 有数据但 `/chat/completions` 404；网关前置的
//! HTML 拦截页返回 200；key 只能读模型不能推理。用户只有保存完、在客户端里
//! 真发一次请求才会发现，而那时排查成本已经很高。
//!
//! 设计要点：
//! - **不落库**：探测目标与结论都是纯草稿（provider 还没有 id）。
//! - **最小成本**：`max*_tokens = 1` + `stream: false`，但仍然会真的调用模型，
//!   所以结论里带 `cost_possible` 让 UI 明确提示。
//! - **2xx 不等于通过**：必须能把响应解析成该协议的**预期最小结构**，
//!   否则判 `Protocol` 失败（这条是防「假绿」的关键）。
//! - **发请求前先过 SSRF 校验**（[`crate::netguard`]）：这是第一个「用户填什么就打什么」
//!   的入口，不校验就是一个 SSRF 面。
//! - 探测与 provider 其余逻辑**无耦合**：调用方给 `reqwest::Client`
//!   （桌面端复用 `core.http`，自动继承出站代理与 10s connect timeout）。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;

use crate::codec::openai::url_join;
use crate::netguard;

/// 单端点探测超时。中转站冷启动偶尔要十几秒，所以给得比 connect timeout 宽。
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// 一次探测（所有端点）的总耗时上限。
pub const PROBE_TOTAL_CAP: Duration = Duration::from_secs(60);
/// 回传 / 落日志的 message 长度上限（UI 要 hover 展示，不能无界）。
pub const PROBE_MESSAGE_MAX: usize = 300;
/// receipt 有效期：超过这个时间的探测结论不再被信任。
pub const RECEIPT_TTL_MS: i64 = 30 * 60 * 1000;

// ================================================================ 输入 / 输出

/// 探测目标（不落库，纯草稿）。
#[derive(Debug, Clone)]
pub struct ProbeTarget {
    /// 四族之一（`openai_compat` / `openai_responses` / `anthropic` / `gemini`）
    pub family: String,
    /// 已 normalize 的 base_url（尾部斜杠已去掉）
    pub base_url: String,
    pub api_key: Option<String>,
    /// 必填：探测必须有模型名
    pub model: String,
    /// JSON map，与 provider 的 `extra_headers` 同格式
    pub extra_headers: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus {
    Passed,
    Failed,
    Skipped,
}

/// 失败分类（对用户可解释）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeCategory {
    /// 401 / 403
    Authentication,
    /// 404 且正文指向模型
    Model,
    /// 404 / 405 / 501，端点本身不认
    EndpointUnsupported,
    /// 400 / 422 其它
    Request,
    /// 429
    RateLimit,
    /// 5xx / 529
    Overloaded,
    /// 超时
    Timeout,
    /// 连接失败 / DNS
    Network,
    /// 2xx 但 body 不是预期结构（拦截页等）
    Protocol,
    /// 被 SSRF 校验拦下
    UrlBlocked,
    /// 未填模型 → Skipped
    NoModel,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeOutcome {
    pub endpoint: &'static str,
    pub status: ProbeStatus,
    pub category: Option<ProbeCategory>,
    /// 已脱敏（不含 key）、已截断
    pub message: String,
    pub latency_ms: u64,
    pub tested_model: Option<String>,
    /// 这次探测是否**可能真的产生了计费**（真把推理请求发出去了）
    pub cost_possible: bool,
    /// 附加信息性探测（如 `openai_compat` 渠道额外探一次 `/responses`）。
    /// **不进「通过」门禁** —— 它只回答「这个中转站能不能接 Codex」。
    pub informational: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftProbeReport {
    pub run_id: String,
    pub tested_at: i64,
    pub fingerprint: String,
    pub results: Vec<ProbeOutcome>,
}

#[derive(Debug, Clone)]
pub struct ProbeConfig {
    pub per_probe_timeout: Duration,
    pub total_cap: Duration,
    pub allow_loopback: bool,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            per_probe_timeout: PROBE_TIMEOUT,
            total_cap: PROBE_TOTAL_CAP,
            allow_loopback: false,
        }
    }
}

// ================================================================ 端点与 payload 派生

/// 鉴权头拼法（照抄 `InboundWire::apply_auth` 与 `discover.rs` 的口径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Auth {
    /// `Authorization: Bearer <key>`
    Bearer,
    /// `x-api-key: <key>` + `anthropic-version`
    AnthropicKey,
    /// `x-goog-api-key: <key>`（头传递，避免 key 落进访问日志）
    GoogleKey,
}

/// 「通过」判定要看的顶层结构。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// chat：`choices` 数组
    Choices,
    /// responses：`output` 数组 或 `status` 字符串
    OutputOrStatus,
    /// messages：`content` 数组
    Content,
    /// gemini：`candidates` 数组
    Candidates,
}

struct ProbeEndpoint {
    endpoint: &'static str,
    path: String,
    body: Value,
    auth: Auth,
    expect: Expect,
    informational: bool,
}

/// 按 family 派生要探测的端点。
///
/// provider 只有 `base_url + family`，没有「端点」概念，所以从 family 推导；
/// 引入 `native_endpoints` 是另一个迭代的事（刻意的范围收敛）。
fn endpoints_for(family: &str, model: &str) -> Vec<ProbeEndpoint> {
    let m = model.to_string();
    match family {
        "openai_compat" => vec![
            ProbeEndpoint {
                endpoint: "chat_completions",
                path: "/chat/completions".into(),
                body: json!({
                    "model": m,
                    "messages": [{"role": "user", "content": "ping"}],
                    "max_tokens": 1,
                    "stream": false
                }),
                auth: Auth::Bearer,
                expect: Expect::Choices,
                informational: false,
            },
            // 附加信息性探测：这个中转站能不能接 Codex（Responses 线）。
            // 失败不影响「通过」，只在 UI 上灰显。
            ProbeEndpoint {
                endpoint: "responses",
                path: "/responses".into(),
                body: json!({
                    "model": m,
                    "input": "ping",
                    "max_output_tokens": 1,
                    "stream": false
                }),
                auth: Auth::Bearer,
                expect: Expect::OutputOrStatus,
                informational: true,
            },
        ],
        "openai_responses" => vec![ProbeEndpoint {
            endpoint: "responses",
            path: "/responses".into(),
            body: json!({
                "model": m,
                "input": "ping",
                "max_output_tokens": 1,
                "stream": false
            }),
            auth: Auth::Bearer,
            expect: Expect::OutputOrStatus,
            informational: false,
        }],
        "anthropic" => vec![ProbeEndpoint {
            endpoint: "messages",
            path: "/v1/messages".into(),
            body: json!({
                "model": m,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "ping"}],
                "stream": false
            }),
            auth: Auth::AnthropicKey,
            expect: Expect::Content,
            informational: false,
        }],
        "gemini" => vec![ProbeEndpoint {
            endpoint: "gemini_generate",
            path: format!("/v1beta/models/{m}:generateContent"),
            body: json!({
                "contents": [{"parts": [{"text": "ping"}]}],
                "generationConfig": {"maxOutputTokens": 1}
            }),
            auth: Auth::GoogleKey,
            expect: Expect::Candidates,
            informational: false,
        }],
        _ => Vec::new(),
    }
}

/// family 对应的主端点名（用于「没发请求就失败」的行也能给出可读标签）。
fn primary_endpoint_label(family: &str) -> &'static str {
    match family {
        "openai_compat" => "chat_completions",
        "openai_responses" => "responses",
        "anthropic" => "messages",
        "gemini" => "gemini_generate",
        _ => "unknown",
    }
}

fn apply_auth(
    req: reqwest::RequestBuilder,
    auth: Auth,
    key: Option<&str>,
) -> reqwest::RequestBuilder {
    let Some(k) = key.filter(|k| !k.is_empty()) else {
        return req;
    };
    match auth {
        Auth::Bearer => req.bearer_auth(k),
        Auth::AnthropicKey => req
            .header("x-api-key", k)
            .header("anthropic-version", crate::discover::ANTHROPIC_VERSION),
        Auth::GoogleKey => req.header("x-goog-api-key", k),
    }
}

/// 与 `proxy.rs` 同口径：extra_headers 是 JSON map，值非字符串的项忽略。
fn apply_extra_headers(req: reqwest::RequestBuilder, raw: Option<&str>) -> reqwest::RequestBuilder {
    let Some(raw) = raw else { return req };
    let Ok(map) = serde_json::from_str::<serde_json::Map<String, Value>>(raw) else {
        return req;
    };
    let mut out = req;
    for (k, v) in map {
        if let Some(s) = v.as_str() {
            if let (Ok(name), Ok(val)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(s),
            ) {
                out = out.header(name, val);
            }
        }
    }
    out
}

// ================================================================ 结论分类（纯函数）

/// 依据上游状态码 + 响应体判定探测结论。
///
/// 2xx **且** body 能解析出该协议的预期最小结构才算「通过」。
/// 只判 2xx 不够 —— 上游返回 HTML 拦截页也是 200（我们踩过，见
/// `docs/bug和优化清单.md`）。
pub fn classify_probe_response(
    status: u16,
    body: &str,
) -> (ProbeStatus, Option<ProbeCategory>, String) {
    if (200..300).contains(&status) {
        return classify_probe_success(body);
    }
    let category = match status {
        401 | 403 => ProbeCategory::Authentication,
        404 | 405 | 501 => {
            if looks_like_model_issue(body) {
                ProbeCategory::Model
            } else {
                ProbeCategory::EndpointUnsupported
            }
        }
        429 => ProbeCategory::RateLimit,
        400 | 422 => ProbeCategory::Request,
        s if (500..600).contains(&s) => ProbeCategory::Overloaded,
        s if (400..500).contains(&s) => ProbeCategory::Request,
        _ => ProbeCategory::Protocol,
    };
    (
        ProbeStatus::Failed,
        Some(category),
        format!("HTTP {status}：{}", summarize_body(body)),
    )
}

/// 2xx 的判定：合法 JSON 且不含 `error` 才算走到「可能通过」这一步。
/// 预期结构的校验在 [`structure_ok`]（需要端点类型，故由调用方判）。
fn classify_probe_success(body: &str) -> (ProbeStatus, Option<ProbeCategory>, String) {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return (
            ProbeStatus::Failed,
            Some(ProbeCategory::Protocol),
            "HTTP 200 但响应体为空".into(),
        );
    }
    let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
        // 最常见：网关 / CDN 的 HTML 拦截页
        return (
            ProbeStatus::Failed,
            Some(ProbeCategory::Protocol),
            format!(
                "响应不是合法 JSON（可能是拦截页 / HTML）：{}",
                summarize_body(trimmed)
            ),
        );
    };
    // 有些中转站用 200 + error 对象报错
    if let Some(err) = v.get("error") {
        if !err.is_null() {
            let msg = err
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| err.to_string());
            return (
                ProbeStatus::Failed,
                Some(classify_error_text(&msg)),
                format!("上游以 200 返回错误：{msg}"),
            );
        }
    }
    (ProbeStatus::Passed, None, "通过".into())
}

/// 2xx 的 body 是否含该端点的预期最小结构。
fn structure_ok(v: &Value, expect: Expect) -> bool {
    let has_array = |key: &str| v.get(key).map(Value::is_array).unwrap_or(false);
    match expect {
        Expect::Choices => has_array("choices"),
        Expect::Content => {
            has_array("content") || v.get("type").and_then(Value::as_str) == Some("message")
        }
        Expect::Candidates => has_array("candidates"),
        Expect::OutputOrStatus => {
            has_array("output") || v.get("status").and_then(Value::as_str).is_some()
        }
    }
}

/// 正文里是否有「模型不存在」的语义（决定 404 判 `Model` 还是 `EndpointUnsupported`）。
///
/// 分两种正文形态：JSON 错误体可以看宽一点（`not found` 基本就是在说模型）；
/// 非 JSON（HTML 拦截页 / 空体）里出现 `Not Found` 只是端点级的 404 提示，
/// 一律按 `EndpointUnsupported` —— 否则 nginx 的 `<html>404 Not Found</html>`
/// 会被误报成「模型不存在」，把用户引到错误的排查方向。
fn looks_like_model_issue(body: &str) -> bool {
    let b = body.to_ascii_lowercase();
    let t = b.trim_start();
    let json_like = t.starts_with('{') || t.starts_with('[');
    let strong = ["model", "模型", "deployment", "does not exist", "no such"];
    if strong.iter().any(|k| b.contains(k)) {
        return true;
    }
    json_like && b.contains("not found")
}

/// 从错误文本推断分类（用于 200 + error 对象这类「状态码没说实话」的情形）。
fn classify_error_text(msg: &str) -> ProbeCategory {
    let m = msg.to_ascii_lowercase();
    if ["model", "模型", "deployment", "not found", "does not exist"]
        .iter()
        .any(|k| m.contains(k))
    {
        return ProbeCategory::Model;
    }
    if [
        "api key",
        "apikey",
        "unauthorized",
        "invalid key",
        "authentication",
        "鉴权",
        "密钥",
    ]
    .iter()
    .any(|k| m.contains(k))
    {
        return ProbeCategory::Authentication;
    }
    if [
        "rate limit",
        "rate_limit",
        "quota",
        "too many requests",
        "限速",
        "额度",
    ]
    .iter()
    .any(|k| m.contains(k))
    {
        return ProbeCategory::RateLimit;
    }
    if ["overload", "busy", "capacity", "过载"]
        .iter()
        .any(|k| m.contains(k))
    {
        return ProbeCategory::Overloaded;
    }
    ProbeCategory::Request
}

/// 把响应体压成一行摘要（供 message 用），去掉控制字符与多余空白。
fn summarize_body(body: &str) -> String {
    let one_line: String = body
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let squeezed = one_line.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&squeezed, 160)
}

/// 按字符（不是字节）截断，避免把中文切成半个字。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// 脱敏：抹掉明文密钥 + 常见 key 形态，再截断到 [`PROBE_MESSAGE_MAX`]。
///
/// 上游的错误体经常把请求回显回来（含鉴权头或 URL 里的 key），
/// 而这条 message 会进 UI 也会进日志，必须先洗一遍。
pub fn sanitize_message(msg: &str, secret: Option<&str>) -> String {
    let mut out = msg.to_string();
    if let Some(s) = secret.filter(|s| !s.is_empty()) {
        out = out.replace(s, "***");
    }
    out = redact_key_like(&out);
    truncate_chars(&out, PROBE_MESSAGE_MAX)
}

/// 把形如 `sk-xxxx` / `AIza...` / `Bearer xxx` 的片段替换成占位符。
fn redact_key_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for token in s.split_inclusive(char::is_whitespace) {
        let bare = token.trim_end();
        let tail = &token[bare.len()..];
        if is_key_like(bare) {
            out.push_str("***");
        } else {
            out.push_str(bare);
        }
        out.push_str(tail);
    }
    out
}

fn is_key_like(token: &str) -> bool {
    let t = token.trim_matches(|c: char| {
        c == '"' || c == '\'' || c == ',' || c == ';' || c == ':' || c == '(' || c == ')'
    });
    ["sk-", "sk_", "AIza", "xai-", "Bearer "]
        .iter()
        .any(|p| t.starts_with(p))
        && t.len() >= 12
}

// ================================================================ 探测执行

/// 探测草稿渠道。**永不返回 Err** —— 所有失败都变成 `results` 里的一行：
/// 「地址被拦」「连不上」对用户是有意义的结论，不是内部错误。
pub async fn probe_draft(
    client: &reqwest::Client,
    target: &ProbeTarget,
    cfg: &ProbeConfig,
) -> DraftProbeReport {
    let started = Instant::now();
    let tested_at = crate::store::now_ms();
    let fingerprint = compute_probe_fingerprint(target, cfg);
    let label = primary_endpoint_label(&target.family);
    let mut results = Vec::new();

    if target.model.trim().is_empty() {
        results.push(ProbeOutcome {
            endpoint: label,
            status: ProbeStatus::Skipped,
            category: Some(ProbeCategory::NoModel),
            message: "未填写模型，无法探测端点".into(),
            latency_ms: 0,
            tested_model: None,
            cost_possible: false,
            informational: false,
        });
        return finish(results, tested_at, fingerprint);
    }

    // SSRF 校验：**发请求之前**。被拦下的地址一个字节都不发。
    let base = match netguard::validate_outbound_url(&target.base_url, cfg.allow_loopback) {
        Ok(u) => u,
        Err(msg) => {
            results.push(ProbeOutcome {
                endpoint: label,
                status: ProbeStatus::Failed,
                category: Some(ProbeCategory::UrlBlocked),
                message: sanitize_message(&msg, target.api_key.as_deref()),
                latency_ms: 0,
                tested_model: None,
                cost_possible: false,
                informational: false,
            });
            return finish(results, tested_at, fingerprint);
        }
    };

    let endpoints = endpoints_for(&target.family, target.model.trim());
    if endpoints.is_empty() {
        results.push(ProbeOutcome {
            endpoint: "unknown",
            status: ProbeStatus::Failed,
            category: Some(ProbeCategory::Protocol),
            message: format!("未知协议族 {}：无法派生探测端点", target.family),
            latency_ms: 0,
            tested_model: None,
            cost_possible: false,
            informational: false,
        });
        return finish(results, tested_at, fingerprint);
    }

    for ep in &endpoints {
        if started.elapsed() >= cfg.total_cap {
            results.push(ProbeOutcome {
                endpoint: ep.endpoint,
                status: ProbeStatus::Skipped,
                category: Some(ProbeCategory::Timeout),
                message: "本次探测总耗时已达上限，该端点未探测".into(),
                latency_ms: 0,
                tested_model: Some(target.model.clone()),
                cost_possible: false,
                informational: ep.informational,
            });
            continue;
        }
        results.push(run_probe(client, &base, target, ep, cfg).await);
    }
    finish(results, tested_at, fingerprint)
}

fn finish(results: Vec<ProbeOutcome>, tested_at: i64, fingerprint: String) -> DraftProbeReport {
    DraftProbeReport {
        // 全仓统一用 v7（时间有序，索引友好）；workspace 只开了 v7 feature
        run_id: uuid::Uuid::now_v7().to_string(),
        tested_at,
        fingerprint,
        results,
    }
}

async fn run_probe(
    client: &reqwest::Client,
    base: &Url,
    target: &ProbeTarget,
    ep: &ProbeEndpoint,
    cfg: &ProbeConfig,
) -> ProbeOutcome {
    let url = url_join(base.as_str(), &ep.path);
    let req = client
        .post(&url)
        .timeout(cfg.per_probe_timeout)
        .json(&ep.body);
    let req = apply_auth(req, ep.auth, target.api_key.as_deref());
    let req = apply_extra_headers(req, target.extra_headers.as_deref());

    let t0 = Instant::now();
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            let (category, msg) = classify_transport_error(&e);
            return outcome(
                ep,
                target,
                ProbeStatus::Failed,
                Some(category),
                msg,
                t0.elapsed(),
            );
        }
    };
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    let (st, cat, msg) = classify_probe_response(status, &body);

    // 2xx 但结构不对 → 判 Protocol 失败（防「假绿」）
    let (st, cat, msg) = match (st, cat) {
        (ProbeStatus::Passed, _) => {
            let ok = serde_json::from_str::<Value>(&body)
                .map(|v| structure_ok(&v, ep.expect))
                .unwrap_or(false);
            if ok {
                (ProbeStatus::Passed, None, "通过".to_string())
            } else {
                (
                    ProbeStatus::Failed,
                    Some(ProbeCategory::Protocol),
                    format!(
                        "HTTP {status} 但响应缺少 {} 结构（端点可能不是该协议）",
                        expect_label(ep.expect)
                    ),
                )
            }
        }
        (st2, cat2) => (st2, cat2, msg),
    };

    outcome(ep, target, st, cat, msg, t0.elapsed())
}

fn expect_label(expect: Expect) -> &'static str {
    match expect {
        Expect::Choices => "choices",
        Expect::Content => "content",
        Expect::Candidates => "candidates",
        Expect::OutputOrStatus => "output / status",
    }
}

fn classify_transport_error(e: &reqwest::Error) -> (ProbeCategory, String) {
    if e.is_timeout() {
        (ProbeCategory::Timeout, format!("请求超时：{e}"))
    } else if e.is_connect() {
        (ProbeCategory::Network, format!("连接失败：{e}"))
    } else {
        (ProbeCategory::Network, format!("请求失败：{e}"))
    }
}

fn outcome(
    ep: &ProbeEndpoint,
    target: &ProbeTarget,
    status: ProbeStatus,
    category: Option<ProbeCategory>,
    message: String,
    elapsed: Duration,
) -> ProbeOutcome {
    ProbeOutcome {
        endpoint: ep.endpoint,
        status,
        category,
        message: sanitize_message(&message, target.api_key.as_deref()),
        latency_ms: elapsed.as_millis() as u64,
        tested_model: Some(target.model.clone()),
        // 只有真的把请求发出去过，才可能产生计费
        cost_possible: true,
        informational: ep.informational,
    }
}

// ================================================================ 指纹与 receipt

/// 不可逆指纹：family + 规范化 URL + model + 超时 + allow_loopback + SHA256(api_key)。
///
/// 用途是把「这次探测」和「这次保存」对上。receipt **不落库**：进程内存储、重启即失效，
/// 这是刻意的 —— 半年前测过的渠道不该被信任。
pub fn compute_probe_fingerprint(target: &ProbeTarget, cfg: &ProbeConfig) -> String {
    let mut h = Sha256::new();
    h.update(b"jai-probe-v1\0");
    h.update(target.family.as_bytes());
    h.update(b"\0");
    h.update(target.base_url.trim().trim_end_matches('/').as_bytes());
    h.update(b"\0");
    h.update(target.model.trim().as_bytes());
    h.update(b"\0");
    h.update(cfg.per_probe_timeout.as_millis().to_string().as_bytes());
    h.update(b"\0");
    h.update([cfg.allow_loopback as u8]);
    h.update(b"\0");
    h.update(target.extra_headers.as_deref().unwrap_or("").as_bytes());
    h.update(b"\0");
    // 密钥只以摘要形式参与，指纹里不含明文
    let mut kh = Sha256::new();
    kh.update(target.api_key.as_deref().unwrap_or("").as_bytes());
    h.update(hex(&kh.finalize()));
    hex(&h.finalize())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// 进程内 receipt：只记住「刚刚真的探测通过」这件事，不落库。
#[derive(Default)]
pub struct ProbeReceiptStore {
    inner: Mutex<HashMap<String, (i64, DraftProbeReport)>>,
}

impl ProbeReceiptStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记下一次探测结论（按指纹覆盖，天然只留最新一次）。
    pub fn put(&self, report: &DraftProbeReport) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.insert(
            report.fingerprint.clone(),
            (report.tested_at, report.clone()),
        );
    }

    /// 该指纹是否可作「通过」凭据。
    ///
    /// 三个条件：① 存在且未过期（[`RECEIPT_TTL_MS`]）；② 至少有一个**非信息性**端点
    /// 真的通过（全是 Skipped 不算 —— 那等于什么都没测）；③ 没有任何非信息性端点失败。
    /// 信息性探测（`informational`）不参与门禁。
    pub fn validate(&self, fingerprint: &str, now_ms: i64) -> bool {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let Some((at, report)) = g.get(fingerprint) else {
            return false;
        };
        if now_ms.saturating_sub(*at) > RECEIPT_TTL_MS {
            return false;
        }
        let real: Vec<&ProbeOutcome> = report.results.iter().filter(|o| !o.informational).collect();
        !real.is_empty()
            && real.iter().any(|o| o.status == ProbeStatus::Passed)
            && real.iter().all(|o| o.status != ProbeStatus::Failed)
    }

    /// 清掉过期项（避免进程长期运行后无界增长）。
    pub fn prune(&self, now_ms: i64) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.retain(|_, (at, _)| now_ms.saturating_sub(*at) <= RECEIPT_TTL_MS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(family: &str) -> ProbeTarget {
        ProbeTarget {
            family: family.into(),
            base_url: "https://api.example.com/v1".into(),
            api_key: Some("sk-secret-abcdefghijklmnop".into()),
            model: "m-test".into(),
            extra_headers: None,
        }
    }

    // ---------------- payload 与鉴权派生矩阵

    #[test]
    fn payloads_are_minimal_and_stream_off() {
        let cases = [
            (
                "openai_compat",
                "chat_completions",
                "max_tokens",
                Auth::Bearer,
            ),
            (
                "openai_responses",
                "responses",
                "max_output_tokens",
                Auth::Bearer,
            ),
            ("anthropic", "messages", "max_tokens", Auth::AnthropicKey),
        ];
        for (family, endpoint, token_key, auth) in cases {
            let eps = endpoints_for(family, "gpt-x");
            let ep = eps.iter().find(|e| e.endpoint == endpoint).unwrap();
            assert_eq!(ep.body.get("stream"), Some(&json!(false)), "{family}");
            assert_eq!(ep.body.get(token_key), Some(&json!(1)), "{family}");
            assert_eq!(
                ep.body.get("model"),
                Some(&json!("gpt-x")),
                "{family} 必须带上用户填的模型名"
            );
            assert_eq!(ep.auth, auth, "{family} 鉴权头拼法");
            assert!(!ep.informational);
        }
        // gemini：路径含模型名 + maxOutputTokens=1 + 头传 key
        let eps = endpoints_for("gemini", "gemini-2.0-flash");
        assert_eq!(
            eps[0].path,
            "/v1beta/models/gemini-2.0-flash:generateContent"
        );
        assert_eq!(eps[0].body["generationConfig"]["maxOutputTokens"], json!(1));
        assert_eq!(eps[0].auth, Auth::GoogleKey);
        assert!(
            eps[0].body.get("stream").is_none(),
            "gemini 没有 stream 字段"
        );
    }

    #[test]
    fn openai_compat_gets_informational_responses_probe() {
        let eps = endpoints_for("openai_compat", "m");
        assert_eq!(eps.len(), 2);
        assert!(!eps[0].informational, "主探测是 chat_completions");
        assert!(eps[1].informational, "responses 是信息性探测");
        assert_eq!(eps[1].endpoint, "responses");
        // 其它族没有附加探测
        assert_eq!(endpoints_for("anthropic", "m").len(), 1);
        assert!(endpoints_for("unknown_family", "m").is_empty());
    }

    #[test]
    fn endpoint_labels_cover_all_families() {
        for (f, label) in [
            ("openai_compat", "chat_completions"),
            ("openai_responses", "responses"),
            ("anthropic", "messages"),
            ("gemini", "gemini_generate"),
        ] {
            assert_eq!(primary_endpoint_label(f), label);
        }
        assert_eq!(primary_endpoint_label("nope"), "unknown");
    }

    // ---------------- 失败分类矩阵

    #[test]
    fn auth_rate_limit_and_overload_statuses() {
        let cases = [
            (401, ProbeCategory::Authentication),
            (403, ProbeCategory::Authentication),
            (429, ProbeCategory::RateLimit),
            (500, ProbeCategory::Overloaded),
            (503, ProbeCategory::Overloaded),
            (529, ProbeCategory::Overloaded),
            (400, ProbeCategory::Request),
            (422, ProbeCategory::Request),
        ];
        for (status, want) in cases {
            let (st, cat, _) = classify_probe_response(status, "{}");
            assert_eq!(st, ProbeStatus::Failed, "status {status}");
            assert_eq!(cat, Some(want), "status {status}");
        }
    }

    #[test]
    fn not_found_splits_between_model_and_endpoint() {
        // 正文指向模型 → Model
        for body in [
            r#"{"error":{"message":"The model `gpt-9` does not exist"}}"#,
            r#"{"error":{"message":"model not found"}}"#,
            r#"{"error":{"message":"模型不存在"}}"#,
        ] {
            let (_, cat, _) = classify_probe_response(404, body);
            assert_eq!(cat, Some(ProbeCategory::Model), "{body}");
        }
        // 正文不像模型问题（例如 nginx 的 HTML 404）→ EndpointUnsupported
        for body in ["<html>404 Not Found</html>", "", "{}"] {
            let (_, cat, _) = classify_probe_response(404, body);
            assert_eq!(cat, Some(ProbeCategory::EndpointUnsupported), "{body}");
        }
        assert_eq!(
            classify_probe_response(405, "").1,
            Some(ProbeCategory::EndpointUnsupported)
        );
        assert_eq!(
            classify_probe_response(501, "{}").1,
            Some(ProbeCategory::EndpointUnsupported)
        );
    }

    /// 这条最重要：网关 / CDN 的拦截页也是 200，只判 2xx 会「假绿」。
    #[test]
    fn html_intercept_page_on_200_is_protocol_failure() {
        let (st, cat, msg) =
            classify_probe_response(200, "<html><body>Access denied</body></html>");
        assert_eq!(st, ProbeStatus::Failed);
        assert_eq!(cat, Some(ProbeCategory::Protocol));
        assert!(msg.contains("不是合法 JSON"), "{msg}");

        // 空 body 同样是失败
        let (st, cat, _) = classify_probe_response(200, "   ");
        assert_eq!(st, ProbeStatus::Failed);
        assert_eq!(cat, Some(ProbeCategory::Protocol));
    }

    /// 200 + error 对象（状态码没说实话的中转站）。
    #[test]
    fn error_object_inside_200_is_classified() {
        let (st, cat, msg) =
            classify_probe_response(200, r#"{"error":{"message":"invalid api key provided"}}"#);
        assert_eq!(st, ProbeStatus::Failed);
        assert_eq!(cat, Some(ProbeCategory::Authentication));
        assert!(msg.contains("200 返回错误"), "{msg}");

        let (_, cat, _) = classify_probe_response(200, r#"{"error":{"message":"模型不存在"}}"#);
        assert_eq!(cat, Some(ProbeCategory::Model));

        let (_, cat, _) =
            classify_probe_response(200, r#"{"error":{"message":"rate limit exceeded"}}"#);
        assert_eq!(cat, Some(ProbeCategory::RateLimit));

        let (_, cat, _) = classify_probe_response(200, r#"{"error":{"message":"whatever"}}"#);
        assert_eq!(cat, Some(ProbeCategory::Request));

        // error: null 不算错（有些上游恒带该字段）
        let (st, _, _) = classify_probe_response(200, r#"{"error":null,"choices":[]}"#);
        assert_eq!(st, ProbeStatus::Passed);
    }

    #[test]
    fn expected_structure_matrix() {
        assert!(structure_ok(&json!({"choices": []}), Expect::Choices));
        assert!(!structure_ok(&json!({"data": []}), Expect::Choices));

        assert!(structure_ok(&json!({"content": []}), Expect::Content));
        assert!(structure_ok(&json!({"type": "message"}), Expect::Content));
        assert!(!structure_ok(&json!({"completion": "x"}), Expect::Content));

        assert!(structure_ok(&json!({"output": []}), Expect::OutputOrStatus));
        assert!(structure_ok(
            &json!({"status": "completed"}),
            Expect::OutputOrStatus
        ));
        assert!(!structure_ok(&json!({"id": "x"}), Expect::OutputOrStatus));

        assert!(structure_ok(&json!({"candidates": []}), Expect::Candidates));
        assert!(!structure_ok(&json!({"choices": []}), Expect::Candidates));
    }

    // ---------------- 脱敏与截断

    #[test]
    fn sanitize_hides_secret_and_key_like_tokens() {
        let secret = "sk-secret-abcdefghijklmnop";
        // 明文密钥（上游把请求回显了）
        let out = sanitize_message(
            &format!("upstream said: Authorization: Bearer {secret} is bad"),
            Some(secret),
        );
        assert!(!out.contains(secret), "{out}");
        assert!(out.contains("***"), "{out}");

        // 没给 secret 时也要按形态抹掉
        let out = sanitize_message("key sk-abcdefghijklmnop leaked", None);
        assert!(!out.contains("sk-abcdefghijklmnop"), "{out}");
        let out = sanitize_message("key AIzaSyABCDEFGHIJKLMNOP leaked", None);
        assert!(!out.contains("AIzaSyABCDEFGHIJKLMNOP"), "{out}");

        // 短 token / 正常文本不受影响
        let out = sanitize_message("model sk-1 not found", None);
        assert_eq!(out, "model sk-1 not found");
    }

    #[test]
    fn sanitize_truncates_by_chars() {
        let long = "中".repeat(PROBE_MESSAGE_MAX + 50);
        let out = sanitize_message(&long, None);
        assert_eq!(out.chars().count(), PROBE_MESSAGE_MAX + 1, "截断后带省略号");
        assert!(out.ends_with('…'));
        assert_eq!(sanitize_message("ok", None), "ok");
    }

    #[test]
    fn summarize_body_squeezes_and_strips_controls() {
        assert_eq!(summarize_body("a\n\n  b\tc"), "a b c");
    }

    // ---------------- 指纹

    #[test]
    fn fingerprint_is_stable_and_irreversible() {
        let cfg = ProbeConfig::default();
        let a = compute_probe_fingerprint(&target("openai_compat"), &cfg);
        let b = compute_probe_fingerprint(&target("openai_compat"), &cfg);
        assert_eq!(a, b, "同输入必须同指纹");
        assert_eq!(a.len(), 64, "sha256 hex");
        // 不可逆：指纹里不含明文 key 的任何片段
        assert!(!a.contains("sk-secret"), "{a}");
        assert!(!a.contains("abcdefghijklmnop"), "{a}");
    }

    #[test]
    fn fingerprint_changes_with_any_input() {
        let cfg = ProbeConfig::default();
        let base_fp = compute_probe_fingerprint(&target("openai_compat"), &cfg);

        let mut t = target("openai_compat");
        t.api_key = Some("sk-other-abcdefghijklmnop".into());
        assert_ne!(compute_probe_fingerprint(&t, &cfg), base_fp, "换 key");

        let mut t = target("openai_compat");
        t.model = "other-model".into();
        assert_ne!(compute_probe_fingerprint(&t, &cfg), base_fp, "换模型");

        let mut t = target("openai_compat");
        t.base_url = "https://api.other.com/v1".into();
        assert_ne!(compute_probe_fingerprint(&t, &cfg), base_fp, "换 URL");

        assert_ne!(
            compute_probe_fingerprint(&target("anthropic"), &cfg),
            base_fp,
            "换协议族"
        );

        let mut c = cfg.clone();
        c.allow_loopback = true;
        assert_ne!(
            compute_probe_fingerprint(&target("openai_compat"), &c),
            base_fp,
            "换 allow_loopback"
        );

        let mut c = cfg.clone();
        c.per_probe_timeout = Duration::from_secs(3);
        assert_ne!(
            compute_probe_fingerprint(&target("openai_compat"), &c),
            base_fp,
            "换超时"
        );
    }

    #[test]
    fn fingerprint_ignores_trailing_slash_and_padding() {
        let cfg = ProbeConfig::default();
        let a = compute_probe_fingerprint(&target("openai_compat"), &cfg);
        let mut t = target("openai_compat");
        t.base_url = "https://api.example.com/v1/".into();
        assert_eq!(compute_probe_fingerprint(&t, &cfg), a, "尾部斜杠不影响");
        t.base_url = "  https://api.example.com/v1  ".into();
        assert_eq!(compute_probe_fingerprint(&t, &cfg), a, "首尾空白不影响");
    }

    // ---------------- receipt

    fn report(fp: &str, at: i64, results: Vec<ProbeOutcome>) -> DraftProbeReport {
        DraftProbeReport {
            run_id: "r".into(),
            tested_at: at,
            fingerprint: fp.into(),
            results,
        }
    }

    fn outcome_of(status: ProbeStatus, informational: bool) -> ProbeOutcome {
        ProbeOutcome {
            endpoint: "chat_completions",
            status,
            category: None,
            message: String::new(),
            latency_ms: 1,
            tested_model: Some("m".into()),
            cost_possible: true,
            informational,
        }
    }

    #[test]
    fn receipt_accepts_passed_and_rejects_failed_or_missing() {
        let store = ProbeReceiptStore::new();
        let now = 1_000_000;

        assert!(!store.validate("fp-none", now), "没探测过 → 不通过");

        store.put(&report(
            "fp-ok",
            now,
            vec![outcome_of(ProbeStatus::Passed, false)],
        ));
        assert!(store.validate("fp-ok", now));

        store.put(&report(
            "fp-bad",
            now,
            vec![
                outcome_of(ProbeStatus::Passed, false),
                outcome_of(ProbeStatus::Failed, false),
            ],
        ));
        assert!(!store.validate("fp-bad", now), "有失败 → 不通过");
    }

    /// 全是 Skipped（例如没填模型）不能算「通过」—— 那等于什么都没测。
    #[test]
    fn receipt_rejects_all_skipped() {
        let store = ProbeReceiptStore::new();
        let now = 1_000_000;
        store.put(&report(
            "fp-skip",
            now,
            vec![outcome_of(ProbeStatus::Skipped, false)],
        ));
        assert!(!store.validate("fp-skip", now));
    }

    /// 信息性探测（openai_compat 的 /responses）失败不该挡住保存。
    #[test]
    fn informational_failure_does_not_block_receipt() {
        let store = ProbeReceiptStore::new();
        let now = 1_000_000;
        store.put(&report(
            "fp-info",
            now,
            vec![
                outcome_of(ProbeStatus::Passed, false),
                outcome_of(ProbeStatus::Failed, true),
            ],
        ));
        assert!(store.validate("fp-info", now));
    }

    #[test]
    fn receipt_expires_and_prunes() {
        let store = ProbeReceiptStore::new();
        let at = 1_000_000;
        store.put(&report(
            "fp-old",
            at,
            vec![outcome_of(ProbeStatus::Passed, false)],
        ));
        assert!(
            store.validate("fp-old", at + RECEIPT_TTL_MS),
            "TTL 边界内仍有效"
        );
        assert!(
            !store.validate("fp-old", at + RECEIPT_TTL_MS + 1),
            "超过 TTL 失效"
        );

        store.prune(at + RECEIPT_TTL_MS + 1);
        assert!(!store.validate("fp-old", at), "prune 后彻底消失");
    }

    #[test]
    fn receipt_overwrites_same_fingerprint() {
        let store = ProbeReceiptStore::new();
        let now = 1_000_000;
        store.put(&report(
            "fp",
            now,
            vec![outcome_of(ProbeStatus::Failed, false)],
        ));
        assert!(!store.validate("fp", now));
        // 再测一次通过了 → 覆盖
        store.put(&report(
            "fp",
            now + 1,
            vec![outcome_of(ProbeStatus::Passed, false)],
        ));
        assert!(store.validate("fp", now + 1));
    }
}
