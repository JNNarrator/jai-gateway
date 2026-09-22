//! 推理回放自适应的**直通路径**端到端用例（`codec::replay`）。
//!
//! 与 `m5_anthropic_inbound.rs` 里的 `adaptive_reasoning_replay_*` 互补：那条走的是
//! **跨族转换**（Anthropic 入站 → openai_compat 上游，经 IR + `openai` 编码器）；
//! 这条走**同族直通**（OpenAI 入站 → openai_compat 上游，字节转发，不经编码器），
//! 直通路径的推理占位只能在原始 JSON 上补（`replay::inject_placeholders`）。
//!
//! 拓扑：OpenAI 客户端 → JAI →（同族直通）→ mock 上游（第 1 次一律 400 要求非空推理）
//!
//! 期望：**客户端只看到 200** —— 网关识别报错 → 记下该渠道 → 补占位原地重试一次。
//! 旧行为：400 原样交付给客户端（该用例会失败）。

use axum::body::Body;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// mock 上游：第 1 次一律拒；之后要求**每个** assistant 消息都带**非空**推理字段。
/// 返回 (端口, 捕获槽)，捕获槽存最后一次收到的 body（= 重试那次的 body）。
async fn spawn_strict_replay_mock() -> (u16, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(Mutex::new(Vec::new()));
    let captured2 = captured.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |axum::Json(body): axum::Json<Value>| async move {
            let n = seen.fetch_add(1, Ordering::SeqCst);
            captured2.lock().unwrap().push(body.to_string());
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

struct Fix {
    port: u16,
    key: String,
    upstream_bodies: Arc<Mutex<Vec<String>>>,
    /// 保持 stop_tx 存活：发送端全部 drop 后 watch::changed() 立即 Err → 网关立刻停机
    _keepalive: (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ),
}

impl Fix {
    /// 历史里有 assistant 轮，但**都没有推理字段**（模拟客户端裁掉历史思考）。
    async fn chat(&self) -> (u16, String) {
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{}/v1/chat/completions", self.port);
        let resp = client
            .post(&url)
            .bearer_auth(&self.key)
            .json(&json!({
                "model":"test-model-1",
                "messages":[
                    {"role":"user","content":"hi"},
                    {"role":"assistant","content":"previous answer"},
                    {"role":"user","content":"continue"}
                ]
            }))
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        (status, text)
    }
}

async fn fixture() -> Fix {
    let (up_port, upstream_bodies) = spawn_strict_replay_mock().await;

    let dir = std::env::temp_dir().join(format!(
        "jai-replay-pass-{}-{}",
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
                id: "p-rp-direct".into(),
                // 中性名字：模型名 / base_url / 供应商名都不含 deepseek ⇒ 推断不命中，
                // 只能靠「学习」这条路把推理回放打开。
                name: "relay-mock".into(),
                base_url: format!("http://127.0.0.1:{up_port}/v1"),
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
                max_tools: None,
                reasoning_effort_levels: None,
                created_at: now,
                updated_at: now,
            },
        )?;
        store::model_upsert(
            c,
            "p-rp-direct",
            "test-model-1",
            Some(200000),
            8192,
            None,
            None,
        )?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();

    let key = "sk-jai-replay-pass-0000000000";
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
        upstream_bodies,
        _keepalive: (stop_tx, guard),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn passthrough_learns_reasoning_replay_and_retries_transparently() {
    let fx = fixture().await;
    let (status, text) = fx.chat().await;
    assert_eq!(
        status, 200,
        "直通路径也应学习并重试，客户端不该看到上游那个 400：{text}"
    );
    assert!(text.contains("replay ok"), "{text}");

    let bodies = fx.upstream_bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2, "应发生两次上游调用（原请求 + 兼容重试）");
    // 第 1 次：网关还不知道该渠道要非空推理 → assistant 上没有字段
    let first: Value = serde_json::from_str(&bodies[0]).unwrap();
    let first_assistant = first["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "assistant")
        .unwrap();
    assert_eq!(
        first_assistant.get("reasoning_content"),
        None,
        "第一次不该凭空发明推理字段（网关不伪造内容）"
    );
    // 第 2 次（重试）：补上了非空占位
    let second: Value = serde_json::from_str(&bodies[1]).unwrap();
    let second_assistant = second["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "assistant")
        .unwrap();
    assert_eq!(
        second_assistant["reasoning_content"].as_str(),
        Some("[reasoning not retained for this turn]"),
        "重试那次应补上非空推理占位：{second_assistant:?}"
    );

    // 学习结果已落库（供应商级），下次请求**第一次**就带占位，不再多一次往返
    let (status2, _) = fx.chat().await;
    assert_eq!(status2, 200);
    let bodies = fx.upstream_bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 3, "第二次客户端请求应一次到位（不再重试）");
    let third: Value = serde_json::from_str(&bodies[2]).unwrap();
    let third_assistant = third["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "assistant")
        .unwrap();
    assert!(
        third_assistant
            .get("reasoning_content")
            .and_then(Value::as_str)
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "已学习的渠道后续请求应直接带占位：{third_assistant:?}"
    );
}
