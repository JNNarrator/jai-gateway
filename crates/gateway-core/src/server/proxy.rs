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
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::codec::anthropic as anthropic_codec;
use crate::codec::openai::{error_body, extract_usage, peek, url_join, PeekRequest, UsageScanner};
use crate::router::{self, AttemptVerdict};
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
    /// 鉴权失败限速（roadmap M2）
    pub rate: Arc<super::ratelimit::AuthRateLimiter>,
    pub version: String,
    pub started_at_ms: u64,
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
            rate: Arc::new(super::ratelimit::AuthRateLimiter::new()),
            version: env!("CARGO_PKG_VERSION").to_string(),
            started_at_ms: store::now_ms() as u64,
        }
    }
}

// ---------------------------------------------------------------- 中间件

/// 安全中间件：Host/Origin 校验 + 鉴权限速判定 + 强制鉴权。/healthz 豁免。
pub async fn security_mw(State(ctx): State<GatewayCtx>, req: Request, next: Next) -> Response {
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
        Ok(_key) => next.run(req).await,
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
        stop_reason: None,
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

/// GET /v1/models 查询行：(模型名, 供应商名, 上下文窗口, 旧 vision 列, 输入模态串, 输出模态串)。
type ModelListRow = (
    String,
    String,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
);

/// GET /v1/models —— 数据库内启用模型的去重聚合输出。
pub async fn models_list(State(ctx): State<GatewayCtx>) -> Response {
    let list = {
        let db = ctx.db.clone();
        tokio::task::spawn_blocking(move || {
            db.with(|c| -> Result<Vec<ModelListRow>, store::StoreError> {
                let mut stmt = c.prepare(
                    "SELECT m.model_name, p.name, m.context_window, m.supports_multimodal, \
                            m.input_modalities, m.output_modalities \
                      FROM models m \
                      JOIN providers p ON p.id=m.provider_id \
                      WHERE m.enabled=1 AND p.enabled=1 \
                      ORDER BY p.priority ASC, m.rowid ASC",
                )?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, Option<i64>>(2)?,
                            r.get::<_, Option<i64>>(3)?,
                            r.get::<_, Option<String>>(4)?,
                            r.get::<_, Option<String>>(5)?,
                        ))
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
                .filter(|(id, owner, _ctx, _l, _i, _o)| seen.insert(format!("{owner}/{id}")))
                .map(|(id, owner, ctx, legacy, input_raw, output_raw)| {
                    // context_window 为 NULL 时给保守默认 128k（与 schema 注释/UI 编辑页一致），
                    // 供客户端模型目录解析模型上下文窗口、计算 ctx 占用百分比。
                    let context_window = ctx.unwrap_or(128_000);
                    let input = crate::modality::parse_opt(input_raw.as_deref());
                    let output = crate::modality::parse_opt(output_raw.as_deref());
                    // supportsMultimodal 自 0010 起降为**派生视图**：集合优先、缺失回落旧列。
                    let supports_multimodal = crate::modality::derive_supports_multimodal(
                        input.as_deref(),
                        legacy.map(|v| v != 0),
                    );
                    // 仅追加字段，旧客户端不受影响：contextWindow / supportsMultimodal 语义未变。
                    json!({
                        "id": format!("{owner}/{id}"),
                        "object": "model",
                        "owned_by": owner,
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
    /// 本次渠道失败，且允许转移到下一渠道。
    /// 携带最后一个 HTTP 错误（若为网络级失败则无），供全渠道失败时原样回传。
    Failed {
        kind: &'static str,
        summary: String,
        /// 最后一个带 HTTP 状态的上游错误（status + body + content-type）
        last_http: Option<UpstreamError>,
    },
}

/// 可直接回传的上游错误响应（保留原始状态码与方言形状）。
struct UpstreamError {
    status: StatusCode,
    content_type: Option<HeaderValue>,
    body: Bytes,
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
    if let Some(provider_name) = &provider_filter {
        candidates.retain(|c| c.provider_name == *provider_name);
    }
    // 高级路由：健康感知 + 同优先级权重负载均衡
    candidates = router::order_candidates(candidates, store::now_ms());
    // Responses 入站优先走同协议族直通（openai_responses），
    // 避免同名模型被 openai_compat 转换渠道按优先级截胡导致 400。
    candidates.sort_by_key(|c| c.family != wire.family());

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
        );
        return wire.error_response(
            StatusCode::NOT_FOUND,
            &msg,
            "invalid_request_error",
            Some("model_not_found"),
        );
    }

    // ---- 逐渠道尝试（按 priority, rowid 序）----
    // 故障转移：每个渠道失败时按 router 分类决定切换或停止；
    // 全部失败返回最后一个错误（roadmap M2：单轮遍历一遍即止，返回最后一个错误）。
    let mut last_kind: &'static str = "ProviderOther";
    let mut last_summary = String::from("所有渠道均失败");
    let mut last_http: Option<UpstreamError> = None;
    for cand in &candidates {
        // 旧版 /v1/completions 只支持 OpenAI 兼容直通，不做跨族转换。
        if wire == InboundWire::Completions && cand.family != wire.family() {
            continue;
        }
        if cand.family != wire.family() {
            match try_converted_candidate(&ctx, wire, &peeked, cand, &body, started).await {
                Attempt::Delivered(resp) => return resp,
                Attempt::Failed {
                    kind,
                    summary,
                    last_http: eh,
                } => {
                    last_kind = kind;
                    last_summary = summary;
                    if let Some(e) = eh {
                        last_http = Some(e);
                    }
                }
            }
            continue;
        }

        match try_candidate(&ctx, wire, &peeked, cand, &body, &inbound_headers, started).await {
            Attempt::Delivered(resp) => return resp,
            Attempt::Failed {
                kind,
                summary,
                last_http: eh,
            } => {
                last_kind = kind;
                last_summary = summary;
                if let Some(e) = eh {
                    last_http = Some(e);
                }
            }
        }
    }

    // 全部渠道失败。
    // - 若存在带 HTTP 状态的上游错误：原样回传（保留状态码与协议方言形状，
    //   如 Anthropic 线 429/529）—— roadmap M2「返回最后一个错误」
    // - 否则（网络级失败）：统一 502
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
        );
        let mut b = Response::builder().status(err.status);
        b = match err.content_type {
            Some(ct) => b.header(header::CONTENT_TYPE, ct),
            None => b.header(header::CONTENT_TYPE, "application/json"),
        };
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
    );
    wire.error_response(
        StatusCode::BAD_GATEWAY,
        &last_summary,
        "api_error",
        Some("all_providers_failed"),
    )
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
            );
            return Attempt::Failed {
                kind: "UpstreamAuth",
                summary: format!("{}：{msg}", cand.provider_name),
                last_http: None,
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
    let body = match (
        crate::codec::Family::from_db_str(&cand.family),
        channel_policy_of(cand),
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
                        );
                    }
                }
            }
            match policy.effort.as_ref() {
                Some(effort) => {
                    match crate::effort::normalize_body(&body, family, &effort.levels) {
                        Some(next) => Bytes::from(next),
                        None => body,
                    }
                }
                None => body,
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
            );
            return Attempt::Failed {
                kind: "ProviderOther",
                summary: format!("{}：{msg}", cand.provider_name),
                last_http: None,
            };
        }
    };

    let up_status =
        StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    if !up_status.is_success() {
        // 先取头再消费 body（bytes() 会拿走 Response 的所有权）
        let upstream_ct = resp.headers().get(header::CONTENT_TYPE).cloned();
        let mut bytes = resp.bytes().await.unwrap_or_default();
        if bytes.len() > MAX_ERROR_BODY {
            bytes.truncate(MAX_ERROR_BODY);
        }
        let hint = String::from_utf8_lossy(&bytes).into_owned();
        let snippet: String = hint.chars().take(240).collect();

        // 错误分类：可转移 → 下一渠道；确定性错误 → 原样返回
        match router::classify_status(up_status.as_u16(), &snippet) {
            AttemptVerdict::Stop { kind } => {
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
                );
                let mut b = Response::builder().status(up_status);
                b = match upstream_ct {
                    Some(ct) => b.header(header::CONTENT_TYPE, ct),
                    None => b.header(header::CONTENT_TYPE, "application/json"),
                };
                return Attempt::Delivered(
                    b.body(Body::from(bytes))
                        .unwrap_or_else(|_| empty_resp(up_status)),
                );
            }
            AttemptVerdict::Failover { kind } => {
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
                );
                return Attempt::Failed {
                    kind,
                    summary: format!(
                        "{}：上游 {kind}（HTTP {}）",
                        cand.provider_name,
                        up_status.as_u16()
                    ),
                    last_http: Some(UpstreamError {
                        status: up_status,
                        content_type: upstream_ct,
                        body: bytes,
                    }),
                };
            }
            AttemptVerdict::Success => unreachable!("非 2xx 不会判 Success"),
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

    let delivered = if ct_is_sse || peeked.stream {
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
        plain_response(
            ctx.clone(),
            wire,
            peeked.clone(),
            cand.clone(),
            resp,
            up_status,
            started,
        )
        .await
    };
    Attempt::Delivered(delivered)
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
        let policy = channel_policy_of(cand);
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
                kind: "ProviderOther",
                summary: format!("未知上游协议族: {other}"),
                last_http: None,
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
                kind: "UpstreamAuth",
                summary: format!("{}：{msg}", cand.provider_name),
                last_http: None,
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
            );
            return Attempt::Failed {
                kind: "ProviderOther",
                summary: format!("{}：{msg}", cand.provider_name),
                last_http: None,
            };
        }
    };

    let up_status =
        StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    if !up_status.is_success() {
        let mut bytes = resp.bytes().await.unwrap_or_default();
        if bytes.len() > MAX_ERROR_BODY {
            bytes.truncate(MAX_ERROR_BODY);
        }
        let hint = String::from_utf8_lossy(&bytes).into_owned();
        let snippet: String = hint.chars().take(240).collect();

        match router::classify_status(up_status.as_u16(), &snippet) {
            AttemptVerdict::Stop { kind } => {
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
                );
                // 确定性错误：翻译为入站方言（OpenAI schema）
                let (tname, code) = match kind {
                    "ContextTooLong" => ("invalid_request_error", Some("context_length_exceeded")),
                    _ => ("invalid_request_error", None),
                };
                return Attempt::Delivered(wire.error_response(up_status, &snippet, tname, code));
            }
            AttemptVerdict::Failover { kind } => {
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
                );
                return Attempt::Failed {
                    kind,
                    summary: format!(
                        "{}：上游 {kind}（HTTP {}）",
                        cand.provider_name,
                        up_status.as_u16()
                    ),
                    last_http: None,
                };
            }
            AttemptVerdict::Success => unreachable!("非 2xx 不会判 Success"),
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

    if ct_is_sse || req.stream {
        convert_streaming_response(
            ctx.clone(),
            wire,
            peeked.clone(),
            cand.clone(),
            resp,
            up_status,
            started,
            crate::codec::capability::tool_identities_of(&req),
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
            );
            return Attempt::Delivered(wire.error_response(
                StatusCode::BAD_GATEWAY,
                msg,
                "api_error",
                Some("structured_output_validation_failed"),
            ));
        }
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
    );

    // 渲染为入站形状（OpenAI / Anthropic）
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
) -> Attempt {
    let mut upstream_stream = resp.bytes_stream();

    // 首字节持票
    let first = tokio::time::timeout(UPSTREAM_FIRST_BYTE_TIMEOUT, upstream_stream.next()).await;
    let first_chunk = match first {
        Ok(Some(Ok(b))) => b,
        Ok(Some(Err(e))) => {
            let msg = format!("上游流建立失败: {e}");
            return Attempt::Delivered(wire.error_response(
                StatusCode::BAD_GATEWAY,
                &msg,
                "api_error",
                Some("upstream_stream_error"),
            ));
        }
        Ok(None) => {
            return Attempt::Delivered(wire.error_response(
                StatusCode::BAD_GATEWAY,
                "上游流立即关闭",
                "api_error",
                Some("upstream_empty_stream"),
            ));
        }
        Err(_) => {
            return Attempt::Delivered(wire.error_response(
                wire.overloaded_status(),
                "上游首字节超时(60s)",
                "api_error",
                Some("upstream_first_byte_timeout"),
            ));
        }
    };

    // 按入站线选择渲染器
    enum SseRenderer {
        OpenAi(crate::codec::openai::RenderState),
        Anthropic(crate::codec::anthropic::AnthropicRenderState),
        Responses(crate::codec::responses::RenderState),
    }
    let mut renderer = match wire {
        InboundWire::OpenAi | InboundWire::Completions => {
            SseRenderer::OpenAi(crate::codec::openai::RenderState {
                id: format!("chatcmpl-jai-{}", started.elapsed().as_millis()),
                model: peeked.model.clone(),
                started: false,
            })
        }
        InboundWire::Anthropic => {
            SseRenderer::Anthropic(crate::codec::anthropic::AnthropicRenderState {
                message_id: format!("msg_jai_{}", started.elapsed().as_millis()),
                model: peeked.model.clone(),
                active_block: None,
                text_started: false,
                active_tool_index: None,
                next_block_index: 0,
                finished: false,
            })
        }
        InboundWire::Responses => SseRenderer::Responses(crate::codec::responses::RenderState {
            response_id: format!("resp_jai_{}", started.elapsed().as_millis()),
            model: peeked.model.clone(),
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
        }),
    };
    // 渲染单个 IR 事件 → SSE 输出帧（一个 IR 事件可能展开多个 SSE 事件）
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
            // 用 id 去重而非数事件：部分上游（如 Gemini）每帧都带 Start，且 index 恒为 0。
            let mut tool_call_ids: std::collections::HashSet<String> =
                std::collections::HashSet::new();
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
                                        );
                                        continue;
                                    }
                                },
                                "openai_responses" => {
                                    // Responses 上游的 SSE 尚未支持流式转换；
                                    // 若上游返回 SSE，暂时按无法解析处理。
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
                                        Some("[convert] openai_responses SSE 暂不支持流式转换".into()),
                                    );
                                    continue;
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
                            None,
                            None,
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
) -> Response {
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
            );
            return wire.error_response(
                StatusCode::BAD_GATEWAY,
                &msg,
                "api_error",
                Some("upstream_stream_error"),
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
            );
            return wire.error_response(
                StatusCode::BAD_GATEWAY,
                msg,
                "api_error",
                Some("upstream_empty_stream"),
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
            );
            return wire.error_response(
                wire.overloaded_status(),
                "上游首字节超时(60s)",
                "api_error",
                Some("upstream_first_byte_timeout"),
            );
        }
    };

    // 通过管道把剩余流喂给客户端；usage 扫描伴随进行。
    // 注意：直通流式的 `tool_calls` 落库为 0 —— 字节直通不解析 SSE 语义，
    // 仅 UsageScanner 做关键字级扫描；诊断「模型有没有发起工具调用」请看
    // 转换路径（跨族）的行，或本行同时看 is_stream=1 + route_mode=passthrough。
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(64);

    let mut scanner = UsageScanner::new();
    scanner.feed(&first_chunk);

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
            0,
            Some("InvalidRequest".into()),
            Some("client disconnected early".into()),
        );
        return empty_resp(StatusCode::OK);
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
                            0,
                            Some("Overloaded".into()),
                            Some(format!(
                                "Overloaded upstream={} idle timeout",
                                status.as_u16()
                            )),
                        );
                        break;
                    }
                    Ok(Some(Ok(chunk))) => {
                        scanner.feed(&chunk);
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
                                0,
                                Some("InvalidRequest".into()),
                                Some("client disconnected mid-stream".into()),
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
                            0,
                            Some("ProviderOther".into()),
                            Some(msg),
                        );
                        break;
                    }
                    Ok(None) => {
                        // 正常结束
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
                            0,
                            None,
                            None,
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
        .unwrap_or_else(|_| empty_resp(status))
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let resp = models_list(State(ctx)).await;
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

        let resp = models_list(State(ctx)).await;
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
