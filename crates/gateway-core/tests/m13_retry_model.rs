//! M13 集成测试：重试模型（D9-T3）—— 候选分组 + 双档预算 + 跨组规则 + 首字节 failover。
//!
//! 背景：改造前是「单轮遍历一遍候选」——没有预算、没有跨组语义，
//! 且首字节阶段的失败（响应头已到、首字节未到）被直接变成客户端可见的 502，
//! 而 `first_byte_verdict` 当年想表达的「首字节失败可切换」从未接线。
//!
//! 用例：
//! 1. **401 不跨组**：同族候选全 401 → 转换族候选零请求
//! 2. **429 跨组**：同族候选全 429 → 落到转换族候选并成功
//! 3. **405 跨组**：端点不支持（EndpointUnsupported）→ 同样允许跨组
//! 4. **组内预算**：4 个同族候选全 500 → 只试 3 个就跳组（第 4 个零请求）
//! 5. **总预算**：meta 放宽组内预算到 10、总预算 6 → 总尝试次数恰好 6
//! 6. **非幂等**：Responses + `store:true` + 上游 500 → 只尝试 1 次
//! 7. **首字节 failover**：A 建连后流立即结束（首字节前）→ 落到 B 并成功
//! 8. **首字节失败无候选可试**：保留 `upstream_empty_stream` 专属诊断码（不退化成
//!    `all_providers_failed`）
//!
//! 注意：所有出站 client 必须 `.no_proxy()` —— 本机若开系统代理（Clash 等），
//! 打本地 mock 的请求会被代理接走并回 502（见 docs/bug和优化清单.md）。

use axum::body::Body;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// 每次被请求的时刻（按到达顺序）—— 用长度即「被请求了几次」。
type Hits = Arc<Mutex<Vec<std::time::Instant>>>;

// ---------------------------------------------------------------- mock 上游

/// 上游 2xx 时回什么。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// 普通 JSON（非流式）
    Json,
    /// 完整 SSE（首字节正常）
    Sse,
    /// `text/event-stream` 但**流立即结束** —— 首字节阶段失败
    EmptySse,
}

/// 起一个 mock 上游：固定状态码 + 固定响应形状，并记录被调用次数。
///
/// 四条路径都注册（`/v1/chat/completions`、`/v1/messages`、`/responses`、
/// `/v1/completions`），这样同一个 mock 既能当同族直通上游，也能当转换上游。
async fn spawn_mock(
    status: u16,
    mode: Mode,
    family: &'static str,
    tag: &'static str,
) -> (u16, Hits) {
    let hits: Hits = Arc::new(Mutex::new(Vec::new()));

    let handler = {
        let hits = hits.clone();
        move || {
            let hits = hits.clone();
            async move {
                hits.lock().unwrap().push(std::time::Instant::now());
                if !(200..300).contains(&status) {
                    let body = json!({"error": {
                        "message": format!("mock {status} from {tag}"),
                        "type": "invalid_request_error",
                        "code": null
                    }});
                    return Response::builder()
                        .status(status)
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap();
                }
                match mode {
                    Mode::EmptySse => Response::builder()
                        .status(200)
                        .header("content-type", "text/event-stream")
                        .body(Body::empty())
                        .unwrap(),
                    Mode::Sse => {
                        let sse = if family == "anthropic" {
                            // Anthropic 事件流：message_start → content_block_delta → message_stop
                            let frames = [
                                json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"m-test","content":[],"stop_reason":null,"usage":{"input_tokens":1,"output_tokens":0}}}),
                                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":format!("from-{tag}")}}),
                                json!({"type":"content_block_stop","index":0}),
                                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}),
                                json!({"type":"message_stop"}),
                            ];
                            frames
                                .iter()
                                .map(|f| format!("event: x\ndata: {f}\n\n"))
                                .collect::<String>()
                        } else {
                            let frames = [
                                json!({"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":format!("from-{tag}")},"finish_reason":null}]}),
                                json!({"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}),
                            ];
                            frames
                                .iter()
                                .map(|f| format!("data: {f}\n\n"))
                                .collect::<String>()
                                + "data: [DONE]\n\n"
                        };
                        Response::builder()
                            .status(200)
                            .header("content-type", "text/event-stream")
                            .body(Body::from(sse))
                            .unwrap()
                    }
                    Mode::Json => {
                        let body = if family == "anthropic" {
                            json!({
                                "id": "msg_1",
                                "type": "message",
                                "role": "assistant",
                                "model": "m-test",
                                "content": [{"type": "text", "text": format!("from-{tag}")}],
                                "stop_reason": "end_turn",
                                "usage": {"input_tokens": 1, "output_tokens": 1}
                            })
                        } else {
                            json!({
                                "id": tag,
                                "object": "chat.completion",
                                "choices": [{
                                    "index": 0,
                                    "message": {"role": "assistant", "content": format!("from-{tag}")},
                                    "finish_reason": "stop"
                                }],
                                "usage": {"prompt_tokens": 1, "completion_tokens": 1}
                            })
                        };
                        Response::builder()
                            .status(200)
                            .header("content-type", "application/json")
                            .body(Body::from(body.to_string()))
                            .unwrap()
                    }
                }
            }
        }
    };

    let app = Router::new()
        .route("/v1/chat/completions", post(handler.clone()))
        .route("/v1/completions", post(handler.clone()))
        .route("/v1/messages", post(handler.clone()))
        .route("/responses", post(handler.clone()))
        // 非 anthropic 族的 base_url 带 `/v1`，url_join 后是 `/v1/responses`
        .route("/v1/responses", post(handler));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr.port(), hits)
}

// ---------------------------------------------------------------- 夹具

/// 一个渠道的规格：`(id, family, 状态码, 2xx 时的响应形状)`
type Spec = (&'static str, &'static str, u16, Mode);

struct Fixture {
    port: u16,
    key: String,
    /// 按渠道 id 索引的命中记录
    hits: std::collections::HashMap<&'static str, Hits>,
    _db: Db,
    _keepalive: (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ),
}

impl Fixture {
    /// 命中次数。
    fn hits_of(&self, id: &str) -> usize {
        self.hits
            .get(id)
            .map(|h| h.lock().unwrap().len())
            .unwrap_or(0)
    }

    /// 全部渠道的命中次数之和。
    fn hits_total(&self) -> usize {
        self.hits.values().map(|h| h.lock().unwrap().len()).sum()
    }

    /// 返回 (状态码, 响应头, body)
    async fn post(&self, path: &str, body: Value) -> (u16, reqwest::header::HeaderMap, Value) {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let url = format!("http://127.0.0.1:{}{path}", self.port);
        let resp = client
            .post(&url)
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        (status, headers, body)
    }

    /// 流式响应不是 JSON，单独取原始文本。
    async fn post_text(&self, path: &str, body: Value) -> (u16, String) {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let url = format!("http://127.0.0.1:{}{path}", self.port);
        let resp = client
            .post(&url)
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        (status, resp.text().await.unwrap_or_default())
    }
}

/// 起一个完整 JAI 网关。`specs` 里的顺序即 priority 顺序（1, 2, 3…）。
///
/// `meta` 用于覆盖重试预算（`retry_max_per_group` / `retry_max_total`）。
async fn fixture(specs: &[Spec], meta: &[(&str, &str)]) -> Fixture {
    let mut hits = std::collections::HashMap::new();
    let mut ports = Vec::new();
    for (id, family, status, mode) in specs {
        let (port, h) = spawn_mock(*status, *mode, family, id).await;
        ports.push(port);
        hits.insert(*id, h);
    }

    // 主库与日志管道必须指向同一文件，否则写入侧落不到读侧
    let (db, main_path) = {
        let dir = std::env::temp_dir().join(format!(
            "jai-m13rm-{}-{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.db");
        let p = path.to_str().unwrap().to_string();
        (Db::open(&p).unwrap(), p)
    };
    let (logs, _task) = gateway_core::store::logs::spawn_logger(&main_path).unwrap();

    let now = store::now_ms();
    db.with(|c| {
        for (i, (id, family, _status, _mode)) in specs.iter().enumerate() {
            let port = ports[i];
            let row = store::ProviderRow {
                id: (*id).into(),
                name: (*id).into(),
                // Anthropic 上游的 base_url 不带 /v1（url_join 会补 /v1/messages）
                base_url: if *family == "anthropic" {
                    format!("http://127.0.0.1:{port}")
                } else {
                    format!("http://127.0.0.1:{port}/v1")
                },
                family: (*family).into(),
                enabled: true,
                priority: i as i64 + 1,
                weight: 1,
                extra_headers: None,
                api_key: Some(format!("sk-{id}")),
                website: None,
                last_ok_at: None,
                last_err_at: None,
                last_err_msg: None,
                max_tools: None,
                reasoning_effort_levels: None,
                created_at: now,
                updated_at: now,
            };
            store::provider_insert(c, &row)?;
            store::model_upsert(c, id, "m-test", Some(128000), 4096, None, None)?;
        }
        for (k, v) in meta {
            store::meta_set(c, k, v)?;
        }
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
        hits,
        _db: db,
        _keepalive: (stop_tx, guard),
    }
}

fn chat_body() -> Value {
    json!({
        "model": "m-test",
        "stream": false,
        "messages": [{"role": "user", "content": "hi"}]
    })
}

fn chat_stream_body() -> Value {
    json!({
        "model": "m-test",
        "stream": true,
        "messages": [{"role": "user", "content": "hi"}]
    })
}

// ---------------------------------------------------------------- 用例

/// T3-1：401（渠道凭据类终态）**绝不跨组** —— 跨协议重试会让权限错误被另一个
/// 协议的成功掩盖。
#[tokio::test(flavor = "multi_thread")]
async fn channel_auth_terminal_does_not_cross_group() {
    let fx = fixture(
        &[
            ("n1", "openai_compat", 401, Mode::Json),
            ("n2", "openai_compat", 401, Mode::Json),
            ("c1", "anthropic", 200, Mode::Json),
        ],
        &[],
    )
    .await;
    let (status, _h, _b) = fx.post("/v1/chat/completions", chat_body()).await;

    assert_eq!(status, 401, "同族候选全 401 → 回最后一个上游错误（401）");
    assert_eq!(fx.hits_of("n1"), 1);
    assert_eq!(fx.hits_of("n2"), 1, "同族内应继续换候选");
    assert_eq!(
        fx.hits_of("c1"),
        0,
        "转换族候选**零请求**：凭据类错误不跨组"
    );
}

/// T3-2：429（Retryable）允许跨组。
#[tokio::test(flavor = "multi_thread")]
async fn retryable_failure_crosses_group() {
    let fx = fixture(
        &[
            ("n1", "openai_compat", 429, Mode::Json),
            ("n2", "openai_compat", 429, Mode::Json),
            ("c1", "anthropic", 200, Mode::Json),
        ],
        &[],
    )
    .await;
    let (status, _h, body) = fx.post("/v1/chat/completions", chat_body()).await;

    assert_eq!(status, 200, "同族全 429 → 应跨到转换族候选");
    assert_eq!(body["choices"][0]["message"]["content"], "from-c1");
    assert_eq!(fx.hits_of("c1"), 1);
}

/// T3-3：405（EndpointUnsupported）同样允许跨组 —— 「这个端点不接受这个方法」
/// 换个协议族也许就支持。
#[tokio::test(flavor = "multi_thread")]
async fn endpoint_unsupported_crosses_group() {
    let fx = fixture(
        &[
            ("n1", "openai_compat", 405, Mode::Json),
            ("n2", "openai_compat", 405, Mode::Json),
            ("c1", "anthropic", 200, Mode::Json),
        ],
        &[],
    )
    .await;
    let (status, _h, body) = fx.post("/v1/chat/completions", chat_body()).await;

    assert_eq!(status, 200);
    assert_eq!(body["choices"][0]["message"]["content"], "from-c1");
    assert_eq!(fx.hits_of("c1"), 1);
}

/// T3-4：组内预算（默认 3）—— 4 个同族候选全 500，只试 3 个就跳组，
/// 第 4 个零请求。
#[tokio::test(flavor = "multi_thread")]
async fn per_group_budget_caps_same_group_attempts() {
    let fx = fixture(
        &[
            ("n1", "openai_compat", 500, Mode::Json),
            ("n2", "openai_compat", 500, Mode::Json),
            ("n3", "openai_compat", 500, Mode::Json),
            ("n4", "openai_compat", 500, Mode::Json),
            ("c1", "anthropic", 200, Mode::Json),
        ],
        &[],
    )
    .await;
    let (status, _h, body) = fx.post("/v1/chat/completions", chat_body()).await;

    assert_eq!(status, 200, "组内预算耗尽后应跨组并成功");
    assert_eq!(body["choices"][0]["message"]["content"], "from-c1");
    for id in ["n1", "n2", "n3"] {
        assert_eq!(fx.hits_of(id), 1, "{id} 应被尝试");
    }
    assert_eq!(fx.hits_of("n4"), 0, "组内预算 3 → 第 4 个同族候选不该被碰");
    assert_eq!(fx.hits_of("c1"), 1);
}

/// T3-5：总预算封顶。meta 把组内预算放宽到 10、总预算设为 6：
/// 4 个同族 + 4 个转换族全 500 → 总尝试次数恰好 6（4 同族 + 2 转换族）。
#[tokio::test(flavor = "multi_thread")]
async fn total_budget_caps_overall_attempts() {
    let fx = fixture(
        &[
            ("n1", "openai_compat", 500, Mode::Json),
            ("n2", "openai_compat", 500, Mode::Json),
            ("n3", "openai_compat", 500, Mode::Json),
            ("n4", "openai_compat", 500, Mode::Json),
            ("c1", "anthropic", 500, Mode::Json),
            ("c2", "anthropic", 500, Mode::Json),
            ("c3", "anthropic", 500, Mode::Json),
            ("c4", "anthropic", 500, Mode::Json),
        ],
        &[("retry_max_per_group", "10"), ("retry_max_total", "6")],
    )
    .await;
    let (status, _h, _b) = fx.post("/v1/chat/completions", chat_body()).await;

    assert_eq!(
        status, 500,
        "直通路径全失败会原样回传最后一个上游错误（500）"
    );
    assert_eq!(fx.hits_total(), 6, "总预算 6 → 恰好尝试 6 次");
    assert_eq!(fx.hits_of("n4"), 1, "组内预算放宽到 10 → 4 个同族都被试");
    assert_eq!(fx.hits_of("c1"), 1);
    assert_eq!(fx.hits_of("c2"), 1);
    assert_eq!(fx.hits_of("c3"), 0, "总预算用尽 → 后面的转换族候选不再尝试");
    assert_eq!(fx.hits_of("c4"), 0);
}

/// T3-6：非幂等请求（Responses + `store:true`）预算压到 (1,1) —— 只尝试 1 次，
/// 不重发（重发会在上游产生第二次持久化副作用）。
#[tokio::test(flavor = "multi_thread")]
async fn non_idempotent_request_gets_single_attempt() {
    let specs: [Spec; 3] = [
        ("n1", "openai_responses", 500, Mode::Json),
        ("n2", "openai_responses", 500, Mode::Json),
        ("n3", "openai_responses", 500, Mode::Json),
    ];

    // 对照：同样的候选，不带 store → 3 个都被尝试
    let fx = fixture(&specs, &[]).await;
    let body = json!({"model": "m-test", "input": "hi", "store": false});
    let (status, _h, _b) = fx.post("/v1/responses", body).await;
    assert_eq!(status, 500, "全失败 → 原样回传最后一个上游错误");
    assert_eq!(fx.hits_total(), 3, "幂等请求应走满组内预算");

    // 带 store:true → 只 1 次
    let fx = fixture(&specs, &[]).await;
    let body = json!({"model": "m-test", "input": "hi", "store": true});
    let (status, _h, _b) = fx.post("/v1/responses", body).await;
    assert_eq!(status, 500);
    assert_eq!(
        fx.hits_total(),
        1,
        "非幂等请求只允许尝试一次（重发会产生第二次副作用）"
    );
}

/// T3-7：首字节阶段失败（响应头已到、首字节未到）允许 failover ——
/// 这正是老 `first_byte_verdict` 想表达却从未接线的语义。
#[tokio::test(flavor = "multi_thread")]
async fn stream_fails_over_before_first_byte() {
    let fx = fixture(
        &[
            ("n1", "openai_compat", 200, Mode::EmptySse),
            ("n2", "openai_compat", 200, Mode::Sse),
        ],
        &[],
    )
    .await;
    let (status, text) = fx
        .post_text("/v1/chat/completions", chat_stream_body())
        .await;

    assert_eq!(status, 200, "首字节失败应切到下一个候选，而不是回 502");
    assert!(
        text.contains("from-n2"),
        "应拿到第二个候选的流式内容，实际: {text}"
    );
    assert_eq!(fx.hits_of("n1"), 1);
    assert_eq!(fx.hits_of("n2"), 1);
}

/// T3-8：首字节失败但**没有**候选可试时，保留专属诊断码 ——
/// 别退化成笼统的 `all_providers_failed`（那会让排障信息变少）。
#[tokio::test(flavor = "multi_thread")]
async fn first_byte_failure_keeps_specific_error_code() {
    let fx = fixture(&[("n1", "openai_compat", 200, Mode::EmptySse)], &[]).await;
    let (status, text) = fx
        .post_text("/v1/chat/completions", chat_stream_body())
        .await;

    assert_eq!(status, 502);
    assert!(
        text.contains("upstream_empty_stream"),
        "应保留首字节阶段的专属诊断码，实际: {text}"
    );
}

/// T3-9：meta `(1,1)` 等价于「关闭重试」= 改造前行为 —— 一次失败即止。
#[tokio::test(flavor = "multi_thread")]
async fn budget_one_one_disables_retry() {
    let fx = fixture(
        &[
            ("n1", "openai_compat", 500, Mode::Json),
            ("n2", "openai_compat", 500, Mode::Json),
            ("c1", "anthropic", 200, Mode::Json),
        ],
        &[("retry_max_per_group", "1"), ("retry_max_total", "1")],
    )
    .await;
    let (status, _h, _b) = fx.post("/v1/chat/completions", chat_body()).await;

    assert_eq!(status, 500, "原样回传第一个候选的上游错误");
    assert_eq!(fx.hits_total(), 1, "预算 (1,1) → 只尝试第一个候选");
}
