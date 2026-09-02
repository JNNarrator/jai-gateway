//! M6 集成测试：Responses API 入站（Codex 原生线）→ OpenAI 上游（roadmap M6 验收）。
//!
//! 拓扑：客户端(Responses 形状 / Codex) → JAI 网关 → mock OpenAI chat completions 上游
//!
//! 用例：
//! 1. 非流式文本
//! 2. 非流式工具调用
//! 3. 流式文本（SSE → Responses 事件流 + [DONE] 收尾）
//! 4. 模型不存在时返回 Responses 错误形状

use axum::body::Body;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use serde_json::{json, Value};

// ---------------------------------------------------------------- mock 上游

async fn spawn_openai_mock(mode: &'static str) -> u16 {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || async move {
            match mode {
                "stream" => {
                    let sse = "data: {\"id\":\"chatcmpl_s\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}]}\n\n\
                               data: {\"id\":\"chatcmpl_s\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello from responses\"},\"finish_reason\":null}]}\n\n\
                               data: {\"id\":\"chatcmpl_s\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":2}}\n\n\
                               data: [DONE]\n\n";
                    Response::builder()
                        .status(200)
                        .header("content-type", "text/event-stream")
                        .body(Body::from(sse))
                        .unwrap()
                }
                "text" => Response::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "id":"chatcmpl_x","object":"chat.completion","model":"gpt-4o",
                            "choices":[{"index":0,
                                "message":{"role":"assistant","content":"converted-from-openai"},
                                "finish_reason":"stop"}],
                            "usage":{"prompt_tokens":6,"completion_tokens":4}
                        })
                        .to_string(),
                    ))
                    .unwrap(),
                "tool" => Response::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "id":"chatcmpl_y","object":"chat.completion","model":"gpt-4o",
                            "choices":[{"index":0,
                                "message":{"role":"assistant","content":null,
                                    "tool_calls":[{"id":"call_gpt","type":"function",
                                        "function":{"name":"get_weather","arguments":"{\"city\":\"tokyo\"}"}}]},
                                "finish_reason":"tool_calls"}],
                            "usage":{"prompt_tokens":8,"completion_tokens":5}
                        })
                        .to_string(),
                    ))
                    .unwrap(),
                _ => Response::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(Body::from(json!({}).to_string()))
                    .unwrap(),
            }
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

// ---------------------------------------------------------------- 夹具

struct Fixture {
    port: u16,
    key: String,
    _keepalive: (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ),
}

impl Fixture {
    async fn post_responses(&self, body: Value) -> (u16, Value) {
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{}/v1/responses", self.port);
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

    async fn post_responses_raw(&self, body: Value) -> (u16, String) {
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{}/v1/responses", self.port);
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

async fn fixture(mode: &'static str) -> Fixture {
    let up_port = spawn_openai_mock(mode).await;

    let (db, main_path) = {
        let dir = std::env::temp_dir().join(format!(
            "jai-m6-{}-{}",
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
                id: "p-oai".into(),
                name: "openai-mock".into(),
                base_url: format!("http://127.0.0.1:{up_port}/v1"),
                family: "openai_compat".into(),
                enabled: true,
                priority: 1,
                weight: 1,
                extra_headers: None,
                api_key: Some("sk-oai".into()),
                website: None,
                last_ok_at: None,
                last_err_at: None,
                last_err_msg: None,
                created_at: now,
                updated_at: now,
            },
        )?;
        store::model_upsert(c, "p-oai", "gpt-4o", Some(128000), 4096)?;
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
    Fixture {
        port,
        key: key.to_string(),
        _keepalive: (stop_tx, guard),
    }
}

// ---------------------------------------------------------------- 用例

fn responses_body(model: &str, stream: bool) -> Value {
    json!({
        "model": model,
        "instructions": "Be concise.",
        "input": "hello from codex",
        "stream": stream,
        "max_output_tokens": 1024
    })
}

/// M6 验收：Responses 非流式文本 → OpenAI 上游 → Responses response 对象
#[tokio::test(flavor = "multi_thread")]
async fn responses_to_openai_text() {
    let fx = fixture("text").await;
    let (status, body) = fx.post_responses(responses_body("gpt-4o", false)).await;
    assert_eq!(status, 200);
    assert_eq!(body["object"], "response");
    assert_eq!(body["status"], "completed");
    assert_eq!(body["output"][0]["type"], "message");
    assert_eq!(
        body["output"][0]["content"][0]["text"],
        "converted-from-openai"
    );
    assert_eq!(body["usage"]["input_tokens"], 6);
}

/// M6 验收：Responses 工具调用 → OpenAI 上游 function_call 输出
#[tokio::test(flavor = "multi_thread")]
async fn responses_to_openai_tool_call() {
    let fx = fixture("tool").await;
    let body = json!({
        "model":"gpt-4o",
        "instructions":"Use tools",
        "input":"weather in tokyo?",
        "tools":[{"type":"function","name":"get_weather","description":"w","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}],
        "tool_choice":"auto",
        "stream":false
    });
    let (status, body) = fx.post_responses(body).await;
    assert_eq!(status, 200);
    let fc = &body["output"][0];
    assert_eq!(fc["type"], "function_call");
    assert_eq!(fc["name"], "get_weather");
    assert_eq!(fc["call_id"], "call_gpt");
    assert!(fc["arguments"].as_str().unwrap().contains("tokyo"));
}

/// M6 验收：Responses 流式文本 → Responses SSE 事件 + [DONE]
#[tokio::test(flavor = "multi_thread")]
async fn responses_to_openai_stream() {
    let fx = fixture("stream").await;
    let (status, text) = fx.post_responses_raw(responses_body("gpt-4o", true)).await;
    assert_eq!(status, 200);
    assert!(text.contains("response.created"), "应发 response.created");
    assert!(text.contains("response.output_text.delta"), "应发文本增量");
    assert!(text.contains("response.completed"), "应发 completed");
    assert!(text.contains("hello from responses"), "内容转换");
}

/// M6 验收：模型不存在时返回 Responses 错误形状
#[tokio::test(flavor = "multi_thread")]
async fn responses_model_not_found_shape() {
    let fx = fixture("text").await;
    let (status, body) = fx
        .post_responses(responses_body("no-such-model", false))
        .await;
    assert_eq!(status, 404);
    assert!(body["error"].is_object(), "Responses 错误形状: {body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("no-such-model"));
}
