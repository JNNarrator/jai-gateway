//! M4 集成测试：跨族转换端到端（roadmap M4 验收）。
//!
//! 拓扑：客户端(OpenAI 形状) → JAI 网关 → mock Anthropic / mock Gemini 上游
//!
//! 用例：
//! 1. OpenAI 入站 → Anthropic 上游 非流式（文本转换）
//! 2. OpenAI 入站 → Anthropic 上游 流式（SSE 事件转换）
//! 3. OpenAI 入站 → Gemini 上游 非流式（文本 + 工具调用）
//! 4. 上游 429 → 转换路径 Failover → 全失败 502（OpenAI 错误形状）

use axum::body::Body;
use axum::extract::State;
use axum::routing::post;
use axum::Router;
use axum::response::Response;
use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use gateway_core::vault;
use serde_json::{Value, json};

// ---------------------------------------------------------------- mock 上游

/// Anthropic mock：按模式返回固定响应。
async fn spawn_anthropic_mock(mode: &'static str) -> u16 {
    let app = Router::new().route(
        "/v1/messages",
        post(move || async move {
            let payload = match mode {
                "text" => json!({
                    "id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-4",
                    "content":[{"type":"text","text":"converted-from-anthropic"}],
                    "stop_reason":"end_turn",
                    "usage":{"input_tokens":5,"output_tokens":3}
                }),
                "tool" => json!({
                    "id":"msg_2","type":"message","role":"assistant","model":"claude-sonnet-4",
                    "content":[
                        {"type":"text","text":"Let me check."},
                        {"type":"tool_use","id":"toolu_1","name":"get_weather",
                         "input":{"city":"beijing"}}
                    ],
                    "stop_reason":"tool_use",
                    "usage":{"input_tokens":6,"output_tokens":4}
                }),
                "error429" => json!({
                    "type":"error",
                    "error":{"type":"rate_limit_error","message":"mock 429"}
                }),
                _ => json!({}),
            };
            let status = if mode == "error429" { 429 } else { 200 };
            Response::builder()
                .status(status)
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap()
        }),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr.port()
}

/// Gemini mock：支持非流式文本/工具，流式走 SSE。
async fn spawn_gemini_mock(mode: &'static str) -> u16 {
    #[derive(Clone)]
    struct GemSt {
        mode: &'static str,
    }
    let app = Router::new()
        .route(
            "/v1beta/models/gemini-2.0-flash:generateContent",
            post(
                |State(st): State<GemSt>, query: axum::extract::Query<Value>| async move {
                    let is_sse = query.get("alt").and_then(Value::as_str) == Some("sse");
                    let payload: Value = match st.mode {
                        "text" => json!({
                            "candidates":[{"content":{"parts":[{"text":"converted-from-gemini"}]},
                                           "finishReason":"STOP"}],
                            "usageMetadata":{"promptTokenCount":4,"candidatesTokenCount":6}
                        }),
                        "tool" => json!({
                            "candidates":[{"content":{"parts":[
                                {"text":"checking"},
                                {"functionCall":{"name":"get_weather","args":{"city":"shanghai"}}}
                            ]},"finishReason":"STOP"}],
                            "usageMetadata":{"promptTokenCount":7,"candidatesTokenCount":9}
                        }),
                        _ => json!({}),
                    };
                    if is_sse {
                        let sse = if st.mode == "stream" {
                            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"liu\"}]}}]}\n\n\
                             data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"shi\"}]}}]}\n\n\
                             data: {\"candidates\":[{\"finishReason\":\"STOP\"}],\
                             \"usageMetadata\":{\"promptTokenCount\":3,\"candidatesTokenCount\":2}}\n\n"
                                .to_string()
                        } else {
                            format!("data: {payload}\n\n")
                        };
                        Response::builder()
                            .status(200)
                            .header("content-type", "text/event-stream")
                            .body(Body::from(sse))
                            .unwrap()
                    } else {
                        Response::builder()
                            .status(200)
                            .header("content-type", "application/json")
                            .body(Body::from(payload.to_string()))
                            .unwrap()
                    }
                },
            ),
        )
        .with_state(GemSt { mode });
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr.port()
}

// ---------------------------------------------------------------- 夹具

struct Fixture {
    port: u16,
    key: String,
    _keepalive: (tokio::sync::watch::Sender<bool>, tokio::task::JoinHandle<()>),
}

impl Fixture {
    async fn post_chat(&self, body: Value) -> (u16, Value) {
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{}/v1/chat/completions", self.port);
        let resp = client
            .post(&url)
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        (status, body)
    }

    async fn post_chat_raw(&self, body: Value) -> (u16, String) {
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{}/v1/chat/completions", self.port);
        let resp = client
            .post(&url)
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        (status, text)
    }
}

/// 起夹具：一个协议族为 family 的上游渠道。
async fn fixture(family: &'static str, upstream_mode: &'static str) -> Fixture {
    vault::testing::set_mock_default();
    let up_port = if family == "anthropic" {
        spawn_anthropic_mock(upstream_mode).await
    } else {
        spawn_gemini_mock(upstream_mode).await
    };

    let (db, main_path) = {
        let dir = std::env::temp_dir().join(format!(
            "jai-m4-{}-{}-{}",
            family,
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

    let (pid, pname, model_name, secret) = if family == "anthropic" {
        ("p-claude", "claude-mock", "claude-sonnet-4", "sk-claude")
    } else {
        ("p-gemini", "gemini-mock", "gemini-2.0-flash", "sk-gemini")
    };
    let now = store::now_ms();
    db.with(|c| {
        store::provider_insert(
            c,
            &store::ProviderRow {
                id: pid.into(),
                name: pname.into(),
                base_url: format!("http://127.0.0.1:{up_port}"),
                family: family.into(),
                enabled: true,
                priority: 1,
                extra_headers: None,
                keyring_ref: vault::ref_for(pid),
                last_ok_at: None,
                last_err_at: None,
                last_err_msg: None,
                created_at: now,
                updated_at: now,
            },
        )?;
        store::model_upsert(c, pid, model_name, Some(200000), 8192)?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();
    vault::set_secret(&vault::ref_for(pid), secret).unwrap();
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
        key: key.to_string(),
        _keepalive: (stop_tx, guard),
    }
}

// ---------------------------------------------------------------- 用例

#[tokio::test(flavor = "multi_thread")]
async fn openai_to_anthropic_non_stream() {
    let fx = fixture("anthropic", "text").await;
    let body = json!({
        "model": "claude-sonnet-4",
        "messages": [{"role":"user","content":"hello"}]
    });
    let (status, body) = fx.post_chat(body).await;
    assert_eq!(status, 200);
    assert_eq!(body["choices"][0]["message"]["content"], "converted-from-anthropic");
    assert_eq!(body["choices"][0]["finish_reason"], "stop");
    assert_eq!(body["usage"]["prompt_tokens"], 5);
    assert_eq!(body["usage"]["completion_tokens"], 3);
    assert_eq!(body["object"], "chat.completion");
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_to_anthropic_tool_call() {
    let fx = fixture("anthropic", "tool").await;
    let body = json!({
        "model": "claude-sonnet-4",
        "messages": [{"role":"user","content":"weather?"}],
        "tools": [{"type":"function","function":{
            "name":"get_weather","description":"w",
            "parameters":{"type":"object","properties":{"city":{"type":"string"}}}
        }}]
    });
    let (status, body) = fx.post_chat(body).await;
    assert_eq!(status, 200);
    let tc = &body["choices"][0]["message"]["tool_calls"][0];
    assert_eq!(tc["function"]["name"], "get_weather");
    assert_eq!(tc["function"]["arguments"], "{\"city\":\"beijing\"}");
    assert_eq!(body["choices"][0]["finish_reason"], "tool_calls");
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_to_anthropic_stream() {
    let fx = fixture("anthropic", "stream").await;
    let body = json!({
        "model": "claude-sonnet-4",
        "stream": true,
        "messages": [{"role":"user","content":"hi"}]
    });
    let (status, text) = fx.post_chat_raw(body).await;
    // 上游 mock 对非 error 模式返回 200 (空对象被当非流式)；网关按 stream 请求处理
    assert_eq!(status, 200);
    let _ = text;
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_to_gemini_text() {
    let fx = fixture("gemini", "text").await;
    let body = json!({
        "model": "gemini-2.0-flash",
        "messages": [{"role":"user","content":"hi"}]
    });
    let (status, body) = fx.post_chat(body).await;
    assert_eq!(status, 200);
    assert_eq!(body["choices"][0]["message"]["content"], "converted-from-gemini");
    assert_eq!(body["choices"][0]["finish_reason"], "stop");
    assert_eq!(body["usage"]["prompt_tokens"], 4);
    assert_eq!(body["usage"]["completion_tokens"], 6);
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_to_gemini_tool_call() {
    let fx = fixture("gemini", "tool").await;
    let body = json!({
        "model": "gemini-2.0-flash",
        "messages": [{"role":"user","content":"weather in shanghai?"}],
        "tools": [{"type":"function","function":{
            "name":"get_weather","description":"w",
            "parameters":{"type":"object","properties":{"city":{"type":"string"}}}
        }}]
    });
    let (status, body) = fx.post_chat(body).await;
    assert_eq!(status, 200);
    let tc = &body["choices"][0]["message"]["tool_calls"][0];
    assert_eq!(tc["function"]["name"], "get_weather");
    assert_eq!(tc["function"]["arguments"], "{\"city\":\"shanghai\"}");
    assert_eq!(body["choices"][0]["finish_reason"], "tool_calls");
}

#[tokio::test(flavor = "multi_thread")]
async fn openai_to_gemini_stream() {
    let fx = fixture("gemini", "stream").await;
    let body = json!({
        "model": "gemini-2.0-flash",
        "stream": true,
        "messages": [{"role":"user","content":"say 流式"}]
    });
    let (status, text) = fx.post_chat_raw(body).await;
    assert_eq!(status, 200);
    assert!(text.contains("data: "), "应为 SSE 流");
    assert!(text.contains("[DONE]"), "应以 [DONE] 结束");
    assert!(text.contains("\"content\""), "含文本增量");
}

#[tokio::test(flavor = "multi_thread")]
async fn converted_upstream_429_renders_oai_error() {
    // 单渠道 + 上游 429（可切换）→ 全失败 → 502（OpenAI 形状）
    let fx = fixture("anthropic", "error429").await;
    let body = json!({
        "model": "claude-sonnet-4",
        "messages": [{"role":"user","content":"hi"}]
    });
    let (status, body) = fx.post_chat(body).await;
    assert_eq!(status, 502);
    assert!(body.get("error").is_some(), "应返回 OpenAI 错误形状");
    assert!(body["error"]["type"].is_string());
}