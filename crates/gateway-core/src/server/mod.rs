//! 网关监督面：绑定与端口顺延、/healthz、安全中间件装配、直通代理挂载。
//!
//! 设计依据：roadmap M0/M1 + 稳定性基线 §5-2（超时常量在 proxy 模块）。

pub mod proxy;
pub mod ratelimit;
pub mod registry;
pub mod security;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use std::net::{SocketAddr, TcpListener};
use thiserror::Error;
use tokio::net::TcpListener as AsyncTcpListener;

pub use proxy::GatewayCtx;

pub const DEFAULT_PORT: u16 = 1314;
/// 端口顺延最大尝试次数：1314..1314+PORT_SCAN_TRIES
pub const PORT_SCAN_TRIES: u16 = 16;

#[derive(Debug, Error)]
pub enum BindError {
    #[error("主机部分不是合法 IP 地址: {0}")]
    InvalidHost(String),
    #[error("{PORT_SCAN_TRIES} 个候选端口均被占用 ({from_port} 起)")]
    NoFreePort { from_port: u16 },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// 在 `preferred_port` 起的连续候选中寻找第一个可用端口并绑定（roadmap M0 验收 3）。
///
/// 仅接受 IP 字面量作为 host（M0 固定 127.0.0.1；域名解析随 M1 引入上游配置时再说）。
/// 返回 (实际监听器, 实际端口)。
pub fn bind_with_fallback(
    host: &str,
    preferred_port: u16,
) -> Result<(AsyncTcpListener, u16), BindError> {
    let ip: std::net::IpAddr = host
        .parse()
        .map_err(|_| BindError::InvalidHost(host.to_string()))?;
    for offset in 0..PORT_SCAN_TRIES {
        let port = preferred_port.saturating_add(offset);
        let addr = SocketAddr::from((ip, port));
        match TcpListener::bind(addr) {
            Ok(std_listener) => {
                let actual_port = std_listener.local_addr()?.port();
                std_listener.set_nonblocking(true)?;
                let listener = AsyncTcpListener::from_std(std_listener)?;
                return Ok((listener, actual_port));
            }
            // 仅"端口被占"允许顺延；权限等其他错误立即失败
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(BindError::NoFreePort {
        from_port: preferred_port,
    })
}

// ---------------------------------------------------------------- healthz

#[derive(Serialize)]
pub struct Health {
    pub ok: bool,
    pub version: String,
    pub started_at_ms: u64,
}

async fn healthz(State(ctx): State<GatewayCtx>) -> Json<Health> {
    Json(Health {
        ok: true,
        version: ctx.version.clone(),
        started_at_ms: ctx.started_at_ms,
    })
}

// ---------------------------------------------------------------- 路由表

/// 全量路由。M1 增加鉴权/安全层与两条业务端点；
/// 后续里程碑沿此扩展（M3 加 /v1/messages，M6 加 /v1/responses）。
pub fn build_router(ctx: GatewayCtx) -> Router {
    use axum::middleware;

    Router::new()
        .route("/v1/chat/completions", post(proxy::chat_completions))
        .route("/v1/models", get(proxy::models_list))
        // M3：Anthropic 入站（Claude Code 直连）+ count_tokens 粗估
        .route("/v1/messages", post(proxy::anthropic_messages))
        .route(
            "/v1/messages/count_tokens",
            post(proxy::anthropic_count_tokens),
        )
        // M6：Responses API 入站（Codex 原生线）
        .route("/v1/responses", post(proxy::responses))
        // 第二阶段：旧版 OpenAI text completions（仅 openai_compat 直通）
        .route("/v1/completions", post(proxy::completions))
        // MCP 元数据服务：网关登记的 MCP Server / Skill 台账（只读，不执行工具）
        .route("/mcp", post(registry::mcp_endpoint))
        .layer(middleware::from_fn_with_state(
            ctx.clone(),
            proxy::security_mw,
        ))
        .route("/healthz", get(healthz))
        .with_state(ctx)
}

/// 以优雅停机方式运行直到收到关停信号（监督进程喂入）。
///
/// `into_make_service_with_connect_info`：让限速/审计中间件能拿到对端 IP
/// （axum 0.8 中 ConnectInfo 扩展注入依赖此服务构建器）。
pub async fn run_until_shutdown(
    listener: AsyncTcpListener,
    app: Router,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> std::io::Result<()> {
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = shutdown.changed().await;
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    static PORT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// 绑定真实端口的用例必须串行（M0 验收 3）。
    #[tokio::test]
    async fn port_fallback_skips_occupied() {
        let _g = PORT_TEST_LOCK.lock().await;
        let blocker = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let occupied = blocker.local_addr().unwrap().port();

        let (_listener, actual) = bind_with_fallback("127.0.0.1", occupied).expect("应顺延成功");
        assert_eq!(actual, occupied + 1);
    }

    #[tokio::test]
    async fn bind_zero_is_random_and_ok() {
        let _g = PORT_TEST_LOCK.lock().await;
        let (_l, p) = bind_with_fallback("127.0.0.1", 0).unwrap();
        assert_ne!(p, 0);
    }

    #[test]
    fn invalid_host_rejected() {
        assert!(matches!(
            bind_with_fallback("not_a_host!!", DEFAULT_PORT),
            Err(BindError::InvalidHost(_))
        ));
    }

    // ---------------- security ----------------

    mod sec {
        use super::super::security::*;
        use axum::http::{HeaderMap, HeaderValue};

        #[test]
        fn ct_eq_basics() {
            assert!(ct_eq("sk-jai-abc", "sk-jai-abc"));
            assert!(!ct_eq("sk-jai-abc", "sk-jai-abd"));
            assert!(!ct_eq("short", "longer-string"));
        }

        #[test]
        fn host_header_parsing() {
            assert_eq!(hostname_of_host_header("127.0.0.1:1314"), "127.0.0.1");
            assert_eq!(hostname_of_host_header("localhost"), "localhost");
            assert_eq!(hostname_of_host_header("[::1]:8080"), "::1");
            assert_eq!(
                hostname_of_host_header("evil.example.com"),
                "evil.example.com"
            );
        }

        #[test]
        fn host_check_blocks_non_loopback() {
            let mut h = HeaderMap::new();
            h.insert(
                axum::http::header::HOST,
                HeaderValue::from_static("evil.com:1314"),
            );
            assert!(check_host(&h).is_err());

            h.insert(
                axum::http::header::HOST,
                HeaderValue::from_static("127.0.0.1:9999"),
            );
            assert!(check_host(&h).is_ok());

            // 缺失 Host 也拒绝
            let empty = HeaderMap::new();
            assert!(check_host(&empty).is_err());
        }

        #[test]
        fn origin_default_deny_and_allowlist() {
            let mut h = HeaderMap::new();
            // 非浏览器客户端无 Origin：放行
            assert!(check_origin(&HeaderMap::new(), &[]).is_ok());

            h.insert(
                axum::http::header::ORIGIN,
                HeaderValue::from_static("https://chat.example.com"),
            );
            assert!(check_origin(&h, &[]).is_err(), "默认拒绝远程来源");

            assert!(check_origin(&h, &["https://chat.example.com".into()]).is_ok());
            assert!(check_origin(&h, &["*".into()]).is_ok());
            assert!(check_origin(&h, &["https://other.io".into()]).is_err());

            // 本机来源天然放行（无清单）
            h.insert(
                axum::http::header::ORIGIN,
                HeaderValue::from_static("http://localhost:5173"),
            );
            assert!(check_origin(&h, &[]).is_ok());
        }
    }
}
