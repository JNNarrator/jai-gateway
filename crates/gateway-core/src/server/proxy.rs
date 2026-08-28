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
use crate::vault;

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

// ---------------------------------------------------------------- 入站线（wire）

/// 入站协议线。差异（路径/鉴权头/错误形状/日志族）全部收敛在此。
/// M1 OpenAI 直通、M3 Anthropic 直通、M6 Responses 入站共用路由/故障转移骨架。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundWire {
    OpenAi,
    Anthropic,
    Responses,
}

impl InboundWire {
    /// 要求的渠道协议族（providers.family 值）。Responses 线对应新增的
    /// `openai_responses` 上游族；若渠道是 openai_compat 则走跨族转换。
    pub fn family(&self) -> &'static str {
        match self {
            InboundWire::OpenAi => "openai_compat",
            InboundWire::Anthropic => "anthropic",
            InboundWire::Responses => "openai_responses",
        }
    }

    /// 日志 inbound_family 字段值
    pub fn log_family(&self) -> &'static str {
        match self {
            InboundWire::OpenAi => "openai",
            InboundWire::Anthropic => "anthropic",
            InboundWire::Responses => "responses",
        }
    }

    /// 上游路径（base_url 拼接；Responses 线始终走转换，不直接使用）
    pub fn upstream_path(&self) -> &'static str {
        match self {
            InboundWire::OpenAi => "/chat/completions",
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
            InboundWire::OpenAi | InboundWire::Responses => req.bearer_auth(secret),
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
            InboundWire::OpenAi => {
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
            InboundWire::OpenAi | InboundWire::Responses => StatusCode::SERVICE_UNAVAILABLE,
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
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            // 流式可能长达数分钟：不设总超时，交给分块空闲判定
            .build()
            .expect("reqwest client 构建失败");
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
    error_kind: Option<String>,
    error_summary: Option<String>,
) {
    let (ui, uo, ucr, ucw) = usage.map(extract_usage).unwrap_or((None, None, None, None));
    logs.emit(LogEvent {
        ts: store::now_ms(),
        inbound_family: inbound_family.into(),
        route_mode: "passthrough",
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
        tool_calls: 0,
        error_kind,
        error_summary,
    });
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
        InboundWire::OpenAi | InboundWire::Responses => {
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

/// GET /v1/models —— 数据库内启用模型的去重聚合输出。
pub async fn models_list(State(ctx): State<GatewayCtx>) -> Response {
    let list = {
        let db = ctx.db.clone();
        tokio::task::spawn_blocking(move || {
            db.with(|c| -> Result<Vec<(String, String)>, store::StoreError> {
                let mut stmt = c.prepare(
                    "SELECT m.model_name, p.name FROM models m \
                     JOIN providers p ON p.id=m.provider_id \
                     WHERE m.enabled=1 AND p.enabled=1 \
                     ORDER BY p.priority ASC, m.rowid ASC",
                )?;
                let rows = stmt
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
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
                .filter(|(id, owner)| seen.insert(format!("{owner}/{id}")))
                .map(|(id, owner)| {
                    json!({"id": format!("{owner}/{id}"), "object": "model", "owned_by": owner})
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
        if cand.family != wire.family() {
            // 跨族转换（M4）：入站 OpenAI → 上游 Anthropic / Gemini
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
                if name_lower.starts_with("x-stainless-")
                    || name_lower.starts_with("x-request-id")
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
    let secret = {
        let ref_ = cand.keyring_ref.clone();
        match tokio::task::spawn_blocking(move || vault::get_secret(&ref_)).await {
            Ok(Ok(Some(k))) => k,
            Ok(Ok(None)) => {
                let msg = "密钥环中缺少该供应商凭据，请在设置中重新录入";
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
                    Some("UpstreamAuth".into()),
                    Some(msg.to_string()),
                );
                return Attempt::Failed {
                    kind: "UpstreamAuth",
                    summary: format!("{}：{msg}", cand.provider_name),
                    last_http: None,
                };
            }
            Ok(Err(e)) => {
                let msg = format!("keyring 读取失败: {e}");
                provider_mark_fail(&ctx.db, &cand.provider_id, &msg);
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
                    Some("ProviderOther".into()),
                    Some(msg.clone()),
                );
                return Attempt::Failed {
                    kind: "ProviderOther",
                    summary: format!("{}：{msg}", cand.provider_name),
                    last_http: None,
                };
            }
            Err(e) => {
                eprintln!("[vault] join: {e}");
                return Attempt::Failed {
                    kind: "ProviderOther",
                    summary: format!("{}：密钥环任务失败", cand.provider_name),
                    last_http: None,
                };
            }
        }
    };

    // ---- 组装上游请求（body 原样字节）----
    let url = url_join(&cand.base_url, wire.upstream_path());
    let mut out_req = wire.apply_auth(ctx.http.post(&url), &secret);
    if let Some(eh_raw) = cand.extra_headers.as_deref() {
        match serde_json::from_str::<serde_json::Map<String, Value>>(eh_raw) {
            Ok(map) => {
                for (k, v) in map {
                    if let Some(s) = v.as_str() {
                        if let (Ok(name), Ok(val)) = (
                            HeaderName::from_bytes(k.as_bytes()),
                            HeaderValue::from_str(s),
                        ) {
                            out_req = out_req.header(name, val);
                        }
                    }
                }
            }
            Err(e) => eprintln!("[proxy] extra_headers 解析失败(忽略): {e}"),
        }
    }
    out_req = forward_inbound_headers(out_req, inbound_headers);

    let upstream = out_req.body(body.clone()).send().await;
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
    };
    // M5：Anthropic 入站历史 tool id 反解（含 tool_id_map 超长回落）
    if wire == InboundWire::Anthropic {
        resolve_anthropic_inbound_tool_ids(&ctx.db, &mut req);
    }

    // Skill 注入：把启用的技能内容追加到 system（仅跨族转换路径，避免破坏直通字节）
    let _ = ctx.db.with_any(|c| {
        crate::skills::apply_enabled_skills(c, &mut req);
        Ok::<_, String>(())
    });

    // 2) 护栏（M4：blocks ≤ 64 / args ≤ 256KB）
    if let Err(msg) = crate::codec::ir::validate_guards(&req) {
        return Attempt::Delivered(wire.error_response(
            StatusCode::BAD_REQUEST,
            &msg,
            "invalid_request_error",
            None,
        ));
    }

    // 2.5) 跨族不支持的能力面：response_format(json_schema) → 400（仅 OpenAI 入站有该字段）
    if wire == InboundWire::OpenAi && req.extensions.contains_key("response_format") {
        let msg =
            "response_format（json_schema）在跨协议转换中不支持；支持子集：无。请改用提示词约束输出";
        return Attempt::Delivered(wire.error_response(
            StatusCode::BAD_REQUEST,
            msg,
            "invalid_request_error",
            Some("response_format_not_supported"),
        ));
    }

    // 2.6) 未建模字段：Lenient 丢弃 + 单条 WARN 汇总（§7）
    if let Some(note) = crate::codec::ir::extension_warn_note(&req) {
        eprintln!("[convert] {note}");
    }

    // 3) 按上游协议族编码
    let (url, auth_builder) = match cand.family.as_str() {
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
            let out = ctx.http.post(&url).header("x-api-key", "");
            (url, (out, body))
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
            let out = ctx.http.post(&url).header("x-goog-api-key", "");
            (url, (out, body))
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
            let out = ctx.http.post(&url).bearer_auth("");
            (url, (out, body))
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
    let (out, body_json) = auth_builder;
    let secret = {
        let ref_ = cand.keyring_ref.clone();
        match tokio::task::spawn_blocking(move || vault::get_secret(&ref_)).await {
            Ok(Ok(Some(k))) => k,
            Ok(Ok(None)) => {
                let msg = "密钥环中缺少该供应商凭据，请在设置中重新录入";
                provider_mark_fail(&ctx.db, &cand.provider_id, msg);
                return Attempt::Failed {
                    kind: "UpstreamAuth",
                    summary: format!("{}：{msg}", cand.provider_name),
                    last_http: None,
                };
            }
            Ok(Err(e)) => {
                let msg = format!("keyring 读取失败: {e}");
                provider_mark_fail(&ctx.db, &cand.provider_id, &msg);
                return Attempt::Failed {
                    kind: "ProviderOther",
                    summary: format!("{}：{msg}", cand.provider_name),
                    last_http: None,
                };
            }
            Err(e) => {
                eprintln!("[vault] join: {e}");
                return Attempt::Failed {
                    kind: "ProviderOther",
                    summary: format!("{}：密钥环任务失败", cand.provider_name),
                    last_http: None,
                };
            }
        }
    };
    let mut out_req = match cand.family.as_str() {
        "anthropic" => out.header("x-api-key", &secret),
        "gemini" => out.header("x-goog-api-key", &secret),
        "openai_compat" => out.bearer_auth(&secret),
        _ => out,
    };
    if let Some(eh_raw) = cand.extra_headers.as_deref() {
        if let Ok(map) = serde_json::from_str::<serde_json::Map<String, Value>>(eh_raw) {
            for (k, v) in map {
                if let Some(s) = v.as_str() {
                    if let (Ok(name), Ok(val)) = (
                        HeaderName::from_bytes(k.as_bytes()),
                        HeaderValue::from_str(s),
                    ) {
                        out_req = out_req.header(name, val);
                    }
                }
            }
        }
    }
    drop(url);

    let resp = match out_req
        .json(&body_json)
        .timeout(Duration::from_secs(300))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
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
                req.stream,
                None,
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
                emit_log(
                    &ctx.logs,
                    wire.log_family(),
                    Some(peeked),
                    Some(&cand.provider_id),
                    cand.upstream_model_id.clone(),
                    up_status.as_u16() as i64,
                    ms_since(started),
                    req.stream,
                    None,
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
                emit_log(
                    &ctx.logs,
                    wire.log_family(),
                    Some(peeked),
                    Some(&cand.provider_id),
                    cand.upstream_model_id.clone(),
                    up_status.as_u16() as i64,
                    ms_since(started),
                    req.stream,
                    None,
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
        )
        .await
    } else {
        convert_plain_response(
            ctx.clone(),
            wire,
            peeked.clone(),
            cand.clone(),
            resp,
            up_status,
            started,
        )
        .await
    }
}

/// 转换路径：非流式解析 + 渲染为入站形状。
async fn convert_plain_response(
    ctx: GatewayCtx,
    wire: InboundWire,
    peeked: PeekRequest,
    cand: store::RouteCandidate,
    resp: reqwest::Response,
    status: StatusCode,
    started: Instant,
) -> Attempt {
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
        other => Err(format!("未知上游协议族: {other}")),
    };
    let mut resp = match parsed {
        Ok(r) => r,
        Err(msg) => {
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

    let usage = &resp.usage;
    let uval = json!({
        "prompt_tokens": usage.input_tokens,
        "completion_tokens": usage.output_tokens,
    });
    emit_log(
        &ctx.logs,
        wire.log_family(),
        Some(&peeked),
        Some(&cand.provider_id),
        cand.upstream_model_id.clone(),
        status.as_u16() as i64,
        ms_since(started),
        false,
        Some(&uval),
        None,
        None,
    );

    // 渲染为入站形状（OpenAI / Anthropic）
    let rendered = match wire {
        InboundWire::OpenAi => crate::codec::openai::render_response(&resp).to_string(),
        InboundWire::Anthropic => crate::codec::anthropic::render_response(&resp).to_string(),
        InboundWire::Responses => crate::codec::responses::render_response(&resp).to_string(),
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(rendered))
        .map(Attempt::Delivered)
        .unwrap_or_else(|_| Attempt::Delivered(empty_resp(status)))
}

/// 转换路径：流式。逐 SSE 事件 parse → IR StreamEvent → render 回入站 SSE。
async fn convert_streaming_response(
    ctx: GatewayCtx,
    wire: InboundWire,
    peeked: PeekRequest,
    cand: store::RouteCandidate,
    resp: reqwest::Response,
    status: StatusCode,
    started: Instant,
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
        InboundWire::OpenAi => SseRenderer::OpenAi(crate::codec::openai::RenderState {
            id: format!("chatcmpl-jai-{}", started.elapsed().as_millis()),
            model: peeked.model.clone(),
            started: false,
        }),
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
            item_started: false,
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
                .map(|payload| format!("data: {payload}\n\n"))
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

            // 解析并发送当前行缓冲（首字节可能已包含整个 SSE 流）
            macro_rules! drain_sse_lines {
                () => {{
                    while let Some(pos) = line_buf.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = line_buf.drain(..=pos).collect();
                        let line = line.strip_suffix(b"\r").unwrap_or(&line);
                        let line: &[u8] = line;
                        if !line.starts_with(b"data:") {
                            continue;
                        }
                        let payload: &[u8] = trim_ascii(&line[5..]);
                        if payload.is_empty() || payload == b"[DONE]" {
                            continue;
                        }
                        let mut events: Vec<crate::codec::ir::StreamEvent> =
                            match upstream_family.as_str() {
                                "anthropic" => {
                                    match crate::codec::anthropic::parse_stream_event(payload) {
                                        Ok(v) => v,
                                        Err(e) => {
                                            eprintln!("[convert] anthropic SSE: {e}");
                                            continue;
                                        }
                                    }
                                }
                                "gemini" => match crate::codec::gemini::parse_stream_event(payload) {
                                    Ok(v) => v,
                                    Err(e) => {
                                        eprintln!("[convert] gemini SSE: {e}");
                                        continue;
                                    }
                                },
                                "openai_compat" => match crate::codec::openai::parse_stream_event(payload)
                                {
                                    Ok(v) => v,
                                    Err(e) => {
                                        eprintln!("[convert] openai SSE: {e}");
                                        continue;
                                    }
                                },
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
                            for frame in render_frame(&mut renderer, &ev) {
                                if tx.send(Ok(Bytes::from(frame))).await.is_err() {
                                    emit_log(
                                        &ctx2.logs,
                                        wire2.log_family(),
                                        Some(&peeked2),
                                        Some(&pid),
                                        None,
                                        status.as_u16() as i64,
                                        ms_since(t0),
                                        true,
                                        None,
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
            drain_sse_lines!();

            loop {
                let nxt = tokio::time::timeout(STREAM_IDLE_TIMEOUT, upstream_stream.next());
                match nxt.await {
                    Err(_) => {
                        let msg = format!("上游流空闲超时({}s)", STREAM_IDLE_TIMEOUT.as_secs());
                        let _ = tx.send(Ok(error_sse_frame(wire2, &msg))).await;
                        drop(tx);
                        emit_log(
                            &ctx2.logs,
                            wire2.log_family(),
                            Some(&peeked2),
                            Some(&pid),
                            None,
                            status.as_u16() as i64,
                            ms_since(t0),
                            true,
                            None,
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
                        drain_sse_lines!();
                    }
                    Ok(Some(Err(e))) => {
                        let msg = format!("stream aborted by upstream: {e}");
                        let _ = tx.send(Ok(error_sse_frame(wire2, &msg))).await;
                        provider_mark_fail(&ctx2.db, &pid, &msg);
                        emit_log(
                            &ctx2.logs,
                            wire2.log_family(),
                            Some(&peeked2),
                            Some(&pid),
                            None,
                            status.as_u16() as i64,
                            ms_since(t0),
                            true,
                            None,
                            Some("ProviderOther".into()),
                            Some(format!("[convert] {msg}")),
                        );
                        drop(tx);
                        break;
                    }
                    Ok(None) => {
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
                        emit_log(
                            &ctx2.logs,
                            wire2.log_family(),
                            Some(&peeked2),
                            Some(&pid),
                            None,
                            status.as_u16() as i64,
                            ms_since(t0),
                            true,
                            None,
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

    // 通过管道把剩余流喂给客户端；usage 扫描伴随进行
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
}
