//! M5 集成测试：Anthropic 入站 × OpenAI/Gemini 上游（roadmap M5 验收）。
//!
//! 拓扑：客户端(Anthropic 形状 / Claude Code) → JAI 网关 → mock OpenAI / mock Gemini 上游
//!
//! 用例：
//! 1. Anthropic 入站 → OpenAI 上游 非流式（文本）
//! 2. Anthropic 入站 → OpenAI 上游 非流式（工具调用）
//! 3. Anthropic 入站 → Gemini 上游 非流式（文本）
//! 4. Anthropic 入站 → Gemini 上游 流式（SSE 事件转换 + message_stop 收尾）

use axum::body::Body;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use gateway_core::vault;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------- mock 上游

/// OpenAI mock：读取请求验证转换正确性（由 codec 单测保证），返回固定响应。
/// `tool_long` 模式会捕获第二次请求里的 `tool_call_id`，用于 M5 超长 id 映射验收。
async fn spawn_openai_mock(mode: &'static str) -> (u16, Arc<Mutex<Option<String>>>) {
    let captured = Arc::new(Mutex::new(None));
    let captured2 = captured.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |axum::Json(body): axum::Json<Value>| async move {
            // 流式：完整 SSE 体（单 chunk 场景，同时守护首字节行缓冲不丢）
            if mode == "stream" {
                let sse = "data: {\"id\":\"chatcmpl_s\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}]}\n\n\
                           data: {\"id\":\"chatcmpl_s\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi \"},\"finish_reason\":null}]}\n\n\
                           data: {\"id\":\"chatcmpl_s\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"from-openai\"},\"finish_reason\":null}]}\n\n\
                           data: {\"id\":\"chatcmpl_s\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\n\
                           data: [DONE]\n\n";
                return Response::builder()
                    .status(200)
                    .header("content-type", "text/event-stream")
                    .body(Body::from(sse))
                    .unwrap();
            }

            let payload: Value = if mode == "tool_long" {
                // 第二轮：如果上游收到 role=tool，捕获 tool_call_id 并返回普通文本
                if let Some(msgs) = body.get("messages").and_then(Value::as_array) {
                    for m in msgs {
                        if m.get("role").and_then(Value::as_str) == Some("tool") {
                            if let Some(id) = m.get("tool_call_id").and_then(Value::as_str) {
                                *captured2.lock().unwrap() = Some(id.to_string());
                            }
                            return Response::builder()
                                .status(200)
                                .header("content-type", "application/json")
                                .body(Body::from(
                                    json!({
                                        "id":"chatcmpl_lt","object":"chat.completion","model":"gpt-4o",
                                        "choices":[{"index":0,
                                            "message":{"role":"assistant","content":"result accepted"},
                                            "finish_reason":"stop"}],
                                        "usage":{"prompt_tokens":2,"completion_tokens":1}
                                    })
                                    .to_string(),
                                ))
                                .unwrap();
                        }
                    }
                }
                // 第一轮：返回一个超长 tool_use id，迫使网关落到 tool_id_map
                let long_id = format!("call_{}", "x".repeat(80));
                json!({
                    "id":"chatcmpl_lt","object":"chat.completion","model":"gpt-4o",
                    "choices":[{"index":0,
                        "message":{"role":"assistant","content":null,
                            "tool_calls":[{"id":long_id,"type":"function",
                                "function":{"name":"get_weather","arguments":"{\"city\":\"long\"}"}}]},
                        "finish_reason":"tool_calls"}],
                    "usage":{"prompt_tokens":8,"completion_tokens":5}
                })
            } else if mode == "text" {
                json!({
                    "id":"chatcmpl_x","object":"chat.completion","model":"gpt-4o",
                    "choices":[{"index":0,
                        "message":{"role":"assistant","content":"converted-from-openai"},
                        "finish_reason":"stop"}],
                    "usage":{"prompt_tokens":6,"completion_tokens":4}
                })
            } else if mode == "tool" {
                json!({
                    "id":"chatcmpl_y","object":"chat.completion","model":"gpt-4o",
                    "choices":[{"index":0,
                        "message":{"role":"assistant","content":null,
                            "tool_calls":[{"id":"call_gpt","type":"function",
                                "function":{"name":"get_weather","arguments":"{\"city\":\"tokyo\"}"}}]},
                        "finish_reason":"tool_calls"}],
                    "usage":{"prompt_tokens":8,"completion_tokens":5}
                })
            } else {
                json!({})
            };
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
    (addr.port(), captured)
}

/// Gemini mock：文本 + 流式。
async fn spawn_gemini_mock(mode: &'static str) -> u16 {
    #[derive(Clone)]
    struct GemSt {
        mode: &'static str,
    }
    let app = Router::new()
        .route(
            "/v1beta/models/gemini-2.0-flash:generateContent",
            post(
                |axum::extract::State(st): axum::extract::State<GemSt>,
                 query: axum::extract::Query<Value>| async move {
                    let is_sse = query.get("alt").and_then(Value::as_str) == Some("sse");
                    if is_sse {
                        let sse = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"from-gemini-1\"}]}}]}\n\n\
                                   data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"from-gemini-2\"}]}}]}\n\n\
                                   data: {\"candidates\":[{\"finishReason\":\"STOP\"}],\
                                   \"usageMetadata\":{\"promptTokenCount\":3,\"candidatesTokenCount\":4}}\n\n";
                        Response::builder()
                            .status(200)
                            .header("content-type", "text/event-stream")
                            .body(Body::from(sse))
                            .unwrap()
                    } else {
                        let payload: Value = match st.mode {
                            "text" => json!({
                                "candidates":[{"content":{"parts":[{"text":"from-gemini"}]},
                                               "finishReason":"STOP"}],
                                "usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":6}
                            }),
                            _ => json!({}),
                        };
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
    /// 仅 OpenAI `tool_long` mock 使用：第二轮收到的上游 tool_call_id。
    tool_capture: Option<Arc<Mutex<Option<String>>>>,
    _keepalive: (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ),
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
        let text = resp.text().await.unwrap_or_default();
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        (status, body)
    }

    async fn post_messages_raw(&self, body: Value) -> (u16, String) {
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
        let text = resp.text().await.unwrap_or_default();
        (status, text)
    }
}

async fn fixture(family: &'static str, upstream_mode: &'static str) -> Fixture {
    vault::testing::set_mock_default();
    let (up_port, tool_capture) = if family == "openai_compat" {
        let (p, c) = spawn_openai_mock(upstream_mode).await;
        (p, Some(c))
    } else {
        (spawn_gemini_mock(upstream_mode).await, None)
    };

    let (db, main_path) = {
        let dir = std::env::temp_dir().join(format!(
            "jai-m5-{}-{}-{}",
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

    let (pid, pname, model_name, secret, base_suffix) = if family == "openai_compat" {
        ("p-oai", "openai-mock", "gpt-4o", "sk-oai", "/v1")
    } else {
        ("p-gem", "gemini-mock", "gemini-2.0-flash", "sk-gem", "")
    };
    let now = store::now_ms();
    db.with(|c| {
        store::provider_insert(
            c,
            &store::ProviderRow {
                id: pid.into(),
                name: pname.into(),
                base_url: format!("http://127.0.0.1:{up_port}{base_suffix}"),
                family: family.into(),
                enabled: true,
                priority: 1,
                weight: 1,
                extra_headers: None,
                keyring_ref: vault::ref_for(pid),
                last_ok_at: None,
                last_err_at: None,
                last_err_msg: None,
                created_at: now,
                updated_at: now,
            },
        )?;
        store::model_upsert(c, pid, model_name, Some(128000), 4096)?;
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
        tool_capture,
        _keepalive: (stop_tx, guard),
    }
}

// ---------------------------------------------------------------- 用例

fn anth_messages_body(model: &str, stream: bool) -> Value {
    json!({
        "model": model,
        "max_tokens": 1024,
        "stream": stream,
        "messages": [{"role":"user","content":"hello from claude code"}]
    })
}

/// M5 验收：Claude Code 以 Anthropic 形状请求 → OpenAI 上游文本
#[tokio::test(flavor = "multi_thread")]
async fn anthropic_inbound_to_openai_text() {
    let fx = fixture("openai_compat", "text").await;
    let (status, body) = fx.post_messages(anth_messages_body("gpt-4o", false)).await;
    if status != 200 {
        panic!("status={status} body={body}");
    }
    assert_eq!(body["type"], "message", "Anthropic 响应形状");
    assert_eq!(body["content"][0]["text"], "converted-from-openai");
    assert_eq!(body["stop_reason"], "end_turn");
    assert_eq!(body["usage"]["input_tokens"], 6);
    assert_eq!(body["usage"]["output_tokens"], 4);
}

/// M5 验收：Anthropic 入站 × OpenAI 上游工具调用（Claude Code 发起工具）
#[tokio::test(flavor = "multi_thread")]
async fn anthropic_inbound_to_openai_tool_call() {
    let fx = fixture("openai_compat", "tool").await;
    let body = json!({
        "model":"gpt-4o","max_tokens":1024,
        "tools":[{"name":"get_weather","description":"w",
                  "input_schema":{"type":"object","properties":{"city":{"type":"string"}}}}],
        "tool_choice":{"type":"auto"},
        "messages":[{"role":"user","content":"weather in tokyo?"}]
    });
    let (status, body) = fx.post_messages(body).await;
    assert_eq!(status, 200);
    assert_eq!(body["stop_reason"], "tool_use", "工具调用原因");
    let tc = &body["content"][0]; // tool_use 块
    assert_eq!(tc["type"], "tool_use");
    assert_eq!(tc["name"], "get_weather");
    assert_eq!(tc["input"]["city"], "tokyo");
    // toolu_ 前缀编码（Anthropic 客户端可回传）
    assert!(tc["id"].as_str().unwrap().starts_with("toolu_"));
}

/// M5 验收：Anthropic 入站 × Gemini 上游文本
#[tokio::test(flavor = "multi_thread")]
async fn anthropic_inbound_to_gemini_text() {
    let fx = fixture("gemini", "text").await;
    let (status, body) = fx
        .post_messages(anth_messages_body("gemini-2.0-flash", false))
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["type"], "message");
    assert_eq!(body["content"][0]["text"], "from-gemini");
    assert_eq!(body["usage"]["input_tokens"], 5);
    assert_eq!(body["usage"]["output_tokens"], 6);
}

/// M5 验收：Anthropic 入站 × Gemini 上游 流式（SSE → Anthropic 事件流 + message_stop 收尾）
#[tokio::test(flavor = "multi_thread")]
async fn anthropic_inbound_to_gemini_stream() {
    let fx = fixture("gemini", "stream").await;
    let (status, text) = fx
        .post_messages_raw(anth_messages_body("gemini-2.0-flash", true))
        .await;
    if status != 200 || !text.contains("content_block_delta") {
        panic!("status={status} text={text:?}");
    }
    assert!(
        text.contains("event: message_start"),
        "Anthropic SSE 起始事件"
    );
    assert!(text.contains("event: content_block_delta"), "文本增量事件");
    assert!(text.contains("message_stop"), "message_stop 收尾");
    assert!(text.contains("from-gemini-1"), "内容转换");
    assert!(text.contains("from-gemini-2"), "内容转换 2");
}

/// M5 验收：Anthropic 入站 × OpenAI 上游 流式（单 chunk 首字节不丢 + OpenAI SSE 解析）
#[tokio::test(flavor = "multi_thread")]
async fn anthropic_inbound_to_openai_stream() {
    let fx = fixture("openai_compat", "stream").await;
    let (status, text) = fx
        .post_messages_raw(anth_messages_body("gpt-4o", true))
        .await;
    if status != 200 {
        panic!("status={status} text={text:?}");
    }
    assert!(
        text.contains("event: message_start"),
        "Anthropic SSE 起始事件"
    );
    assert!(text.contains("event: content_block_delta"), "文本增量事件");
    assert!(text.contains("event: message_delta"), "终局 usage 补齐事件");
    assert!(text.contains("message_stop"), "message_stop 收尾");
    assert!(text.contains("hi "), "OpenAI 文本增量 1");
    assert!(text.contains("from-openai"), "OpenAI 文本增量 2");
}

/// M5 验收：超长 tool id 回落 tool_id_map —— 首轮短 id 返回，二轮按短 id 回传时
/// 网关应还原成上游原始长 id。
#[tokio::test(flavor = "multi_thread")]
async fn anthropic_inbound_to_openai_long_tool_id_roundtrip() {
    let fx = fixture("openai_compat", "tool_long").await;
    let first = json!({
        "model":"gpt-4o","max_tokens":1024,
        "tools":[{"name":"get_weather","description":"w",
                  "input_schema":{"type":"object","properties":{"city":{"type":"string"}}}}],
        "tool_choice":{"type":"auto"},
        "messages":[{"role":"user","content":"weather in long id?"}]
    });
    let (status, body) = fx.post_messages(first).await;
    assert_eq!(status, 200);
    let id = body["content"][0]["id"]
        .as_str()
        .expect("tool_use id")
        .to_string();
    assert!(id.starts_with("toolu_"), "应使用 toolu_ 前缀: {id}");
    assert!(
        id.len() <= 64,
        "超长 id 应被映射为 ≤64 字符，实际 {} 字符: {id}",
        id.len()
    );

    let second = json!({
        "model":"gpt-4o","max_tokens":1024,
        "tools":[{"name":"get_weather","description":"w",
                  "input_schema":{"type":"object","properties":{"city":{"type":"string"}}}}],
        "tool_choice":{"type":"auto"},
        "messages":[
            {"role":"user","content":"weather in long id?"},
            {"role":"assistant","content":[
                {"type":"text","text":"checking"},
                {"type":"tool_use","id":id,"name":"get_weather","input":{"city":"long"}}
            ]},
            {"role":"user","content":[
                {"type":"tool_result","tool_use_id":id,
                 "content":[{"type":"text","text":"done"}]}
            ]}
        ]
    });
    let (status, _) = fx.post_messages(second).await;
    assert_eq!(status, 200, "二轮回传应正常完成");
    let expected = format!("call_{}", "x".repeat(80));
    let captured = fx
        .tool_capture
        .as_ref()
        .expect("tool_long fixture 应提供捕获槽")
        .lock()
        .unwrap()
        .clone();
    assert_eq!(
        captured.as_deref(),
        Some(expected.as_str()),
        "上游应收到原始长 id，而不是网关短 id"
    );
}
