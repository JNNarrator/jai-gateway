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
                "stream_split_usage" => {
                    // scnet/超算 的真实末帧形状（2026-09-15 抓包）：finish_reason 帧不带 usage，
                    // usage 单独一帧且 choices 非空（只有一个空 delta）。此前该 usage 帧被整帧
                    // 丢弃 → 出站 usage 恒 0，dsh-tui 上下文占比统计不出来。
                    let sse = "data: {\"id\":\"chatcmpl_s\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello from scnet\"},\"finish_reason\":null}]}\n\n\
                               data: {\"id\":\"chatcmpl_s\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
                               data: {\"id\":\"chatcmpl_s\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{}}],\"usage\":{\"prompt_tokens\":259132,\"completion_tokens\":1805,\"total_tokens\":260937,\"prompt_tokens_details\":{\"cached_tokens\":258944}}}\n\n\
                               data: [DONE]\n\n";
                    Response::builder()
                        .status(200)
                        .header("content-type", "text/event-stream")
                        .body(Body::from(sse))
                        .unwrap()
                }
                "stream_tool" => {
                    // 并行两个工具调用（index 0/1）：真实上游只在首帧带 id，
                    // 后续 arguments 增量帧不带 —— tool_calls 落库按 id 去重计数。
                    let sse = "data: {\"id\":\"chatcmpl_t\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",\"type\":\"function\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n\
                               data: {\"id\":\"chatcmpl_t\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"city\\\":\\\"tokyo\\\"}\"}}]},\"finish_reason\":null}]}\n\n\
                               data: {\"id\":\"chatcmpl_t\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call_b\",\"type\":\"function\",\"function\":{\"name\":\"get_time\",\"arguments\":\"{}\"}}]},\"finish_reason\":null}]}\n\n\
                               data: {\"id\":\"chatcmpl_t\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":7}}\n\n\
                               data: [DONE]\n\n";
                    Response::builder()
                        .status(200)
                        .header("content-type", "text/event-stream")
                        .body(Body::from(sse))
                        .unwrap()
                }
                "stream_length" => {
                    // 输出被 max_output_tokens 截断：上游 finish_reason = "length"
                    let sse = "data: {\"id\":\"chatcmpl_l\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"cut here\"},\"finish_reason\":null}]}\n\n\
                               data: {\"id\":\"chatcmpl_l\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"length\"}],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":8}}\n\n\
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
                "text_length" => Response::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "id":"chatcmpl_l2","object":"chat.completion","model":"gpt-4o",
                            "choices":[{"index":0,
                                "message":{"role":"assistant","content":"cut here"},
                                "finish_reason":"length"}],
                            "usage":{"prompt_tokens":6,"completion_tokens":8}
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
                "stream_length_clipped_with_text" => {
                    // P2 形状：有正文，但 completion_tokens 恰好等于客户端声明的
                    // max_output_tokens（responses_body 声明 1024）→ 预算被真正用尽，
                    // 用户拿到的是被掐短的答案。
                    let sse = "data: {\"id\":\"chatcmpl_c\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"The fix is to move the let binding\"},\"finish_reason\":null}]}\n\n\
                               data: {\"id\":\"chatcmpl_c\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"length\"}],\"usage\":{\"prompt_tokens\":280726,\"completion_tokens\":1024}}\n\n\
                               data: [DONE]\n\n";
                    Response::builder()
                        .status(200)
                        .header("content-type", "text/event-stream")
                        .body(Body::from(sse))
                        .unwrap()
                }
                "stream_length_reasoning_only" => {
                    // 真机形状（2026-09-22 基元律动/deepseek-flash，460k 上下文，
                    // completion_tokens=16/96/165/250）：整轮输出预算被**推理**吃光 ——
                    // 只有 reasoning_content 增量，正文一个字都没有，上游以
                    // finish_reason="length" 收尾。客户端侧表现为
                    // `EMPTY_MODEL_RESPONSE`「模型没有产生任何输出」。
                    let sse = "data: {\"id\":\"chatcmpl_r\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"Hmm, no obvious shadow at the function top level.\"},\"finish_reason\":null}]}\n\n\
                               data: {\"id\":\"chatcmpl_r\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"Wait - line 1478 is my new code.\"},\"finish_reason\":null}]}\n\n\
                               data: {\"id\":\"chatcmpl_r\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"length\"}],\"usage\":{\"prompt_tokens\":460817,\"completion_tokens\":165}}\n\n\
                               data: [DONE]\n\n";
                    Response::builder()
                        .status(200)
                        .header("content-type", "text/event-stream")
                        .body(Body::from(sse))
                        .unwrap()
                }
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
    db: Db,
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

    /// 等待后台日志管道落库后取最近 N 条（route_mode / usage 落库断言用）
    async fn recent_logs(&self, n: i64) -> Vec<gateway_core::store::logs::LogRowView> {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        gateway_core::store::logs::logs_recent(&self.db, n).unwrap()
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
                max_tools: None,
                reasoning_effort_levels: None,
                created_at: now,
                updated_at: now,
            },
        )?;
        store::model_upsert(c, "p-oai", "gpt-4o", Some(128000), 4096, None, None)?;
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
        db,
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

    // bug 11 回归：tool_calls 必须真数（此前 emit_log_with 里写死 0，
    // 排查 MCP/工具问题时被该列误导为「模型没发起工具调用」）。
    let rows = fx.recent_logs(10).await;
    let row = rows
        .iter()
        .find(|r| r.http_status == 200)
        .expect("应有成功日志");
    assert_eq!(row.route_mode, "converted");
    assert_eq!(row.tool_calls, 1, "非流式转换应落 1 次工具调用");
}

/// bug 11 回归（流式）：并行两个工具调用必须数到 2，且 Responses 出站
/// 仍按 function_call item 渲染。
#[tokio::test(flavor = "multi_thread")]
async fn responses_to_openai_stream_tool_calls_logged() {
    let fx = fixture("stream_tool").await;
    let body = json!({
        "model":"gpt-4o",
        "instructions":"Use tools",
        "input":"weather and time in tokyo?",
        "tools":[
            {"type":"function","name":"get_weather","description":"w","parameters":{"type":"object"}},
            {"type":"function","name":"get_time","description":"t","parameters":{"type":"object"}}
        ],
        "tool_choice":"auto",
        "stream":true
    });
    let (status, text) = fx.post_responses_raw(body).await;
    assert_eq!(status, 200);
    assert!(
        text.contains("\"type\":\"function_call\"") && text.contains("get_weather"),
        "出站应含 function_call item: {text}"
    );
    assert!(
        text.contains("call_a") && text.contains("call_b"),
        "两个调用都要出现: {text}"
    );

    let rows = fx.recent_logs(10).await;
    let row = rows
        .iter()
        .find(|r| r.http_status == 200)
        .expect("应有成功日志");
    assert_eq!(row.route_mode, "converted");
    assert!(row.is_stream);
    assert_eq!(row.tool_calls, 2, "并行工具调用应数到 2（按 id 去重）");
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

/// M6 回归：上游把 stop_reason 与 usage 拆成两帧、且 usage 帧带非空 choices
/// （scnet/超算 形状，2026-09-15 抓包）时，客户端必须拿到真实 usage，
/// 且收尾帧只出现一次——补 usage 不得造成重复的 response.completed。
#[tokio::test(flavor = "multi_thread")]
async fn responses_to_openai_stream_usage_split_frames() {
    let fx = fixture("stream_split_usage").await;
    let (status, text) = fx.post_responses_raw(responses_body("gpt-4o", true)).await;
    assert_eq!(status, 200);
    assert!(text.contains("hello from scnet"), "内容转换: {text}");
    let completed = text.matches("\"type\":\"response.completed\"").count();
    assert_eq!(
        completed, 1,
        "completed 帧必须唯一，实际 {completed} 个: {text}"
    );
    let completed_line = text
        .lines()
        .find(|l| l.contains("\"type\":\"response.completed\""))
        .expect("应有 completed 帧");
    // 此前 usage 恒为 0（dsh-tui ctx 占比统计不出来的根因）
    assert!(
        completed_line.contains("\"input_tokens\":259132"),
        "input_tokens 应为真实值: {completed_line}"
    );
    assert!(
        completed_line.contains("\"output_tokens\":1805"),
        "output_tokens 应为真实值: {completed_line}"
    );
    assert!(
        completed_line.contains("\"cached_tokens\":258944"),
        "缓存读细分应保留: {completed_line}"
    );

    // 落库口径同样回归：usage 真实 + route_mode 必须标成 converted
    // （此前 emit_log 把 route_mode 硬编码为 passthrough，转换请求也记成直通，
    // 排查本次 ctx 恒 0 时正是被该字段误导）。
    let rows = fx.recent_logs(10).await;
    let row = rows
        .iter()
        .find(|r| r.http_status == 200)
        .expect("应有成功日志");
    assert_eq!(row.route_mode, "converted", "跨族转换请求必须标 converted");
    assert_eq!(row.usage_input, Some(259132), "日志 usage_input 应为真实值");
    assert_eq!(row.usage_output, Some(1805), "日志 usage_output 应为真实值");
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

/// 回归（真实事故）：上游 `finish_reason:"length"`（输出被 max_output_tokens 截断）必须
/// 带全 `status:"incomplete"` + `incomplete_details.reason:"max_output_tokens"`。
///
/// 背景：早期只把 `response.status` 写成 `incomplete` 且**从不输出** `incomplete_details`。
/// 只读 `status` 的严格客户端（PI-Desktop 的 Responses 适配器）因此把截断映射成
/// `stopReason:"error"` + 「Response incomplete without a provider reason」并标**可重试**
/// → 重发同一请求最多 10 次；JAI 日志里同一 `usage_input` 出现 10 次、每次都跑满输出上限，
/// 会话里同一句话被拼 2/4/…/176 遍。截断不是错误：OpenAI 语义里它是 incomplete + reason，
/// 客户端据此判 `stopReason:"length"`（不重试）。
///
/// 事件名**仍是** `response.completed`（刻意，见 `responses.rs::TERMINAL_EVENT`）：
/// `openai@6.x` 的 `ResponseStream` 只在 `response.completed` 上累积快照，改发
/// `response.incomplete` 会让截断响应反而丢掉 usage。所以这里反向断言「不得出现
/// response.incomplete」，把该兼容决策钉在测试里。
#[tokio::test(flavor = "multi_thread")]
async fn responses_stream_length_is_incomplete_with_reason() {
    let fx = fixture("stream_length").await;
    let (status, text) = fx.post_responses_raw(responses_body("gpt-4o", true)).await;
    assert_eq!(status, 200);
    assert!(text.contains("cut here"), "正文仍要完整下发: {text}");
    assert!(
        text.contains("\"status\":\"incomplete\""),
        "截断必须报 incomplete: {text}"
    );
    assert!(
        text.contains("\"incomplete_details\":{\"reason\":\"max_output_tokens\"}"),
        "incomplete 必须带 reason（缺 reason → 客户端当可重试错误重发 10 次）: {text}"
    );
    assert!(
        text.contains("\"type\":\"response.completed\"")
            && text.contains("event: response.completed"),
        "终局事件名保持 response.completed: {text}"
    );
    assert!(
        !text.contains("response.incomplete"),
        "不得引入 response.incomplete（openai@6.x SDK 会忽略它 → 丢 usage）: {text}"
    );

    // 落库口径：stop_reason 必须记下 max_tokens（此前该列恒 NULL，排查时看不到截断）
    let rows = fx.recent_logs(10).await;
    let row = rows
        .iter()
        .find(|r| r.http_status == 200 && r.is_stream)
        .expect("应有流式成功日志");
    assert_eq!(row.route_mode, "converted");
    assert_eq!(
        row.stop_reason.as_deref(),
        Some("max_tokens"),
        "request_logs.stop_reason 应为 max_tokens"
    );
    // 有可见正文的截断是**正常**截断，不得被标成异常（否则日志被噪音淹没）
    assert_eq!(
        row.error_kind, None,
        "有正文的 length 截断不该带 error_kind"
    );
}

/// P0 回归：**零可见输出的截断轮**必须在 `request_logs` 里被标出来。
///
/// 真机形状（2026-09-22 基元律动/deepseek-flash，460k 上下文，completion_tokens
/// 只有 16/96/165/250）：上游 `finish_reason:"length"`，整轮只有 reasoning_content，
/// 正文为空、无工具调用。客户端（PI-Desktop）据此判 silent turn → 追加
/// `<no_output_recovery>` 重跑一次 → 仍静默 → `EMPTY_MODEL_RESPONSE`，用户看到的是
/// 一句无从下手的「模型没有产生任何输出」。
///
/// 网关侧本来就能分辨这件事（`finish_reason=length` + 可见输出为 0），此前却把它记成
/// `error_kind=NULL` 的干净 200，于是 JAI 自己的日志页与统计里完全看不出异常 ——
/// 本次排查只能靠手工比对 `request_logs` 才发现。
#[tokio::test(flavor = "multi_thread")]
async fn responses_stream_length_without_visible_output_is_flagged() {
    let fx = fixture("stream_length_reasoning_only").await;
    let (status, text) = fx.post_responses_raw(responses_body("gpt-4o", true)).await;
    assert_eq!(status, 200);

    // 线上形状**不变**：仍是 incomplete + reason（截断不是错误，见
    // responses.rs::finish_status 的注释 —— 标成错误会把一次截断放大成重试风暴）
    assert!(text.contains("\"status\":\"incomplete\""), "{text}");
    assert!(
        text.contains("\"incomplete_details\":{\"reason\":\"max_output_tokens\"}"),
        "{text}"
    );
    // 本用例的上游整轮只有推理、没有正文 —— 这正是「客户端认为什么都没说」的成因
    assert!(
        !text.contains("output_text.delta"),
        "本用例上游只有推理、不应有正文增量: {text}"
    );
    assert!(
        text.contains("response.reasoning_summary_text.delta"),
        "推理增量应当照常下发: {text}"
    );

    let rows = fx.recent_logs(10).await;
    let row = rows
        .iter()
        .find(|r| r.http_status == 200 && r.is_stream)
        .expect("应有流式成功日志");
    assert_eq!(row.stop_reason.as_deref(), Some("max_tokens"));
    assert_eq!(
        row.error_kind.as_deref(),
        Some("OutputTruncatedEmpty"),
        "零可见输出的截断轮必须被标记（否则日志/统计里与正常 200 无法区分）"
    );
    let summary = row.error_summary.as_deref().unwrap_or_default();
    assert!(
        summary.contains("没有任何可见输出"),
        "摘要要说清「没有可见输出」: {summary}"
    );
    assert!(
        summary.contains("max_output_tokens=1024"),
        "摘要要带上本轮生效的输出预算: {summary}"
    );
}

/// P2（转换路径，PI-Desktop 实际走的那条）：有正文且撞上预算上限 → `OutputBudgetClipped`。
///
/// 转换路径与直通路径共用 `truncation_diagnostic`，但**接线**是各自的（转换路径取
/// `req.params.max_output_tokens` 与 IR 的 `usage.output_tokens`）—— 这里钉住接线。
#[tokio::test(flavor = "multi_thread")]
async fn responses_stream_budget_clipped_with_text_is_flagged() {
    let fx = fixture("stream_length_clipped_with_text").await;
    let (status, text) = fx.post_responses_raw(responses_body("gpt-4o", true)).await;
    assert_eq!(status, 200);
    // 线上形状不变：仍是有正文的 incomplete
    assert!(
        text.contains("The fix is to move the let binding"),
        "{text}"
    );
    assert!(text.contains("\"status\":\"incomplete\""), "{text}");

    let rows = fx.recent_logs(10).await;
    let row = rows
        .iter()
        .find(|r| r.http_status == 200 && r.is_stream)
        .expect("应有流式成功日志");
    assert_eq!(row.route_mode, "converted");
    assert_eq!(row.stop_reason.as_deref(), Some("max_tokens"));
    assert_eq!(
        row.error_kind.as_deref(),
        Some("OutputBudgetClipped"),
        "转换路径也要标出「被预算掐短」"
    );
    let summary = row.error_summary.as_deref().unwrap_or_default();
    assert!(summary.contains("max_output_tokens=1024"), "{summary}");
}

/// 非流式同口径：截断 → `status:incomplete` + `incomplete_details.reason`，
/// 且日志 stop_reason 落 `max_tokens`。
#[tokio::test(flavor = "multi_thread")]
async fn responses_non_stream_length_is_incomplete_with_reason() {
    let fx = fixture("text_length").await;
    let (status, body) = fx.post_responses(responses_body("gpt-4o", false)).await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "incomplete", "非流式也要如实报 incomplete");
    assert_eq!(
        body["incomplete_details"]["reason"], "max_output_tokens",
        "非流式也要带 reason: {body}"
    );
    assert_eq!(body["output"][0]["content"][0]["text"], "cut here");

    let rows = fx.recent_logs(10).await;
    let row = rows
        .iter()
        .find(|r| r.http_status == 200 && !r.is_stream)
        .expect("应有非流式成功日志");
    assert_eq!(row.stop_reason.as_deref(), Some("max_tokens"));
}

/// 正常结束不得带 `incomplete_details`，日志 stop_reason 记 `end_turn`。
#[tokio::test(flavor = "multi_thread")]
async fn responses_stream_stop_is_completed_end_turn() {
    let fx = fixture("stream").await;
    let (status, text) = fx.post_responses_raw(responses_body("gpt-4o", true)).await;
    assert_eq!(status, 200);
    assert!(text.contains("\"type\":\"response.completed\""));
    assert!(
        !text.contains("incomplete_details"),
        "正常结束不该带 incomplete_details: {text}"
    );
    let rows = fx.recent_logs(10).await;
    let row = rows
        .iter()
        .find(|r| r.http_status == 200 && r.is_stream)
        .expect("应有流式成功日志");
    assert_eq!(row.stop_reason.as_deref(), Some("end_turn"));
}
