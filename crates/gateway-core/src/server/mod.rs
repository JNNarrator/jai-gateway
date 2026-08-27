//! 网关监督面（M0 范围）：绑定与端口顺延、/healthz。
//!
//! 设计依据：roadmap M0（端口占用自动顺延、healthz）+
//! 稳定性基线 §5-2（后续在此扩展超时配置）。业务路由自 M1 起挂载。

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use std::net::{SocketAddr, TcpListener};
use thiserror::Error;
use tokio::net::TcpListener as AsyncTcpListener;

pub const DEFAULT_PORT: u16 = 1314;
/// 端口顺延最大尝试次数：1314..1314+PORT_SCAN_TRIES
pub const PORT_SCAN_TRIES: u16 = 16;

#[derive(Debug, Clone)]
pub struct AppState {
    pub version: String,
    pub started_at_ms: u64,
}

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
/// 返回 (实际监听器, 实际端口)。绑定次序即"自动顺延"，UI 展示返回的实际端口。
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
                std_listener.set_nonblocking(true)?;
                let listener = AsyncTcpListener::from_std(std_listener)?;
                return Ok((listener, port));
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

#[derive(Serialize)]
pub struct Health {
    pub ok: bool,
    pub version: String,
    pub started_at_ms: u64,
}

async fn healthz(State(st): State<AppState>) -> Json<Health> {
    Json(Health {
        ok: true,
        version: st.version.clone(),
        started_at_ms: st.started_at_ms,
    })
}

/// M0 全量路由表。M1 起在 `proxy_routes()` 内按里程碑扩充。
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .with_state(state)
}

/// 以优雅停机方式运行直到收到关停信号（监督进程喂入）。
pub async fn run_until_shutdown(
    listener: AsyncTcpListener,
    app: Router,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> std::io::Result<()> {
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown.changed().await;
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 绑定真实端口的测试必须串行：并行时「下一个端口」可能被
    /// 其他测试的临时监听抢走，导致顺延断言偶发失败。
    /// 用 tokio Mutex：需跨 await 持有（clippy::await_holding_lock）。
    static PORT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// 直连 handler 的纯单元断言：无网络依赖，任何环境都跑。
    #[tokio::test]
    async fn healthz_handler_returns_ok_and_version() {
        let st = AppState {
            version: "0.1.0".into(),
            started_at_ms: 42,
        };
        let Json(h) = healthz(State(st)).await;
        assert!(h.ok);
        assert_eq!(h.version, "0.1.0");
        assert_eq!(h.started_at_ms, 42);
    }

    fn http_get(port: u16, path: &str) -> String {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port))
            .expect("loopback connect（本地沙箱可能禁外连，见 ignore 说明）");
        write!(
            s,
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut buf = String::new();
        s.read_to_string(&mut buf).unwrap();
        buf
    }

    /// 完整 HTTP 回环验证。CI（GitHub Actions）与真机均会执行；
    /// 本地受限开发沙箱禁 outbound connect，故标 ignore。
    #[tokio::test]
    #[ignore = "需 loopback 连接权限；CI 与真机运行（cargo test -- --ignored 验证）"]
    async fn healthz_http_roundtrip() {
        let _guard = PORT_TEST_LOCK.lock().await;
        let state = AppState {
            version: "0.1.0".into(),
            started_at_ms: 1234567890,
        };
        let (listener, port) = bind_with_fallback("127.0.0.1", 0).unwrap();
        let app = build_router(state);

        let (tx, rx) = tokio::sync::watch::channel(false);
        let server = tokio::spawn(run_until_shutdown(listener, app, rx));
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let resp = http_get(port, "/healthz");
        assert!(resp.starts_with("HTTP/1.1 200"), "status: {resp}");
        assert!(resp.contains("\"ok\":true"), "body: {resp}");
        assert!(resp.contains("\"version\":\"0.1.0\""));

        tx.send(true).unwrap();
        server.await.unwrap().unwrap();
    }

    /// M0 验收标准 3：首选端口被占 → 自动顺延到下一个可用端口。
    #[tokio::test]
    async fn port_fallback_skips_occupied() {
        let _guard = PORT_TEST_LOCK.lock().await;
        // 占住一个真实端口
        let blocker = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let occupied = blocker.local_addr().unwrap().port();

        let (_listener, actual) = bind_with_fallback("127.0.0.1", occupied).expect("应顺延成功");
        assert_eq!(actual, occupied + 1, "必须从被占端口的下一个端口继续");
    }

    #[test]
    fn invalid_host_is_rejected_immediately() {
        let err = bind_with_fallback("not_a_host!!", DEFAULT_PORT).unwrap_err();
        assert!(matches!(err, BindError::InvalidHost(_)));
    }
}
