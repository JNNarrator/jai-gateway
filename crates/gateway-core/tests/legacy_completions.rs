//! 旧版 OpenAI text completions 入站冒烟测试。
//!
//! 拓扑：客户端 POST /v1/completions → JAI → mock OpenAI /completions 上游。
//! 当前只承诺 openai_compat 直通，不做跨族转换。

use axum::body::Body;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use serde_json::{json, Value};

async fn spawn_completions_mock() -> u16 {
    let app = Router::new().route(
        "/v1/completions",
        post(|| async move {
            let payload = json!({
                "id": "cmpl-1",
                "object": "text_completion",
                "created": 1,
                "model": "gpt-3.5-turbo-instruct",
                "choices": [{
                    "text": "hello from completions",
                    "index": 0,
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 3, "total_tokens": 4}
            });
            Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
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
    addr.port()
}

#[tokio::test(flavor = "multi_thread")]
async fn legacy_completions_passthrough() {
    let up_port = spawn_completions_mock().await;

    let (db, main_path) = {
        let dir = std::env::temp_dir().join(format!(
            "jai-completions-{}-{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.db");
        let p = path.to_str().unwrap().to_string();
        (Db::open(&p).unwrap(), p)
    };
    let (logs, _task) = {
        let (h, t) = gateway_core::store::logs::spawn_logger(&main_path).unwrap();
        (h, t)
    };

    let pid = "p-openai";
    let model = "gpt-3.5-turbo-instruct";
    let now = store::now_ms();
    db.with(|c| {
        store::provider_insert(
            c,
            &store::ProviderRow {
                id: pid.into(),
                name: "openai-mock".into(),
                base_url: format!("http://127.0.0.1:{up_port}/v1"),
                family: "openai_compat".into(),
                enabled: true,
                priority: 1,
                weight: 1,
                extra_headers: None,
                api_key: Some("sk-openai".into()),
                website: None,
                last_ok_at: None,
                last_err_at: None,
                last_err_msg: None,
                created_at: now,
                updated_at: now,
            },
        )?;
        store::model_upsert(c, pid, model, Some(8192), 2048)?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();
    
    let key = "sk-jai-integration-test-0000000000000000";
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

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/v1/completions"))
        .bearer_auth(key)
        .json(&json!({"model": model, "prompt": "hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["choices"][0]["text"], "hello from completions");

    let _ = stop_tx.send(true);
    let _ = guard.await;
}
