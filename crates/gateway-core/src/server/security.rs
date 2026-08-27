//! 网关入口安全：强制鉴权、Host/Origin 校验（需求「本地安全要求」三条）。
//!
//! - 所有代理端点即使本机访问也强制 sk-jai-* 鉴权
//! - Host 必须为回环地址（防 DNS rebinding —— Ollama/LM Studio 历史 CVE 类问题）
//! - Origin 存在时必须是本机来源；设置页白名单可放行指定浏览器应用

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::codec::openai;
use crate::store::{self, Db};

/// 常量时间比较：先各自 sha256 归一为定长摘要再异或折叠，
/// 与逐字节短路比较不同，不泄漏匹配前缀长度。
pub fn ct_eq(a: &str, b: &str) -> bool {
    let da = Sha256::digest(a.as_bytes());
    let db = Sha256::digest(b.as_bytes());
    let mut diff = 0u8;
    for (x, y) in da.iter().zip(db.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 提取 Host 头的 hostname 部分（剥离端口；容忍 IPv6 括号形式）。
pub fn hostname_of_host_header(h: &str) -> String {
    let h = h.trim();
    if h.starts_with('[') {
        if let Some(end) = h.find(']') {
            return h[1..end].to_ascii_lowercase();
        }
    }
    match h.rsplit_once(':') {
        // 无括号 IPv6 裸地址没有端口（含多个冒号）→ 视为裸地址
        Some((host, port)) if !port.is_empty() && !host.contains(':') => {
            host.to_ascii_lowercase()
        }
        _ => h.to_ascii_lowercase(),
    }
}

pub fn is_loopback_host(name: &str) -> bool {
    matches!(name, "127.0.0.1" | "localhost" | "::1")
}

/// 校验 Host 头。None 表示缺失或非法。
// Err 直接携带现成响应体进中间件，装盒无意义
#[allow(clippy::result_large_err)]
pub fn check_host(headers: &HeaderMap) -> Result<(), Response> {
    let raw = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if raw.is_empty() {
        return Err(deny(
            StatusCode::FORBIDDEN,
            "missing Host header",
            "host_forbidden",
        ));
    }
    let host = hostname_of_host_header(raw);
    if is_loopback_host(&host) {
        Ok(())
    } else {
        Err(deny(
            StatusCode::FORBIDDEN,
            "Host 非本机回环地址（防御 DNS rebinding）",
            "host_forbidden",
        ))
    }
}

/// Origin 白名单检查。meta cors_allow = JSON ["https://a.b", "*"]。
/// 头不存在（非浏览器客户端）→ 放行；列表含 "*" 或精确命中 → 放行；
/// 默认拒绝远程 http(s) 来源。
#[allow(clippy::result_large_err)]
pub fn check_origin(headers: &HeaderMap, allowlist: &[String]) -> Result<(), Response> {
    let raw = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    let Some(origin) = raw else { return Ok(()) };

    if let Some(host) = origin_host(origin) {
        if is_loopback_host(&host) || host == "tauri.localhost" {
            return Ok(());
        }
    }

    if allowlist.iter().any(|a| a == "*" || a == origin) {
        return Ok(());
    }

    Err(deny(
        StatusCode::FORBIDDEN,
        "Origin 不在允许列表（可在设置中添加跨域白名单）",
        "origin_forbidden",
    ))
}

/// 极简 Origin 解析：scheme://host[:port]/… → hostname。仅识别 http/https。
fn origin_host(origin: &str) -> Option<String> {
    let rest = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))?;
    let hostport = rest.split(['/','?']).next()?;
    if hostport.is_empty() {
        return None;
    }
    Some(hostname_of_host_header(hostport))
}

#[derive(Debug)]
pub struct AuthedKey {
    pub id: String,
}

/// 依据 Bearer / x-api-key 头对活跃网关密钥做常量时间认证。
/// 命中后节流回写 last_used_at。
pub async fn authenticate(db: &Db, headers: &HeaderMap) -> Result<AuthedKey, Response> {
    let presented = bearer_token(headers).or_else(|| x_api_key(headers));
    let Some(token) = presented else {
        return Err(openai_error_unauthorized("缺少 API Key"));
    };

    let token2 = token.clone();
    let db2 = db.clone();
    let active = tokio::task::spawn_blocking(move || -> Result<Vec<(String, String)>, store::StoreError> {
        db2.with(|c| {
            Ok(store::gw_key_active(c)?
                .map(|k| vec![(k.id, k.key)])
                .unwrap_or_default())
        })
    })
    .await;

    let pairs = match active {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            eprintln!("[auth] db error: {e}");
            return Err(openai_error_internal("internal auth error"));
        }
        Err(e) => {
            eprintln!("[auth] task join: {e}");
            return Err(openai_error_internal("internal auth error"));
        }
    };

    for (id, key) in pairs {
        if ct_eq(&token2, &key) {
            // 节流写 best-effort，绝不拖慢主路径（detached）
            let db2 = db.clone();
            let id2 = id.clone();
            tokio::task::spawn_blocking(move || {
                let _ = db2.with(|c| store::gw_key_touch(c, &id2));
            });
            return Ok(AuthedKey { id });
        }
    }
    Err(openai_error_unauthorized("API Key 无效"))
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    v.strip_prefix("Bearer ")
        .or_else(|| v.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn x_api_key(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

// ---------------------------------------------------------------- 响应助手

fn deny(status: StatusCode, message: &str, code: &str) -> Response {
    (
        status,
        Json(json!({ "error": { "message": message, "code": code } })),
    )
        .into_response()
}

fn openai_error_unauthorized(message: &str) -> Response {
    let mut resp = (
        StatusCode::UNAUTHORIZED,
        Json(openai::error_body(
            message,
            "invalid_request_error",
            Some("invalid_api_key"),
        )),
    )
        .into_response();
    resp.headers_mut().insert(
        "WWW-Authenticate",
        HeaderValue::from_static("Bearer"),
    );
    resp
}

fn openai_error_internal(message: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(openai::error_body(message, "api_error", None)),
    )
        .into_response()
}

/// CORS 白名单缓存句柄（避免每请求反序列化 meta JSON）：
/// 每 5 秒失效一次的极简 TTL 缓存。
type CorsCache = Arc<std::sync::Mutex<Option<(i64, Vec<String>)>>>;

#[derive(Clone)]
pub struct CorsAllowlist {
    cache: CorsCache,
    missing_hits: Arc<AtomicU64>,
}

impl Default for CorsAllowlist {
    fn default() -> Self {
        Self::new()
    }
}

impl CorsAllowlist {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(std::sync::Mutex::new(None)),
            missing_hits: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 上限节流：未配置时每 600 次打一条日志提示可配置性。
    fn note_missing(&self) {
        let n = self.missing_hits.fetch_add(1, Ordering::Relaxed);
        if n.is_multiple_of(600) {
            eprintln!("[cors] 未配置白名单，浏览器来源一律拒绝（第 {n} 次）");
        }
    }

    pub async fn get(&self, db: &Db) -> Vec<String> {
        let now_ms = crate::store::now_ms();
        if let Some((ts, list)) = self.cache.lock().unwrap().as_ref() {
            if now_ms - ts < 5000 {
                return list.clone();
            }
        }
        let read = {
            let db2 = db.clone();
            tokio::task::spawn_blocking(move || {
                db2.with(|c| store::meta_get(c, "cors_allow"))
                    .ok()
                    .flatten()
            })
            .await
        };
        let list: Vec<String> = match read {
            Ok(Some(raw)) => serde_json::from_str(&raw).unwrap_or_default(),
            _ => {
                self.note_missing();
                Vec::new()
            }
        };
        *self.cache.lock().unwrap() = Some((now_ms, list.clone()));
        list
    }
}
