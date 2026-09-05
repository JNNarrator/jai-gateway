//! M10 集成测试：出站代理配置（D8）。
//!
//! 拓扑：客户端 → JAI 网关 →（代理？）→ mock 上游
//!
//! 用例：
//! 1. 启用代理（无绕过）→ 请求确实经代理转发（代理计数 +1，上游直连计数 0）
//! 2. 启用代理 + 绕过 127.0.0.1 → 请求直连上游（上游计数 +1，代理计数 0）
//! 3. 未配置代理（默认）→ 直连上游（默认行为回归）

use axum::body::Body;
use axum::extract::State;
use axum::http::{Method, Uri};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use gateway_core::netcfg::{self, ProxyConfig};
use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const UP_BODY: &str = r#"{"id":"x","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"proxied"},"finish_reason":"stop"}]}"#;

/// mock 上游：统计收到的请求数。
async fn spawn_upstream() -> (u16, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let h2 = hits.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || async move {
            h2.fetch_add(1, Ordering::SeqCst);
            Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(Body::from(UP_BODY))
                .unwrap()
        }),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr.port(), hits)
}

/// mock HTTP 代理：记录目标并返回固定响应（绝对形式 URI）。
async fn spawn_proxy() -> (u16, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>) {
    #[derive(Clone)]
    struct PSt(Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>);
    let hits = Arc::new(AtomicUsize::new(0));
    let log = Arc::new(Mutex::new(Vec::new()));
    let st = PSt(hits.clone(), log.clone());
    let app = Router::new().fallback(
        move |State(st): State<PSt>, uri: Uri, method: Method| async move {
            st.0.fetch_add(1, Ordering::SeqCst);
            st.1.lock().unwrap().push(format!(
                "{} {} {}",
                method,
                uri.scheme_str().unwrap_or("-"),
                uri.path()
            ));
            Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(Body::from(UP_BODY))
                .unwrap()
        },
    );
    let app = app.with_state(st);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr.port(), hits, log)
}

struct Fix {
    port: u16,
    key: String,
    up_hits: Arc<AtomicUsize>,
    px_hits: Arc<AtomicUsize>,
    px_log: Arc<Mutex<Vec<String>>>,
    /// 保持 stop_tx 存活：发送端全部 drop 后 watch::changed() 立即 Err → 网关立刻停机
    _keepalive: (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ),
}

impl Fix {
    async fn chat(&self) -> (u16, String) {
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{}/v1/chat/completions", self.port);
        let resp = client
            .post(&url)
            .bearer_auth(&self.key)
            .json(&json!({"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}))
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        (status, text)
    }
}

/// mode: "default"（不配置代理）/ "proxied"（代理无绕过）/ "bypassed"（代理 + 绕过 127.0.0.1）
async fn fixture(mode: &'static str) -> Fix {
    let (up_port, up_hits) = spawn_upstream().await;
    let (px_port, px_hits, px_log) = spawn_proxy().await;

    let dir = std::env::temp_dir().join(format!(
        "jai-m10-{}-{}",
        std::process::id(),
        rand::random::<u32>()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("main.db");
    let p = path.to_str().unwrap().to_string();
    let db = Db::open(&p).unwrap();
    let (logs, _task) = {
        let (h, t) = gateway_core::store::logs::spawn_logger(&p).unwrap();
        (h, t)
    };

    let now = store::now_ms();
    db.with(|c| {
        store::provider_insert(
            c,
            &store::ProviderRow {
                id: "p-openai".into(),
                name: "openai-mock".into(),
                base_url: format!("http://127.0.0.1:{up_port}/v1"), // API 根含 /v1（openai 约定）
                family: "openai_compat".into(),
                enabled: true,
                priority: 1,
                weight: 1,
                extra_headers: None,
                api_key: Some("sk-up".to_string()),
                website: None,
                last_ok_at: None,
                last_err_at: None,
                last_err_msg: None,
                created_at: now,
                updated_at: now,
            },
        )?;
        store::model_upsert(c, "p-openai", "gpt-4o", Some(200000), 8192)?;
        // 按 mode 落代理配置（须在 GatewayCtx::new 之前）
        match mode {
            "proxied" => ProxyConfig {
                enabled: true,
                url: format!("http://127.0.0.1:{px_port}"),
                bypass: vec![],
            }
            .save(c)?,
            "bypassed" => ProxyConfig {
                enabled: true,
                url: format!("http://127.0.0.1:{px_port}"),
                bypass: vec!["127.0.0.1".into()],
            }
            .save(c)?,
            _ => {}
        }
        Ok::<_, store::StoreError>(())
    })
    .unwrap();

    let key = "sk-jai-m10-0000000000000000";
    db.with(|c| {
        store::gw_key_rotate(c, key, Some("test"))?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();

    let ctx = GatewayCtx::new(db.clone(), logs);
    let app = server::build_router(ctx);
    let (listener, port) = server::bind_with_fallback("127.0.0.1", 0).unwrap();
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let guard = tokio::spawn(async move {
        let _ = server::run_until_shutdown(listener, app, stop_rx).await;
    });
    Fix {
        port,
        key: key.to_string(),
        up_hits,
        px_hits,
        px_log,
        _keepalive: (stop_tx, guard),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn upstream_goes_through_configured_proxy() {
    let fx = fixture("proxied").await;
    let (status, text) = fx.chat().await;
    assert_eq!(status, 200);
    assert!(text.contains("proxied"), "{text}");
    // 请求确实走了代理；上游从未被直连
    assert_eq!(fx.px_hits.load(Ordering::SeqCst), 1, "代理应收到 1 次转发");
    assert_eq!(fx.up_hits.load(Ordering::SeqCst), 0, "上游不应被直连");
    let log = fx.px_log.lock().unwrap();
    assert!(
        log.iter()
            .any(|l| l.contains("http") && l.contains("/v1/chat/completions")),
        "代理应记录绝对形式目标: {log:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn bypassed_host_goes_direct() {
    let fx = fixture("bypassed").await;
    let (status, text) = fx.chat().await;
    assert_eq!(status, 200);
    assert!(text.contains("proxied"), "{text}");
    // 绕过 127.0.0.1 → 直连上游，代理不被触及
    assert_eq!(fx.up_hits.load(Ordering::SeqCst), 1, "上游应被直连");
    assert_eq!(fx.px_hits.load(Ordering::SeqCst), 0, "代理不应收到请求");
}

#[tokio::test(flavor = "multi_thread")]
async fn default_no_proxy_keeps_direct_behavior() {
    let fx = fixture("default").await;
    let (status, text) = fx.chat().await;
    assert_eq!(status, 200);
    assert!(text.contains("proxied"), "{text}");
    assert_eq!(fx.up_hits.load(Ordering::SeqCst), 1);
    assert_eq!(fx.px_hits.load(Ordering::SeqCst), 0);
    // 代理元数据未配置时 from_meta 应给出默认关闭
    assert!(!netcfg::ProxyConfig::default().enabled);
}
