//! M9 集成测试：能力声明 + 兼容性规划层（capability.rs）跨族行为。
//!
//! 拓扑：客户端(OpenAI chat / Responses) → JAI 网关 → mock 上游（anthropic / openai_compat）
//!
//! 用例：
//! 1. OpenAI 入站 json_schema → anthropic 上游：指令注入 system（合并单条）+ 非流式输出校验通过
//! 2. 同一路径上游输出非法 JSON → 502 structured_output_validation_failed
//! 3. OpenAI 入站 json_schema → openai_compat 上游：response_format 原样外传
//! 4. Responses 入站 reasoning.effort → openai_compat 上游：reasoning_effort 注入
//! 5. 工具声明超 max_tools（129）→ 400 tools_limit_exceeded

use axum::body::Body;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

// ---------------------------------------------------------------- mock 上游

#[derive(Clone, Copy)]
enum Mode {
    OpenaiText,
    OpenaiReasoning,
    /// openai_compat 上游返回 function_call（name=shell），用于扩展工具还原断言
    OpenaiToolCall,
    AnthropicValidJson,
    AnthropicInvalidJson,
}

/// 起一个上游 mock：捕获所有请求体到 shared，按 mode 返回固定响应。
/// 挂载路径：openai_compat → /chat/completions，anthropic → /v1/messages。
async fn spawn_upstream(mode: Mode, shared: Arc<Mutex<Vec<Value>>>) -> (u16, &'static str) {
    let (path, reply): (&'static str, String) = match mode {
        Mode::OpenaiText | Mode::OpenaiReasoning => (
            "/chat/completions",
            json!({
                "id":"chatcmpl_x","object":"chat.completion","model":"gpt-4o",
                "choices":[{"index":0,
                    "message":{"role":"assistant","content":"{\"temp\": 18, \"unit\": \"c\"}"},
                    "finish_reason":"stop"}],
                "usage":{"prompt_tokens":6,"completion_tokens":4}
            })
            .to_string(),
        ),
        Mode::OpenaiToolCall => (
            "/chat/completions",
            json!({
                "id":"chatcmpl_t","object":"chat.completion","model":"gpt-4o",
                "choices":[{"index":0,
                    "message":{"role":"assistant","content":null,
                        "tool_calls":[{"id":"call_s1","type":"function",
                            "function":{"name":"shell","arguments":"{\"command\":\"ls -la\"}"}}]},
                    "finish_reason":"tool_calls"}],
                "usage":{"prompt_tokens":6,"completion_tokens":4}
            })
            .to_string(),
        ),
        Mode::AnthropicValidJson => (
            "/v1/messages",
            json!({
                "id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-4",
                "content":[{"type":"text","text":"{\"temp\": 18, \"unit\": \"c\"}"}],
                "stop_reason":"end_turn",
                "usage":{"input_tokens":6,"output_tokens":4}
            })
            .to_string(),
        ),
        Mode::AnthropicInvalidJson => (
            "/v1/messages",
            json!({
                "id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-4",
                "content":[{"type":"text","text":"今天天气不错，不是 JSON"}],
                "stop_reason":"end_turn",
                "usage":{"input_tokens":6,"output_tokens":4}
            })
            .to_string(),
        ),
    };
    let shared2 = shared.clone();
    let app = Router::new().route(
        path,
        post(move |body: axum::body::Bytes| async move {
            if let Ok(v) = serde_json::from_slice::<Value>(&body) {
                shared2.lock().await.push(v);
            }
            Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(Body::from(reply.clone()))
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
    (addr.port(), path)
}

// ---------------------------------------------------------------- 夹具

struct Fixture {
    port: u16,
    key: String,
    captures: Arc<Mutex<Vec<Value>>>,
    _keepalive: (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ),
}

impl Fixture {
    /// 取 mock 收到的第一个请求体（并移除）。
    async fn upstream_body(&self) -> Value {
        let mut guard = self.captures.lock().await;
        assert!(!guard.is_empty(), "mock 应收到上游请求");
        guard.remove(0)
    }

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
}

async fn fixture(mode: Mode) -> Fixture {
    let captures = Arc::new(Mutex::new(Vec::<Value>::new()));
    let (up_port, up_path) = spawn_upstream(mode, captures.clone()).await;

    let (db, main_path) = {
        let dir = std::env::temp_dir().join(format!(
            "jai-m9-{}-{}",
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

    let family = match mode {
        Mode::AnthropicValidJson | Mode::AnthropicInvalidJson => "anthropic",
        _ => "openai_compat",
    };
    let model = if family == "anthropic" {
        "claude-sonnet-4"
    } else {
        "gpt-4o"
    };

    let now = store::now_ms();
    db.with(|c| {
        store::provider_insert(
            c,
            &store::ProviderRow {
                id: "p-up".into(),
                name: "upstream-mock".into(),
                base_url: format!("http://127.0.0.1:{up_port}"),
                family: family.into(),
                enabled: true,
                priority: 1,
                weight: 1,
                extra_headers: None,
                api_key: Some("sk-up".into()),
                website: None,
                last_ok_at: None,
                last_err_at: None,
                last_err_msg: None,
                created_at: now,
                updated_at: now,
            },
        )?;
        store::model_upsert(c, "p-up", model, Some(128000), 4096)?;
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
    // up_path 未直接使用——保留以约束 mock 挂载路径
    let _ = up_path;
    Fixture {
        port,
        key: key.to_string(),
        captures,
        _keepalive: (stop_tx, guard),
    }
}

// ---------------------------------------------------------------- 用例

fn chat_json_schema_body() -> Value {
    json!({
        "model": "claude-sonnet-4",
        "messages": [{"role": "user", "content": "what is the weather?"}],
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "weather",
                "description": "weather reply",
                "schema": {
                    "type": "object",
                    "properties": {"temp": {"type": "number"}, "unit": {"type": "string"}}
                },
                "strict": true
            }
        }
    })
}

/// M9-1：OpenAI 入站 json_schema → anthropic 上游，指令注入 + 校验通过
#[tokio::test(flavor = "multi_thread")]
async fn openai_json_schema_to_anthropic_injects_instruction_and_validates() {
    let fx = fixture(Mode::AnthropicValidJson).await;
    let (status, body) = fx.post_chat(chat_json_schema_body()).await;
    assert_eq!(status, 200, "body: {body}");

    // 上游收到：system 单条且含降级指令；响应 format 不传给 anthropic（无该参数）
    let upstream = fx.upstream_body().await;
    let system = upstream["system"].as_str().expect("anthropic system");
    assert!(
        !system.contains("response_format"),
        "anthropic 不应收到 response_format 参数"
    );
    assert!(system.contains("Return only valid JSON"));
    assert!(system.contains("JSON Schema"));

    // 校验通过 → 正常回传
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "{\"temp\": 18, \"unit\": \"c\"}"
    );
}

/// M9-2：同一路径上游输出非法 JSON → 502 校验失败（非流式降级校验）
#[tokio::test(flavor = "multi_thread")]
async fn openai_json_schema_to_anthropic_validation_failure() {
    let fx = fixture(Mode::AnthropicInvalidJson).await;
    let (status, body) = fx.post_chat(chat_json_schema_body()).await;
    assert_eq!(status, 502, "body: {body}");
    assert_eq!(body["error"]["code"], "structured_output_validation_failed");
}

/// M9-3：Responses 入站 text.format(json_schema) → openai_compat 上游：response_format 外传
/// （跨族转换路径：Responses 入站 ≠ openai_compat 上游，走 planner + openai encode）
#[tokio::test(flavor = "multi_thread")]
async fn openai_json_schema_to_openai_compat_passes_format() {
    let fx = fixture(Mode::OpenaiText).await;
    let (status, body) = fx
        .post_responses(json!({
            "model": "gpt-4o",
            "input": "what is the weather?",
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": "weather",
                    "description": "weather reply",
                    "schema": {
                        "type": "object",
                        "properties": {"temp": {"type": "number"}, "unit": {"type": "string"}}
                    },
                    "strict": true
                }
            }
        }))
        .await;
    assert_eq!(status, 200, "body: {body}");

    let upstream = fx.upstream_body().await;
    let rf = upstream["response_format"]
        .as_object()
        .expect("response_format 应外传给 openai_compat 上游");
    assert_eq!(rf["type"], "json_schema");
    assert_eq!(rf["json_schema"]["name"], "weather");
    assert_eq!(body["object"], "response");
}

/// M9-4：Responses 入站 reasoning.effort → openai_compat 上游：reasoning_effort 注入
#[tokio::test(flavor = "multi_thread")]
async fn responses_reasoning_effort_to_openai_compat() {
    let fx = fixture(Mode::OpenaiReasoning).await;
    let (status, body) = fx
        .post_responses(json!({
            "model": "gpt-4o",
            "input": "think then answer",
            "reasoning": {"effort": "high"}
        }))
        .await;
    assert_eq!(status, 200, "body: {body}");

    let upstream = fx.upstream_body().await;
    assert_eq!(upstream["reasoning_effort"], "high");
}

/// M9-5：工具声明超 max_tools → 400 tools_limit_exceeded（跨族转换路径）
#[tokio::test(flavor = "multi_thread")]
async fn too_many_tools_rejected() {
    let fx = fixture(Mode::AnthropicValidJson).await;
    let tools: Vec<Value> = (0..129u32)
        .map(|i| {
            json!({
                "type": "function",
                "function": {
                    "name": format!("tool_{i}"),
                    "description": "t",
                    "parameters": {"type": "object"}
                }
            })
        })
        .collect();
    let (status, body) = fx
        .post_chat(json!({
            "model": "claude-sonnet-4",
            "messages": [{"role": "user", "content": "hi"}],
            "tools": tools
        }))
        .await;
    assert_eq!(status, 400, "body: {body}");
    assert_eq!(body["error"]["code"], "tools_limit_exceeded");
}

/// M9-6：Responses 入站扩展工具（shell）全链路——声明/调用折叠为 function →
/// openai_compat 上游返回 function_call → 还原为 shell_call item（§10）
#[tokio::test(flavor = "multi_thread")]
async fn extended_shell_tool_folds_and_restores() {
    let fx = fixture(Mode::OpenaiToolCall).await;
    let (status, body) = fx
        .post_responses(json!({
            "model": "gpt-4o",
            "input": [
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "list files"}]},
                {"type": "shell_call", "call_id": "call_s1", "name": "bash",
                 "action": {"command": "ls -la"}}
            ],
            "tools": [
                {"type": "shell", "description": "run shell"},
                {"type": "function", "name": "get_weather",
                 "description": "w", "parameters": {"type": "object"}}
            ]
        }))
        .await;
    assert_eq!(status, 200, "body: {body}");

    // 上游侧：声明折叠（shell → function name=shell）+ 调用折叠（function_call name=shell）
    let upstream = fx.upstream_body().await;
    let tools = upstream["tools"].as_array().expect("上游应收到工具声明");
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    assert!(
        names.contains(&"shell"),
        "shell 应折叠为 function 声明: {names:?}"
    );
    assert!(names.contains(&"get_weather"));
    let tool_call = upstream["messages"]
        .as_array()
        .expect("上游应收到消息")
        .iter()
        .find_map(|m| m["tool_calls"].as_array())
        .and_then(|calls| calls.first())
        .expect("上游应收到折叠后的 function_call");
    assert_eq!(tool_call["function"]["name"], "shell");
    assert_eq!(
        tool_call["function"]["arguments"],
        "{\"command\":\"ls -la\"}"
    );

    // 回程：function_call(name=shell) → 还原为 shell_call item
    let out = body["output"].as_array().expect("响应应有 output");
    let shell = out
        .iter()
        .find(|i| i["type"] == "shell_call")
        .expect("应还原为 shell_call item");
    assert_eq!(shell["call_id"], "call_s1");
    assert_eq!(shell["action"]["command"], "ls -la");
}
