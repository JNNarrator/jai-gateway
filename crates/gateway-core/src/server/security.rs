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
use crate::store::keyrules::KeyRules;
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
        Some((host, port)) if !port.is_empty() && !host.contains(':') => host.to_ascii_lowercase(),
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
    let hostport = rest.split(['/', '?']).next()?;
    if hostport.is_empty() {
        return None;
    }
    Some(hostname_of_host_header(hostport))
}

#[derive(Debug, Clone)]
pub struct AuthedKey {
    pub id: String,
}

/// 依据 Bearer / x-api-key 头对**全部未吊销**网关密钥做常量时间认证。
/// 命中后节流回写 last_used_at。
///
/// 多密钥（D9-T6a）：一把密钥对应一个客户端 / 一个人，可独立吊销。所以这里是
/// 「取全部活跃密钥 → 逐个比对」，命中即返回该密钥的 id（后续按 key 归因要用）。
///
/// **不做短路优化**：不要「先比 prefix 再比全文」—— prefix 是 14 字符明文，
/// 拿它做快速筛选会让「哪些 prefix 存在」可被时序区分。密钥数量是个位数，
/// 全量 `ct_eq` 遍历的成本可忽略。
// Err 直接携带 axum Response（鉴权失败即返回的错误体），体积超 clippy
// result_large_err 阈值；属 API 形状选择，仅在认证失败路径构造。
#[allow(clippy::result_large_err)]
pub async fn authenticate(db: &Db, headers: &HeaderMap) -> Result<AuthedKey, Response> {
    let presented = bearer_token(headers).or_else(|| x_api_key(headers));
    let Some(token) = presented else {
        return Err(openai_error_unauthorized("缺少 API Key"));
    };

    let token2 = token.clone();
    let db2 = db.clone();
    let active = tokio::task::spawn_blocking(
        move || -> Result<Vec<(String, String)>, store::StoreError> {
            db2.with(|c| {
                Ok(store::gw_keys_active(c)?
                    .into_iter()
                    .map(|k| (k.id, k.key))
                    .collect())
            })
        },
    )
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
    let v = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
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
    resp.headers_mut()
        .insert("WWW-Authenticate", HeaderValue::from_static("Bearer"));
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

// ------------------------------------------------------ 密钥规则缓存（D9-T6b）

/// 规则的 TTL（毫秒）。与 `CorsAllowlist` 同一档：5 秒足够挡掉同一客户端
/// 连续请求的重复查库，又不至于让「改了规则」长时间不生效。
const RULES_TTL_MS: i64 = 5000;

/// 缓存内容：key_id → (写入时刻, 规则)。用 `Arc<KeyRules>` 让热路径只克隆指针。
type RulesMap = std::collections::HashMap<String, (i64, Arc<KeyRules>)>;

/// 密钥规则的缓存句柄（避免每个请求都查库）。
///
/// 与 `CorsAllowlist` 同一套模式（5 秒 TTL），差异在于**保存后立刻失效**：
/// 用户刚在界面上写完规则就去测，等 5 秒才知道生效会以为是坏的，
/// 所以 `gateway_key_rules_set` 会调 [`KeyRulesCache::invalidate`]。
#[derive(Clone)]
pub struct KeyRulesCache {
    cache: Arc<std::sync::Mutex<RulesMap>>,
}

impl Default for KeyRulesCache {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyRulesCache {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// 取某把密钥的规则；未命中 / 过期则查库。
    ///
    /// 查库失败按**不限制**处理（打一条日志）：路由本身也要查库，DB 真坏了请求
    /// 在下游照样会 500；把规则查询失败升级成「全拒」只会让所有客户端一起收到
    /// 403，把一个本机单人用的网关变成 fail-closed 的边界，收益为负。
    pub async fn get(&self, db: &Db, key_id: &str) -> Arc<KeyRules> {
        let now_ms = crate::store::now_ms();
        if let Some((ts, rules)) = self.cache.lock().unwrap().get(key_id) {
            if now_ms - ts < RULES_TTL_MS {
                return rules.clone();
            }
        }
        let db2 = db.clone();
        let id2 = key_id.to_string();
        let read = tokio::task::spawn_blocking(move || {
            db2.with(|c| store::keyrules::key_rules_get(c, &id2))
        })
        .await;
        let rules = match read {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                eprintln!("[rules] 读取密钥规则失败，本次按不限制处理: {e}");
                KeyRules::default()
            }
            Err(e) => {
                eprintln!("[rules] 规则查询任务失败，本次按不限制处理: {e}");
                KeyRules::default()
            }
        };
        let rules = Arc::new(rules);
        self.cache
            .lock()
            .unwrap()
            .insert(key_id.to_string(), (now_ms, rules.clone()));
        rules
    }

    /// 让缓存立刻失效。`None` = 全部清空（留给「批量改规则」类路径）。
    pub fn invalidate(&self, key_id: Option<&str>) {
        let mut c = self.cache.lock().unwrap();
        match key_id {
            Some(id) => {
                c.remove(id);
            }
            None => c.clear(),
        }
    }
}
