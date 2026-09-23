//! OpenAI / Anthropic 同族直通代理（M1 直通 + M2 多渠道故障转移 + M3 Anthropic 入站）。
//!
//! 约束（roadmap M1/M2/M3）：
//! - 同族字节级透传（验收：上游收到的 body SHA-256 == 客户端发出值）
//! - 多渠道：按 `priority, rowid` 序逐渠道尝试；{连接拒绝、超时、UpstreamAuth、
//!   RateLimit、Overloaded、上游 5xx} → 下一渠道；InvalidRequest / ContextTooLong
//!   → 即刻返回不切换；**首个字节下发下游后禁止切换**
//! - SSE 全程管道转发 + usage 旁路扫描落日志（绝不反压客户端）
//! - 超时三件套：上游连接 10s（client 构造）、首字节 60s、流空闲 120s
//! - Anthropic 线：x-api-key + anthropic-version 头（缺省注入默认版本）、
//!   错误 Anthropic 化（type:error）、Overloaded→HTTP 529

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::{to_bytes, Body};
use axum::extract::{ConnectInfo, Extension, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::codec::anthropic as anthropic_codec;
use crate::codec::openai::{error_body, extract_usage, peek, url_join, PeekRequest, UsageScanner};
use crate::router;
use crate::store::keyrules::KeyRules;
use crate::store::logs::LogEvent;
use crate::store::{self, Db};

use super::ratelimit::BanStatus;
use super::security::{self, CorsAllowlist};

// ---------------------------------------------------------------- 配置常量

/// 上游首字节超时（稳定性基线 §5-2）
pub const UPSTREAM_FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(60);
/// 流空闲读取超时（连续两个数据块之间）
pub const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// 非流式响应整体读取上限
pub const NONSTREAM_READ_TIMEOUT: Duration = Duration::from_secs(300);
/// 下游请求体上限（32MB，图片 base64 场景兜底）
pub const MAX_REQUEST_BODY: usize = 32 * 1024 * 1024;
/// 上游错误体读取上限（原样转发用）
const MAX_ERROR_BODY: usize = 1024 * 1024;
/// SSE 转换路径单行缓冲上限：上游无换行刷流时断开，防无界内存
/// （roadmap 稳定性 finding「无终止标记流」修复）。
pub const MAX_SSE_LINE_BYTES: usize = 1 << 20; // 1 MiB
/// SSE 行长时间未完成（一直收字节但无换行/终止标记）断开阈值；
/// 可用环境变量 `JAI_SSE_LINE_HOLD_SECS` 覆盖（集成测试提速用）。
pub const SSE_LINE_HOLD_TIMEOUT: Duration = Duration::from_secs(90);

fn sse_line_hold_timeout() -> Duration {
    std::env::var("JAI_SSE_LINE_HOLD_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(SSE_LINE_HOLD_TIMEOUT)
}

/// 上游**连接类**失败在**同一渠道**内的额外重试次数。
///
/// 为什么需要：连接失败多为瞬时（连接池里的死连接、上游 LB 瞬时拒绝、TLS 抖动），
/// 而「换下一渠道」对**单渠道模型**根本无路可走 —— 实测「基元律动」的 502 占全量 9.78%
/// （2026-09-20 当天 20%），当时唯一能救回来的就是客户端自己重试。
/// 只重试连接类失败：此时上游未回响应头、下游也一个字节未收到，重试安全且幂等。
/// **不重试**超时与请求构造失败（理由见下）。
pub const UPSTREAM_CONNECT_RETRY: usize = 1;

/// 该 `send()` 失败是否值得同渠道立刻再试一次。
///
/// 判据刻意保守，只排除两类：
/// - `is_timeout()`：`.timeout(300s)` / 10s 建连超时的等待预算已经花掉，重试等于把等待翻倍；
/// - `is_builder()`：请求本身构造失败（非法 URL 等），重试必然再失败。
///
/// 其余 send 阶段错误（建连失败、握手失败、**响应头到达前连接被重置**）一律按「连接类」处理。
/// 这里**有意不**收窄到 `is_connect()`：上游偶发在发出响应头前 RST，这类错误 reqwest 常归到
/// request/body 而不是 connect，只看 `is_connect()` 会把真实场景漏掉。
///
/// 安全性：走到这里意味着 `send()` 返回了 Err —— 响应头都还没到，下游一个字节都没收到，
/// 因此重试不会造成重复输出；代价上限是同渠道多打一次上游。
fn should_retry_connect(e: &reqwest::Error, tries: usize) -> bool {
    tries < UPSTREAM_CONNECT_RETRY && !e.is_timeout() && !e.is_builder()
}

// ---------------------------------------------------------------- 入站线（wire）

/// 入站协议线。差异（路径/鉴权头/错误形状/日志族）全部收敛在此。
/// M1 OpenAI 直通、M3 Anthropic 直通、M6 Responses 入站共用路由/故障转移骨架。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundWire {
    OpenAi,
    Anthropic,
    Responses,
    Completions,
}

impl InboundWire {
    /// 要求的渠道协议族（providers.family 值）。Responses 线对应新增的
    /// `openai_responses` 上游族；若渠道是 openai_compat 则走跨族转换。
    pub fn family(&self) -> &'static str {
        match self {
            InboundWire::OpenAi | InboundWire::Completions => "openai_compat",
            InboundWire::Anthropic => "anthropic",
            InboundWire::Responses => "openai_responses",
        }
    }

    /// 日志 inbound_family 字段值
    pub fn log_family(&self) -> &'static str {
        match self {
            InboundWire::OpenAi | InboundWire::Completions => "openai",
            InboundWire::Anthropic => "anthropic",
            InboundWire::Responses => "responses",
        }
    }

    /// 上游路径（base_url 拼接；Responses 线始终走转换，不直接使用）
    pub fn upstream_path(&self) -> &'static str {
        match self {
            InboundWire::OpenAi => "/chat/completions",
            InboundWire::Completions => "/completions",
            InboundWire::Anthropic => "/v1/messages",
            InboundWire::Responses => "/responses",
        }
    }

    /// 上游鉴权头组装
    pub fn apply_auth(
        &self,
        req: reqwest::RequestBuilder,
        secret: &str,
    ) -> reqwest::RequestBuilder {
        match self {
            InboundWire::OpenAi | InboundWire::Responses | InboundWire::Completions => {
                req.bearer_auth(secret)
            }
            InboundWire::Anthropic => req.header("x-api-key", secret).header(
                "anthropic-version",
                anthropic_codec::DEFAULT_ANTHROPIC_VERSION,
            ),
        }
    }

    /// 错误响应形状（入站协议方言）
    pub fn error_response(
        &self,
        status: StatusCode,
        message: &str,
        err_type: &str,
        code: Option<&str>,
    ) -> Response {
        match self {
            InboundWire::OpenAi | InboundWire::Completions => {
                (status, Json(error_body(message, err_type, code))).into_response()
            }
            InboundWire::Anthropic => {
                (status, Json(anthropic_codec::error_body(message, err_type))).into_response()
            }
            InboundWire::Responses => (
                status,
                Json(crate::codec::responses::error_body(message, err_type, code)),
            )
                .into_response(),
        }
    }

    /// Overloaded → Anthropic 侧保留 529（roadmap M3 验收 4）
    pub fn overloaded_status(&self) -> StatusCode {
        match self {
            InboundWire::OpenAi | InboundWire::Responses | InboundWire::Completions => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            InboundWire::Anthropic => {
                StatusCode::from_u16(529).unwrap_or(StatusCode::SERVICE_UNAVAILABLE)
            }
        }
    }
}

// ---------------------------------------------------------------- 共享上下文

#[derive(Clone)]
pub struct GatewayCtx {
    pub db: Db,
    pub logs: crate::store::logs::LogHandle,
    pub http: reqwest::Client,
    pub cors: Arc<CorsAllowlist>,
    /// 密钥白/黑名单缓存（D9-T6b）：5s TTL，保存规则后由 IPC 侧主动失效。
    /// 桌面端 AppCore 持有同一份（见 `with_rules`），所以「刚保存就去请求」立刻生效。
    pub rules: Arc<security::KeyRulesCache>,
    /// 鉴权失败限速（roadmap M2）
    pub rate: Arc<super::ratelimit::AuthRateLimiter>,
    pub version: String,
    pub started_at_ms: u64,
    /// 推理回放兼容的**自适应**标记表（`codec::replay`）：推断 + 学习 + 遗忘。
    /// 持久化在 `meta` 表，这里只是写穿缓存。
    pub replay: Arc<crate::codec::replay::Registry>,
}

impl GatewayCtx {
    pub fn new(db: Db, logs: crate::store::logs::LogHandle) -> Self {
        // 出站代理（D8）：启动时读 meta 构建；保存后重启网关生效（与端口约定一致）
        let proxy = db.with_any(crate::netcfg::ProxyConfig::from_meta).ok();
        let http = crate::netcfg::build_client(proxy.as_ref(), Duration::from_secs(10));
        Self {
            db,
            logs,
            http,
            cors: Arc::new(CorsAllowlist::new()),
            rules: Arc::new(security::KeyRulesCache::new()),
            rate: Arc::new(super::ratelimit::AuthRateLimiter::new()),
            version: env!("CARGO_PKG_VERSION").to_string(),
            started_at_ms: store::now_ms() as u64,
            replay: Arc::new(crate::codec::replay::Registry::new()),
        }
    }
}

impl GatewayCtx {
    /// 换用外部持有的规则缓存。
    ///
    /// 桌面端把这一份存在 `AppCore` 里：`gateway_key_rules_set` 保存完就
    /// [`security::KeyRulesCache::invalidate`]，否则用户要等 5s TTL 才看到生效。
    /// 测试不调用 —— 各自一份更省事。
    pub fn with_rules(mut self, rules: Arc<security::KeyRulesCache>) -> Self {
        self.rules = rules;
        self
    }
}

// ---------------------------------------------------------------- 中间件

/// 安全中间件：Host/Origin 校验 + 鉴权限速判定 + 强制鉴权。/healthz 豁免。
pub async fn security_mw(State(ctx): State<GatewayCtx>, mut req: Request, next: Next) -> Response {
    if req.uri().path() == "/healthz" {
        return next.run(req).await;
    }
    let headers = req.headers().clone();

    if let Err(resp) = security::check_host(&headers) {
        return resp;
    }

    let allowlist = ctx.cors.get(&ctx.db).await;
    if let Err(resp) = security::check_origin(&headers, &allowlist) {
        return resp;
    }

    // 鉴权限速：封禁中的源直接 429（不进入凭据比对，也不泄露 401 语义）
    let peer_ip = req
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip())
        .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    match ctx.rate.status(peer_ip, store::now_ms()) {
        BanStatus::Banned(remaining_ms) => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(error_body(
                    &format!(
                        "鉴权失败次数过多，该来源被临时封禁（剩余 {}s）",
                        remaining_ms / 1000
                    ),
                    "rate_limit_error",
                    Some("source_banned"),
                )),
            )
                .into_response();
        }
        BanStatus::Allowed => {}
    }
    match security::authenticate(&ctx.db, &headers).await {
        // D9-T6b：鉴权结果**必须带进请求扩展** —— `dispatch` 与 `models_list`
        // 靠它问「这条请求用的是哪把密钥」。此前这里是 `Ok(_key) =>`，直接丢掉，
        // 于是「按密钥过滤」根本无处安放（这正是 T6b 的前置缺失）。
        Ok(key) => {
            req.extensions_mut().insert(key);
            next.run(req).await
        }
        Err(resp) => {
            // 记录失败：同一源窗口内超阈值即封禁
            ctx.rate.record_failure(peer_ip, store::now_ms());
            resp
        }
    }
}

// ---------------------------------------------------------------- 日志

/// 日志的「路由模式」：同族直通 vs 跨族转换。
///
/// 此前 `emit_log` 把 route_mode 硬编码为 "passthrough"，跨族转换路径
/// （try_converted_candidate / convert_plain_response / convert_streaming_response）
/// 也一律记成 passthrough，于是日志与统计里的「路由模式」字段完全不可信——
/// 排查「客户端 ctx 恒 0」时就因该字段显示 passthrough 而误判请求走的是字节直通。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RouteMode {
    Passthrough,
    Converted,
}

impl RouteMode {
    fn as_str(self) -> &'static str {
        match self {
            RouteMode::Passthrough => "passthrough",
            RouteMode::Converted => "converted",
        }
    }
}

/// 同族直通路径的日志入口（含路由尚未确定的早期失败：那类请求没有转换过，
/// 沿用 passthrough 语义）。
#[allow(clippy::too_many_arguments)]
fn emit_log(
    logs: &crate::store::logs::LogHandle,
    inbound_family: &str,
    peeked: Option<&PeekRequest>,
    provider_id: Option<&str>,
    upstream_model_id: Option<String>,
    status: i64,
    duration_ms: i64,
    is_stream: bool,
    usage: Option<&Value>,
    tool_calls: i64,
    error_kind: Option<String>,
    error_summary: Option<String>,
    // 响应侧结束原因（IR 口径：`end_turn` / `max_tokens` / `tool_use` / `safety` / `other`）。
    // 放末位是为了让既有调用点只需追加一个实参；未知（错误路径、未采集）传 `None`。
    stop_reason: Option<&str>,
) {
    emit_log_with(
        RouteMode::Passthrough,
        logs,
        inbound_family,
        peeked,
        provider_id,
        upstream_model_id,
        status,
        duration_ms,
        is_stream,
        usage,
        tool_calls,
        error_kind,
        error_summary,
        stop_reason,
    );
}

/// 落一条请求日志；`mode` 决定 route_mode 字段（同族直通 / 跨族转换）。
#[allow(clippy::too_many_arguments)]
fn emit_log_with(
    mode: RouteMode,
    logs: &crate::store::logs::LogHandle,
    inbound_family: &str,
    peeked: Option<&PeekRequest>,
    provider_id: Option<&str>,
    upstream_model_id: Option<String>,
    status: i64,
    duration_ms: i64,
    is_stream: bool,
    usage: Option<&Value>,
    tool_calls: i64,
    error_kind: Option<String>,
    error_summary: Option<String>,
    // 响应侧结束原因（见 [`emit_log`]；未知传 `None`）
    stop_reason: Option<&str>,
) {
    let (ui, uo, ucr, ucw) = usage.map(extract_usage).unwrap_or((None, None, None, None));
    logs.emit(LogEvent {
        ts: store::now_ms(),
        inbound_family: inbound_family.into(),
        route_mode: mode.as_str(),
        model_name: peeked.map(|p| p.model.clone()).unwrap_or_default(),
        provider_id: provider_id.map(str::to_string),
        upstream_model_id,
        http_status: status,
        stop_reason: stop_reason.map(str::to_string),
        usage_input: ui,
        usage_output: uo,
        usage_cache_read: ucr,
        usage_cache_write: ucw,
        duration_ms,
        is_stream,
        tool_calls,
        error_kind,
        error_summary,
    });
}

/// 统计 IR 响应里 assistant 发起的工具调用次数（落 `request_logs.tool_calls`）。
fn count_ir_tool_uses(resp: &crate::codec::ir::CanonicalResponse) -> i64 {
    resp.output
        .iter()
        .filter(|b| matches!(b, crate::codec::ir::Block::ToolUse { .. }))
        .count() as i64
}

/// 统计**同族直通**响应体里的工具调用次数（按入站线形状取字段，不做完整解析）。
/// 字节直通不解析语义，这里只按形状数数组元素；解析失败一律记 0——
/// 该字段仅用于诊断，绝不能因为统计而影响转发。
fn count_tool_calls_in_body(wire: InboundWire, bytes: &[u8]) -> i64 {
    let Ok(v) = serde_json::from_slice::<Value>(bytes) else {
        return 0;
    };
    match wire {
        // OpenAI Chat：choices[*].message.tool_calls
        InboundWire::OpenAi | InboundWire::Completions => v
            .get("choices")
            .and_then(Value::as_array)
            .map(|cs| {
                cs.iter()
                    .map(|c| {
                        c.pointer("/message/tool_calls")
                            .and_then(Value::as_array)
                            .map_or(0, |a| a.len() as i64)
                    })
                    .sum()
            })
            .unwrap_or(0),
        // Anthropic：content[*].type == "tool_use"
        InboundWire::Anthropic => v
            .get("content")
            .and_then(Value::as_array)
            .map(|bs| {
                bs.iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
                    .count() as i64
            })
            .unwrap_or(0),
        // Responses：output[*].type == "function_call"
        InboundWire::Responses => v
            .get("output")
            .and_then(Value::as_array)
            .map(|is| {
                is.iter()
                    .filter(|i| i.get("type").and_then(Value::as_str) == Some("function_call"))
                    .count() as i64
            })
            .unwrap_or(0),
    }
}

/// 上游**原始**结束原因 → IR 口径日志字符串（与 `StopReason::as_log_str` 同词表）。
///
/// 各族词表只差别名与大小写，这里用一张表统一，保证 `request_logs.stop_reason`
/// 在直通与转换两条路径上口径一致。
fn log_stop_reason(raw: &str) -> &'static str {
    match raw {
        "stop" | "end_turn" | "stop_sequence" | "completed" => "end_turn",
        "length" | "max_tokens" | "max_output_tokens" => "max_tokens",
        "tool_calls" | "function_call" | "tool_use" => "tool_use",
        "content_filter" | "refusal" => "safety",
        // `incomplete` 且拿不到细分 reason：无法区分截断/安全，如实记 other
        _ => "other",
    }
}

/// 直通**非流式**：按入站线形状从响应体取结束原因（诊断字段；解析失败记 `None`）。
///
/// 该字段此前恒为 NULL —— 排查「模型为什么反复重发」时看不到是 `max_tokens` 截断。
fn stop_reason_in_body(wire: InboundWire, bytes: &[u8]) -> Option<&'static str> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let raw = match wire {
        InboundWire::OpenAi | InboundWire::Completions => v
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)?,
        InboundWire::Anthropic => v.get("stop_reason").and_then(Value::as_str)?,
        InboundWire::Responses => {
            // Responses 的终局状态在 `status`；截断/安全的细分在 `incomplete_details.reason`
            let status = v.get("status").and_then(Value::as_str)?;
            if status == "incomplete" {
                v.pointer("/incomplete_details/reason")
                    .and_then(Value::as_str)
                    .unwrap_or("incomplete")
            } else {
                status
            }
        }
    };
    Some(log_stop_reason(raw))
}

/// 直通**流式**的结束原因探针（纯诊断；绝不参与转发，字节原样透传）。
///
/// 字节直通不解析 SSE 语义（与「直通流式 `tool_calls` 恒 0」同源约束），只做关键字级扫描。
/// 两个坑必须绕开：
///
/// 1. **必须逐块扫，不能只在结束时看尾巴**：Responses 的终局帧把整个 `response`
///    （含**全部正文**）嵌在同一帧里 —— 截断响应正文可达 30KB+，固定 16KB 窗口会把
///    帧首的 `status` / `incomplete_details` 挤出窗口 → 恰好丢掉「被截断」这个最想看的结论。
/// 2. **`incomplete_details.reason` 优先，`status` 只作兜底**：同一帧里 `output[*]` 的
///    item 也带 `status`，且截断响应末尾的 item 反而是 `"completed"`；只按「最后一次
///    `status`」判定会把截断误记成 `end_turn`（比 NULL 更坏：静默错误）。
struct StopReasonProbe {
    wire: InboundWire,
    /// `incomplete_details.reason` 命中（截断/安全的唯一权威）
    reason: Option<&'static str>,
    /// 兜底命中：`finish_reason` / `stop_reason` / `status`
    fallback: Option<&'static str>,
    /// 末尾窗口：只用于「标记被 TCP 分块切开」的兜底
    tail: Vec<u8>,
}

impl StopReasonProbe {
    fn new(wire: InboundWire) -> Self {
        Self {
            wire,
            reason: None,
            fallback: None,
            tail: Vec::new(),
        }
    }

    fn feed(&mut self, chunk: &[u8]) {
        self.scan(&String::from_utf8_lossy(chunk));
        keep_tail(&mut self.tail, chunk);
    }

    /// 结束原因（IR 口径）；扫不到记 `None`。
    fn finish(&mut self) -> Option<&'static str> {
        // 兜底再扫一遍末尾窗口：标记被分块切开时逐块扫描会漏
        if self.reason.is_none() || self.fallback.is_none() {
            let tail = String::from_utf8_lossy(&self.tail).into_owned();
            self.scan(&tail);
        }
        self.reason.or(self.fallback)
    }

    fn scan(&mut self, s: &str) {
        // 权威：`incomplete_details.reason`（值域白名单，避免正文/其它对象的 reason 误命中）
        if let Some(r) = last_json_string(s, "\"reason\":\"") {
            if r == "max_output_tokens" || r == "content_filter" {
                self.reason = Some(log_stop_reason(r));
            }
        }
        let raw = match self.wire {
            InboundWire::OpenAi | InboundWire::Completions => {
                last_json_string(s, "\"finish_reason\":\"")
            }
            InboundWire::Anthropic => last_json_string(s, "\"stop_reason\":\""),
            InboundWire::Responses => last_json_string(s, "\"status\":\""),
        };
        if let Some(raw) = raw {
            self.fallback = Some(log_stop_reason(raw));
        }
    }
}

/// 取 `key`（形如 `"finish_reason":"`，自带两侧引号与冒号）之后**最后一次**出现的
/// JSON 字符串值。内容里的同类文本会被 JSON 转义成 `\"`，不会误命中。
fn last_json_string<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let start = s.rfind(key)? + key.len();
    let rest = &s[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// 直通流式保留的末尾窗口字节数（只用于跨块拆分的标记兜底，见 [`StopReasonProbe`]）。
const PASSTHROUGH_TAIL_BYTES: usize = 16 * 1024;

/// 维护末尾窗口：追加新块并裁掉窗口外的旧字节。
fn keep_tail(tail: &mut Vec<u8>, chunk: &[u8]) {
    tail.extend_from_slice(chunk);
    if tail.len() > PASSTHROUGH_TAIL_BYTES {
        let cut = tail.len() - PASSTHROUGH_TAIL_BYTES;
        tail.drain(..cut);
    }
}

/// **截断诊断**：`stop_reason` 属截断类时，判断这一轮的截断有没有真的伤到用户。
///
/// 两档，同一份判定、同一条日志行（不新增行类型、不动响应头、不改 HTTP 状态码）：
///
/// - **`OutputTruncatedEmpty`（零可见输出）**：整轮没有产出任何可见内容（正文为空、且没有
///   工具调用），而 reasoning/thinking 有内容。这种轮次对客户端等于「模型什么都没说」——
///   PI-Desktop 的 agent 循环会把它判为 silent turn、追加 `<no_output_recovery>` 提示重跑
///   一次，仍静默则报 `EMPTY_MODEL_RESPONSE`，用户看到的是一句无从下手的「模型没有产生任何
///   输出」。而网关侧**本来就能分辨**：上游明确给了 `finish_reason:"length"`，且可见输出是 0。
///   此前这条信息只落在 `stop_reason` 一个字段上，`error_kind` 恒为 NULL ⇒ 日志与统计里它
///   与一次正常 200 完全无法区分（2026-09-22 真机：基元律动/deepseek-flash，460k 上下文，
///   output=16/96/165/250，四条全被记成成功，只能靠手工比对 `request_logs` 才看出来）。
///
/// - **`OutputBudgetClipped`（被预算掐短）**：有可见输出，但**确实撞上了预算上限**
///   （`output_tokens >= output_budget`）。用户拿到的是被掐短的答案 —— 不像零输出那样让
///   客户端整轮报废，所以严重度更低，但同样值得知道「是哪一轮、被什么掐的」。
///
/// 为什么第二档要求「撞上预算」而不是「只要 `length` + 有正文」：后者会把客户端**故意**
/// 设小预算的场景也标成异常（`max_tokens=100` 拿到 100 token 正文是照做，不是故障），
/// 而 agent 客户端在窗口边缘会把预算压到几十 token ⇒ 每一轮都命中，日志被噪音淹没。
/// 真机频率佐证：全库 7995 行里 `stop_reason=max_tokens` 只有 11 行（0.14%）。
///
/// 已知的保守缺口（宁可漏判，绝不误判）：`output_budget` 未知时第二档不触发；上游若在
/// **客户端声明的预算之前**就自行截断（`output_tokens < output_budget`），也不触发。
///
/// 返回 `(error_kind, summary)`；不适用时返回 `None`。**不改 HTTP 状态码** —— 上游确实
/// 返回了 200，协议上没错，这里只补可观测性（与 `SseParseWarn` / `CapabilityWarn` 同一档）。
///
/// `stop_reason` 取 IR 的**日志名**口径（见 [`log_stop_reason`]）：转换路径给
/// `StopReason::as_log_str()`，直通路径给 [`StopReasonProbe`] 的扫描结果，两条路径因此
/// 共用同一份判定。
fn truncation_diagnostic(
    stop_reason: Option<&str>,
    saw_visible_output: bool,
    output_budget: Option<u32>,
    output_tokens: u64,
) -> Option<(&'static str, String)> {
    let budget = match output_budget {
        Some(b) => format!("本轮 max_output_tokens={b}"),
        None => "本轮请求未声明 max_output_tokens".to_string(),
    };

    match stop_reason {
        // ---- 安全拦截：只有「零可见输出」才是异常（有正文说明拦截没生效到正文上）----
        Some("safety") if !saw_visible_output => Some((
            "OutputTruncatedEmpty",
            format!(
                "上游以 finish_reason=content_filter 拦截，且本轮没有任何可见输出（正文为空、\
                 无工具调用）——客户端会判为「模型什么都没说」。{budget}。\
                 处理：检查内容策略，或换用不触发拦截的输入。"
            ),
        )),

        // ---- 长度截断：分「零输出」与「被掐短」两档 ----
        Some("max_tokens") if !saw_visible_output => Some((
            "OutputTruncatedEmpty",
            format!(
                "上游以 finish_reason=length 截断（输出预算耗尽），但本轮没有任何可见输出\
                 （正文为空、无工具调用）——客户端会判为「模型什么都没说」。{budget}，\
                 实际产出 {output_tokens} token（可能全耗在推理里）。\
                 处理：压缩上下文后重试，或提高该模型的输出预算。"
            ),
        )),

        Some("max_tokens") if saw_visible_output => {
            // 只有「确实撞上上限」才算被掐短，见上方文档注释
            let budget_n = output_budget?;
            if output_tokens < u64::from(budget_n) {
                return None;
            }
            Some((
                "OutputBudgetClipped",
                format!(
                    "上游以 finish_reason=length 截断，且本轮有可见输出——用户拿到的是被掐短\
                     的答案。{budget}，实际产出 {output_tokens} token，说明预算被真正用尽。\
                     处理：提高该模型的输出预算，或让客户端在贴近窗口时先压缩上下文。"
                ),
            ))
        }

        _ => None,
    }
}

/// 直通流式的**观测探针**：既回答「这一轮有没有产出用户看得见的东西」，也数出工具调用。
///
/// 直通是**字节级转发**（验收：上游收到的 body SHA-256 == 客户端发出值），探针只**看**
/// 不参与转发，一个字节都不改写。它补的是两件此前直通流式拿不到的信息：
/// 1. **可见输出**：收尾日志要能分辨「零可见输出的截断轮」与「被预算掐短」。转换路径已由
///    [`truncation_diagnostic`] 覆盖，直通路径此前没有任何可见输出信息，于是同样形状的
///    轮次在那里仍是 `error_kind=NULL` 的干净 200。
/// 2. **工具调用计数**：`request_logs.tool_calls` 在直通流式下此前硬编码 0（「字节直通不
///    解析 SSE 语义」），于是「模型到底有没有发起工具调用」在直通行上看不出来，只能去翻
///    转换路径的行。非流式直通早有 `count_tool_calls_in_body`，现在流式对齐。
///
/// 实现取「按 `data:` 行切分 + 复用各族已有的 `parse_stream_event`」，而不是关键字扫描：
/// 关键字扫描分不清 `"content":""`（空增量）与 `"content":"x"`，也分不清
/// `"tool_calls":null` 与真正的工具调用 —— 而误判的代价是把正常轮次标成异常、把工具调用
/// 数错。真机上游（tokenrhythm）**每帧都带 `"content":""`**，正是关键字扫描会翻车的地方。
///
/// 代价与边界：
/// - 必须**全程**解析，不能像「只判可见输出」那样见到正文就提前收工 —— 工具调用可能出现在
///   正文之后，提前退出会漏数。每帧一次小对象 JSON 解析，与转换路径同量级。
/// - 解析失败一律当「没看到」（宁可漏判，绝不误判）；行缓冲超限（无换行的巨块）即放弃观测
///   并停止累积，绝不让观测拖累转发（此时 `tool_calls` 可能偏低）。
struct PassthroughStreamProbe {
    wire: InboundWire,
    /// 未成行的残留字节（按 `\n` 切帧）
    buf: Vec<u8>,
    /// 已确认产出过可见内容（正文或工具调用）
    seen_visible: bool,
    /// 本回合 assistant 发起的工具调用 id（**按 id 去重**，与转换路径同一口径：
    /// 部分上游每帧都带 Start 且 index 恒为 0，按 id 去重才不会重复计数）
    tool_call_ids: std::collections::HashSet<String>,
    /// 已放弃观测（病态输入）
    given_up: bool,
}

impl PassthroughStreamProbe {
    fn new(wire: InboundWire) -> Self {
        Self {
            wire,
            buf: Vec::new(),
            seen_visible: false,
            tool_call_ids: std::collections::HashSet::new(),
            given_up: false,
        }
    }

    fn feed(&mut self, chunk: &[u8]) {
        if self.given_up {
            return;
        }
        self.buf.extend_from_slice(chunk);
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = line.strip_suffix(b"\r").unwrap_or(&line);
            if !line.starts_with(b"data:") {
                continue;
            }
            let payload = trim_ascii(&line[5..]);
            if payload.is_empty() || payload == b"[DONE]" {
                continue;
            }
            self.observe(payload);
        }
        if self.buf.len() > MAX_SSE_LINE_BYTES {
            self.given_up = true;
            self.buf.clear();
        }
    }

    /// 观察一帧 `data:` 载荷：累计工具调用 id，并判定是否产出可见内容。
    ///
    /// **推理/thinking 不算可见输出** —— 客户端看不到推理，只有推理的一轮对用户等于
    /// 「什么都没说」（真机故障形状，见 [`truncation_diagnostic`]）。
    fn observe(&mut self, payload: &[u8]) {
        use crate::codec::ir::StreamEvent as Ev;
        let parsed = match self.wire {
            InboundWire::OpenAi | InboundWire::Completions => {
                crate::codec::openai::parse_stream_event(payload)
            }
            InboundWire::Anthropic => crate::codec::anthropic::parse_stream_event(payload),
            InboundWire::Responses => crate::codec::responses::parse_stream_event(payload),
        };
        let events = match parsed {
            Ok(events) => events,
            Err(_) => return,
        };
        for ev in &events {
            match ev {
                Ev::TextDelta { text } => {
                    if !text.trim().is_empty() {
                        self.seen_visible = true;
                    }
                }
                Ev::ToolCallStart { id, .. } => {
                    self.seen_visible = true;
                    if !id.is_empty() {
                        self.tool_call_ids.insert(id.clone());
                    }
                }
                Ev::ToolCallArgsDelta { .. } => self.seen_visible = true,
                _ => {}
            }
        }
    }
}

fn empty_resp(status: StatusCode) -> Response {
    (status, Json(json!({}))).into_response()
}

/// 转换流中途出错时发给客户端的 SSE 错误帧（protocol-ir §5-E/§6）。
/// Anthropic 线使用 `event: error`；OpenAI 线使用 `data: {"error":...}`。
fn error_sse_frame(wire: InboundWire, message: &str) -> Bytes {
    match wire {
        InboundWire::Anthropic => {
            let payload = anthropic_codec::error_body(message, "api_error").to_string();
            Bytes::from(format!("event: error\ndata: {payload}\n\n"))
        }
        InboundWire::OpenAi | InboundWire::Responses | InboundWire::Completions => {
            let payload =
                crate::codec::responses::error_body(message, "api_error", None).to_string();
            Bytes::from(format!("data: {payload}\n\n"))
        }
    }
}

/// M5：Anthropic 入站历史中的 tool id 解析。
/// 先查超长 id 映射表，再回落 `decode_anthropic_tool_id` 的确定性反解。
fn resolve_anthropic_inbound_tool_ids(db: &Db, req: &mut crate::codec::ir::CanonicalRequest) {
    use crate::codec::ir::Block;
    for m in &mut req.messages {
        for b in &mut m.blocks {
            match b {
                Block::ToolUse { id, .. } => {
                    if let Some(canonical) = lookup_tool_id(db, id) {
                        *id = canonical;
                    }
                }
                Block::ToolResult { call_id, .. } => {
                    if let Some(canonical) = lookup_tool_id(db, call_id) {
                        *call_id = canonical;
                    }
                }
                _ => {}
            }
        }
    }
}

/// 查 tool_id_map；`decode_anthropic_tool_id` 会先剥掉 `toolu_` 前缀，
/// 因此这里同时尝试原样 id 和补回前缀后的完整 outbound_id。
fn lookup_tool_id(db: &Db, id: &str) -> Option<String> {
    if let Some(canonical) = db.with_any(|c| store::tool_id_get(c, id)).ok().flatten() {
        return Some(canonical);
    }
    let full = format!("toolu_{id}");
    db.with_any(|c| store::tool_id_get(c, &full)).ok().flatten()
}

/// M5：Anthropic 出站 tool_use id 映射。
/// 普通 id 使用确定性 base58 内嵌编码；超过 Anthropic 64 字符上限时，
/// 在 `tool_id_map` 落一条短 id → 原始 id 的映射并返回短 id。
fn map_anthropic_tool_id(db: &Db, canonical_id: &str) -> String {
    if canonical_id.starts_with("toolu_") {
        return canonical_id.to_string();
    }
    let encoded = crate::codec::ir::canonical_to_anthropic_id(canonical_id);
    if encoded.len() <= 64 {
        return encoded;
    }
    let digest = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(canonical_id.as_bytes());
        format!("{:x}", h.finalize())
    };
    let outbound = format!("toolu_{}", &digest[..56]);
    if let Err(e) = db.with_any(|c| store::tool_id_put(c, &outbound, canonical_id)) {
        eprintln!("[convert] tool_id_map 写入失败: {e}");
    }
    outbound
}

/// 极简 ASCII 空白 trim（u8 slice 无内置 trim）。
fn trim_ascii(s: &[u8]) -> &[u8] {
    let start = s
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(s.len());
    let end = s
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|i| i + 1)
        .unwrap_or(start);
    &s[start..end]
}

/// Gemini 目标：把 IR 中的 http(s) 图片 URL 拉取为 base64 inline（M4-G-d）。
/// 8s 超时；拉取失败返回 Err（调用方按 400 回给客户端）。
async fn resolve_remote_images(
    ctx: &GatewayCtx,
    req: &mut crate::codec::ir::CanonicalRequest,
) -> Result<(), String> {
    use crate::codec::ir::Block;

    for m in &mut req.messages {
        for b in &mut m.blocks {
            if let Block::Image {
                media_type,
                data_base64,
                url,
            } = b
            {
                // 已有 base64：跳过
                if data_base64.is_some() {
                    continue;
                }
                let Some(url_str) = url.clone() else {
                    continue;
                };
                if !(url_str.starts_with("http://") || url_str.starts_with("https://")) {
                    return Err(format!(
                        "不支持的图片源: {url_str}（仅 http/https 或 data URL）"
                    ));
                }
                let resp =
                    tokio::time::timeout(Duration::from_secs(8), ctx.http.get(&url_str).send())
                        .await
                        .map_err(|_| format!("图片拉取超时(8s): {url_str}"))?
                        .map_err(|e| format!("图片拉取失败: {e}"))?;
                if !resp.status().is_success() {
                    return Err(format!(
                        "图片拉取失败: {url_str} → HTTP {}",
                        resp.status().as_u16()
                    ));
                }
                // 先取 content-type 头再消费 body（bytes() 拿走所有权）
                let mime = resp
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string)
                    .unwrap_or_else(|| "image/png".to_string());
                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| format!("图片读取失败: {e}"))?;
                if bytes.len() > 10 * 1024 * 1024 {
                    return Err(format!("图片过大(>{})", "10MB"));
                }
                let b64 = base64_like(&bytes);
                *media_type = mime;
                *data_base64 = Some(b64);
                *url = None;
            }
        }
    }
    Ok(())
}

/// base64 编码（不引入额外依赖，用 chunk 手写或最小实现）。
fn base64_like(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(n >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[n as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn ms_since(t: Instant) -> i64 {
    t.elapsed().as_millis() as i64
}

fn provider_mark_ok(db: &Db, pid: &str) {
    let d2 = db.clone();
    let id = pid.to_string();
    tokio::task::spawn_blocking(move || {
        let _ = d2.with(|c| store::provider_mark_ok(c, &id));
    });
}

fn provider_mark_fail(db: &Db, pid: &str, msg: &str) {
    let d2 = db.clone();
    let id = pid.to_string();
    let msg = msg.to_string();
    tokio::task::spawn_blocking(move || {
        let _ = d2.with(|c| store::provider_mark_err(c, &id, &msg));
    });
}

// ---------------------------------------------------------------- handlers

/// GET /v1/models 查询行。
///
/// 用结构体而不是元组：T6b 加上 `provider_id` 后字段到七个，元组写法
/// （`|(id, owner, _ctx, _l, _i, _o)|`）已经看不出谁是谁了。
struct ModelListRow {
    model_name: String,
    provider_id: String,
    provider_name: String,
    context_window: Option<i64>,
    legacy_multimodal: Option<i64>,
    input_modalities: Option<String>,
    output_modalities: Option<String>,
}

/// 取本次请求那把密钥的规则（5s TTL 缓存，见 [`security::KeyRulesCache`]）。
///
/// `None`（请求扩展里没有鉴权结果）只可能出现在中间件被绕过的测试路径 —— 按
/// 「不限制」处理，与「没配规则的密钥」行为一致。
async fn key_rules_of(ctx: &GatewayCtx, key: Option<&security::AuthedKey>) -> Arc<KeyRules> {
    match key {
        Some(k) => ctx.rules.get(&ctx.db, &k.id).await,
        None => Arc::new(KeyRules::default()),
    }
}

/// GET /v1/models —— 数据库内启用模型的去重聚合输出。
///
/// D9-T6b：按**调用方那把密钥**的规则过滤。不过滤的话客户端会看到一堆自己
/// 根本调不通的模型（选到了必然 403），体验比看不到更差。
pub async fn models_list(
    State(ctx): State<GatewayCtx>,
    Extension(authed): Extension<security::AuthedKey>,
) -> Response {
    let rules = key_rules_of(&ctx, Some(&authed)).await;
    let list = {
        let db = ctx.db.clone();
        tokio::task::spawn_blocking(move || {
            db.with(|c| -> Result<Vec<ModelListRow>, store::StoreError> {
                let mut stmt = c.prepare(
                    "SELECT m.model_name, p.id, p.name, m.context_window, m.supports_multimodal, \
                            m.input_modalities, m.output_modalities \
                      FROM models m \
                      JOIN providers p ON p.id=m.provider_id \
                      WHERE m.enabled=1 AND p.enabled=1 \
                      ORDER BY p.priority ASC, m.rowid ASC",
                )?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok(ModelListRow {
                            model_name: r.get(0)?,
                            provider_id: r.get(1)?,
                            provider_name: r.get(2)?,
                            context_window: r.get(3)?,
                            legacy_multimodal: r.get(4)?,
                            input_modalities: r.get(5)?,
                            output_modalities: r.get(6)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
        })
        .await
    };

    let data = match list {
        Ok(Ok(rows)) => {
            let mut seen = std::collections::HashSet::new();
            rows.into_iter()
                // 密钥规则：渠道轴与模型轴都要通过（空规则 ⇒ 全通过）
                .filter(|r| {
                    rules.allows_provider(&r.provider_id) && rules.allows_model(&r.model_name)
                })
                .filter(|r| seen.insert(format!("{}/{}", r.provider_name, r.model_name)))
                .map(|r| {
                    // context_window 为 NULL 时给保守默认 128k（与 schema 注释/UI 编辑页一致），
                    // 供客户端模型目录解析模型上下文窗口、计算 ctx 占用百分比。
                    let context_window = r.context_window.unwrap_or(128_000);
                    let input = crate::modality::parse_opt(r.input_modalities.as_deref());
                    let output = crate::modality::parse_opt(r.output_modalities.as_deref());
                    // supportsMultimodal 自 0010 起降为**派生视图**：集合优先、缺失回落旧列。
                    let supports_multimodal = crate::modality::derive_supports_multimodal(
                        input.as_deref(),
                        r.legacy_multimodal.map(|v| v != 0),
                    );
                    // 仅追加字段，旧客户端不受影响：contextWindow / supportsMultimodal 语义未变。
                    //
                    // 刻意**不**发布 `models.max_output_tokens`：那一列在本仓库里是「模型元数据」，
                    // 不是网关要替客户端执行的策略。发布它等于让客户端把它当自己的输出上限
                    // （PI-Desktop 就会读走并采纳），而网关一旦替客户端决定输出预算，就等于
                    // 从上游远端插手了 agent 的上下文规划 —— 本次真机故障（预算被压到几十个
                    // token → 模型把整轮耗在推理上 → 正文一个字不出）正是这条链路的产物。
                    // 网关只做转发与诊断，不发明预算。
                    json!({
                        "id": format!("{}/{}", r.provider_name, r.model_name),
                        "object": "model",
                        "owned_by": r.provider_name,
                        "contextWindow": context_window,
                        "supportsMultimodal": supports_multimodal,
                        "inputModalities": input,
                        "outputModalities": output,
                    })
                })
                .collect::<Vec<_>>()
        }
        Ok(Err(e)) => {
            eprintln!("[models] db: {e}");
            return Json(json!({
                "object": "list", "data": [],
                "error": {"message": format!("查询模型列表失败: {e}"), "type": "api_error"}
            }))
            .into_response();
        }
        Err(e) => {
            eprintln!("[models] join: {e}");
            return Json(json!({
                "object": "list", "data": [],
                "error": {"message": "internal error", "type": "api_error"}
            }))
            .into_response();
        }
    };

    Json(json!({ "object": "list", "data": data })).into_response()
}

/// 单渠道尝试的返回：已交付 / 可转移失败
enum Attempt {
    /// 已经向客户端交付（最终响应或确定性错误）
    Delivered(Response),
    /// 本次渠道失败，且**允许**转移到下一渠道（是否真的转移由 `router::AttemptFlow`
    /// 按失败分类 + 预算裁决；例如确定性 4xx 会在这里被拦下）。
    /// 携带最后一个 HTTP 错误（若为网络级失败则无），供全渠道失败时原样回传。
    Failed {
        /// 行为分类（D9-T3）：决定换不换候选、能不能跨组、值不值得退避
        class: router::FailureClass,
        /// 落库 `request_logs.error_kind` 的字符串（与 `classify_status` 逐字一致）
        kind: &'static str,
        summary: String,
        /// 最后一个带 HTTP 状态的上游错误（status + body + content-type）
        last_http: Option<UpstreamError>,
        /// 上游给出的退避时长（毫秒，已解析 `Retry-After` / `retry-after-ms`）。
        ///
        /// `dispatch` 在切换下一候选前按它等待（有封顶 + 抖动，见 `router`）。
        /// 只对限速 / 过载类失败有意义，所以别的失败路径一律填 `None`。
        retry_after_ms: Option<u64>,
        /// 首字节阶段（已收到响应头、**还没有任何字节下发给客户端**）失败时，
        /// 预构造的「最终失败」响应。
        ///
        /// 此时允许 failover（这正是 `first_byte_verdict` 当年想表达、
        /// 却从未接线的语义）。但若后面没有候选可试，必须原样回这个响应 ——
        /// 否则 `upstream_stream_error` / `upstream_empty_stream` /
        /// `upstream_first_byte_timeout` 这些专属诊断码会退化成笼统的
        /// `all_providers_failed`，排障信息反而变少。
        fallback: Option<Response>,
    },
}

/// 可直接回传的上游错误响应（保留原始状态码与方言形状）。
/// 上游错误响应里值得回传给客户端的头（白名单，见 [`FORWARD_ERROR_HEADERS`]）。
///
/// 只在错误路径上用；正常响应走各自的转换/直通渲染，不经过这里。
const FORWARD_ERROR_HEADERS: [&str; 3] = ["retry-after", "retry-after-ms", "x-request-id"];

struct UpstreamError {
    status: StatusCode,
    content_type: Option<HeaderValue>,
    /// 白名单内的上游响应头（目前是退避头 + 报障关联键）
    headers: Vec<(HeaderName, HeaderValue)>,
    body: Bytes,
}
/// 从上游错误响应头里解析退避时长（毫秒）。
///
/// 优先 `retry-after-ms`（非标准但精确，值本身就是毫秒；PI-Desktop 与 zcode
/// 都发它），回退标准 `Retry-After`（delta-seconds 或 HTTP-date）。
///
/// 解析不出来就返回 `None` —— 调用方回退到指数退避，绝不因为头写坏而卡住。
fn retry_after_from_headers(headers: &[(HeaderName, HeaderValue)]) -> Option<u64> {
    let get = |name: &str| {
        headers
            .iter()
            .find(|(n, _)| n.as_str() == name)
            .and_then(|(_, v)| v.to_str().ok())
    };
    if let Some(raw) = get("retry-after-ms") {
        if let Ok(ms) = raw.trim().parse::<f64>() {
            if ms.is_finite() && ms >= 0.0 {
                return Some(ms.round() as u64);
            }
        }
    }
    get("retry-after").and_then(|raw| router::parse_retry_after(raw, store::now_ms() / 1000))
}

/// 首字节阶段（已收到响应头、**尚未向下游发出任何字节**）的失败。
///
/// 返回 `Attempt::Failed` 让 `dispatch` 有机会换下一个候选；同时把「最终失败」
/// 响应预构造进 `fallback`，真的没候选可试时原样回它，保住专属诊断码。
fn first_byte_failure(
    wire: InboundWire,
    msg: &str,
    status: StatusCode,
    code: &'static str,
    kind: &'static str,
) -> Attempt {
    Attempt::Failed {
        class: router::FailureClass::from_kind(kind),
        kind,
        summary: msg.to_string(),
        last_http: None,
        retry_after_ms: None,
        fallback: Some(wire.error_response(status, msg, "api_error", Some(code))),
    }
}

/// 请求是否**非幂等**（重发会产生服务端副作用）→ 预算压到 (1, 1)，一次失败即止。
///
/// 目前只有 Responses 协议有这种开关：
/// - `store: true`：上游会把这次响应持久化（OpenAI 侧可 `GET /v1/responses/{id}` 取回）
/// - `background: true`：异步任务，重发等于提交两次任务
///
/// 其余协议只在「已向下游 commit 后断流」这一种情形下才不可重试，
/// 而那由 `FailureClass::CommittedStreamError` 负责，不在这里判。
/// 请求体解析不出来时按幂等处理 —— 那种请求会在解码阶段被判 CallerTerminal。
fn is_non_idempotent(wire: InboundWire, body: &[u8]) -> bool {
    if wire != InboundWire::Responses {
        return false;
    }
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    v.get("store").and_then(Value::as_bool).unwrap_or(false)
        || v.get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

/// POST /v1/chat/completions —— OpenAI 线主入口。
pub async fn chat_completions(State(ctx): State<GatewayCtx>, req: Request) -> Response {
    dispatch(InboundWire::OpenAi, ctx, req).await
}

/// POST /v1/messages —— Anthropic 线主入口（M3，Claude Code 直连）。
pub async fn anthropic_messages(State(ctx): State<GatewayCtx>, req: Request) -> Response {
    dispatch(InboundWire::Anthropic, ctx, req).await
}

/// POST /v1/responses —— Responses API 入站（M6，Codex 原生线）。
pub async fn responses(State(ctx): State<GatewayCtx>, req: Request) -> Response {
    dispatch(InboundWire::Responses, ctx, req).await
}

/// POST /v1/completions —— 旧版 OpenAI text completions 入站（仅 openai_compat 直通）。
pub async fn completions(State(ctx): State<GatewayCtx>, req: Request) -> Response {
    dispatch(InboundWire::Completions, ctx, req).await
}

/// POST /v1/messages/count_tokens —— 粗估端点（M3，避免 Claude Code 降级）。
pub async fn anthropic_count_tokens(State(_ctx): State<GatewayCtx>, req: Request) -> Response {
    let body = match to_bytes(req.into_body(), MAX_REQUEST_BODY).await {
        Ok(b) => b,
        Err(_) => {
            return InboundWire::Anthropic.error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "请求体超过 32MB 上限",
                "invalid_request_error",
                None,
            );
        }
    };
    match anthropic_codec::count_tokens(&body) {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(msg) => InboundWire::Anthropic.error_response(
            StatusCode::BAD_REQUEST,
            &msg,
            "invalid_request_error",
            None,
        ),
    }
}

/// 直通主流程：路由候选 → 逐渠道尝试（故障转移）。
async fn dispatch(wire: InboundWire, ctx: GatewayCtx, req: Request) -> Response {
    let started = Instant::now();
    // D9-T6b：鉴权中间件把 `AuthedKey` 放进了请求扩展。**必须在消费 body 之前取出来**
    // （后面 `req.into_body()` 会把请求拆掉），规则过滤要用它定位密钥。
    let authed = req.extensions().get::<security::AuthedKey>().cloned();
    // 直通路径需要把下游安全请求头带到上游（Content-Type/Accept 等）
    let inbound_headers = req.headers().clone();

    let body = match to_bytes(req.into_body(), MAX_REQUEST_BODY).await {
        Ok(b) => b,
        Err(_) => {
            return wire.error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "请求体超过 32MB 上限",
                "invalid_request_error",
                Some("request_too_large"),
            );
        }
    };

    let peeked = match peek(&body) {
        Ok(p) => p,
        Err(msg) => {
            emit_log(
                &ctx.logs,
                wire.log_family(),
                None,
                None,
                None,
                400,
                ms_since(started),
                false,
                None,
                0,
                Some("InvalidRequest".into()),
                Some(msg.clone()),
                None,
            );
            return wire.error_response(
                StatusCode::BAD_REQUEST,
                &msg,
                "invalid_request_error",
                None,
            );
        }
    };

    // ---- 路由候选 ----
    let model = peeked.model.clone();
    // 支持 `供应商名/模型名` 限定 ID（多供应商模型列表用），
    // 也兼容裸模型名（沿用 priority 路由）。
    let (provider_filter, model_key) = match model.split_once('/') {
        Some((p, m)) if !p.trim().is_empty() && !m.trim().is_empty() => {
            (Some(p.trim().to_string()), m.trim().to_string())
        }
        _ => (None, model.clone()),
    };
    // 限定名 `供应商/模型` 只用于 JAI 路由；发给上游时必须换回真实模型名。
    let body = if provider_filter.is_some() {
        rewrite_body_model(&body, &model_key)
    } else {
        body
    };
    let mut candidates = {
        let db = ctx.db.clone();
        let model_key2 = model_key.clone();
        match tokio::task::spawn_blocking(move || {
            db.with(|c| store::route_candidates(c, &model_key2))
        })
        .await
        {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                eprintln!("[route] db: {e}");
                return internal_error(&ctx, wire, &peeked, &started);
            }
            Err(e) => {
                eprintln!("[route] join: {e}");
                return internal_error(&ctx, wire, &peeked, &started);
            }
        }
    };
    // 限定名 `供应商/模型`：该供应商**优先**，其余同模型候选保留为**后备**。
    //
    // 旧行为是 `retain` 掉其它供应商，等于把故障转移彻底关掉 —— 只要客户端用的是
    // `/v1/models` 推荐的限定名（Reasonix 就是），这家上游一抖动就**没有任何退路**
    // （实测该上游 502 占 9.78%）。语义按「优先」而非「只用」实现。
    //
    // 但保留一条硬约束：**指定供应商一个候选都没命中时仍然 404**。
    // 否则供应商名打错会静默换家出答案，比报错难排查得多。
    if let Some(provider_name) = &provider_filter {
        if !candidates.iter().any(|c| c.provider_name == *provider_name) {
            candidates.clear();
        }
    }
    let now = store::now_ms();
    // 高级路由：健康感知 + 同优先级权重负载均衡
    candidates = router::order_candidates(candidates, now);
    // Responses 入站优先走同协议族直通（openai_responses），
    // 避免同名模型被 openai_compat 转换渠道按优先级截胡导致 400。
    candidates.sort_by_key(|c| c.family != wire.family());
    // 限定名的「优先」在这里兑现，且**健康优先于指定**：
    // 指定渠道已知不健康时不去抢健康备渠道的位置（否则每次都要先撞一次已知失败），
    // 但在同一健康档内把指定供应商提到最前。四档顺序：
    //   健康+指定 → 健康+其它 → 不健康+指定 → 不健康+其它
    // 用稳定排序，故每档内部沿用上面已算好的「同族优先 + 优先级 + 权重」序。
    if let Some(provider_name) = &provider_filter {
        candidates.sort_by_key(|c| {
            let named = c.provider_name == *provider_name;
            (!router::is_healthy(c, now), !named)
        });
    }

    if candidates.is_empty() {
        let msg = format!("模型 {model:?} 不存在或其渠道未启用");
        emit_log(
            &ctx.logs,
            wire.log_family(),
            Some(&peeked),
            None,
            None,
            404,
            ms_since(started),
            peeked.stream,
            None,
            0,
            Some("InvalidRequest".into()),
            Some(msg.clone()),
            None,
        );
        return wire.error_response(
            StatusCode::NOT_FOUND,
            &msg,
            "invalid_request_error",
            Some("model_not_found"),
        );
    }

    // ---- 密钥规则过滤（D9-T6b）----
    //
    // 位置刻意排在「模型不存在 / 指定供应商不存在」两个 404 **之后**：那两个是
    // 「东西本来就没有」，而这里被挡掉是「你有，但不给你用」—— 按验收要求必须
    // 是 **403 `model_not_allowed`**，不能退化成 404（用户会以为模型没了，
    // 实际上换把密钥就能用）。
    //
    // 反过来，指定 `供应商/模型` 而该供应商被规则挡住时：只要还有别的候选可用
    // 就照常走（沿用「指定优先、其余后备」的既有语义）；一个都不剩才 403。
    let rules = key_rules_of(&ctx, authed.as_ref()).await;
    if !rules.is_empty() {
        candidates
            .retain(|c| rules.allows_provider(&c.provider_id) && rules.allows_model(&model_key));
        if candidates.is_empty() {
            let msg = format!(
                "模型 {model:?} 不被当前 API Key 的规则允许（该密钥限制了可用的渠道 / 模型，\
                 可在「网关」页的密钥规则里调整）"
            );
            emit_log(
                &ctx.logs,
                wire.log_family(),
                Some(&peeked),
                None,
                None,
                403,
                ms_since(started),
                peeked.stream,
                None,
                0,
                Some("ModelNotAllowed".into()),
                Some(msg.clone()),
                None,
            );
            return wire.error_response(
                StatusCode::FORBIDDEN,
                &msg,
                "permission_error",
                Some("model_not_allowed"),
            );
        }
    }
    // ---- 重试预算（D9-T3）----
    // meta 覆盖项：`retry_max_per_group` / `retry_max_total`；缺失 / 非法用默认 (3, 6)。
    // 设成 (1, 1) 等于关掉重试 = 改造前行为。
    let budget = {
        let db = ctx.db.clone();
        match tokio::task::spawn_blocking(move || {
            db.with(|c| {
                Ok(router::retry_budget_from_meta(|k| {
                    store::meta_get(c, k).ok().flatten()
                }))
            })
        })
        .await
        {
            Ok(Ok(b)) => b,
            // 读不到 meta 不该挡路：回退默认预算
            _ => router::RetryBudget::default(),
        }
    };

    // ---- 逐渠道尝试（分组 + 预算 + 跨组规则）----
    // D9-T3：把「单轮遍历一遍」换成带预算与跨组语义的状态机。
    //
    // 分组：原生协议组（同族，字节直通）在前，转换组（跨族，走 IR）在后。
    // 组内顺序沿用上面的排序结果（priority + 健康 + 权重 + 限定名优先），
    // 所以**分组本身不改变现状行为**，只是把「隐式的 family 排序」显式化。
    //
    // 跨组规则是这一步的核心语义：凭据类失败（401/403）**只在同族组内**换候选，
    // 绝不跨组 —— 跨协议重试会让权限 / 语义错误被另一个协议的成功掩盖。
    // 只有限速 / 过载 / 端点不支持这类「可降级」失败才允许落到转换组。
    let (native, conversion) = router::group_candidates(candidates, wire.family());
    let groups: [Vec<store::RouteCandidate>; 2] = [
        native,
        // 旧版 /v1/completions 只支持 OpenAI 兼容直通，不做跨族转换
        if wire == InboundWire::Completions {
            Vec::new()
        } else {
            conversion
        },
    ];
    // 非幂等请求（Responses 带 `store` / `background`）预算压到 (1,1)：重发会有副作用。
    //
    // 注：原生组为空（候选全是跨族）时，「组内预算」同样作用于转换组 ——
    // 即最多试 `per_group` 个转换族候选就停，而不是把候选列表走完。
    let mut flow = router::AttemptFlow::new(budget, is_non_idempotent(wire, &body));
    let mut gi = 0usize;
    let mut ci = 0usize;
    let mut last_kind: &'static str = "ProviderOther";
    let mut last_summary = String::from("所有渠道均失败");
    let mut last_http: Option<UpstreamError> = None;
    let mut last_retry_after_ms: Option<u64> = None;
    // 首字节阶段的失败会带一份「预构造的失败响应」：真的没候选可试时用它兜底
    let mut last_fallback: Option<Response> = None;
    let mut total_backoff_ms: u64 = 0;

    'attempts: loop {
        // 定位下一个候选；本组走完就顺延到下一组，两组都走完则结束
        while ci >= groups[gi].len() {
            if gi + 1 < groups.len() && !groups[gi + 1].is_empty() {
                gi += 1;
                ci = 0;
            } else {
                break 'attempts;
            }
        }
        let cand = &groups[gi][ci];
        ci += 1;
        flow.record_attempt();

        // 跨族：推理回放兼容的**自适应**一环在 `try_converted_candidate` 的错误分类处
        // （400 走 `Stop` 直接交付，不会冒泡到这里），这里只做常规分类处理。
        let attempt = if cand.family != wire.family() {
            try_converted_candidate(&ctx, wire, &peeked, cand, &body, started).await
        } else {
            try_candidate(&ctx, wire, &peeked, cand, &body, &inbound_headers, started).await
        };

        let (class, kind, summary, eh, retry_after_ms, fallback) = match attempt {
            Attempt::Delivered(resp) => return resp,
            Attempt::Failed {
                class,
                kind,
                summary,
                last_http,
                retry_after_ms,
                fallback,
            } => (class, kind, summary, last_http, retry_after_ms, fallback),
        };
        last_kind = kind;
        last_summary = summary;
        if let Some(e) = eh {
            last_http = Some(e);
        }
        // 只有**最近一次**失败发生在首字节阶段时才用预构造响应兜底：
        // 否则该回后面那个候选的真实错误（上游错误体原样 / 502）
        last_fallback = if fallback.is_some() { fallback } else { None };

        // 本组还有候选吗？（决定 `next` 是「组内换」还是「跨组 / 停」）
        let group_exhausted = ci >= groups[gi].len();
        let has_next = !group_exhausted || groups[gi + 1..].iter().any(|g| !g.is_empty());

        if retry_after_ms.is_some() {
            // 即便不值得等，也要记住它：全失败时附在合成响应上（客户端退避依据），
            // 与改造前一致 —— 那个头是客户端唯一的退避依据。
            last_retry_after_ms = retry_after_ms;
        }
        // 退避：**只在上游明确给出 Retry-After 且该类失败值得等**时等待。
        //
        // 上游没说就不等 —— 500/503 往往是瞬时故障，而下一个候选是**另一个上游**，
        // 等它对这个上游毫无意义，只会让客户端白白多等（实测 500 占比不低，
        // 给每次切换都加 1s 会让多渠道路由整体变慢）。
        if let Some(ms) = retry_after_ms.filter(|_| class.waits_for_backoff()) {
            let remaining = router::MAX_TOTAL_BACKOFF_MS.saturating_sub(total_backoff_ms);
            if has_next && remaining > 0 {
                let delay = router::backoff_delay_ms(Some(ms), 0, router::RETRY_AFTER_JITTER_PCT)
                    .min(remaining);
                tokio::time::sleep(Duration::from_millis(delay)).await;
                total_backoff_ms += delay;
            }
        }

        match flow.next(class, group_exhausted) {
            router::FlowStep::Stop => break,
            router::FlowStep::NextInGroup => continue,
            router::FlowStep::NextGroup => {
                // 跳过本组剩余候选（组内预算已耗尽），进转换组
                if gi + 1 >= groups.len() {
                    break;
                }
                gi += 1;
                ci = 0;
            }
        }
    }
    // 全部渠道失败。
    // - 首字节阶段的失败已经预构造了响应（含 `upstream_stream_error` 这类专属
    //   诊断码）：此时没有任何候选可试了，原样回它，别退化成笼统的
    //   `all_providers_failed`
    // - 若存在带 HTTP 状态的上游错误：原样回传（保留状态码与协议方言形状，
    //   如 Anthropic 线 429/529）—— roadmap M2「返回最后一个错误」
    // - 否则（网络级失败）：统一 502
    if let Some(resp) = last_fallback {
        return resp;
    }
    if let Some(err) = last_http {
        emit_log(
            &ctx.logs,
            wire.log_family(),
            Some(&peeked),
            None,
            None,
            err.status.as_u16() as i64,
            ms_since(started),
            peeked.stream,
            None,
            0,
            Some(last_kind.into()),
            Some(last_summary.clone()),
            None,
        );
        let mut b = Response::builder().status(err.status);
        b = match err.content_type {
            Some(ct) => b.header(header::CONTENT_TYPE, ct),
            None => b.header(header::CONTENT_TYPE, "application/json"),
        };
        // 回传退避头：上游 429/503 的 Retry-After 是客户端退避的唯一依据，
        // 此前这里只重建 status + content-type，头全丢 → 客户端退避与上游错拍。
        for (name, value) in &err.headers {
            b = b.header(name, value);
        }
        return b
            .body(Body::from(err.body))
            .unwrap_or_else(|_| empty_resp(err.status));
    }

    emit_log(
        &ctx.logs,
        wire.log_family(),
        Some(&peeked),
        None,
        None,
        502,
        ms_since(started),
        peeked.stream,
        None,
        0,
        Some(last_kind.into()),
        Some(last_summary.clone()),
        None,
    );
    let mut resp = wire.error_response(
        StatusCode::BAD_GATEWAY,
        &last_summary,
        "api_error",
        Some("all_providers_failed"),
    );
    // 转换路径全失败会走到这里（它刻意不原样回传上游错误体），但退避信息
    // 不该跟着丢：上游明确给了 Retry-After 就带上，客户端才有退避依据。
    if let Some(ms) = last_retry_after_ms {
        if let Ok(v) = HeaderValue::from_str(&ms.div_ceil(1000).to_string()) {
            resp.headers_mut().insert(header::RETRY_AFTER, v);
        }
    }
    resp
}

/// 把下游安全请求头透传给上游（同族字节直通时需要保留，例如
/// `Content-Type: application/json` 与流式用的 `Accept: text/event-stream`）。
/// 坚决不透传 `Authorization` / `x-api-key`（避免把网关 Key 泄漏到上游）。
fn forward_inbound_headers(
    mut req: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    const FORWARD: [&str; 6] = [
        "content-type",
        "accept",
        "accept-language",
        "user-agent",
        "x-request-id",
        "x-stainless-*",
    ];
    for key in &FORWARD {
        if *key == "x-stainless-*" {
            // 部分 SDK（Anthropic/OpenAI 官方 SDK）会带多个 x-stainless-* 头，
            // 直接按前缀透传，方便上游风控识别为官方客户端。
            for (name, value) in headers.iter() {
                let name_lower = name.as_str().to_ascii_lowercase();
                if name_lower.starts_with("x-stainless-") || name_lower.starts_with("x-request-id")
                {
                    req = req.header(name, value);
                }
            }
            continue;
        }
        if let Some(value) = headers.get(*key) {
            req = req.header(*key, value);
        }
    }
    req
}

/// 把请求体里的 `model` 字段替换为上游真实模型名（去掉 `供应商/` 前缀）。
/// 仅用于限定 ID；解析失败时原样返回，避免破坏透传。
fn rewrite_body_model(body: &Bytes, new_model: &str) -> Bytes {
    let Ok(mut v) = serde_json::from_slice::<Value>(body) else {
        return body.clone();
    };
    v["model"] = Value::String(new_model.to_string());
    match serde_json::to_vec(&v) {
        Ok(bytes) => Bytes::from(bytes),
        Err(_) => body.clone(),
    }
}

/// 渠道声明的覆盖项 → 规划用 [`ChannelPolicy`]（0011 推理档位值域 + 0012 工具数上限）。
/// 模型级优先、回落供应商级（在 `store::route_candidates` 的 COALESCE 里解析）；
/// 两级都没声明 → `None`，调用方短路，行为与旧版一致（网关不发明限制）。
fn channel_policy_of(
    cand: &store::RouteCandidate,
    reasoning_replay: bool,
) -> Option<crate::codec::capability::ChannelPolicy> {
    let policy = crate::codec::capability::ChannelPolicy {
        effort: crate::codec::capability::EffortPolicy::new(
            cand.reasoning_effort_levels.as_ref(),
            format!("供应商「{}」", cand.provider_name),
        ),
        max_tools: cand
            .max_tools
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n > 0),
        reasoning_replay,
    };
    (!policy.is_empty()).then_some(policy)
}

/// 尝试单渠道（同族直通）。失败已按 router 分类，只返回「可转移」失败。
async fn try_candidate(
    ctx: &GatewayCtx,
    wire: InboundWire,
    peeked: &PeekRequest,
    cand: &store::RouteCandidate,
    body: &Bytes,
    inbound_headers: &HeaderMap,
    started: Instant,
) -> Attempt {
    // 原始入站字节：推理回放兼容需要**用原始请求重试一次**（重试时 `replay` 标记已学到，
    // 重新走一遍这里的注入逻辑），所以必须在 `body` 被改写结果遮蔽前留一份。
    let raw_body = body;
    // ---- 取上游密钥 ----
    let secret = match cand.api_key.as_deref() {
        Some(k) => k.to_string(),
        None => {
            let msg = "该供应商尚未录入 API Key，请在设置中补录";
            provider_mark_fail(&ctx.db, &cand.provider_id, msg);
            emit_log(
                &ctx.logs,
                wire.log_family(),
                Some(peeked),
                Some(&cand.provider_id),
                None,
                500,
                ms_since(started),
                peeked.stream,
                None,
                0,
                Some("UpstreamAuth".into()),
                Some(msg.to_string()),
                None,
            );
            return Attempt::Failed {
                class: router::FailureClass::from_kind("UpstreamAuth"),
                kind: "UpstreamAuth",
                summary: format!("{}：{msg}", cand.provider_name),
                last_http: None,
                retry_after_ms: None,
                fallback: None,
            };
        }
    };

    // 模型别名/映射：models.upstream_model_id 非空时，发给上游的 model 换为真实模型 id
    let body = match cand.upstream_model_id.as_deref() {
        Some(real) => rewrite_body_model(body, real),
        None => body.clone(),
    };

    // 渠道声明归一（0011 推理档位值域 + 0012 工具数上限）：**同族直通也要过一遍** ——
    // 客户端发的东西这家上游未必认，而族级能力表管不到「这家上游具体怎么校验」。
    // 真机故障即此两例：`reasoning_effort:"none"` 被原样透传给只认 low..max 的上游
    // → 400 UNSUPPORTED_FIELD；140 个工具被网关自己的 128 硬默认拦掉 → 400
    // tools_limit_exceeded（上游实际接受）。未声明的渠道整段短路（保持原始字节）。
    // 推理回放（自适应，见 `codec::replay`）：同族直通不做编解码，`openai` 编码器插不上
    // 手，所以这里在**已确认需要**（推断命中或学习命中）时直接改 JSON —— 未命中时
    // policy 为 `None`，整段短路，字节原样（与旧版一致，网关不发明字段）。
    let replay_needed = crate::codec::replay::enabled_for(&ctx.replay, &ctx.db, cand);
    let body = match (
        crate::codec::Family::from_db_str(&cand.family),
        channel_policy_of(cand, replay_needed),
    ) {
        (Some(family), Some(policy)) => {
            // 工具数上限：声明了就拦（与跨族路径同一语义、同一错误码）
            if let Some(max) = policy.max_tools {
                if let Some(n) = crate::codec::capability::body_tool_count(&body) {
                    if n > max {
                        let msg = format!(
                            "工具声明数 {n} 超过该渠道声明上限 {max}（可在供应商/模型设置里调整该上限）"
                        );
                        emit_log(
                            &ctx.logs,
                            wire.log_family(),
                            Some(peeked),
                            Some(&cand.provider_id),
                            cand.upstream_model_id.clone(),
                            400,
                            ms_since(started),
                            peeked.stream,
                            None,
                            0,
                            Some("InvalidRequest".into()),
                            Some(msg.clone()),
                            None,
                        );
                        return Attempt::Delivered(wire.error_response(
                            StatusCode::BAD_REQUEST,
                            &msg,
                            "invalid_request_error",
                            Some("tools_limit_exceeded"),
                        ));
                    }
                }
            }
            if let Some(effort) = policy.effort.as_ref() {
                if let Some(cur) = crate::effort::body_effort(&body, family) {
                    if !matches!(
                        crate::effort::place(&cur, &effort.levels),
                        crate::effort::Placement::Passthrough
                    ) {
                        emit_log(
                            &ctx.logs,
                            wire.log_family(),
                            Some(peeked),
                            Some(&cand.provider_id),
                            cand.upstream_model_id.clone(),
                            200,
                            ms_since(started),
                            peeked.stream,
                            None,
                            0,
                            Some("CapabilityWarn".into()),
                            Some(format!(
                                "reasoning.effort({cur})：{}声明档位为 {} → 已按该值域归一",
                                effort.source,
                                effort.levels.join("/")
                            )),
                            None,
                        );
                    }
                }
            }
            let body = match policy.effort.as_ref() {
                Some(effort) => {
                    match crate::effort::normalize_body(&body, family, &effort.levels) {
                        Some(next) => Bytes::from(next),
                        None => body,
                    }
                }
                None => body,
            };
            // 推理回放：给缺推理字段的 assistant 消息补上非空占位（只在已确认需要时）。
            if policy.reasoning_replay {
                match crate::codec::replay::inject_placeholders(&body) {
                    Some(next) => Bytes::from(next),
                    None => body,
                }
            } else {
                body
            }
        }
        _ => body,
    };

    // ---- 组装上游请求（body 原样字节）----
    // 包成闭包以便重建：reqwest 的 RequestBuilder 是一次性的，
    // 同渠道重试（should_retry_connect）必须拿一份全新请求。
    let build_upstream = || {
        let url = url_join(&cand.base_url, wire.upstream_path());
        let mut r = wire.apply_auth(ctx.http.post(&url), &secret);
        if let Some(eh_raw) = cand.extra_headers.as_deref() {
            match serde_json::from_str::<serde_json::Map<String, Value>>(eh_raw) {
                Ok(map) => {
                    for (k, v) in map {
                        if let Some(s) = v.as_str() {
                            if let (Ok(name), Ok(val)) = (
                                HeaderName::from_bytes(k.as_bytes()),
                                HeaderValue::from_str(s),
                            ) {
                                r = r.header(name, val);
                            }
                        }
                    }
                }
                Err(e) => eprintln!("[proxy] extra_headers 解析失败(忽略): {e}"),
            }
        }
        forward_inbound_headers(r, inbound_headers)
    };

    // 同渠道重试：连接类失败时立刻用一份新请求再试一次（见 UPSTREAM_CONNECT_RETRY）。
    let mut connect_tries = 0usize;
    let upstream = loop {
        match build_upstream().body(body.clone()).send().await {
            Ok(r) => break Ok(r),
            Err(e) if should_retry_connect(&e, connect_tries) => {
                connect_tries += 1;
                eprintln!(
                    "[proxy] 上游连接失败，同渠道重试 {connect_tries}/{UPSTREAM_CONNECT_RETRY}: {e}"
                );
            }
            Err(e) => break Err(e),
        }
    };
    let resp = match upstream {
        Ok(r) => r,
        Err(e) => {
            // 连接拒绝 / 连接超时（10s connect timeout 在此兑现）→ 故障转移
            let msg = format!("上游连接失败: {e}");
            provider_mark_fail(&ctx.db, &cand.provider_id, &msg);
            emit_log(
                &ctx.logs,
                wire.log_family(),
                Some(peeked),
                Some(&cand.provider_id),
                None,
                502,
                ms_since(started),
                peeked.stream,
                None,
                0,
                Some("ProviderOther".into()),
                Some(msg.clone()),
                None,
            );
            return Attempt::Failed {
                // 建连失败：网络级瞬时故障，换个候选有意义
                class: router::FailureClass::Retryable,
                kind: "ProviderOther",
                summary: format!("{}：{msg}", cand.provider_name),
                last_http: None,
                retry_after_ms: None,
                fallback: None,
            };
        }
    };

    let up_status =
        StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    if !up_status.is_success() {
        // 先取头再消费 body（bytes() 会拿走 Response 的所有权）
        let upstream_ct = resp.headers().get(header::CONTENT_TYPE).cloned();
        // 错误响应头白名单：退避依据（PI-Desktop / zcode 都会解析 retry-after /
        // retry-after-ms，缺了它们客户端只能按自己的指数退避重试 → 对已限速的上游
        // 重试放大）+ 向上游报障的关联键。其余上游头一律不回传。
        let upstream_headers: Vec<(HeaderName, HeaderValue)> = FORWARD_ERROR_HEADERS
            .iter()
            .filter_map(|name| {
                resp.headers()
                    .get(*name)
                    .map(|v| (HeaderName::from_static(name), v.clone()))
            })
            .collect();
        let mut bytes = resp.bytes().await.unwrap_or_default();
        if bytes.len() > MAX_ERROR_BODY {
            bytes.truncate(MAX_ERROR_BODY);
        }
        let hint = String::from_utf8_lossy(&bytes).into_owned();
        let snippet: String = hint.chars().take(240).collect();

        // 推理回放兼容的**自适应**一环（`codec::replay`）：直通路径同样要能「识别报错 →
        // 记下该渠道 → 用兼容原地重试一次」。400 会落到下面的 `Stop` 分支**直接交付**
        // （不冒泡到 `dispatch` 的 `Failed`），所以钩子必须在这里。
        if crate::codec::replay::is_replay_rejection(up_status.as_u16(), &snippet)
            && crate::codec::replay::should_retry_after_rejection(&ctx.replay, &ctx.db, cand)
        {
            emit_log(
                &ctx.logs,
                wire.log_family(),
                Some(peeked),
                Some(&cand.provider_id),
                cand.upstream_model_id.clone(),
                up_status.as_u16() as i64,
                ms_since(started),
                peeked.stream,
                None,
                0,
                Some("CapabilityWarn".into()),
                Some(format!(
                    "{}：上游要求回放推理（HTTP {}）→ 已记下该渠道并要求非空推理回放，正在重试同一请求",
                    cand.provider_name,
                    up_status.as_u16()
                )),
                None,
            );
            let retried = Box::pin(try_candidate(
                ctx,
                wire,
                peeked,
                cand,
                raw_body,
                inbound_headers,
                started,
            ))
            .await;
            // 重试仍以同一状态码被拒 ⇒ 这个开关对它无效 → 抑制，避免下次再试
            if let Attempt::Delivered(r) = &retried {
                if r.status() == up_status {
                    ctx.replay.suppress(&ctx.db, &cand.provider_id);
                }
            }
            return retried;
        }

        // 错误分类：可转移 → 下一渠道；确定性错误 → 原样返回。
        // D9-T3：分类改用 `classify_failure`（多了「能不能跨组」一维），
        // 但落库的 `kind` 字符串仍由它携带，与 `classify_status` 逐字一致。
        let failure = router::classify_failure(up_status.as_u16(), &snippet);
        let kind = failure.as_kind();
        if failure.class == router::FailureClass::CallerTerminal {
            provider_mark_fail(&ctx.db, &cand.provider_id, &snippet);
            emit_log(
                &ctx.logs,
                wire.log_family(),
                Some(peeked),
                Some(&cand.provider_id),
                cand.upstream_model_id.clone(),
                up_status.as_u16() as i64,
                ms_since(started),
                peeked.stream,
                None,
                0,
                Some(kind.into()),
                Some(if kind == "ContextTooLong" {
                    format!("{}：上下文长度超限（{kind}）", cand.provider_name)
                } else {
                    format!("{}：{snippet}", cand.provider_name)
                }),
                None,
            );
            let mut b = Response::builder().status(up_status);
            b = match upstream_ct {
                Some(ct) => b.header(header::CONTENT_TYPE, ct),
                None => b.header(header::CONTENT_TYPE, "application/json"),
            };
            // 确定性错误也带上退避头（上游 429 被归类为 Stop 时客户端同样需要）
            for (name, value) in &upstream_headers {
                b = b.header(name, value);
            }
            return Attempt::Delivered(
                b.body(Body::from(bytes))
                    .unwrap_or_else(|_| empty_resp(up_status)),
            );
        }
        {
            provider_mark_fail(&ctx.db, &cand.provider_id, &snippet);
            emit_log(
                &ctx.logs,
                wire.log_family(),
                Some(peeked),
                Some(&cand.provider_id),
                cand.upstream_model_id.clone(),
                up_status.as_u16() as i64,
                ms_since(started),
                peeked.stream,
                None,
                0,
                Some(kind.into()),
                Some(format!(
                    "{}：「{kind}」即将切换渠道：{snippet}",
                    cand.provider_name
                )),
                None,
            );
            // 退避时长必须在 headers 被 move 进 UpstreamError 之前算出来。
            let retry_after_ms = retry_after_from_headers(&upstream_headers);
            return Attempt::Failed {
                class: failure.class,
                kind,
                summary: format!(
                    "{}：上游 {kind}（HTTP {}）",
                    cand.provider_name,
                    up_status.as_u16()
                ),
                last_http: Some(UpstreamError {
                    status: up_status,
                    content_type: upstream_ct,
                    headers: upstream_headers,
                    body: bytes,
                }),
                retry_after_ms,
                fallback: None,
            };
        }
    }

    provider_mark_ok(&ctx.db, &cand.provider_id);

    // ---- 成功路径分流 ----
    let ct_is_sse = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("text/event-stream"))
        .unwrap_or(false);

    // 直通流式：首字节阶段失败会返回 `Attempt::Failed`（可 failover），
    // 非流式与已 commit 后的失败一律 `Delivered`。
    if ct_is_sse || peeked.stream {
        streaming_response(
            ctx.clone(),
            wire,
            peeked.clone(),
            cand.clone(),
            resp,
            up_status,
            started,
        )
        .await
    } else {
        Attempt::Delivered(
            plain_response(
                ctx.clone(),
                wire,
                peeked.clone(),
                cand.clone(),
                resp,
                up_status,
                started,
            )
            .await,
        )
    }
}

// ---------------------------------------------------------------- M4/M5：跨族转换

/// 尝试单渠道的**跨族转换**：
/// - M4：入站 OpenAI → 上游 Anthropic / Gemini
/// - M5：入站 Anthropic → 上游 OpenAI / Gemini
///
/// 全链路：InboundCodec 解码 → IR → UpstreamCodec 编码 → 上游 → 响应解析 → 渲染回入站形状。
async fn try_converted_candidate(
    ctx: &GatewayCtx,
    wire: InboundWire,
    peeked: &PeekRequest,
    cand: &store::RouteCandidate,
    body: &Bytes,
    started: Instant,
) -> Attempt {
    // 原始入站字节：推理回放兼容需要**用原始请求重试一次**（重试会重新解码 + 重新
    // 规划，此时 `replay` 标记已学到），所以必须在 `body` 被编码结果遮蔽前留一份。
    let raw_body = body;
    // 1) 解码入站（按入站线分发）
    let mut req = match wire {
        InboundWire::OpenAi => match crate::codec::openai::decode_request(body) {
            Ok(r) => r,
            Err(msg) => {
                return Attempt::Delivered(wire.error_response(
                    StatusCode::BAD_REQUEST,
                    &msg,
                    "invalid_request_error",
                    None,
                ));
            }
        },
        InboundWire::Anthropic => match crate::codec::anthropic::decode_request(body) {
            Ok(r) => r,
            Err(msg) => {
                return Attempt::Delivered(wire.error_response(
                    StatusCode::BAD_REQUEST,
                    &msg,
                    "invalid_request_error",
                    None,
                ));
            }
        },
        InboundWire::Responses => match crate::codec::responses::decode_request(body) {
            Ok(r) => r,
            Err(msg) => {
                return Attempt::Delivered(wire.error_response(
                    StatusCode::BAD_REQUEST,
                    &msg,
                    "invalid_request_error",
                    None,
                ));
            }
        },
        InboundWire::Completions => {
            return Attempt::Delivered(wire.error_response(
                StatusCode::BAD_REQUEST,
                "旧版 /v1/completions 仅支持 openai_compat 直通，不支持跨族转换",
                "invalid_request_error",
                None,
            ));
        }
    };
    // 模型别名/映射：models.upstream_model_id 非空时，编码给上游前换为真实模型 id
    if let Some(real) = cand.upstream_model_id.as_deref() {
        req.model = real.to_string();
    }

    // M5：Anthropic 入站历史 tool id 反解（含 tool_id_map 超长回落）
    if wire == InboundWire::Anthropic {
        resolve_anthropic_inbound_tool_ids(&ctx.db, &mut req);
    }

    // 2) 护栏（M4：单条消息 blocks ≤ 64 / args ≤ 256KB）
    if let Err(msg) = crate::codec::ir::validate_guards(&req) {
        emit_log_with(
            RouteMode::Converted,
            &ctx.logs,
            wire.log_family(),
            Some(peeked),
            Some(cand.provider_id.as_str()),
            cand.upstream_model_id.clone(),
            400,
            ms_since(started),
            peeked.stream,
            None,
            0,
            Some("InvalidRequest".into()),
            Some(msg.clone()),
            None,
        );
        return Attempt::Delivered(wire.error_response(
            StatusCode::BAD_REQUEST,
            &msg,
            "invalid_request_error",
            None,
        ));
    }

    // 2.5) 能力声明 + 兼容性规划（capability.rs）：降级执行 / Lenient WARN / 能力面 400
    // 取代原 response_format 硬编码 400 与 extension_warn_note 汇总，拒绝语义与错误码保持
    if let Some(family) = crate::codec::Family::from_db_str(&cand.family) {
        // 渠道声明的推理档位值域（0011）：未声明时 policy 为 None，行为与旧版一致
        // 渠道声明的推理档位值域（0011）+ 推理回放兼容（自适应，codec::replay）：
        // 两者都未命中时 policy 为 None，行为与旧版一致（网关不发明限制）。
        let policy = channel_policy_of(
            cand,
            crate::codec::replay::enabled_for(&ctx.replay, &ctx.db, cand),
        );
        let plan = crate::codec::capability::plan_compatibility_with(
            &req,
            crate::codec::capability::caps_of(family),
            policy.as_ref(),
        );
        let outcome = plan.resolve(&mut req);
        if let Some((msg, code)) = outcome.rejection {
            emit_log_with(
                RouteMode::Converted,
                &ctx.logs,
                wire.log_family(),
                Some(peeked),
                Some(cand.provider_id.as_str()),
                cand.upstream_model_id.clone(),
                400,
                ms_since(started),
                peeked.stream,
                None,
                0,
                Some("InvalidRequest".into()),
                Some(msg.clone()),
                None,
            );
            return Attempt::Delivered(wire.error_response(
                StatusCode::BAD_REQUEST,
                &msg,
                "invalid_request_error",
                code,
            ));
        }
        if !outcome.warnings.is_empty() {
            // 能力面降级 / Lenient 丢弃 WARN 进结构化日志（UI 日志页可见），
            // 不再仅落控制台
            emit_log_with(
                RouteMode::Converted,
                &ctx.logs,
                wire.log_family(),
                Some(peeked),
                Some(cand.provider_id.as_str()),
                cand.upstream_model_id.clone(),
                200,
                ms_since(started),
                peeked.stream,
                None,
                0,
                Some("CapabilityWarn".into()),
                Some(outcome.warnings.join("；")),
                None,
            );
        }
    }

    // 3) 按上游协议族编码（url + 上游请求体；builder 在 §4 逐次重建）
    let (url, body_json) = match cand.family.as_str() {
        "anthropic" => {
            let body = match crate::codec::anthropic::encode_request(&req) {
                Ok(b) => b,
                Err(msg) => {
                    return Attempt::Delivered(wire.error_response(
                        StatusCode::BAD_REQUEST,
                        &msg,
                        "invalid_request_error",
                        None,
                    ));
                }
            };
            let url = url_join(&cand.base_url, "/v1/messages");
            // 鉴权：x-api-key + anthropic-version（代理调用上游）
            (url, body)
        }
        "gemini" => {
            // Gemini 不接受任意外链（fileData 仅限 GCS URI）：
            // http(s) 图片必须转 inlineData/base64（M4-G-d，8s 超时，失败明确 400）
            let mut req2 = req.clone();
            if let Err(msg) = resolve_remote_images(ctx, &mut req2).await {
                return Attempt::Delivered(wire.error_response(
                    StatusCode::BAD_REQUEST,
                    &msg,
                    "invalid_request_error",
                    Some("image_fetch_failed"),
                ));
            }
            let body = match crate::codec::gemini::encode_request(&req2) {
                Ok(b) => b,
                Err(msg) => {
                    return Attempt::Delivered(wire.error_response(
                        StatusCode::BAD_REQUEST,
                        &msg,
                        "invalid_request_error",
                        None,
                    ));
                }
            };
            let path = crate::codec::gemini::model_url(&req.model);
            let mut url = url_join(&cand.base_url, &path);
            if req.stream {
                url.push_str("?alt=sse");
            }
            // 鉴权：x-goog-api-key
            (url, body)
        }
        "openai_compat" => {
            // M5：Anthropic 入站 → OpenAI 兼容上游
            let body = match crate::codec::openai::encode_request(&req) {
                Ok(b) => b,
                Err(msg) => {
                    return Attempt::Delivered(wire.error_response(
                        StatusCode::BAD_REQUEST,
                        &msg,
                        "invalid_request_error",
                        None,
                    ));
                }
            };
            let url = url_join(&cand.base_url, "/chat/completions");
            (url, body)
        }
        "openai_responses" => {
            // Responses 入站 → Responses 同族上游（dsh/one-model）
            let body = match crate::codec::responses::encode_request(&req) {
                Ok(b) => b,
                Err(msg) => {
                    return Attempt::Delivered(wire.error_response(
                        StatusCode::BAD_REQUEST,
                        &msg,
                        "invalid_request_error",
                        None,
                    ));
                }
            };
            let url = url_join(&cand.base_url, "/responses");
            (url, body)
        }
        other => {
            return Attempt::Failed {
                // 渠道声明了网关不认识的协议族：换个候选有意义
                class: router::FailureClass::Retryable,
                kind: "ProviderOther",
                summary: format!("未知上游协议族: {other}"),
                last_http: None,
                retry_after_ms: None,
                fallback: None,
            };
        }
    };

    // 4) 取上游密钥并组装请求
    let secret = match cand.api_key.as_deref() {
        Some(k) => k.to_string(),
        None => {
            let msg = "该供应商尚未录入 API Key，请在设置中补录";
            provider_mark_fail(&ctx.db, &cand.provider_id, msg);
            return Attempt::Failed {
                class: router::FailureClass::from_kind("UpstreamAuth"),
                kind: "UpstreamAuth",
                summary: format!("{}：{msg}", cand.provider_name),
                last_http: None,
                retry_after_ms: None,
                fallback: None,
            };
        }
    };
    // 包成闭包以便重建：reqwest 的 RequestBuilder 是一次性的，同渠道重试
    // （should_retry_connect）必须拿一份全新请求；各分支的 builder 都是 post(&url)。
    let build_upstream = || {
        let mut r = match cand.family.as_str() {
            "anthropic" => ctx.http.post(&url).header("x-api-key", &secret),
            "gemini" => ctx.http.post(&url).header("x-goog-api-key", &secret),
            "openai_compat" | "openai_responses" => ctx.http.post(&url).bearer_auth(&secret),
            _ => ctx.http.post(&url),
        };
        if let Some(eh_raw) = cand.extra_headers.as_deref() {
            if let Ok(map) = serde_json::from_str::<serde_json::Map<String, Value>>(eh_raw) {
                for (k, v) in map {
                    if let Some(s) = v.as_str() {
                        if let (Ok(name), Ok(val)) = (
                            HeaderName::from_bytes(k.as_bytes()),
                            HeaderValue::from_str(s),
                        ) {
                            r = r.header(name, val);
                        }
                    }
                }
            }
        }
        r
    };

    // 同渠道重试：连接类失败时立刻用一份新请求再试一次（见 UPSTREAM_CONNECT_RETRY）。
    let mut connect_tries = 0usize;
    let upstream = loop {
        match build_upstream()
            .json(&body_json)
            .timeout(Duration::from_secs(300))
            .send()
            .await
        {
            Ok(r) => break Ok(r),
            Err(e) if should_retry_connect(&e, connect_tries) => {
                connect_tries += 1;
                eprintln!(
                    "[proxy] 上游连接失败，同渠道重试 {connect_tries}/{UPSTREAM_CONNECT_RETRY}: {e}"
                );
            }
            Err(e) => break Err(e),
        }
    };

    let resp = match upstream {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("上游连接失败: {e}");
            provider_mark_fail(&ctx.db, &cand.provider_id, &msg);
            emit_log_with(
                RouteMode::Converted,
                &ctx.logs,
                wire.log_family(),
                Some(peeked),
                Some(&cand.provider_id),
                None,
                502,
                ms_since(started),
                req.stream,
                None,
                0,
                Some("ProviderOther".into()),
                Some(format!("[convert] {}：{msg}", cand.provider_name)),
                None,
            );
            return Attempt::Failed {
                // 转换路径的建连失败：同样是网络级瞬时故障
                class: router::FailureClass::Retryable,
                kind: "ProviderOther",
                summary: format!("{}：{msg}", cand.provider_name),
                last_http: None,
                retry_after_ms: None,
                fallback: None,
            };
        }
    };

    let up_status =
        StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    if !up_status.is_success() {
        // 错误响应头白名单（与直通路径一致）：跨族失败**不**原样回传上游错误体
        // （见测试 `converted_upstream_429_renders_oai_error`：跨族必须按入站方言
        // 渲染 502，否则 OpenAI 客户端会收到 Anthropic 形状的错误体），
        // 但退避头要用来做内部调度，不能跟着一起丢。
        let upstream_headers: Vec<(HeaderName, HeaderValue)> = FORWARD_ERROR_HEADERS
            .iter()
            .filter_map(|name| {
                resp.headers()
                    .get(*name)
                    .map(|v| (HeaderName::from_static(name), v.clone()))
            })
            .collect();
        let mut bytes = resp.bytes().await.unwrap_or_default();
        if bytes.len() > MAX_ERROR_BODY {
            bytes.truncate(MAX_ERROR_BODY);
        }
        let hint = String::from_utf8_lossy(&bytes).into_owned();
        let snippet: String = hint.chars().take(240).collect();

        // 推理回放兼容的**自适应**一环（`codec::replay`）：确定性错误里识别「要求回放推理」
        // 的 4xx。400 会落到下面的 `Stop` 分支**直接交付**（不冒泡到 `dispatch` 的 `Failed`，
        // 那里收不到），所以钩子必须在这里。首次遇到 → 学习 + 用兼容**原地重试一次**
        // （重试时规划层会给出非空推理占位），客户端只看到成功；重试仍被同类错误拒绝
        // ⇒ 抑制该开关，之后不再重试（不抖动，也不会把本来能跑的配置永久改坏）。
        // 直通路径（同族字节转发）不经 `openai` 编码器，对应处理在 `try_candidate` 里。
        if crate::codec::replay::is_replay_rejection(up_status.as_u16(), &snippet)
            && crate::codec::replay::should_retry_after_rejection(&ctx.replay, &ctx.db, cand)
        {
            emit_log_with(
                RouteMode::Converted,
                &ctx.logs,
                wire.log_family(),
                Some(peeked),
                Some(&cand.provider_id),
                cand.upstream_model_id.clone(),
                up_status.as_u16() as i64,
                ms_since(started),
                req.stream,
                None,
                0,
                Some("CapabilityWarn".into()),
                Some(format!(
                    "[convert] {}：上游要求回放推理（HTTP {}）→ 已记下该渠道并要求非空推理回放，正在重试同一请求",
                    cand.provider_name,
                    up_status.as_u16()
                )),
                None,
            );
            let retried = Box::pin(try_converted_candidate(
                ctx, wire, peeked, cand, raw_body, started,
            ))
            .await;
            // 重试仍以同一状态码被拒 ⇒ 这个开关对它无效 → 抑制，避免下次再试
            if let Attempt::Delivered(r) = &retried {
                if r.status() == up_status {
                    ctx.replay.suppress(&ctx.db, &cand.provider_id);
                }
            }
            return retried;
        }

        // D9-T3：与直通路径同一口径（`kind` 逐字不变，多出跨组维度）
        let failure = router::classify_failure(up_status.as_u16(), &snippet);
        let kind = failure.as_kind();
        if failure.class == router::FailureClass::CallerTerminal {
            provider_mark_fail(&ctx.db, &cand.provider_id, &snippet);
            emit_log_with(
                RouteMode::Converted,
                &ctx.logs,
                wire.log_family(),
                Some(peeked),
                Some(&cand.provider_id),
                cand.upstream_model_id.clone(),
                up_status.as_u16() as i64,
                ms_since(started),
                req.stream,
                None,
                0,
                Some(kind.into()),
                Some(format!("[convert] {}：{snippet}", cand.provider_name)),
                None,
            );
            // 确定性错误：翻译为入站方言（OpenAI schema）
            let (tname, code) = match kind {
                "ContextTooLong" => ("invalid_request_error", Some("context_length_exceeded")),
                _ => ("invalid_request_error", None),
            };
            return Attempt::Delivered(wire.error_response(up_status, &snippet, tname, code));
        }
        {
            provider_mark_fail(&ctx.db, &cand.provider_id, &snippet);
            emit_log_with(
                RouteMode::Converted,
                &ctx.logs,
                wire.log_family(),
                Some(peeked),
                Some(&cand.provider_id),
                cand.upstream_model_id.clone(),
                up_status.as_u16() as i64,
                ms_since(started),
                req.stream,
                None,
                0,
                Some(kind.into()),
                Some(format!(
                    "[convert] {}：「{kind}」即将切换渠道：{snippet}",
                    cand.provider_name
                )),
                None,
            );
            return Attempt::Failed {
                class: failure.class,
                kind,
                summary: format!(
                    "{}：上游 {kind}（HTTP {}）",
                    cand.provider_name,
                    up_status.as_u16()
                ),
                last_http: None,
                retry_after_ms: retry_after_from_headers(&upstream_headers),
                fallback: None,
            };
        }
    }

    provider_mark_ok(&ctx.db, &cand.provider_id);

    // ---- 成功：解析 + 重渲染 ----
    let ct_is_sse = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.starts_with("text/event-stream"))
        .unwrap_or(false);

    if ct_is_sse {
        convert_streaming_response(
            ctx.clone(),
            wire,
            peeked.clone(),
            cand.clone(),
            resp,
            up_status,
            started,
            crate::codec::capability::tool_identities_of(&req),
            // 本轮生效的输出预算（请求侧声明，或 1.5) 按模型配置兜底后的值）。
            // 只给「零可见输出的截断轮」诊断落日志用，不参与编码。
            req.params.max_output_tokens,
        )
        .await
    } else {
        // 结构化输出降级校验标志（capability 规划层写入 extensions）
        let validate_output = req
            .extensions
            .get("__jai_validate_output")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        convert_plain_response(
            ctx.clone(),
            wire,
            peeked.clone(),
            cand.clone(),
            resp,
            up_status,
            started,
            validate_output,
            crate::codec::capability::tool_identities_of(&req),
            req.params.max_output_tokens,
            // 客户端要流式、上游却回整包 JSON → 由 convert_plain_response 补成 SSE
            req.stream,
        )
        .await
    }
}

/// 转换路径：非流式解析 + 渲染为入站形状。
#[allow(clippy::too_many_arguments)] // 与 convert_streaming_response（7 参）同族，保持平铺
async fn convert_plain_response(
    ctx: GatewayCtx,
    wire: InboundWire,
    peeked: PeekRequest,
    cand: store::RouteCandidate,
    resp: reqwest::Response,
    status: StatusCode,
    started: Instant,
    validate_output: bool,
    tool_identities: Vec<crate::codec::capability::ToolIdentity>,
    // 本轮生效的输出预算；只用于「零可见输出的截断轮」诊断（见
    // `empty_truncation_diagnostic`）。
    output_budget: Option<u32>,
    // 客户端要流式但上游没按 SSE 回时，把整包响应补成入站 SSE（见函数内注释）。
    as_stream: bool,
) -> Attempt {
    let bytes = match tokio::time::timeout(NONSTREAM_READ_TIMEOUT, resp.bytes()).await {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            let msg = format!("上游响应读取失败: {e}");
            emit_log_with(
                RouteMode::Converted,
                &ctx.logs,
                wire.log_family(),
                Some(&peeked),
                Some(&cand.provider_id),
                None,
                502,
                ms_since(started),
                false,
                None,
                0,
                Some("ProviderOther".into()),
                Some(format!("[convert] {msg}")),
                None,
            );
            return Attempt::Delivered(wire.error_response(
                StatusCode::BAD_GATEWAY,
                &msg,
                "api_error",
                None,
            ));
        }
        Err(_) => {
            emit_log_with(
                RouteMode::Converted,
                &ctx.logs,
                wire.log_family(),
                Some(&peeked),
                Some(&cand.provider_id),
                None,
                504,
                ms_since(started),
                false,
                None,
                0,
                Some("Overloaded".into()),
                Some(format!(
                    "[convert] Overloaded read timeout (status={})",
                    status.as_u16()
                )),
                None,
            );
            return Attempt::Delivered(wire.error_response(
                wire.overloaded_status(),
                "上游响应读取超时(300s)",
                "api_error",
                Some("upstream_read_timeout"),
            ));
        }
    };

    // 按上游协议族解析 IR
    let parsed = match cand.family.as_str() {
        "anthropic" => crate::codec::anthropic::parse_response(&bytes),
        "gemini" => crate::codec::gemini::parse_response(&bytes),
        "openai_compat" => crate::codec::openai::parse_response(&bytes),
        "openai_responses" => crate::codec::responses::parse_response(&bytes),
        other => Err(format!("未知上游协议族: {other}")),
    };
    let mut resp = match parsed {
        Ok(r) => r,
        Err(msg) => {
            emit_log_with(
                RouteMode::Converted,
                &ctx.logs,
                wire.log_family(),
                Some(&peeked),
                Some(&cand.provider_id),
                None,
                502,
                ms_since(started),
                false,
                None,
                0,
                Some("ProviderOther".into()),
                Some(format!("[convert] 解析失败: {msg}")),
                None,
            );
            return Attempt::Delivered(wire.error_response(
                StatusCode::BAD_GATEWAY,
                &msg,
                "api_error",
                None,
            ));
        }
    };

    // M5：Anthropic 入站出站 tool id 先映射（含超长 tool_id_map 回落）
    if wire == InboundWire::Anthropic {
        for b in &mut resp.output {
            if let crate::codec::ir::Block::ToolUse { id, .. } = b {
                *id = map_anthropic_tool_id(&ctx.db, id);
            }
        }
    }

    // 结构化输出降级：非流式校验上游输出为合法 JSON（capability 规划层标记，
    // 仅降级路径；原生 json_schema 上游保证格式）
    if validate_output {
        let text = resp
            .output
            .iter()
            .filter_map(|b| match b {
                crate::codec::ir::Block::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        if serde_json::from_str::<Value>(&text).is_err() {
            let msg = "上游未遵守 JSON 结构化输出约束（降级校验失败）";
            emit_log_with(
                RouteMode::Converted,
                &ctx.logs,
                wire.log_family(),
                Some(&peeked),
                Some(&cand.provider_id),
                cand.upstream_model_id.clone(),
                502,
                ms_since(started),
                false,
                None,
                0,
                Some("ProviderOther".into()),
                Some(format!("[convert] {msg}")),
                None,
            );
            return Attempt::Delivered(wire.error_response(
                StatusCode::BAD_GATEWAY,
                msg,
                "api_error",
                Some("structured_output_validation_failed"),
            ));
        }
    }

    // 客户端要流式、上游却回了整包 JSON（忽略了 stream=true）：不能当 SSE 解析
    // ——整包里没有 `data:` 行，一帧都发不出去，客户端只会看到
    // 「200 + text/event-stream + 零帧」的静默空轮。改为把整包响应补成入站 SSE。
    //
    // 必须放在下面那条非流式 `emit_log_with` **之前**：deliver_synthesized_stream 自己
    // 会按 `stream=true` 落一条日志，否则同一请求会留下两条（一条 stream=false 的假象）。
    if as_stream {
        return deliver_synthesized_stream(
            ctx,
            wire,
            peeked,
            cand,
            status,
            started,
            tool_identities,
            output_budget,
            &resp,
        );
    }

    let usage = &resp.usage;
    let uval = json!({
        "prompt_tokens": usage.input_tokens,
        "completion_tokens": usage.output_tokens,
    });
    emit_log_with(
        RouteMode::Converted,
        &ctx.logs,
        wire.log_family(),
        Some(&peeked),
        Some(&cand.provider_id),
        cand.upstream_model_id.clone(),
        status.as_u16() as i64,
        ms_since(started),
        false,
        Some(&uval),
        count_ir_tool_uses(&resp),
        None,
        None,
        Some(resp.stop_reason.as_log_str()),
    );

    let rendered = match wire {
        InboundWire::OpenAi | InboundWire::Completions => {
            crate::codec::openai::render_response(&resp).to_string()
        }
        InboundWire::Anthropic => crate::codec::anthropic::render_response(&resp).to_string(),
        InboundWire::Responses => {
            // 扩展工具折叠还原（§10）：function_call → shell_call 等原始 item
            let value = crate::codec::responses::render_response(&resp);
            crate::codec::responses::restore_extended_items(value, &tool_identities).to_string()
        }
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(rendered))
        .map(Attempt::Delivered)
        .unwrap_or_else(|_| Attempt::Delivered(empty_resp(status)))
}

/// 入站线的 SSE 渲染器（三种入站形状各一份状态）。
///
/// 从 [`convert_streaming_response`] 提到模块级，好让「上游整包 JSON → 补成 SSE」的
/// [`deliver_synthesized_stream`] 走**同一套**渲染口径，不复制第二份事件映射。
enum SseRenderer {
    OpenAi(crate::codec::openai::RenderState),
    Anthropic(crate::codec::anthropic::AnthropicRenderState),
    Responses(crate::codec::responses::RenderState),
}

/// 按入站线构造渲染器（含 Responses 侧还原扩展工具 item 用的身份表）。
fn new_sse_renderer(
    wire: InboundWire,
    model: &str,
    started: Instant,
    tool_identities: Vec<crate::codec::capability::ToolIdentity>,
) -> SseRenderer {
    let stamp = started.elapsed().as_millis();
    match wire {
        InboundWire::OpenAi | InboundWire::Completions => {
            SseRenderer::OpenAi(crate::codec::openai::RenderState {
                id: format!("chatcmpl-jai-{stamp}"),
                model: model.to_string(),
                started: false,
            })
        }
        InboundWire::Anthropic => {
            SseRenderer::Anthropic(crate::codec::anthropic::AnthropicRenderState {
                message_id: format!("msg_jai_{stamp}"),
                model: model.to_string(),
                active_block: None,
                text_started: false,
                active_tool_index: None,
                next_block_index: 0,
                finished: false,
            })
        }
        InboundWire::Responses => SseRenderer::Responses(crate::codec::responses::RenderState {
            response_id: format!("resp_jai_{stamp}"),
            model: model.to_string(),
            started: false,
            output_index: 0,
            msg_started: false,
            active_tool_item_id: String::new(),
            active_tool_call_id: String::new(),
            active_tool_name: String::new(),
            active_tool_args: String::new(),
            current_text: String::new(),
            reasoning_started: false,
            current_reasoning: String::new(),
            tool_identities,
            active_tool_type: String::new(),
            completed_items: Vec::new(),
        }),
    }
}

/// 渲染单个 IR 事件 → SSE 输出帧（一个 IR 事件可能展开多个 SSE 事件）。
fn render_frame(renderer: &mut SseRenderer, ev: &crate::codec::ir::StreamEvent) -> Vec<String> {
    match renderer {
        SseRenderer::OpenAi(st) => crate::codec::openai::render_stream_event(ev, st)
            .map(|line| format!("data: {line}\n\n"))
            .into_iter()
            .collect(),
        SseRenderer::Anthropic(st) => crate::codec::anthropic::render_stream_event(ev, st)
            .into_iter()
            .map(|(evt, data)| format!("event: {evt}\ndata: {data}\n\n"))
            .collect(),
        SseRenderer::Responses(st) => crate::codec::responses::render_stream_event(ev, st)
            .into_iter()
            .map(|payload| {
                let event = serde_json::from_str::<Value>(&payload)
                    .ok()
                    .and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_else(|| "response.output_text.delta".to_string());
                format!("event: {event}\ndata: {payload}\n\n")
            })
            .collect(),
    }
}

/// 上游忽略了 `stream: true`、回了整包 JSON 时，把 [`crate::codec::ir::CanonicalResponse`]
/// **补成入站 SSE**。
///
/// 为什么需要：转换路径原先按「`ct_is_sse || req.stream`」选转换器，于是「客户端要流式、
/// 上游却回 JSON」的请求被喂进 SSE 解析器 —— 整包里没有 `data:` 行，一帧都发不出去，
/// 客户端只看到「200 + text/event-stream + 零帧」的**静默空轮**（对 agent 客户端等于丢一轮，
/// 且不报错）。收尾口径与 [`convert_streaming_response`] 的自然结束分支保持一致。
#[allow(clippy::too_many_arguments)] // 与 convert_streaming_response（9 参）同族，保持平铺
fn deliver_synthesized_stream(
    ctx: GatewayCtx,
    wire: InboundWire,
    peeked: PeekRequest,
    cand: store::RouteCandidate,
    status: StatusCode,
    started: Instant,
    tool_identities: Vec<crate::codec::capability::ToolIdentity>,
    // 见 `convert_plain_response` 的同名参数。
    output_budget: Option<u32>,
    resp: &crate::codec::ir::CanonicalResponse,
) -> Attempt {
    use crate::codec::ir::{Block, StreamEvent};

    let mut renderer = new_sse_renderer(wire, &peeked.model, started, tool_identities);

    // Start（渲染侧重置状态）→ 内容增量 → Finish
    let mut events: Vec<StreamEvent> = vec![StreamEvent::Start {
        model: resp.model.clone(),
    }];
    let mut tool_index = 0usize;
    for b in &resp.output {
        match b {
            Block::Text { text } => events.push(StreamEvent::TextDelta { text: text.clone() }),
            Block::Thinking { text, .. } => {
                events.push(StreamEvent::ThinkingDelta { text: text.clone() })
            }
            Block::ToolUse { id, name, input } => {
                events.push(StreamEvent::ToolCallStart {
                    index: tool_index,
                    id: id.clone(),
                    name: name.clone(),
                });
                events.push(StreamEvent::ToolCallArgsDelta {
                    index: tool_index,
                    args_fragment: serde_json::to_string(input).unwrap_or_else(|_| "{}".into()),
                });
                events.push(StreamEvent::ToolCallEnd { index: tool_index });
                tool_index += 1;
            }
            _ => {}
        }
    }
    events.push(StreamEvent::Finish {
        stop_reason: resp.stop_reason.clone(),
        usage: resp.usage.clone(),
    });

    let mut body = String::new();
    for ev in &events {
        for frame in render_frame(&mut renderer, ev) {
            body.push_str(&frame);
        }
    }
    // 与流式路径的自然结束一致：Anthropic 补 message_stop，其余补 [DONE]
    if wire == InboundWire::Anthropic {
        body.push_str(&format!(
            "event: message_stop\ndata: {}\n\n",
            crate::codec::anthropic::render_message_stop()
        ));
    } else {
        body.push_str("data: [DONE]\n\n");
    }

    let usage = &resp.usage;
    let uval = json!({
        "prompt_tokens": usage.input_tokens,
        "completion_tokens": usage.output_tokens,
    });
    // 「零可见输出」判定：整包响应里没有任何正文、也没有工具调用（thinking 不算可见）。
    let saw_visible_output = resp.output.iter().any(|b| match b {
        Block::Text { text } => !text.trim().is_empty(),
        Block::ToolUse { .. } => true,
        _ => false,
    });
    let (diag_kind, diag_summary) = truncation_diagnostic(
        Some(resp.stop_reason.as_log_str()),
        saw_visible_output,
        output_budget,
        usage.output_tokens,
    )
    .map(|(k, s)| (Some(k.to_string()), Some(s)))
    .unwrap_or((None, None));
    emit_log_with(
        RouteMode::Converted,
        &ctx.logs,
        wire.log_family(),
        Some(&peeked),
        Some(&cand.provider_id),
        cand.upstream_model_id.clone(),
        status.as_u16() as i64,
        ms_since(started),
        true,
        Some(&uval),
        count_ir_tool_uses(resp),
        diag_kind,
        diag_summary,
        Some(resp.stop_reason.as_log_str()),
    );

    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-jai-mode", HeaderValue::from_static("converted"))
        .body(Body::from(body))
        .map(Attempt::Delivered)
        .unwrap_or_else(|_| Attempt::Delivered(empty_resp(status)))
}

/// 转换路径：流式。逐 SSE 事件 parse → IR StreamEvent → render 回入站 SSE。
#[allow(clippy::too_many_arguments)] // 与 convert_plain_response（9 参）同族，保持平铺
async fn convert_streaming_response(
    ctx: GatewayCtx,
    wire: InboundWire,
    peeked: PeekRequest,
    cand: store::RouteCandidate,
    resp: reqwest::Response,
    status: StatusCode,
    started: Instant,
    tool_identities: Vec<crate::codec::capability::ToolIdentity>,
    // 见 `convert_plain_response` 的同名参数。
    output_budget: Option<u32>,
) -> Attempt {
    let mut upstream_stream = resp.bytes_stream();

    // 首字节持票：**在拿到任何字节之前失败都可以 failover**（此刻客户端一个字节
    // 也还没收到）。预构造失败响应放进 `fallback`，供「真的没候选可试」时原样回。
    let first = tokio::time::timeout(UPSTREAM_FIRST_BYTE_TIMEOUT, upstream_stream.next()).await;
    let first_chunk = match first {
        Ok(Some(Ok(b))) => b,
        Ok(Some(Err(e))) => {
            return first_byte_failure(
                wire,
                &format!("上游流建立失败: {e}"),
                StatusCode::BAD_GATEWAY,
                "upstream_stream_error",
                "ProviderOther",
            );
        }
        Ok(None) => {
            return first_byte_failure(
                wire,
                "上游流立即关闭",
                StatusCode::BAD_GATEWAY,
                "upstream_empty_stream",
                "ProviderOther",
            );
        }
        Err(_) => {
            return first_byte_failure(
                wire,
                "上游首字节超时(60s)",
                wire.overloaded_status(),
                "upstream_first_byte_timeout",
                "Overloaded",
            );
        }
    };

    // 渲染器：按入站线构造（定义已提到模块级，供 deliver_synthesized_stream 复用同一口径）
    let mut renderer = new_sse_renderer(wire, &peeked.model, started, tool_identities);

    // SSE 事件 ⊆ 格式转换：上游原始帧 → IR → 入站帧
    let upstream_family = cand.family.clone();
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(64);
    // 首字节先入行缓冲（不丢数据）
    let first_lines: Vec<u8> = first_chunk.to_vec();

    // Start 事件先发
    for frame in render_frame(
        &mut renderer,
        &crate::codec::ir::StreamEvent::Start {
            model: peeked.model.clone(),
        },
    ) {
        let _ = tx.send(Ok(Bytes::from(frame))).await;
    }

    {
        let ctx2 = ctx.clone();
        let wire2 = wire;
        let peeked2 = peeked.clone();
        let pid = cand.provider_id.clone();
        tokio::spawn(async move {
            let t0 = Instant::now();
            let mut line_buf: Vec<u8> = first_lines;
            // IR 累计的 usage：Finish 事件携带，自然结束时透传落库（修复日志输入/输出恒空）
            let mut last_usage: Option<crate::codec::ir::Usage> = None;
            // 本回合 assistant 发起的工具调用 id 集合（落 request_logs.tool_calls）。
            // 本回合结束原因（IR 口径，落 request_logs.stop_reason）：与 usage 同源，
            // 取**先到的显式 finish_reason**（与 pending_finish 的裁决一致）。
            // 早期该列恒为 NULL —— 排查「模型为什么反复重发」时看不到是 max_tokens 截断。
            let mut last_stop_reason: Option<crate::codec::ir::StopReason> = None;
            // 用 id 去重而非数事件：部分上游（如 Gemini）每帧都带 Start，且 index 恒为 0。
            let mut tool_call_ids: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            // 本轮是否产出过**可见**输出（正文或工具调用）。thinking/reasoning **不算** ——
            // 客户端看不到推理，只有推理的一轮对用户等于「什么都没说」。收尾时与
            // `last_stop_reason` 一起判定「零可见输出的截断轮」（见
            // `empty_truncation_diagnostic`）。
            let mut saw_visible_output = false;
            // 挂起的 Finish：上游常把 stop_reason 与 usage 拆成两帧（先 finish_reason 帧、
            // 后 usage 末帧），且 usage 帧可能带非空 choices。若逐帧渲染，客户端会先收到一个
            // usage 全 0 的收尾帧，第二帧再渲染就是重复的 completed/message_stop。故挂起
            // Finish，等上游 [DONE] 或 EOF 时合并真实 usage 一次性发出（stop_reason 以先到的
            // 显式 finish_reason 帧为准）。
            let mut pending_finish: Option<(
                crate::codec::ir::StopReason,
                crate::codec::ir::Usage,
            )> = None;
            // 行缓冲护栏计时：line_buf 非空且长时间无完整行 → 断开（防无终止标记流拖死下游）
            let mut last_drain_at = Instant::now();
            let line_hold = sse_line_hold_timeout();

            // 发出挂起的 Finish（幂等：[DONE] 与 EOF 两处各调一次也只发一次）
            macro_rules! flush_pending_finish {
                () => {{
                    if let Some((stop_reason, usage)) = pending_finish.take() {
                        for frame in render_frame(
                            &mut renderer,
                            &crate::codec::ir::StreamEvent::Finish { stop_reason, usage },
                        ) {
                            if tx.send(Ok(Bytes::from(frame))).await.is_err() {
                                break;
                            }
                        }
                    }
                }};
            }

            // 解析并发送当前行缓冲（首字节可能已包含整个 SSE 流）
            macro_rules! drain_sse_lines {
                () => {{
                    while let Some(pos) = line_buf.iter().position(|&b| b == b'\n') {                        let line: Vec<u8> = line_buf.drain(..=pos).collect();
                        let line = line.strip_suffix(b"\r").unwrap_or(&line);
                        let line: &[u8] = line;
                        if !line.starts_with(b"data:") {
                            continue;
                        }
                        let payload: &[u8] = trim_ascii(&line[5..]);
                        if payload.is_empty() || payload == b"[DONE]" {
                            // OpenAI 系终止标记：usage 末帧已到齐，先补发挂起的 Finish
                            // （不等连接关闭——上游若保持连接，下游会缺收尾帧）
                            if payload == b"[DONE]" {
                                flush_pending_finish!();
                            }
                            continue;
                        }
                        let mut events: Vec<crate::codec::ir::StreamEvent> =
                            match upstream_family.as_str() {
                                "anthropic" => {
                                    match crate::codec::anthropic::parse_stream_event(payload) {
                                        Ok(v) => v,
                                        Err(e) => {
                                            emit_log_with(RouteMode::Converted,
                                                &ctx2.logs,
                                                wire2.log_family(),
                                                Some(&peeked2),
                                                Some(&pid),
                                                None,
                                                200,
                                                t0.elapsed().as_millis() as i64,
                                                true,
                                                None,
                                                0,
                                                Some("SseParseWarn".into()),
                                                Some(format!("[convert] anthropic SSE 帧解析失败已跳过: {e}")),
                                                None,
                                            );
                                            continue;
                                        }
                                    }
                                }
                                "gemini" => match crate::codec::gemini::parse_stream_event(payload) {
                                    Ok(v) => v,
                                    Err(e) => {
                                        emit_log_with(RouteMode::Converted,
                                            &ctx2.logs,
                                            wire2.log_family(),
                                            Some(&peeked2),
                                            Some(&pid),
                                            None,
                                            200,
                                            t0.elapsed().as_millis() as i64,
                                            true,
                                            None,
                                            0,
                                            Some("SseParseWarn".into()),
                                            Some(format!("[convert] gemini SSE 帧解析失败已跳过: {e}")),
                                            None,
                                        );
                                        continue;
                                    }
                                },
                                "openai_compat" => match crate::codec::openai::parse_stream_event(payload)
                                {
                                    Ok(v) => v,
                                    Err(e) => {
                                        emit_log_with(RouteMode::Converted,
                                            &ctx2.logs,
                                            wire2.log_family(),
                                            Some(&peeked2),
                                            Some(&pid),
                                            None,
                                            200,
                                            t0.elapsed().as_millis() as i64,
                                            true,
                                            None,
                                            0,
                                            Some("SseParseWarn".into()),
                                            Some(format!("[convert] openai SSE 帧解析失败已跳过: {e}")),
                                            None,
                                        );
                                        continue;
                                    }
                                },
                                "openai_responses" => {
                                    match crate::codec::responses::parse_stream_event(payload) {
                                        Ok(v) => v,
                                        Err(e) => {
                                            emit_log_with(RouteMode::Converted,
                                                &ctx2.logs,
                                                wire2.log_family(),
                                                Some(&peeked2),
                                                Some(&pid),
                                                None,
                                                200,
                                                t0.elapsed().as_millis() as i64,
                                                true,
                                                None,
                                                0,
                                                Some("SseParseWarn".into()),
                                                Some(format!("[convert] openai_responses SSE 帧解析失败已跳过: {e}")),
                                                None,
                                            );
                                            continue;
                                        }
                                    }
                                }
                                _ => continue,
                            };
                        // M5：Anthropic 入站出站 tool id 先映射（含超长 tool_id_map 回落）
                        if wire2 == InboundWire::Anthropic {
                            for ev in &mut events {
                                if let crate::codec::ir::StreamEvent::ToolCallStart { id, .. } = ev
                                {
                                    *id = map_anthropic_tool_id(&ctx2.db, id);
                                }
                            }
                        }
                        for ev in events {
                            // 可见输出判定（只记一次 true，之后不再逐事件匹配）
                            if !saw_visible_output {
                                match &ev {
                                    crate::codec::ir::StreamEvent::TextDelta { text } => {
                                        saw_visible_output = !text.trim().is_empty();
                                    }
                                    crate::codec::ir::StreamEvent::ToolCallStart { .. }
                                    | crate::codec::ir::StreamEvent::ToolCallArgsDelta { .. } => {
                                        saw_visible_output = true;
                                    }
                                    _ => {}
                                }
                            }
                            if let crate::codec::ir::StreamEvent::ToolCallStart { id, .. } = &ev {
                                if !id.is_empty() {
                                    tool_call_ids.insert(id.clone());
                                }
                            }
                            // Finish 不立刻下发：挂起合并（见 pending_finish 注释）。
                            // 一旦下发，客户端就会先看到一个 usage 全 0 的收尾帧，
                            // 后续真实 usage 只能变成重复的 completed/message_stop。
                            if let crate::codec::ir::StreamEvent::Finish {
                                stop_reason,
                                usage,
                            } = &ev
                            {
                                let is_zero =
                                    usage.input_tokens == 0 && usage.output_tokens == 0;
                                match &mut pending_finish {
                                    None => {
                                        pending_finish =
                                            Some((stop_reason.clone(), usage.clone()));
                                    }
                                    Some((_, prev)) => {
                                        // 后到的真实 usage 覆盖先到的零值占位；
                                        // 零值不覆盖已有真实值（含 usage 帧在前、finish_reason
                                        // 帧在后的供应商顺序）。
                                        let prev_zero =
                                            prev.input_tokens == 0 && prev.output_tokens == 0;
                                        if prev_zero && !is_zero {
                                            *prev = usage.clone();
                                        }
                                    }
                                }
                                if !is_zero || last_usage.is_none() {
                                if last_stop_reason.is_none() {
                                    last_stop_reason = Some(stop_reason.clone());
                                }
                                    last_usage = Some(usage.clone());
                                }
                                continue;
                            }
                            for frame in render_frame(&mut renderer, &ev) {
                                if tx.send(Ok(Bytes::from(frame))).await.is_err() {
                                    emit_log_with(RouteMode::Converted,
                                        &ctx2.logs,
                                        wire2.log_family(),
                                        Some(&peeked2),
                                        Some(&pid),
                                        None,
                                        status.as_u16() as i64,
                                        ms_since(t0),
                                        true,
                                        None,
                                        0,
                                        Some("InvalidRequest".into()),
                                        Some("client disconnected mid-stream".into()),
                                        None,
                                    );
                                    drop(tx);
                                    return;
                                }
                            }
                        }
                    }
                }};
            }
            // 断开辅助：发错误帧 + 落日志 + 关通道（由调用方决定 break/return）
            macro_rules! abort_sse_stream {
                ($msg:expr) => {{
                    let msg: String = $msg;
                    let _ = tx.send(Ok(error_sse_frame(wire2, &msg))).await;
                    emit_log_with(
                        RouteMode::Converted,
                        &ctx2.logs,
                        wire2.log_family(),
                        Some(&peeked2),
                        Some(&pid),
                        None,
                        status.as_u16() as i64,
                        ms_since(t0),
                        true,
                        None,
                        0,
                        Some("Overloaded".into()),
                        Some(format!("[convert] {msg}")),
                        None,
                    );
                    drop(tx);
                }};
            }
            drain_sse_lines!();
            // 首块即超限（如整流挤在一块无换行数据里）→ 直接断开
            if line_buf.len() > MAX_SSE_LINE_BYTES {
                abort_sse_stream!(format!(
                    "上游 SSE 单行超限(>{MAX_SSE_LINE_BYTES}B)且无换行，已断开（疑似无终止标记流）"
                ));
                return;
            }

            loop {
                let nxt = tokio::time::timeout(STREAM_IDLE_TIMEOUT, upstream_stream.next());
                match nxt.await {
                    Err(_) => {
                        let msg = format!("上游流空闲超时({}s)", STREAM_IDLE_TIMEOUT.as_secs());
                        let _ = tx.send(Ok(error_sse_frame(wire2, &msg))).await;
                        drop(tx);
                        emit_log_with(
                            RouteMode::Converted,
                            &ctx2.logs,
                            wire2.log_family(),
                            Some(&peeked2),
                            Some(&pid),
                            None,
                            status.as_u16() as i64,
                            ms_since(t0),
                            true,
                            None,
                            0,
                            Some("Overloaded".into()),
                            Some(format!(
                                "[convert] Overloaded upstream={} idle timeout",
                                status.as_u16()
                            )),
                            // 上游挂死前若已见过 Finish，结束原因照记（比 NULL 有用）
                            last_stop_reason.as_ref().map(|r| r.as_log_str()),
                        );
                        break;
                    }
                    Ok(Some(Ok(chunk))) => {
                        line_buf.extend_from_slice(&chunk);
                        // 缓冲护栏 1：单行超限（无换行刷流）→ 断开，防无界内存
                        if line_buf.len() > MAX_SSE_LINE_BYTES {
                            abort_sse_stream!(format!(
                                "上游 SSE 单行超限(>{MAX_SSE_LINE_BYTES}B)且无换行，已断开（疑似无终止标记流）"
                            ));
                            break;
                        }
                        drain_sse_lines!();
                        // 缓冲护栏 2：一直收字节但长时间无完整行（无终止标记流）→ 断开
                        if line_buf.is_empty() {
                            last_drain_at = Instant::now();
                        } else if last_drain_at.elapsed() > line_hold {
                            abort_sse_stream!(format!(
                                "上游 SSE 行 {}s 未完成（疑似无终止标记流），已断开",
                                line_hold.as_secs()
                            ));
                            break;
                        }
                    }
                    Ok(Some(Err(e))) => {
                        let msg = format!("stream aborted by upstream: {e}");
                        let _ = tx.send(Ok(error_sse_frame(wire2, &msg))).await;
                        provider_mark_fail(&ctx2.db, &pid, &msg);
                        emit_log_with(
                            RouteMode::Converted,
                            &ctx2.logs,
                            wire2.log_family(),
                            Some(&peeked2),
                            Some(&pid),
                            None,
                            status.as_u16() as i64,
                            ms_since(t0),
                            true,
                            None,
                            0,
                            Some("ProviderOther".into()),
                            Some(format!("[convert] {msg}")),
                            last_stop_reason.as_ref().map(|r| r.as_log_str()),
                        );
                        drop(tx);
                        break;
                    }
                    Ok(None) => {
                        // 自然结束：先补发挂起的 Finish（上游无 [DONE] 时靠这里收尾），
                        // 再补 message_stop / [DONE]。
                        flush_pending_finish!();
                        // 自然结束：Anthropic 补 message_stop，OpenAI 补 [DONE]
                        if wire2 == InboundWire::Anthropic {
                            let _ = tx
                                .send(Ok(Bytes::from(format!(
                                    "event: message_stop\ndata: {}\n\n",
                                    crate::codec::anthropic::render_message_stop()
                                ))))
                                .await;
                        } else {
                            let _ = tx.send(Ok(Bytes::from_static(b"data: [DONE]\n\n"))).await;
                        }
                        // 截断诊断（零可见输出 / 被预算掐短）：必须在 `last_usage` 被
                        // `usage_json` 的 map 取走之前算。
                        let (diag_kind, diag_summary) = truncation_diagnostic(
                            last_stop_reason.as_ref().map(|r| r.as_log_str()),
                            saw_visible_output,
                            output_budget,
                            last_usage.as_ref().map(|u| u.output_tokens).unwrap_or(0),
                        )
                        .map(|(k, s)| (Some(k.to_string()), Some(s)))
                        .unwrap_or((None, None));
                        // 结束时透传 IR 累计的 usage（此前硬编码 None 导致日志输入/输出恒空）
                        let usage_json = last_usage.map(|u| {
                            json!({
                                "prompt_tokens": u.input_tokens,
                                "completion_tokens": u.output_tokens,
                                "cache_read_input_tokens": u.cache_read_tokens,
                                "cache_creation_input_tokens": u.cache_write_tokens,
                            })
                        });
                        emit_log_with(
                            RouteMode::Converted,
                            &ctx2.logs,
                            wire2.log_family(),
                            Some(&peeked2),
                            Some(&pid),
                            None,
                            status.as_u16() as i64,
                            ms_since(t0),
                            true,
                            usage_json.as_ref(),
                            tool_call_ids.len() as i64,
                            diag_kind,
                            diag_summary,
                            last_stop_reason.as_ref().map(|r| r.as_log_str()),
                        );
                        drop(tx);
                        break;
                    }
                }
            }
        });
    }

    let client_stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });

    let body = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(
            "x-jai-provider",
            HeaderValue::from_str(&cand.provider_name)
                .unwrap_or(HeaderValue::from_static("unknown")),
        )
        .header("x-jai-mode", HeaderValue::from_static("converted"))
        .body(Body::from_stream(client_stream))
        .unwrap_or_else(|_| empty_resp(status));

    Attempt::Delivered(body)
}

fn internal_error(
    ctx: &GatewayCtx,
    wire: InboundWire,
    p: &PeekRequest,
    started: &Instant,
) -> Response {
    emit_log(
        &ctx.logs,
        wire.log_family(),
        Some(p),
        None,
        None,
        500,
        ms_since(*started),
        p.stream,
        None,
        0,
        Some("ProviderOther".into()),
        Some("internal error".into()),
        None,
    );
    wire.error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "内部错误",
        "api_error",
        None,
    )
}

// ---------------------------------------------------------------- 非流式

async fn plain_response(
    ctx: GatewayCtx,
    wire: InboundWire,
    peeked: PeekRequest,
    cand: store::RouteCandidate,
    resp: reqwest::Response,
    status: StatusCode,
    started: Instant,
) -> Response {
    let bytes = match tokio::time::timeout(NONSTREAM_READ_TIMEOUT, resp.bytes()).await {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            let msg = format!("上游响应读取失败: {e}");
            emit_log(
                &ctx.logs,
                wire.log_family(),
                Some(&peeked),
                Some(&cand.provider_id),
                None,
                502,
                ms_since(started),
                false,
                None,
                0,
                Some("ProviderOther".into()),
                Some(msg.clone()),
                None,
            );
            return wire.error_response(StatusCode::BAD_GATEWAY, &msg, "api_error", None);
        }
        Err(_) => {
            emit_log(
                &ctx.logs,
                wire.log_family(),
                Some(&peeked),
                Some(&cand.provider_id),
                None,
                504,
                ms_since(started),
                false,
                None,
                0,
                Some("Overloaded".into()),
                Some(format!(
                    "Overloaded upstream={} read timeout",
                    status.as_u16()
                )),
                None,
            );
            return wire.error_response(
                wire.overloaded_status(),
                "上游响应读取超时(300s)",
                "api_error",
                Some("upstream_read_timeout"),
            );
        }
    };

    let mut scanner = UsageScanner::new();
    scanner.feed(&bytes);
    let usage = scanner.finish();

    emit_log(
        &ctx.logs,
        wire.log_family(),
        Some(&peeked),
        Some(&cand.provider_id),
        cand.upstream_model_id.clone(),
        status.as_u16() as i64,
        ms_since(started),
        false,
        usage.as_ref(),
        count_tool_calls_in_body(wire, &bytes),
        None,
        None,
        stop_reason_in_body(wire, &bytes),
    );

    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| empty_resp(status))
}

// ---------------------------------------------------------------- 流式

async fn streaming_response(
    ctx: GatewayCtx,
    wire: InboundWire,
    peeked: PeekRequest,
    cand: store::RouteCandidate,
    resp: reqwest::Response,
    status: StatusCode,
    started: Instant,
) -> Attempt {
    // 首字节持票：成功前不给客户端下 200。
    //
    // D9-T3：此前这三个分支直接返回错误响应（包成 `Attempt::Delivered`），
    // 于是「首字节失败可 failover」这条语义（老 `first_byte_verdict` 想表达的）
    // 从未真正生效 —— 恰恰每个上游都会碰到的偶发断流，被白白变成客户端可见的 502。
    // 现在：**只要还没向下游发出任何字节**，就按可转移失败处理，让下一个候选接管；
    // 同时把错误响应塞进 `fallback`，真的没候选可试时原样回它（保住专属诊断码）。
    let mut upstream_stream = resp.bytes_stream();

    // 首字节持票：成功前不给客户端下 200，失败可整体换成错误响应
    let first = tokio::time::timeout(UPSTREAM_FIRST_BYTE_TIMEOUT, upstream_stream.next()).await;
    let first_chunk = match first {
        Ok(Some(Ok(b))) => b,
        Ok(Some(Err(e))) => {
            let msg = format!("上游流建立失败: {e}");
            provider_mark_fail(&ctx.db, &cand.provider_id, &msg);
            emit_log(
                &ctx.logs,
                wire.log_family(),
                Some(&peeked),
                Some(&cand.provider_id),
                None,
                502,
                ms_since(started),
                true,
                None,
                0,
                Some("ProviderOther".into()),
                Some(msg.clone()),
                None,
            );
            return first_byte_failure(
                wire,
                &msg,
                StatusCode::BAD_GATEWAY,
                "upstream_stream_error",
                "ProviderOther",
            );
        }
        Ok(None) => {
            let msg = "上游流立即关闭";
            provider_mark_fail(&ctx.db, &cand.provider_id, msg);
            emit_log(
                &ctx.logs,
                wire.log_family(),
                Some(&peeked),
                Some(&cand.provider_id),
                None,
                502,
                ms_since(started),
                true,
                None,
                0,
                Some("ProviderOther".into()),
                Some(msg.to_string()),
                None,
            );
            return first_byte_failure(
                wire,
                msg,
                StatusCode::BAD_GATEWAY,
                "upstream_empty_stream",
                "ProviderOther",
            );
        }
        Err(_) => {
            emit_log(
                &ctx.logs,
                wire.log_family(),
                Some(&peeked),
                Some(&cand.provider_id),
                None,
                504,
                ms_since(started),
                true,
                None,
                0,
                Some("Overloaded".into()),
                Some(format!(
                    "Overloaded upstream={} first-byte timeout",
                    status.as_u16()
                )),
                None,
            );
            return first_byte_failure(
                wire,
                "上游首字节超时(60s)",
                wire.overloaded_status(),
                "upstream_first_byte_timeout",
                "Overloaded",
            );
        }
    };

    // 通过管道把剩余流喂给客户端；usage 扫描 + 观测探针伴随进行。
    // `tool_calls` 自本版起由 PassthroughStreamProbe 按 IR 口径（工具调用 id 去重）计数，
    // 与非流式直通的 count_tool_calls_in_body 对齐 —— 此前该列在直通流式下恒 0。
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(64);

    let mut scanner = UsageScanner::new();
    scanner.feed(&first_chunk);
    // 结束原因探针（诊断字段，逐块扫描 + 末尾窗口兜底，见 StopReasonProbe）
    let mut probe = StopReasonProbe::new(wire);
    probe.feed(&first_chunk);
    // 观测探针（诊断字段，只读不改写字节，见 PassthroughStreamProbe）：
    // 可见输出 + 工具调用计数
    let mut passthrough = PassthroughStreamProbe::new(wire);
    passthrough.feed(&first_chunk);

    if tx.send(Ok(first_chunk)).await.is_err() {
        // 客户端瞬间断开：记录后退出
        emit_log(
            &ctx.logs,
            wire.log_family(),
            Some(&peeked),
            Some(&cand.provider_id),
            cand.upstream_model_id.clone(),
            status.as_u16() as i64,
            ms_since(started),
            true,
            scanner.finish().as_ref(),
            passthrough.tool_call_ids.len() as i64,
            Some("InvalidRequest".into()),
            Some("client disconnected early".into()),
            None,
        );
        return Attempt::Delivered(empty_resp(StatusCode::OK));
    }

    // 后台泵任务：持有 tx，循环转发并应用空闲超时；结束时负责落日志
    {
        let ctx2 = ctx.clone();
        let wire2 = wire;
        let peeked2 = peeked.clone();
        let cand2_model = cand.upstream_model_id.clone();
        let pid = cand.provider_id.clone();
        tokio::spawn(async move {
            let t0 = Instant::now();
            // probe 随 chunk 前进（见 StopReasonProbe），结束原因只在它内部累积
            loop {
                let nxt = tokio::time::timeout(STREAM_IDLE_TIMEOUT, upstream_stream.next());
                match nxt.await {
                    Err(_) => {
                        // 上游挂死：断开客户端连接（socket 收尾），并落超时日志
                        drop(tx);
                        emit_log(
                            &ctx2.logs,
                            wire2.log_family(),
                            Some(&peeked2),
                            Some(&pid),
                            cand2_model.clone(),
                            status.as_u16() as i64,
                            ms_since(t0),
                            true,
                            scanner.finish().as_ref(),
                            passthrough.tool_call_ids.len() as i64,
                            Some("Overloaded".into()),
                            Some(format!(
                                "Overloaded upstream={} idle timeout",
                                status.as_u16()
                            )),
                            // 上游挂死前若已见过终局帧，结束原因照记（比 NULL 有用）
                            probe.finish(),
                        );
                        break;
                    }
                    Ok(Some(Ok(chunk))) => {
                        scanner.feed(&chunk);
                        probe.feed(&chunk);
                        passthrough.feed(&chunk);
                        if tx.send(Ok(chunk)).await.is_err() {
                            emit_log(
                                &ctx2.logs,
                                wire2.log_family(),
                                Some(&peeked2),
                                Some(&pid),
                                cand2_model.clone(),
                                status.as_u16() as i64,
                                ms_since(t0),
                                true,
                                scanner.finish().as_ref(),
                                passthrough.tool_call_ids.len() as i64,
                                Some("InvalidRequest".into()),
                                Some("client disconnected mid-stream".into()),
                                None,
                            );
                            break;
                        }
                    }
                    Ok(Some(Err(e))) => {
                        // 上游中途断流：明确关闭下游 socket（roadmap M1 验收 2）
                        drop(tx);
                        let msg = format!("stream aborted by upstream: {e}");
                        provider_mark_fail(&ctx2.db, &pid, &msg);
                        emit_log(
                            &ctx2.logs,
                            wire2.log_family(),
                            Some(&peeked2),
                            Some(&pid),
                            cand2_model.clone(),
                            status.as_u16() as i64,
                            ms_since(t0),
                            true,
                            scanner.finish().as_ref(),
                            passthrough.tool_call_ids.len() as i64,
                            Some("ProviderOther".into()),
                            Some(msg),
                            probe.finish(),
                        );
                        break;
                    }
                    Ok(None) => {
                        // 正常结束
                        drop(tx);
                        // 「零可见输出的截断轮」诊断的直通半边：字节转发没有 IR，但探针
                        // 能回答「这一轮有没有产出用户看得见的东西」。
                        let usage_v = scanner.finish();
                        let out_tokens = usage_v
                            .as_ref()
                            .and_then(|u| u.get("completion_tokens"))
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                        let stop = probe.finish();
                        let (diag_kind, diag_summary) = truncation_diagnostic(
                            stop,
                            passthrough.seen_visible,
                            peeked2.max_output_tokens,
                            out_tokens,
                        )
                        .map(|(k, s)| (Some(k.to_string()), Some(s)))
                        .unwrap_or((None, None));
                        emit_log(
                            &ctx2.logs,
                            wire2.log_family(),
                            Some(&peeked2),
                            Some(&pid),
                            cand2_model.clone(),
                            status.as_u16() as i64,
                            ms_since(t0),
                            true,
                            usage_v.as_ref(),
                            passthrough.tool_call_ids.len() as i64,
                            diag_kind,
                            diag_summary,
                            stop,
                        );
                        break;
                    }
                }
            }
        });
    }

    let client_stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });

    Attempt::Delivered(
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(
                "x-jai-provider",
                HeaderValue::from_str(&cand.provider_name)
                    .unwrap_or(HeaderValue::from_static("unknown")),
            )
            .body(Body::from_stream(client_stream))
            .unwrap_or_else(|_| empty_resp(status)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 直接调用 handler 的用例需要一个鉴权结果（D9-T6b 起 handler 会拿它查密钥规则）。
    /// 用固定 id —— 测试库与 `GatewayCtx` 都是新建的，缓存里必然没有它，
    /// 于是查库得到「空规则」⇒ 不过滤。
    fn authed_test_key() -> Extension<security::AuthedKey> {
        Extension(security::AuthedKey {
            id: "k-test-no-rules".to_string(),
        })
    }

    /// bug 11 回归：`tool_calls` 日志列必须真数工具调用，不能再恒 0。
    /// 三种入站线的直通响应体形状各取一次（并行工具调用要全数到）。
    #[test]
    fn count_tool_calls_in_passthrough_bodies() {
        // OpenAI Chat：两条并行 tool_calls
        let openai = r#"{"choices":[{"message":{"role":"assistant","content":null,
            "tool_calls":[{"id":"a","function":{"name":"f"}},{"id":"b","function":{"name":"g"}}]}}]}"#;
        assert_eq!(
            count_tool_calls_in_body(InboundWire::OpenAi, openai.as_bytes()),
            2
        );
        // 纯文本响应 → 0
        let text = r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}]}"#;
        assert_eq!(
            count_tool_calls_in_body(InboundWire::OpenAi, text.as_bytes()),
            0
        );
        // Anthropic：content 里 text 与 tool_use 混排
        let anthropic = r#"{"content":[{"type":"text","text":"thinking"},{"type":"tool_use","id":"t1","name":"f"},
            {"type":"tool_use","id":"t2","name":"g"}]}"#;
        assert_eq!(
            count_tool_calls_in_body(InboundWire::Anthropic, anthropic.as_bytes()),
            2
        );
        // Responses：output 里 function_call 与 message/reasoning 混排
        let responses = r#"{"output":[{"type":"reasoning"},{"type":"function_call","call_id":"c1"},
            {"type":"message"}]}"#;
        assert_eq!(
            count_tool_calls_in_body(InboundWire::Responses, responses.as_bytes()),
            1
        );
        // 非 JSON / 空体：仅诊断字段，不 panic、也不影响转发
        assert_eq!(
            count_tool_calls_in_body(InboundWire::OpenAi, b"data: [DONE]"),
            0
        );
        assert_eq!(count_tool_calls_in_body(InboundWire::Responses, b""), 0);
    }

    /// `stop_reason` 日志列：三族原始词表都要归一到 IR 口径。
    #[test]
    fn log_stop_reason_maps_family_vocabularies() {
        // OpenAI 族
        assert_eq!(log_stop_reason("stop"), "end_turn");
        assert_eq!(log_stop_reason("length"), "max_tokens");
        assert_eq!(log_stop_reason("tool_calls"), "tool_use");
        assert_eq!(log_stop_reason("content_filter"), "safety");
        // Anthropic 族
        assert_eq!(log_stop_reason("end_turn"), "end_turn");
        assert_eq!(log_stop_reason("stop_sequence"), "end_turn");
        assert_eq!(log_stop_reason("max_tokens"), "max_tokens");
        assert_eq!(log_stop_reason("tool_use"), "tool_use");
        assert_eq!(log_stop_reason("refusal"), "safety");
        // Responses 族
        assert_eq!(log_stop_reason("completed"), "end_turn");
        assert_eq!(log_stop_reason("max_output_tokens"), "max_tokens");
        // 认不出的如实记 other，绝不猜
        assert_eq!(log_stop_reason("incomplete"), "other");
        assert_eq!(log_stop_reason("weird_new_reason"), "other");
    }

    /// 直通非流式：按入站线形状取结束原因；截断必须认出来（诊断价值所在）。
    #[test]
    fn stop_reason_in_passthrough_bodies() {
        let oai_cut = r#"{"choices":[{"message":{"role":"assistant","content":"x"},"finish_reason":"length"}]}"#;
        assert_eq!(
            stop_reason_in_body(InboundWire::OpenAi, oai_cut.as_bytes()),
            Some("max_tokens")
        );
        let oai_stop = r#"{"choices":[{"finish_reason":"stop"}]}"#;
        assert_eq!(
            stop_reason_in_body(InboundWire::OpenAi, oai_stop.as_bytes()),
            Some("end_turn")
        );
        let anthropic = r#"{"content":[],"stop_reason":"max_tokens"}"#;
        assert_eq!(
            stop_reason_in_body(InboundWire::Anthropic, anthropic.as_bytes()),
            Some("max_tokens")
        );
        // Responses：incomplete 必须读 incomplete_details.reason 才算得出截断
        let resp_cut =
            r#"{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}"#;
        assert_eq!(
            stop_reason_in_body(InboundWire::Responses, resp_cut.as_bytes()),
            Some("max_tokens")
        );
        let resp_ok = r#"{"status":"completed"}"#;
        assert_eq!(
            stop_reason_in_body(InboundWire::Responses, resp_ok.as_bytes()),
            Some("end_turn")
        );
        // 缺字段 / 非 JSON：None（不 panic、不影响转发）
        assert_eq!(
            stop_reason_in_body(InboundWire::OpenAi, b"{}"),
            None,
            "缺 finish_reason 应记 None"
        );
        assert_eq!(stop_reason_in_body(InboundWire::OpenAi, b"not json"), None);
    }

    /// 测试辅助：把若干块喂给探针并取结束原因（模拟 SSE 分块到达）。
    fn probe_of(wire: InboundWire, chunks: &[&str]) -> Option<&'static str> {
        let mut probe = StopReasonProbe::new(wire);
        for c in chunks {
            probe.feed(c.as_bytes());
        }
        probe.finish()
    }

    /// 直通流式探针：各族末帧形状都要认出来；前导帧的 `finish_reason` 是 null 不算命中。
    #[test]
    fn stop_reason_probe_reads_family_terminal_frames() {
        let oai = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        assert_eq!(probe_of(InboundWire::OpenAi, &[oai]), Some("max_tokens"));
        // 正文里出现的同类文本已被 JSON 转义（\"），不得误命中
        let escaped = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"say \\\"finish_reason\\\":\\\"stop\\\"\"},",
            "\"finish_reason\":\"tool_calls\"}]}\n\n"
        );
        assert_eq!(
            probe_of(InboundWire::OpenAi, &[escaped]),
            Some("tool_use"),
            "应取真正的结束字段而不是正文里的转义文本"
        );
        let anthropic = concat!(
            "event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
        );
        assert_eq!(
            probe_of(InboundWire::Anthropic, &[anthropic]),
            Some("end_turn")
        );
        // 空流 / 无结束字段：None
        assert_eq!(probe_of(InboundWire::OpenAi, &[""]), None);
        assert_eq!(probe_of(InboundWire::OpenAi, &["data: [DONE]\n\n"]), None);
    }

    /// 坑 1 回归：Responses 终局帧把**全部正文**嵌在同一帧里，截断响应正文可达 30KB+。
    /// 固定 16KB 窗口会把帧首的 `status`/`incomplete_details` 挤出窗口 —— 必须逐块扫描。
    #[test]
    fn stop_reason_probe_survives_huge_terminal_frame() {
        let mut frame = String::from(
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\
             \"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"},\
             \"output\":[{\"type\":\"message\",\"status\":\"completed\",\"content\":[{\"type\":\"output_text\",\"text\":\"",
        );
        frame.push_str(&"x".repeat(40 * 1024)); // 正文远大于 16KB 窗口
        frame.push_str("\"}]}]}}\n\n");
        // 单块喂入（真实场景下这一帧往往单独到达）
        assert_eq!(
            probe_of(InboundWire::Responses, &[&frame]),
            Some("max_tokens"),
            "帧首的 incomplete_details 不能被同帧大段正文挤出窗口"
        );
        // 同一帧被切成多块（标记在首块、正文跨多块）也要认出来
        let bytes = frame.as_bytes();
        let cut = bytes.len() / 3;
        let parts: Vec<&str> = vec![
            std::str::from_utf8(&bytes[..cut]).unwrap(),
            std::str::from_utf8(&bytes[cut..]).unwrap(),
        ];
        assert_eq!(probe_of(InboundWire::Responses, &parts), Some("max_tokens"));
    }

    /// 坑 2 回归：同一帧里 `output[*]` 的 item 也带 `status`，且截断响应末尾的 item
    /// 反而是 `completed`；只按「最后一次 status」判定会把截断误记成 end_turn。
    /// `incomplete_details.reason` 必须优先。
    #[test]
    fn stop_reason_probe_prefers_incomplete_reason_over_item_status() {
        let frame =
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\
             \"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"},\
             \"output\":[{\"type\":\"function_call\",\"status\":\"completed\"}]}}\n\n";
        assert_eq!(
            probe_of(InboundWire::Responses, &[frame]),
            Some("max_tokens"),
            "末尾 item 的 status=completed 不得盖掉 incomplete_details.reason"
        );
        // 内容安全同口径
        let filtered = "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\
             \"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"content_filter\"}}}\n\n";
        assert_eq!(
            probe_of(InboundWire::Responses, &[filtered]),
            Some("safety")
        );
        // 正常结束：没有 reason，用 status 兜底
        let ok = "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\
             \"status\":\"completed\",\"output\":[{\"type\":\"message\",\"status\":\"completed\"}]}}\n\n";
        assert_eq!(probe_of(InboundWire::Responses, &[ok]), Some("end_turn"));
    }

    /// 末尾窗口必须是有界滚动缓冲：超限只丢最旧字节，且保留的正是结尾。
    #[test]
    fn keep_tail_is_bounded_and_keeps_the_end() {
        let mut tail = Vec::new();
        keep_tail(&mut tail, b"abcdef");
        assert_eq!(tail, b"abcdef");
        // 灌入远超窗口的数据：长度封顶、内容为结尾
        let big = vec![b'x'; PASSTHROUGH_TAIL_BYTES + 100];
        keep_tail(&mut tail, &big);
        assert_eq!(tail.len(), PASSTHROUGH_TAIL_BYTES);
        keep_tail(&mut tail, b"TAIL-MARKER");
        assert!(tail.ends_with(b"TAIL-MARKER"), "必须保留结尾而不是开头");
    }

    /// 转换路径按 IR 块计数（跨族请求的 tool_calls 落库口径）。
    #[test]
    fn count_ir_tool_uses_counts_tool_use_blocks() {
        use crate::codec::ir::{Block, CanonicalResponse, StopReason, Usage};
        let mk = |output: Vec<Block>| CanonicalResponse {
            id: "r1".into(),
            model: "m".into(),
            output,
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
        };
        let with_tools = mk(vec![
            Block::Text { text: "hi".into() },
            Block::ToolUse {
                id: "t1".into(),
                name: "f".into(),
                input: serde_json::json!({}),
            },
            Block::ToolUse {
                id: "t2".into(),
                name: "g".into(),
                input: serde_json::json!({}),
            },
        ]);
        assert_eq!(count_ir_tool_uses(&with_tools), 2);

        let only_text = mk(vec![Block::Text { text: "hi".into() }]);
        assert_eq!(count_ir_tool_uses(&only_text), 0);
    }

    #[test]
    fn anthropic_error_sse_frame_shape() {
        let bytes = error_sse_frame(InboundWire::Anthropic, "上游中途断开");
        let s = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(
            s.starts_with(
                "event: error
"
            ),
            "Anthropic 应发 event: error，得到: {s:?}"
        );
        assert!(s.contains("\"type\":\"error\""), "应含 Anthropic 错误对象");
        assert!(s.contains("上游中途断开"));
    }

    #[test]
    fn openai_error_sse_frame_shape() {
        let bytes = error_sse_frame(InboundWire::OpenAi, "upstream boom");
        let s = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(s.starts_with("data: "), "OpenAI 应发 data: 错误帧");
        assert!(s.contains("upstream boom"));
    }

    #[test]
    fn long_tool_id_map_roundtrip_in_proxy() {
        let db = Db::in_memory().unwrap();
        let long = format!("call_{}", "x".repeat(80));
        let short = map_anthropic_tool_id(&db, &long);
        assert!(short.starts_with("toolu_"), "短 id 应带前缀: {short}");
        assert!(short.len() <= 64, "短 id 长度 {} 应 ≤ 64", short.len());
        let stored = db
            .with_any(|c| store::tool_id_get(c, &short))
            .unwrap()
            .expect("映射应已落库");
        assert_eq!(stored, long);
    }

    #[tokio::test]
    async fn models_list_exposes_context_window() {
        let db = Db::in_memory().unwrap();
        db.with(|c| {
            let now = store::now_ms();
            store::provider_insert(
                c,
                &store::ProviderRow {
                    id: "p1".into(),
                    name: "prov".into(),
                    base_url: "http://x".into(),
                    family: "openai_compat".into(),
                    enabled: true,
                    priority: 1,
                    weight: 1,
                    extra_headers: None,
                    api_key: None,
                    website: None,
                    last_ok_at: None,
                    last_err_at: None,
                    last_err_msg: None,
                    max_tools: None,
                    reasoning_effort_levels: None,
                    created_at: now,
                    updated_at: now,
                },
            )?;
            // 显式窗口 + NULL（回落 128k）两类都要覆盖
            store::model_upsert(c, "p1", "alpha", Some(65536), 8192, None, None)?;
            store::model_upsert(c, "p1", "beta", None, 4096, None, None)?;
            Ok(())
        })
        .expect("seed 失败");

        let dir = std::env::temp_dir().join(format!("jai-models-test-{}", rand::random::<u32>()));
        let log_path = dir.join("main.db");
        std::fs::create_dir_all(&dir).unwrap();
        let (logs, _t) = crate::store::logs::spawn_logger(log_path.to_str().unwrap()).unwrap();
        let ctx = GatewayCtx::new(db.clone(), logs);

        // D9-T6b：同 `models_list_exposes_supports_multimodal` —— 给一把没配规则的密钥。
        let resp = models_list(State(ctx), authed_test_key()).await;
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        let data = v["data"].as_array().unwrap();
        assert_eq!(data.len(), 2);
        let alpha = data
            .iter()
            .find(|m| m["id"] == "prov/alpha")
            .expect("alpha 模型应存在");
        assert_eq!(alpha["contextWindow"], 65536);
        let beta = data
            .iter()
            .find(|m| m["id"] == "prov/beta")
            .expect("beta 模型应存在");
        assert_eq!(
            beta["contextWindow"], 128000,
            "context_window 为 NULL 时应给保守默认 128k"
        );
    }

    /// P0 回归：**零可见输出的截断轮**必须被标出来。
    ///
    /// 真机形状（2026-09-22 基元律动/deepseek-flash，460k 上下文）：上游
    /// `finish_reason:"length"`，completion_tokens 只有 16/96/165/250，正文为空、
    /// 无工具调用，推理有内容。此前 `error_kind` 恒 NULL ⇒ 与正常 200 无法区分，
    /// 客户端侧只看到一句 `EMPTY_MODEL_RESPONSE`。
    #[test]
    fn truncation_diagnostic_flags_reasoning_only_truncation() {
        let (kind, summary) = truncation_diagnostic(Some("max_tokens"), false, Some(16), 16)
            .expect("零可见输出的 length 截断必须被标记");
        assert_eq!(kind, "OutputTruncatedEmpty");
        assert!(summary.contains("max_output_tokens=16"), "{summary}");
        assert!(summary.contains("没有任何可见输出"), "{summary}");

        // 非截断类结束原因 → 不标记（模型正常说完了 / 正常发起了工具调用）
        assert!(truncation_diagnostic(Some("end_turn"), false, None, 0).is_none());
        assert!(truncation_diagnostic(Some("tool_use"), false, None, 0).is_none());
        // 结束原因未知（上游没给 finish_reason）→ 不标记，避免误伤
        assert!(truncation_diagnostic(None, false, Some(4096), 0).is_none());

        // 零输出的安全拦截：同样标记，但原因措辞不同
        let (kind, summary) = truncation_diagnostic(Some("safety"), false, None, 0)
            .expect("零输出的安全拦截也应标记");
        assert_eq!(kind, "OutputTruncatedEmpty");
        assert!(summary.contains("content_filter"), "{summary}");
        assert!(summary.contains("未声明 max_output_tokens"), "{summary}");
    }

    /// P2：**被预算掐短**（有可见输出、且确实撞上上限）单独一档；没撞上限不标。
    ///
    /// 真机形状（2026-09-22 10:44，基元律动/deepseek-flash，280k 上下文）：客户端声明
    /// `max_output_tokens=8192`（PI-Desktop 的模型配置值），上游 `finish_reason:"length"`，
    /// `completion_tokens` 恰好 8192 —— 预算被真正用尽，用户拿到的是被掐短的答案。
    #[test]
    fn truncation_diagnostic_flags_budget_clipped_with_text() {
        let (kind, summary) = truncation_diagnostic(Some("max_tokens"), true, Some(8192), 8192)
            .expect("有正文且撞上预算上限必须被标记");
        assert_eq!(kind, "OutputBudgetClipped");
        assert!(summary.contains("max_output_tokens=8192"), "{summary}");
        assert!(summary.contains("有可见输出"), "{summary}");

        // 产出略超声明预算（上游计费口径差异）同样算撞上
        assert_eq!(
            truncation_diagnostic(Some("max_tokens"), true, Some(100), 105).map(|(k, _)| k),
            Some("OutputBudgetClipped")
        );

        // **没撞上限**（上游在声明预算之前就自行截断）→ 不标：这一档刻意只认「确实用尽」，
        // 否则客户端故意设小预算的正常场景会被标成异常，窗口边缘每一轮都命中
        assert!(
            truncation_diagnostic(Some("max_tokens"), true, Some(8192), 3000).is_none(),
            "没撞上上限的截断不该被标成「被预算掐短」"
        );
        // 预算未知（客户端未声明）→ 不标：网关不发明预算，也就无从判断「撞上了」
        assert!(truncation_diagnostic(Some("max_tokens"), true, None, 3000).is_none());

        // 有正文的安全拦截不标（拦截没生效到正文上，用户读到了内容）
        assert!(truncation_diagnostic(Some("safety"), true, Some(8192), 8192).is_none());
    }

    #[tokio::test]
    async fn models_list_exposes_supports_multimodal() {
        let db = Db::in_memory().unwrap();
        db.with(|c| {
            let now = store::now_ms();
            store::provider_insert(
                c,
                &store::ProviderRow {
                    id: "p1".into(),
                    name: "prov".into(),
                    base_url: "http://x".into(),
                    family: "openai_compat".into(),
                    enabled: true,
                    priority: 1,
                    weight: 1,
                    extra_headers: None,
                    api_key: None,
                    website: None,
                    last_ok_at: None,
                    last_err_at: None,
                    last_err_msg: None,
                    max_tools: None,
                    reasoning_effort_levels: None,
                    created_at: now,
                    updated_at: now,
                },
            )?;
            // 模态集合三种情形：文本+图像 / 纯文本 / 未知（null）
            store::model_upsert(
                c,
                "p1",
                "vision-model",
                Some(128000),
                8192,
                Some(&[
                    crate::modality::Modality::Text,
                    crate::modality::Modality::Image,
                ]),
                Some(&[crate::modality::Modality::Text]),
            )?;
            store::model_upsert(
                c,
                "p1",
                "text-only",
                Some(128000),
                8192,
                Some(&[crate::modality::Modality::Text]),
                None,
            )?;
            store::model_upsert(c, "p1", "unknown", Some(128000), 8192, None, None)?;
            Ok(())
        })
        .expect("seed 失败");

        let dir =
            std::env::temp_dir().join(format!("jai-models-mm-test-{}", rand::random::<u32>()));
        let log_path = dir.join("main.db");
        std::fs::create_dir_all(&dir).unwrap();
        let (logs, _t) = crate::store::logs::spawn_logger(log_path.to_str().unwrap()).unwrap();
        let ctx = GatewayCtx::new(db.clone(), logs);

        // D9-T6b：handler 现在要一个鉴权结果（它据此查密钥规则）。给一把「没配规则」
        // 的密钥即可 —— 规则为空 ⇒ 不过滤，本用例原本的断言面不变。
        let resp = models_list(State(ctx), authed_test_key()).await;
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        let data = v["data"].as_array().unwrap();
        assert_eq!(data.len(), 3);
        let find = |name: &str| {
            data.iter()
                .find(|m| m["id"] == format!("prov/{name}"))
                .unwrap_or_else(|| panic!("{name} 模型应存在"))
        };
        assert_eq!(find("vision-model")["supportsMultimodal"], true);
        assert_eq!(
            find("vision-model")["inputModalities"],
            serde_json::json!(["text", "image"]),
            "0010：出站追加输入模态集合"
        );
        assert_eq!(
            find("vision-model")["outputModalities"],
            serde_json::json!(["text"])
        );
        assert_eq!(find("text-only")["supportsMultimodal"], false);
        assert_eq!(
            find("text-only")["inputModalities"],
            serde_json::json!(["text"])
        );
        assert_eq!(find("text-only")["outputModalities"], Value::Null);
        assert_eq!(
            find("unknown")["supportsMultimodal"],
            Value::Null,
            "未标注应输出 null（未知），不做臆断"
        );
        assert_eq!(find("unknown")["inputModalities"], Value::Null);
    }

    #[test]
    fn model_modalities_upsert_coalesce_and_manual_override() {
        // 0010 语义：NULL 入站不覆盖已有集合（COALESCE）；手动标注可清除回未知，
        // 且清除时旧 supports_multimodal 列一并置 NULL（不出现幽灵 true）。
        use crate::modality::Modality;
        let db = Db::in_memory().unwrap();
        db.with(|c| {
            let now = store::now_ms();
            store::provider_insert(
                c,
                &store::ProviderRow {
                    id: "p1".into(),
                    name: "prov".into(),
                    base_url: "http://x".into(),
                    family: "openai_compat".into(),
                    enabled: true,
                    priority: 1,
                    weight: 1,
                    extra_headers: None,
                    api_key: None,
                    website: None,
                    last_ok_at: None,
                    last_err_at: None,
                    last_err_msg: None,
                    max_tools: None,
                    reasoning_effort_levels: None,
                    created_at: now,
                    updated_at: now,
                },
            )?;
            let vision = [Modality::Text, Modality::Image];
            store::model_upsert(c, "p1", "m1", Some(128000), 8192, Some(&vision), None)?;
            // 未知（None）再次 upsert 不覆盖已有集合
            store::model_upsert(c, "p1", "m1", Some(128000), 8192, None, None)?;
            let row = store::model_get_by_provider_name(c, "p1", "m1")
                .unwrap()
                .expect("模型应存在");
            assert_eq!(row.input_modalities.as_deref(), Some(&vision[..]));
            assert_eq!(row.supports_multimodal, Some(true));

            // 手动覆盖：文本+图像 → 纯文本 → 未知
            let text_only = [Modality::Text];
            store::model_set_modalities(c, &row.id, Some(&text_only), None)?;
            let row = store::model_get_by_provider_name(c, "p1", "m1")
                .unwrap()
                .unwrap();
            assert_eq!(row.supports_multimodal, Some(false));
            store::model_set_modalities(c, &row.id, None, None)?;
            let row = store::model_get_by_provider_name(c, "p1", "m1")
                .unwrap()
                .unwrap();
            assert_eq!(row.supports_multimodal, None, "None = 回到未知");
            assert_eq!(row.input_modalities, None);
            Ok(())
        })
        .expect("model_modalities_upsert_coalesce_and_manual_override 失败");
    }
}
