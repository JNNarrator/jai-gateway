//! M11 集成测试：上游**连接类**失败在**同一渠道**内的重试
//! （`proxy.rs` 的 `UPSTREAM_CONNECT_RETRY` / `should_retry_connect`）。
//!
//! 背景（2026-09-20 实测）：「基元律动」的 502 占全量 **9.78%**（当天 20%），错误为
//! `上游连接失败: error sending request for url (…)`。当时该模型**只有一个渠道**
//! （`route_candidates` 精确匹配 model_name），于是「切下一渠道」无路可走、又没有同渠道
//! 重试 —— 一次上游抖动就让整个回合失败，只能靠客户端自己重试救回来。
//!
//! 拓扑：
//!   黑洞上游（accept 后立刻断开、不回响应头） ←─┐
//!                                               ├─ JAI 网关（真实 build_router，单渠道）
//!   计数 500 上游（HTTP 500 + 请求计数）      ←─┘
//!
//! 用例：
//! 1. OpenAI 入站（**直通**路径）连接类失败 → 同渠道重试 1 次（建连计数 == 2），最终 502
//! 2. Responses 入站（**转换**路径）连接类失败 → 同渠道重试 1 次（建连计数 == 2），最终 502
//! 3. 上游返回 HTTP 500（链路是通的）→ **不**重试（请求计数 == 1）—— 避免放大上游压力
//!
//! 用例 1/2 覆盖两条各自独立的发送点（`try_candidate` / `try_converted_candidate`），
//! 任一漏改都会让对应用例的计数停在 1。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use serde_json::{json, Value};

// ---------------------------------------------------------------- mock 上游

/// 「黑洞」上游：接受 TCP 连接后**立刻断开**、不回任何字节。
///
/// 这样 reqwest 的 `.send()` 必然返回 Err（响应头都没到），稳定复现「连接类失败」；
/// 同时**统计建连次数** —— 次数就是「网关对该渠道发起了几次请求」，
/// 是同渠道重试最直接的证据（1 次 = 没重试，2 次 = 重试了一次）。
async fn spawn_black_hole() -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    tokio::spawn(async move {
        // 上游一旦开始接受连接就持续运行；测试进程结束即随之销毁。
        while let Ok((stream, _)) = listener.accept().await {
            h.fetch_add(1, Ordering::SeqCst);
            drop(stream); // 不发响应头就断开
        }
    });
    (port, hits)
}

/// 恒 500 的 mock chat completions，并统计**收到的请求数**。
/// 用于证明「HTTP 层失败不触发同渠道重试」：链路是通的，重试没有意义。
async fn spawn_counting_500() -> (u16, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let h = h.clone();
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                Response::builder()
                    .status(500)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"error": {"message": "mock 500", "type": "api_error", "code": null}})
                            .to_string(),
                    ))
                    .unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (port, hits)
}

// ---------------------------------------------------------------- 夹具

/// 起一个完整 JAI 网关（临时文件 DB + 单个 openai_compat 渠道）。
/// **单渠道**是刻意的：这样任何「重试」都只能发生在同渠道内，计数无歧义。
struct Fixture {
    port: u16,
    key: String,
    _keepalive: (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ),
}

async fn fixture(upstream_port: u16) -> Fixture {
    let (db, main_path) = {
        let dir = std::env::temp_dir().join(format!(
            "jai-m11-main-{}-{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.db");
        let p = path.to_str().unwrap().to_string();
        (Db::open(&p).unwrap(), p)
    };
    let (logs, _task) = gateway_core::store::logs::spawn_logger(&main_path).unwrap();

    let now = store::now_ms();
    db.with(|c| {
        store::provider_insert(
            c,
            &store::ProviderRow {
                id: "p-only".into(),
                name: "only".into(),
                base_url: format!("http://127.0.0.1:{upstream_port}/v1"),
                family: "openai_compat".into(),
                enabled: true,
                priority: 100,
                weight: 1,
                extra_headers: None,
                api_key: Some("sk-only".into()),
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
        store::model_upsert(c, "p-only", "m-retry", Some(128000), 4096, None, None)?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();

    let key = "sk-jai-integration-test-connect-retry";
    db.with(|c| {
        store::gw_key_rotate(c, key, Some("test"))?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();

    let ctx = GatewayCtx::new(db.clone(), logs);
    // 显式关掉一切代理再打上游。
    // 不这么做的话，本机若开着系统代理（如 Clash），到 127.0.0.1:<黑洞端口> 的请求会被代理
    // 接走并**回一个 HTTP 502**（而不是发送失败）—— 网关看到的是「上游 5xx」，
    // 于是走 5xx 分支不重试，用例会以「只建连 1 次」的假象失败。
    // 本用例要验的是重试逻辑，必须让失败形态确定落在「连接类」上。
    let mut ctx = ctx;
    ctx.http = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
    let app = server::build_router(ctx);
    let (listener, port) = server::bind_with_fallback("127.0.0.1", 0).unwrap();
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let guard = tokio::spawn(async move {
        let _ = server::run_until_shutdown(listener, app, stop_rx).await;
    });

    Fixture {
        port,
        key: key.to_string(),
        _keepalive: (stop_tx, guard),
    }
}

impl Fixture {
    /// POST 任意入站端点，返回 (状态码, body)
    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{}{path}", self.port);
        let resp = client
            .post(&url)
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        (status, body)
    }
}

fn chat_body() -> Value {
    json!({
        "model": "m-retry",
        "stream": false,
        "messages": [{"role": "user", "content": "hi"}]
    })
}

fn responses_body() -> Value {
    json!({
        "model": "m-retry",
        "stream": false,
        "input": [{"type": "message", "role": "user",
                   "content": [{"type": "input_text", "text": "hi"}]}]
    })
}

/// 等重试/收尾路径彻底跑完，避免计数断言撞上竞态。
async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
}

// ---------------------------------------------------------------- 用例

/// 直通路径（OpenAI 入站 → openai_compat 上游）：连接类失败 → 同渠道重试 1 次。
#[tokio::test(flavor = "multi_thread")]
async fn connect_failure_retries_once_on_passthrough_path() {
    let (up_port, hits) = spawn_black_hole().await;
    let fx = fixture(up_port).await;

    let (status, _body) = fx.post("/v1/chat/completions", chat_body()).await;
    settle().await;

    let n = hits.load(Ordering::SeqCst);
    assert_eq!(
        n, 2,
        "连接类失败应在同渠道重试一次（首次 1 + 重试 1 = 2 次建连），实际 {n} 次"
    );
    assert_eq!(
        status, 502,
        "全渠道失败后应回 502（网络级失败无上游状态码）"
    );
}

/// 转换路径（Responses 入站 → openai_compat 上游）：连接类失败 → 同渠道重试 1 次。
///
/// 这条路径有**独立的发送点**（`try_converted_candidate`），故必须单独覆盖。
#[tokio::test(flavor = "multi_thread")]
async fn connect_failure_retries_once_on_converted_path() {
    let (up_port, hits) = spawn_black_hole().await;
    let fx = fixture(up_port).await;

    let (status, _body) = fx.post("/v1/responses", responses_body()).await;
    settle().await;

    let n = hits.load(Ordering::SeqCst);
    assert_eq!(n, 2, "转换路径的连接类失败同样应重试一次，实际 {n} 次");
    assert_eq!(status, 502, "全渠道失败后应回 502");
}

/// 反证：上游回了 HTTP 500（链路是通的）→ **不**重试，避免放大上游压力。
///
/// 没有这条断言，把重试条件写成「任何失败都重试」也能让上面两条用例通过。
#[tokio::test(flavor = "multi_thread")]
async fn http_5xx_does_not_retry_same_channel() {
    let (up_port, hits) = spawn_counting_500().await;
    let fx = fixture(up_port).await;

    let (status, _body) = fx.post("/v1/chat/completions", chat_body()).await;
    settle().await;

    let n = hits.load(Ordering::SeqCst);
    assert_eq!(
        n, 1,
        "HTTP 层失败（链路已通）不得触发同渠道重试，实际 {n} 次请求"
    );
    assert!(
        status >= 400,
        "上游 5xx 应作为错误回给客户端，实际 {status}"
    );
}
