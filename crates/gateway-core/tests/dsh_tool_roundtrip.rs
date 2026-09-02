//! dsh 工具调用多轮回传回归：assistant function_call（内嵌 / 数组 / 独立 items）
//! 与 function_call_output 走 Responses 入站 → Chat 上游，验证转换后 messages
//! 满足「assistant tool_calls 后必须有配对 tool 消息」的上游约束。
//!
//! 背景：历史 400「An assistant message with tool_calls must be followed by
//! tool messages responding to each tool_call_id」源于 MCP 注入时代；
//! 本测试保障移除注入后转换结构仍正确，且 function_call 数组形态可展开。

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use serde_json::{json, Value};

/// 记录收到请求体的 mock 上游。
async fn spawn_capture_mock(captured: Arc<Mutex<Vec<Value>>>) -> u16 {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |body: axum::body::Bytes| {
            let cap = captured.clone();
            async move {
                if let Ok(v) = serde_json::from_slice::<Value>(&body) {
                    cap.lock().unwrap().push(v);
                }
                Response::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "id":"chatcmpl_c","object":"chat.completion","model":"mock",
                            "choices":[{"index":0,
                                "message":{"role":"assistant","content":"转换成功"},
                                "finish_reason":"stop"}],
                            "usage":{"prompt_tokens":3,"completion_tokens":2}
                        })
                        .to_string(),
                    ))
                    .unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr.port()
}

struct Fixture {
    port: u16,
    key: String,
    _keepalive: (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ),
}

impl Fixture {
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

async fn fixture(captured: Arc<Mutex<Vec<Value>>>) -> Fixture {
    let up_port = spawn_capture_mock(captured).await;

    let (db, main_path) = {
        let dir = std::env::temp_dir().join(format!(
            "jai-tmp-dsh-{}-{}",
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
                id: "p-mock".into(),
                name: "capture-mock".into(),
                base_url: format!("http://127.0.0.1:{up_port}/v1"),
                family: "openai_compat".into(),
                enabled: true,
                priority: 1,
                weight: 1,
                extra_headers: None,
                api_key: Some("sk-mock".into()),
                website: None,
                last_ok_at: None,
                last_err_at: None,
                last_err_msg: None,
                created_at: now,
                updated_at: now,
            },
        )?;
        store::model_upsert(c, "p-mock", "dsh-model", Some(128000), 4096)?;
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

/// 校验 Chat messages：每个 assistant tool_calls 必须有后续 role=tool 配对。
fn assert_tool_pairing(msgs: &[Value]) {
    for (i, m) in msgs.iter().enumerate() {
        if let Some(tcs) = m.get("tool_calls").and_then(Value::as_array) {
            let ids: Vec<&str> = tcs.iter().filter_map(|tc| tc["id"].as_str()).collect();
            let mut found = vec![false; ids.len()];
            for after in &msgs[i + 1..] {
                if after["role"] == "tool" {
                    if let Some(tci) = after["tool_call_id"].as_str() {
                        for (j, id) in ids.iter().enumerate() {
                            if *id == tci {
                                found[j] = true;
                            }
                        }
                    }
                }
            }
            assert!(
                found.iter().all(|&f| f),
                "❌ assistant[{i}] tool_calls {ids:?} 未全部被后续 tool 消息回应\nmessages={msgs:?}"
            );
        }
    }
}

#[tokio::test]
async fn dsh_second_round_embedded_function_call() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let fx = fixture(captured.clone()).await;
    let body = json!({
        "model":"dsh-model",
        "input":[
            {"role":"user","content":[{"type":"input_text","text":"列出目录"}]},
            {"role":"assistant","content":[{"type":"output_text","text":"查看"}],
             "function_call":{"call_id":"call_00_abc|fc_0_call_00_abc","name":"bash","arguments":"{\"command\":\"pwd\"}"}},
            {"type":"function_call_output","call_id":"call_00_abc|fc_0_call_00_abc","output":"/tmp\n"},
            {"role":"user","content":[{"type":"input_text","text":"继续"}]}
        ]
    });
    let (status, resp) = fx.post_responses_raw(body).await;
    assert_eq!(status, 200, "网关应转换成功: {resp}");
    let guard = captured.lock().unwrap();
    let up = guard.last().expect("mock 应收到请求");
    let msgs = up["messages"].as_array().unwrap();
    println!("=== 变体1：内嵌 function_call，转换后 messages ===");
    for m in msgs {
        println!("  {}", serde_json::to_string(m).unwrap());
    }
    assert_tool_pairing(msgs);
}

#[tokio::test]
async fn dsh_second_round_multi_tool_calls() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let fx = fixture(captured.clone()).await;
    // 一个 assistant 消息 2 个 function_call + 2 个 output
    let body = json!({
        "model":"dsh-model",
        "input":[
            {"role":"user","content":[{"type":"input_text","text":"查一下"}]},
            {"role":"assistant","content":[],
             "function_call":[
                {"call_id":"call_00_a|fc_0_call_00_a","name":"bash","arguments":"{\"command\":\"a\"}"},
                {"call_id":"call_01_b|fc_1_call_01_b","name":"bash","arguments":"{\"command\":\"b\"}"}
             ]},
            {"type":"function_call_output","call_id":"call_00_a|fc_0_call_00_a","output":"A\n"},
            {"type":"function_call_output","call_id":"call_01_b|fc_1_call_01_b","output":"B\n"}
        ]
    });
    let (status, resp) = fx.post_responses_raw(body).await;
    assert_eq!(status, 200, "网关应转换成功: {resp}");
    let guard = captured.lock().unwrap();
    let up = guard.last().expect("mock 应收到请求");
    let msgs = up["messages"].as_array().unwrap();
    println!("=== 变体2：多工具调用，转换后 messages ===");
    for m in msgs {
        println!("  {}", serde_json::to_string(m).unwrap());
    }
    assert_tool_pairing(msgs);
}

#[tokio::test]
async fn dsh_second_round_independent_items() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let fx = fixture(captured.clone()).await;
    // function_call / function_call_output 作为独立 input items（非 assistant 内嵌）
    let body = json!({
        "model":"dsh-model",
        "input":[
            {"role":"user","content":[{"type":"input_text","text":"读文件"}]},
            {"type":"function_call","call_id":"call_07_z","name":"read","arguments":"{\"path\":\"/a\"}"},
            {"type":"function_call_output","call_id":"call_07_z","output":"content\n"}
        ]
    });
    let (status, resp) = fx.post_responses_raw(body).await;
    assert_eq!(status, 200, "网关应转换成功: {resp}");
    let guard = captured.lock().unwrap();
    let up = guard.last().expect("mock 应收到请求");
    let msgs = up["messages"].as_array().unwrap();
    println!("=== 变体3：独立 items，转换后 messages ===");
    for m in msgs {
        println!("  {}", serde_json::to_string(m).unwrap());
    }
    assert_tool_pairing(msgs);
}
