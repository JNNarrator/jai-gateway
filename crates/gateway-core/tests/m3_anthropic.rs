//! M3 集成测试：Anthropic 入站直通（roadmap M3 验收 3/4）。
//!
//! 拓扑：mock Anthropic 上游 ←─ JAI 网关（POST /v1/messages）
//!
//! 用例：
//! 1. 直通成功：上游 200 → 客户端收到原样 JSON（含 mock tag）
//! 2. count_tokens：恒返回正整数 input_tokens
//! 3. 错误 Anthropic 化：mock 上游 429 → 客户端收到 Anthropic error schema
//! 4. Overloaded 529 保留：mock 529 → 作为可转移错误/最终错误以 529 回给客户端
//! 5. 日志 inbound_family='anthropic'

use axum::body::Body;
use axum::routing::post;
use axum::Router;
use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use gateway_core::vault;
use serde_json::{json, Value};

// ---------------------------------------------------------------- mock 上游

/// Anthropic 风格 mock：记录收到的认证头，返回固定状态/内容。
async fn spawn_anthropic_mock(
    status: u16,
    inbound_headers: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
) -> u16 {
    let app = Router::new().route(
        "/v1/messages",
        post(
            move |headers: axum::http::HeaderMap, body: String| async move {
                let mut rec = inbound_headers.lock().unwrap();
                for (k, v) in [
                    ("x-api-key", "sk-upstream-abc"),
                    ("anthropic-version", "2023-06-01"),
                ] {
                    if let Some(vv) = headers.get(k).and_then(|v| v.to_str().ok()) {
                        if vv == v {
                            rec.push((k.to_string(), vv.to_string()));
                        }
                    }
                }
                let _ = body;
                let payload = if (400..600).contains(&status) {
                    json!({
                        "type":"error",
                        "error":{"type":"api_error","message":format!("mock {status}")}
                    })
                } else {
                    json!({"id":"msg_mock","type":"message","role":"assistant",
                    "model":"claude-sonnet-4",
                    "content":[{"type":"text","text":"from-anthropic-mock"}],
                    "stop_reason":"end_turn","usage":{"input_tokens":11,"output_tokens":7}})
                };
                Response::builder()
                    .status(status)
                    .header("content-type", "application/json")
                    .header("x-mock-tag", "anthropic")
                    .body(Body::from(payload.to_string()))
                    .unwrap()
            },
        ),
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

use axum::response::Response;

// ---------------------------------------------------------------- 夹具

struct Fixture {
    port: u16,
    db: Db,
    key: String,
    headers: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    _keepalive: (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ),
}

async fn fixture(upstream_status: u16) -> Fixture {
    vault::testing::set_mock_default();

    let headers = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let up_port = spawn_anthropic_mock(upstream_status, headers.clone()).await;

    let (db, main_path) = {
        let dir = std::env::temp_dir().join(format!(
            "jai-m3-{}-{}",
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

    let now = store::now_ms();
    db.with(|c| {
        store::provider_insert(
            c,
            &store::ProviderRow {
                id: "p-claude".into(),
                name: "claude-mock".into(),
                base_url: format!("http://127.0.0.1:{up_port}"),
                family: "anthropic".into(),
                enabled: true,
                priority: 1,
                weight: 1,
                extra_headers: None,
                keyring_ref: vault::ref_for("p-claude"),
                last_ok_at: None,
                last_err_at: None,
                last_err_msg: None,
                created_at: now,
                updated_at: now,
            },
        )?;
        store::model_upsert(c, "p-claude", "claude-sonnet-4", Some(200000), 8192)?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();
    vault::set_secret(&vault::ref_for("p-claude"), "sk-upstream-abc").unwrap();

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

    Fixture {
        port,
        db,
        key: key.to_string(),
        headers,
        _keepalive: (stop_tx, guard),
    }
}

impl Fixture {
    async fn post_messages(&self, body: Value) -> (u16, Value) {
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{}/v1/messages", self.port);
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

    async fn post_count_tokens(&self, body: Value) -> (u16, Value) {
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{}/v1/messages/count_tokens", self.port);
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

fn messages_body() -> Value {
    json!({
        "model": "claude-sonnet-4",
        "max_tokens": 1024,
        "messages": [{"role":"user","content":"hello from cc"}]
    })
}

// ---------------------------------------------------------------- 用例

/// 直通成功 + 上游收到正确认证头 + 日志族为 anthropic
#[tokio::test(flavor = "multi_thread")]
async fn anthropic_passthrough_success() {
    let fx = fixture(200).await;
    let (status, body) = fx.post_messages(messages_body()).await;
    assert_eq!(status, 200);
    assert_eq!(body["content"][0]["text"], "from-anthropic-mock");

    // 认证头正确传递
    assert!(
        fx.headers
            .lock()
            .unwrap()
            .iter()
            .any(|(k, _)| k == "x-api-key" && true),
        "应携带 x-api-key"
    );
    assert!(
        fx.headers
            .lock()
            .unwrap()
            .iter()
            .any(|(k, v)| k == "anthropic-version" && v == "2023-06-01"),
        "应携带 anthropic-version 默认值"
    );

    // 日志族
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    let rows = gateway_core::store::logs::logs_recent(&fx.db, 20).unwrap();
    let ok = rows.iter().find(|r| r.http_status == 200).unwrap();
    assert_eq!(ok.inbound_family, "anthropic");
    assert_eq!(ok.provider_id.as_deref(), Some("p-claude"));
    assert_eq!(ok.usage_input, Some(11));
    assert_eq!(ok.usage_output, Some(7));
}

/// count_tokens 恒正整数（验收 3）
#[tokio::test(flavor = "multi_thread")]
async fn count_tokens_returns_positive() {
    // count_tokens 不需要上游：任何渠道配置缺失也直接估算
    let fx = fixture(200).await;
    for _ in 0..3 {
        let (status, body) = fx.post_count_tokens(messages_body()).await;
        assert_eq!(status, 200);
        let n = body["input_tokens"].as_u64().expect("input_tokens 正整数");
        assert!(n > 0, "估算必须为正");
    }
}

/// Anthropic 错误形状：上游 429 → 客户端收到 Anthropic schema（验收 4）
#[tokio::test(flavor = "multi_thread")]
async fn anthropic_error_schema_on_ratelimit() {
    let fx = fixture(429).await;
    let (status, body) = fx.post_messages(messages_body()).await;
    // 429 属于可切换错误；但只有单渠道 → 全部失败返回最后一个错误。
    // 网关层按入站 Anthropic 方言渲染：type:error 形状（带 429 原状态）。
    assert_eq!(status, 429, "RateLimit 应在 Anthropic 线保留 429");
    assert_eq!(body["type"], "error", "Anthropic 错误形状最外层 type:error");
}

/// Overloaded → HTTP 529 保留（验收 4 的 529 特例）
#[tokio::test(flavor = "multi_thread")]
async fn overloaded_maps_to_529() {
    let fx = fixture(529).await;
    let (status, body) = fx.post_messages(messages_body()).await;
    assert_eq!(status, 529, "Overloaded 必须保留 Anthropic 特有状态码 529");
    assert_eq!(body["error"]["type"], "api_error");
}

/// 缺模型 → 404 也走 Anthropic 形状
#[tokio::test(flavor = "multi_thread")]
async fn model_not_found_anthropic_shape() {
    let fx = fixture(200).await;
    let body = json!({"model":"no-such-model","messages":[{"role":"user","content":"hi"}]});
    let (status, body) = fx.post_messages(body).await;
    assert_eq!(status, 404);
    assert_eq!(body["type"], "error");
}
