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

            // 2026-09-22 新增：把上游实际收到的 body 原样捕获，供「多段 system 合并 /
            // output_config → reasoning_effort / thinking 历史 → reasoning_content」断言用。
            if mode == "capture" {
                *captured2.lock().unwrap() = Some(body.to_string());
                return Response::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "id":"chatcmpl_cap","object":"chat.completion","model":"gpt-4o",
                            "choices":[{"index":0,
                                "message":{"role":"assistant","content":"captured"},
                                "finish_reason":"stop"}],
                            "usage":{"prompt_tokens":1,"completion_tokens":1}
                        })
                        .to_string(),
                    ))
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

/// Anthropic 上游 mock：`capture` 模式把收到的 body 原样存入捕获槽（供「多段 system
/// 合并」断言），并回一个最小的 Anthropic message 形状让回程转换能收尾。
async fn spawn_anthropic_mock(mode: &'static str) -> (u16, Arc<Mutex<Option<String>>>) {
    let captured = Arc::new(Mutex::new(None));
    let captured2 = captured.clone();
    let app = Router::new().route(
        "/v1/messages",
        post(move |axum::Json(body): axum::Json<Value>| async move {
            if mode == "capture" {
                *captured2.lock().unwrap() = Some(body.to_string());
            }
            Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "id":"msg_cap","type":"message","role":"assistant",
                        "model":"claude-sonnet-4",
                        "content":[{"type":"text","text":"from-anthropic"}],
                        "stop_reason":"end_turn",
                        "usage":{"input_tokens":3,"output_tokens":2}
                    })
                    .to_string(),
                ))
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

/// Responses 上游 mock：`/responses`。
///
/// `stream` 模式回**真实的 Responses SSE 形状**（`response.created` /
/// `response.output_text.delta` / `response.completed` + `[DONE]`）；
/// `json` 模式**故意忽略 `stream: true`** 回整包 JSON，用来验证网关侧的补流兜底。
async fn spawn_responses_mock(mode: &'static str) -> u16 {
    let app = Router::new().route(
        "/responses",
        post(move |axum::Json(body): axum::Json<Value>| async move {
            let wants_stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
            if mode == "stream" && wants_stream {
                let mut sse = String::new();
                let created = json!({
                    "type":"response.created","sequence_number":0,
                    "response":{"id":"resp_1","model":"gpt-5"}
                });
                sse.push_str(&format!("event: response.created\ndata: {created}\n\n"));
                let delta = json!({
                    "type":"response.output_text.delta","sequence_number":1,
                    "output_index":0,"content_index":0,"delta":"from-responses"
                });
                sse.push_str(&format!(
                    "event: response.output_text.delta\ndata: {delta}\n\n"
                ));
                let completed = json!({
                    "type":"response.completed","sequence_number":2,
                    "response":{
                        "id":"resp_1","model":"gpt-5","status":"completed",
                        "output":[{"type":"message","role":"assistant",
                            "content":[{"type":"output_text","text":"from-responses"}]}],
                        "usage":{"input_tokens":7,"output_tokens":3}
                    }
                });
                sse.push_str(&format!("event: response.completed\ndata: {completed}\n\n"));
                sse.push_str("data: [DONE]\n\n");
                return Response::builder()
                    .status(200)
                    .header("content-type", "text/event-stream")
                    .body(Body::from(sse))
                    .unwrap();
            }
            // 忽略 stream=true：回整包 JSON（content-type 仍是 application/json）
            Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "id":"resp_1","object":"response","model":"gpt-5","status":"completed",
                        "output":[{"type":"message","role":"assistant",
                            "content":[{"type":"output_text","text":"from-responses"}]}],
                        "usage":{"input_tokens":7,"output_tokens":3}
                    })
                    .to_string(),
                ))
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

/// 「要求回放推理」的严格中继 mock（`openai_compat`），用于验证**自适应**闭环。
///
/// - 第 1 次请求：回 **400** + 「reasoning_content is required ...」，并记下该次的 body；
/// - 第 2 次起：若 assistant 消息都带**非空** `reasoning_content` → 200，否则继续 400。
///
/// 捕获槽里存**最后一次**收到的 body，供断言「重试那次确实补上了非空推理」。
async fn spawn_replay_mock() -> (u16, Arc<Mutex<Option<String>>>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let seen = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(Mutex::new(None));
    let captured2 = captured.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |axum::Json(body): axum::Json<Value>| async move {
            let n = seen.fetch_add(1, Ordering::SeqCst);
            *captured2.lock().unwrap() = Some(body.to_string());
            // 第 1 次一律拒（模拟「网关还不知道这个渠道要非空推理」）
            let assistants: Vec<&Value> = body
                .get("messages")
                .and_then(Value::as_array)
                .map(|ms| {
                    ms.iter()
                        .filter(|m| m.get("role").and_then(Value::as_str) == Some("assistant"))
                        .collect()
                })
                .unwrap_or_default();
            let all_have_nonempty_reasoning = !assistants.is_empty()
                && assistants.iter().all(|m| {
                    m.get("reasoning_content")
                        .and_then(Value::as_str)
                        .map(|s| !s.is_empty())
                        .unwrap_or(false)
                });
            if n == 0 || !all_have_nonempty_reasoning {
                return Response::builder()
                    .status(400)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"error":{"message":"reasoning_content is required for assistant messages"}})
                            .to_string(),
                    ))
                    .unwrap();
            }
            Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "id":"chatcmpl_rp","object":"chat.completion","model":"test-model-1",
                        "choices":[{"index":0,
                            "message":{"role":"assistant","content":"replay ok"},
                            "finish_reason":"stop"}],
                        "usage":{"prompt_tokens":5,"completion_tokens":2}
                    })
                    .to_string(),
                ))
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

async fn fixture(family: &'static str, upstream_mode: &'static str) -> Fixture {
    let (up_port, tool_capture) = if family == "openai_compat" {
        let (p, c) = spawn_openai_mock(upstream_mode).await;
        (p, Some(c))
    } else if family == "anthropic" {
        let (p, c) = spawn_anthropic_mock(upstream_mode).await;
        (p, Some(c))
    } else if family == "openai_responses" {
        (spawn_responses_mock(upstream_mode).await, None)
    } else if family == "replay" {
        let (p, c) = spawn_replay_mock().await;
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
    } else if family == "anthropic" {
        // JAI 对 anthropic 上游固定拼 `/v1/messages`，所以 base_url 只给到 host:port。
        ("p-ant", "anthropic-mock", "claude-sonnet-4", "sk-ant", "")
    } else if family == "openai_responses" {
        // JAI 对 openai_responses 上游固定拼 `/responses`，base_url 只给到 host:port。
        ("p-resp", "responses-mock", "gpt-5", "sk-resp", "")
    } else if family == "replay" {
        // 中性名字：模型名 / base_url / 供应商名都不含 deepseek ⇒ 推断不命中，
        // 只能靠「学习」这条路把推理回放打开（见 codec::replay 与本文件末尾的用例）。
        ("p-rp", "replay-mock", "test-model-1", "sk-rp", "/v1")
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
                // `replay` 只是「选哪个 mock + 用哪套中性名字」的测试开关，
                // 落库的上游族仍是 openai_compat（否则撞 family CHECK 约束）。
                family: if family == "replay" {
                    "openai_compat".into()
                } else {
                    family.into()
                },
                enabled: true,
                priority: 1,
                weight: 1,
                extra_headers: None,
                api_key: Some(secret.to_string()),
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
        store::model_upsert(c, pid, model_name, Some(128000), 4096, None, None)?;
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

/// 2026-09-22：结合 zcode 开源源码对账后修的 Anthropic 入站适配缺陷，一条请求全覆盖。
///
/// 1. **`output_config.effort` 接进档位链路** —— zcode 的 anthropic-messages 内置规则表以
///    `{"thinking":{"type":"adaptive"},"output_config":{"effort":"high"}}` 注入，此前该字段
///    在 JAI 里零引用（连 CapabilityWarn 都不产生），跨族时用户选的档位静默丢失；
/// 2. **入站 `thinking` 内容块不再丢弃** —— 跨族到 thinking 上游要靠它重建
///    `reasoning_content`，否则上游后续轮次校验 400。
///
/// 请求里另带两段 system（prompt cache 断点的常见形态）：这里只顺带守住 **openai encoder**
/// 的合并口径；「多段 system × Anthropic 上游」那条链路由本文件末尾的
/// `openai_chat_multi_system_merges_for_anthropic_upstream` 覆盖。
#[tokio::test(flavor = "multi_thread")]
async fn anthropic_inbound_output_config_and_thinking_reach_upstream() {
    let fx = fixture("openai_compat", "capture").await;
    let body = json!({
        "model":"gpt-4o","max_tokens":1024,
        "thinking":{"type":"adaptive"},
        "output_config":{"effort":"high"},
        "system":[
            {"type":"text","text":"seg-one","cache_control":{"type":"ephemeral"}},
            {"type":"text","text":"seg-two"}
        ],
        "messages":[
            {"role":"user","content":"hi"},
            {"role":"assistant","content":[
                {"type":"thinking","thinking":"prior reasoning","signature":"sig"},
                {"type":"text","text":"prior answer"}
            ]},
            {"role":"user","content":"again"}
        ]
    });
    let (status, resp) = fx.post_messages(body).await;
    assert_eq!(status, 200, "多段 system 不应再被判 400：{resp}");

    let raw = fx
        .tool_capture
        .as_ref()
        .expect("capture fixture 应提供捕获槽")
        .lock()
        .unwrap()
        .clone()
        .expect("capture 模式应捕获上游 body");
    let up: Value = serde_json::from_str(&raw).unwrap();
    let msgs = up["messages"].as_array().expect("上游应有 messages");

    // 1) 多段 system 合并为单条（\n\n 连接）
    assert_eq!(msgs[0]["role"], "system");
    assert_eq!(
        msgs[0]["content"], "seg-one\n\nseg-two",
        "多段 system 应按 IR 契约以 \\n\\n 合并"
    );

    // 2) output_config.effort → 上游 reasoning_effort（该渠道未声明值域 ⇒ 原样透传）
    assert_eq!(up["reasoning_effort"], "high");

    // 3) 入站 thinking 块 → 上游 assistant 消息的 reasoning_content
    let assistant = msgs
        .iter()
        .find(|m| m["role"] == "assistant")
        .expect("应有 assistant 消息");
    assert_eq!(
        assistant["reasoning_content"], "prior reasoning",
        "入站 thinking 块应跨族重建为 reasoning_content"
    );
    assert_eq!(assistant["content"], "prior answer");
}

/// 2026-09-22：`system` 多段 → Anthropic 上游不再被判 400。
///
/// **触发拓扑**（此前我把这条归错到了「Anthropic 入站 × OpenAI 上游」，其实不是）：
/// `anthropic::encode_request` 只在**上游族是 anthropic** 时被调用。Anthropic 入站走
/// anthropic 上游是**同族直通**（字节转发，压根不过 encoder），所以旧行为真正伤到的是
/// 「**别的入站族** × Anthropic 上游」——例如 OpenAI chat 入站：`openai::decode_request`
/// 对每条 `role=system` 消息（以及 system 的多段 content 数组）各 push 一段 `system`，
/// 于是「客户端发了两条 system」在旧代码下直接 400「system 段数为 2」，
/// 而 openai / responses / gemini 三个 encoder 都会 `join("\n\n")` 正常合并。
#[tokio::test(flavor = "multi_thread")]
async fn openai_chat_multi_system_merges_for_anthropic_upstream() {
    let fx = fixture("anthropic", "capture").await;
    let body = json!({
        "model":"claude-sonnet-4",
        "messages":[
            {"role":"system","content":"sys-one"},
            {"role":"system","content":"sys-two"},
            {"role":"user","content":"hello"}
        ]
    });
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/v1/chat/completions", fx.port))
        .bearer_auth(&fx.key)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    assert_eq!(status, 200, "多段 system 不应再被判 400：{text}");

    let raw = fx
        .tool_capture
        .as_ref()
        .expect("capture fixture 应提供捕获槽")
        .lock()
        .unwrap()
        .clone()
        .expect("capture 模式应捕获上游 body");
    let up: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        up["system"], "sys-one\n\nsys-two",
        "多段 system 应按 IR 契约以 \\n\\n 合并后发给 Anthropic 上游"
    );
}

/// 2026-09-22（PI-Desktop 源码对账）：**跨族 + Responses 上游 + 流式**此前是**静默空轮**。
///
/// `convert_streaming_response` 的 `openai_responses` 分支没有解析器，把每个 `data:` 行
/// 直接 `continue` 丢弃 ⇒ `pending_finish` 永不置位、`render_frame` 永不调用，
/// 客户端只拿到「HTTP 200 + text/event-stream + 零帧」。非流式路径反而是好的
/// （`convert_plain_response` 有 `responses::parse_response`），所以症状很隐蔽。
/// 现在按 Responses SSE 形状解析成 IR StreamEvent 再渲染回 Anthropic SSE。
#[tokio::test(flavor = "multi_thread")]
async fn anthropic_inbound_streams_from_responses_upstream() {
    let fx = fixture("openai_responses", "stream").await;
    let (status, raw) = fx
        .post_messages_raw(json!({
            "model":"gpt-5","max_tokens":64,"stream":true,
            "messages":[{"role":"user","content":"hello"}]
        }))
        .await;
    assert_eq!(status, 200, "body={raw}");
    assert!(
        raw.contains("from-responses"),
        "应收到 Responses 上游的文本增量，实际 SSE: {raw}"
    );
    assert!(
        raw.contains("message_stop"),
        "Anthropic 入站应以 message_stop 收尾，实际 SSE: {raw}"
    );
}

/// 2026-09-22：上游**忽略 `stream: true`** 回整包 JSON 时，也要给客户端一个像样的流。
///
/// 旧行为：按 `req.stream` 选了 SSE 解析器，整包里没有 `data:` 行 ⇒ 零帧 + 200。
/// 新行为：`convert_plain_response(as_stream=true)` 整包解析后用同一套渲染器补成 SSE。
#[tokio::test(flavor = "multi_thread")]
async fn streaming_client_gets_synthesized_sse_when_upstream_ignores_stream() {
    let fx = fixture("openai_responses", "json").await;
    let (status, raw) = fx
        .post_messages_raw(json!({
            "model":"gpt-5","max_tokens":64,"stream":true,
            "messages":[{"role":"user","content":"hello"}]
        }))
        .await;
    assert_eq!(status, 200, "body={raw}");
    assert!(
        raw.contains("from-responses"),
        "上游不流式时也应由网关补出内容帧，实际 SSE: {raw}"
    );
}

/// 2026-09-22（PI-Desktop 源码对账）：**自适应**推理回放闭环的端到端验证。
///
/// 拓扑：Anthropic 入站 → JAI → 严格中继（`openai_compat`，要求非空推理回放）。
/// 该渠道的模型名 / base_url / 供应商名**都不含 deepseek** ⇒ 推断不命中，
/// 所以第一次请求网关并不知道要补推理 → 上游 400「reasoning_content is required」。
/// 期望：网关**识别 → 记下该渠道 → 用兼容原地重试**，客户端只看到 200。
#[tokio::test(flavor = "multi_thread")]
async fn adaptive_reasoning_replay_learns_and_retries_transparently() {
    let fx = fixture("replay", "capture").await;
    // 历史里那条 assistant 消息**没有** thinking → 不补就是缺字段
    let (status, body) = fx
        .post_messages(json!({
            "model":"test-model-1","max_tokens":64,
            "messages":[
                {"role":"user","content":"hi"},
                {"role":"assistant","content":"previous answer"},
                {"role":"user","content":"continue"}
            ]
        }))
        .await;
    assert_eq!(
        status, 200,
        "网关应学习并重试，客户端不该看到上游那个 400：{body}"
    );
    assert_eq!(body["content"][0]["text"], "replay ok");

    // 重试那次上游确实收到了**非空** reasoning_content（占位）
    let raw = fx
        .tool_capture
        .as_ref()
        .expect("replay fixture 应提供捕获槽")
        .lock()
        .unwrap()
        .clone()
        .expect("应捕获到上游 body");
    let up: Value = serde_json::from_str(&raw).unwrap();
    let assistant = up["messages"]
        .as_array()
        .expect("应有 messages")
        .iter()
        .find(|m| m["role"] == "assistant")
        .expect("应有 assistant 消息");
    let rc = assistant["reasoning_content"].as_str().unwrap_or_default();
    assert!(
        !rc.is_empty(),
        "重试那次应补上非空推理占位，实际: {assistant:?}"
    );
}
