//! MCP 自动合并 + 自动执行循环集成测试。
//!
//! 拓扑：
//!   客户端(OpenAI chat) → JAI 网关 → mock OpenAI 上游
//!                                        ↑ 自动调用
//!                             mock MCP HTTP Server（tools/list + tools/call）
//!
//! 场景：上游第一轮返回 `mcp_echo` 工具调用，JAI 自动执行 MCP 工具并把结果回填，
//! 第二轮上游返回最终文本，客户端只看到最终答复（非流式）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use gateway_core::vault;
use serde_json::{json, Value};

// ---------------------------------------------------------------- mock MCP

async fn spawn_mcp_mock() -> u16 {
    let app = Router::new().route(
        "/mcp",
        post(|| async move {
            // 简单起见不解析 method，直接返回 tools/list 和 tools/call 都可用同一结果；
            // 实际 `list_tools` 需要 result.tools，`call_tool` 只要求 result。
            let resp = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "tools": [{
                        "name": "mcp_echo",
                        "description": "echo tool",
                        "inputSchema": {"type":"object","properties":{"x":{"type":"string"}}}
                    }],
                    "content": [{"type":"text","text":"pong"}]
                }
            });
            Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(Body::from(resp.to_string()))
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

// ---------------------------------------------------------------- mock 上游

async fn spawn_openai_mock() -> (u16, Arc<Mutex<Option<Value>>>) {
    #[derive(Clone)]
    struct St {
        calls: Arc<AtomicUsize>,
        last_body: Arc<Mutex<Option<Value>>>,
    }

    let st = St {
        calls: Arc::new(AtomicUsize::new(0)),
        last_body: Arc::new(Mutex::new(None)),
    };

    let app = Router::new()
        .route(
            "/v1/chat/completions",
            post(|State(st): State<St>, body: axum::body::Bytes| async move {
                let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                *st.last_body.lock().unwrap() = Some(body.clone());
                let call = st.calls.fetch_add(1, Ordering::SeqCst);

                let payload = if call == 0 {
                    json!({
                        "id": "chatcmpl-mcp-1",
                        "object": "chat.completion",
                        "created": 1,
                        "model": "gpt-4o",
                        "choices": [{
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": null,
                                "tool_calls": [{
                                    "id": "call_1",
                                    "type": "function",
                                    "function": {
                                        "name": "mcp_echo",
                                        "arguments": "{\"x\":\"hi\"}"
                                    }
                                }]
                            },
                            "finish_reason": "tool_calls"
                        }],
                        "usage": {"prompt_tokens": 2, "completion_tokens": 1}
                    })
                } else {
                    json!({
                        "id": "chatcmpl-mcp-2",
                        "object": "chat.completion",
                        "created": 2,
                        "model": "gpt-4o",
                        "choices": [{
                            "index": 0,
                            "message": {"role": "assistant", "content": "done"},
                            "finish_reason": "stop"
                        }],
                        "usage": {"prompt_tokens": 4, "completion_tokens": 2}
                    })
                };
                Response::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap()
            }),
        )
        .with_state(st.clone());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr.port(), st.last_body)
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

async fn fixture() -> (Fixture, Arc<Mutex<Option<Value>>>) {
    vault::testing::set_mock_default();
    let (up_port, last_body) = spawn_openai_mock().await;
    let mcp_port = spawn_mcp_mock().await;

    let (db, main_path) = {
        let dir = std::env::temp_dir().join(format!(
            "jai-mcp-loop-{}-{}",
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
    let model = "gpt-4o";
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
                keyring_ref: vault::ref_for(pid),
                last_ok_at: None,
                last_err_at: None,
                last_err_msg: None,
                created_at: now,
                updated_at: now,
            },
        )?;
        store::model_upsert(c, pid, model, Some(200000), 8192)?;
        store::mcp_insert(
            c,
            &store::McpServerRow {
                id: "m1".into(),
                name: "mock-mcp".into(),
                kind: "http".into(),
                command: None,
                args: None,
                url: Some(format!("http://127.0.0.1:{mcp_port}/mcp")),
                env: None,
                enabled: true,
                created_at: now,
                updated_at: now,
            },
        )?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();
    vault::set_secret(&vault::ref_for(pid), "sk-openai").unwrap();
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
    (
        Fixture {
            port,
            key: key.to_string(),
            _keepalive: (stop_tx, guard),
        },
        last_body,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_tool_auto_executes_and_returns_final_answer() {
    let (fx, last_body) = fixture().await;
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{}/v1/chat/completions", fx.port);
    let resp = client
        .post(&url)
        .bearer_auth(&fx.key)
        .json(&json!({
            "model": "gpt-4o",
            "messages": [{"role":"user","content":"please echo hi"}]
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    if status != 200 {
        let text = resp.text().await.unwrap_or_default();
        panic!("expected 200, got {status}: {text}");
    }
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["choices"][0]["message"]["content"], "done");
    assert_eq!(body["choices"][0]["finish_reason"], "stop");

    // 上游第二轮请求应包含 MCP 工具执行结果（role=tool）。
    let second = last_body.lock().unwrap().clone().expect("应有第二轮请求");
    let messages = second["messages"].as_array().expect("messages array");
    let tool_msg = messages
        .iter()
        .find(|m| m["role"] == "tool")
        .expect("应存在 role=tool 消息");
    assert_eq!(tool_msg["tool_call_id"], "call_1");
    assert!(
        tool_msg["content"]
            .as_str()
            .unwrap_or_default()
            .contains("pong"),
        "MCP 结果应回填到上游第二轮"
    );
}
