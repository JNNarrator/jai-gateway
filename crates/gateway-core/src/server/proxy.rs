//! OpenAI 兼容代理（M1 直通 + M2 多渠道故障转移）。
//!
//! 约束（roadmap M1/M2）：
//! - 同族 `openai_compat` 字节级透传（验收：上游收到的 body SHA-256 == 客户端发出值）
//! - 多渠道：按 `priority, rowid` 序逐渠道尝试；{连接拒绝、超时、UpstreamAuth、
//!   RateLimit、Overloaded、上游 5xx} → 下一渠道；InvalidRequest / ContextTooLong
//!   → 即刻返回不切换；**首个字节下发下游后禁止切换**
//! - SSE 全程管道转发 + usage 旁路扫描落日志（绝不反压客户端）
//! - 超时三件套：上游连接 10s（client 构造）、首字节 60s、流空闲 120s

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::codec::openai::{
    PeekRequest, UsageScanner, error_body, extract_usage, peek, url_join,
};
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
pub async fn security_mw(
    State(ctx): State<GatewayCtx>,
    req: Request,
    next: Next,
) -> Response {
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
    let (ui, uo, ucr, ucw) = usage
        .map(extract_usage)
        .unwrap_or((None, None, None, None));
    logs.emit(LogEvent {
        ts: store::now_ms(),
        inbound_family: "openai".into(),
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

fn oai_error_response(
    status: StatusCode,
    message: &str,
    err_type: &str,
    code: Option<&str>,
) -> Response {
    (status, Json(error_body(message, err_type, code))).into_response()
}

fn empty_resp(status: StatusCode) -> Response {
    (status, Json(json!({}))).into_response()
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
                .filter(|(id, _)| seen.insert(id.clone()))
                .map(|(id, owner)| {
                    json!({"id": id, "object": "model", "owned_by": owner})
                })
                .collect::<Vec<_>>()
        }
        Ok(Err(e)) => {
            eprintln!("[models] db: {e}");
            return oai_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "查询模型列表失败",
                "api_error",
                None,
            );
        }
        Err(e) => {
            eprintln!("[models] join: {e}");
            return oai_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error",
                "api_error",
                None,
            );
        }
    };

    Json(json!({ "object": "list", "data": data })).into_response()
}

/// 单渠道尝试的返回：已交付 / 可转移失败
enum Attempt {
    /// 已经向客户端交付（最终响应或确定性错误）
    Delivered(Response),
    /// 本次渠道失败，且允许转移到下一渠道
    Failed {
        kind: &'static str,
        summary: String,
    },
}

/// POST /v1/chat/completions —— M1 直通 + M2 多渠道故障转移主入口。
pub async fn chat_completions(State(ctx): State<GatewayCtx>, req: Request) -> Response {
    let started = Instant::now();

    let body = match to_bytes(req.into_body(), MAX_REQUEST_BODY).await {
        Ok(b) => b,
        Err(_) => {
            return oai_error_response(
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
            return oai_error_response(StatusCode::BAD_REQUEST, &msg, "invalid_request_error", None);
        }
    };

    // ---- 路由候选 ----
    let model = peeked.model.clone();
    let candidates = {
        let db = ctx.db.clone();
        let model2 = model.clone();
        match tokio::task::spawn_blocking(move || db.with(|c| store::route_candidates(c, &model2)))
            .await
        {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                eprintln!("[route] db: {e}");
                return internal_(&ctx, &peeked, &started);
            }
            Err(e) => {
                eprintln!("[route] join: {e}");
                return internal_(&ctx, &peeked, &started);
            }
        }
    };

    if candidates.is_empty() {
        let msg = format!("模型 {model:?} 不存在或其渠道未启用");
        emit_log(
            &ctx.logs,
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
        return oai_error_response(
            StatusCode::NOT_FOUND,
            &msg,
            "invalid_request_error",
            Some("model_not_found"),
        );
    }

    // ---- 逐渠道尝试（按 priority, rowid 序）----
    // M2 故障转移：每个渠道失败时按 router 分类决定切换或停止；
    // 全部失败返回最后一个错误（roadmap M2）。
    let mut last_kind: &'static str = "ProviderOther";
    let mut last_summary = String::from("所有渠道均失败");
    for cand in &candidates {
        if cand.family != "openai_compat" {
            // 跨族转换自 M4 起提供；当前跳过该渠道尝试下一家
            provider_mark_fail(
                &ctx.db,
                &cand.provider_id,
                &format!("协议族 {} 转换未实现（M4 起可用，已跳过）", cand.family),
            );
            last_kind = "ProviderOther";
            last_summary = format!(
                "渠道 {} 协议族为 {}，跨协议转换自 M4 起提供（已跳过）",
                cand.provider_name, cand.family
            );
            continue;
        }

        match try_openai_candidate(&ctx, &peeked, cand, &body, started).await {
            Attempt::Delivered(resp) => return resp,
            Attempt::Failed { kind, summary } => {
                last_kind = kind;
                last_summary = summary;
            }
        }
    }

    // 全部渠道失败：返回最后一个错误（OpenAI schema）
    emit_log(
        &ctx.logs,
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
    oai_error_response(
        StatusCode::BAD_GATEWAY,
        &last_summary,
        "api_error",
        Some("all_providers_failed"),
    )
}

/// 尝试单渠道（openai_compat 直通）。失败已按 router 分类，只返回「可转移」失败。
async fn try_openai_candidate(
    ctx: &GatewayCtx,
    peeked: &PeekRequest,
    cand: &store::RouteCandidate,
    body: &Bytes,
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
                };
            }
            Ok(Err(e)) => {
                let msg = format!("keyring 读取失败: {e}");
                provider_mark_fail(&ctx.db, &cand.provider_id, &msg);
                emit_log(
                    &ctx.logs,
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
                };
            }
            Err(e) => {
                eprintln!("[vault] join: {e}");
                return Attempt::Failed {
                    kind: "ProviderOther",
                    summary: format!("{}：密钥环任务失败", cand.provider_name),
                };
            }
        }
    };

    // ---- 组装上游请求（body 原样字节）----
    let url = url_join(&cand.base_url, "/chat/completions");
    let mut out_req = ctx.http.post(&url).bearer_auth(&secret);
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

    let upstream = out_req.body(body.clone()).send().await;
    let resp = match upstream {
        Ok(r) => r,
        Err(e) => {
            // 连接拒绝 / 连接超时（10s connect timeout 在此兑现）→ 故障转移
            let msg = format!("上游连接失败: {e}");
            provider_mark_fail(&ctx.db, &cand.provider_id, &msg);
            emit_log(
                &ctx.logs,
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
            };
        }
    };

    let up_status = StatusCode::from_u16(resp.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

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
                    b.body(Body::from(bytes)).unwrap_or_else(|_| empty_resp(up_status)),
                );
            }
            AttemptVerdict::Failover { kind } => {
                provider_mark_fail(&ctx.db, &cand.provider_id, &snippet);
                emit_log(
                    &ctx.logs,
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
        streaming_response(ctx.clone(), peeked.clone(), cand.clone(), resp, up_status, started)
            .await
    } else {
        plain_response(ctx.clone(), peeked.clone(), cand.clone(), resp, up_status, started).await
    };
    Attempt::Delivered(delivered)
}

fn internal_(ctx: &GatewayCtx, p: &PeekRequest, started: &Instant) -> Response {
    emit_log(
        &ctx.logs,
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
    oai_error_response(StatusCode::INTERNAL_SERVER_ERROR, "内部错误", "api_error", None)
}

// ---------------------------------------------------------------- 非流式

async fn plain_response(
    ctx: GatewayCtx,
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
            return oai_error_response(StatusCode::BAD_GATEWAY, &msg, "api_error", None);
        }
        Err(_) => {
            emit_log(
                &ctx.logs,
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
            return oai_error_response(
                StatusCode::GATEWAY_TIMEOUT,
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
            return oai_error_response(
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
            return oai_error_response(
                StatusCode::BAD_GATEWAY,
                msg,
                "api_error",
                Some("upstream_empty_stream"),
            );
        }
        Err(_) => {
            emit_log(
                &ctx.logs,
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
            return oai_error_response(
                StatusCode::GATEWAY_TIMEOUT,
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
            HeaderValue::from_str(&cand.provider_name).unwrap_or(HeaderValue::from_static("unknown")),
        )
        .body(Body::from_stream(client_stream))
        .unwrap_or_else(|_| empty_resp(status))
}