//! M2 集成测试：多渠道故障转移真实链路（roadmap M2 验收 1/2/3）。
//!
//! 拓扑：
//!   mock 上游 A（一级渠道） ←─┐
//!                             ├─ JAI 网关（真实 build_router）
//!   mock 上游 B（二级渠道） ←─┘
//!
//! 用例：
//! 1. 一级恒 500 → 请求成功，命中二级渠道（日志 provider_id=p-B）
//! 2. 反转 priority → 命中 A
//! 3. response_format 类确定性 400 不触发切换（A 恒 400 → 原样 400 返回，B 不被尝试）
//! 4. 流式请求同样顺延

use axum::body::Body;
use axum::routing::post;
use axum::Router;
use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use serde_json::{json, Value};

// ---------------------------------------------------------------- mock 上游

/// 起一个返回固定(状态码, 形状化 body)的 mock chat completions 服务。
/// 4xx 返回 OpenAI 错误形状，2xx 返回 chat.completion 形状。返回实际端口。
async fn spawn_mock(status: u16, tag: &'static str) -> u16 {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || async move {
            let body = if (400..500).contains(&status) {
                json!({"error": {"message": format!("mock {status} from {tag}"),
                                  "type": "invalid_request_error", "code": null}})
            } else {
                json!({"id": tag, "object": "chat.completion", "choices": [{"index":0,
                    "message":{"role":"assistant","content":format!("from-{tag}")},
                    "finish_reason":"stop"}],
                    "usage":{"prompt_tokens":1,"completion_tokens":1}})
            };
            Response::builder()
                .status(status)
                .header("x-mock-tag", tag)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
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

use axum::response::Response;

// ---------------------------------------------------------------- 夹具

/// 起一个完整 JAI 网关（临时文件 DB + mock keyring + 两个 openai_compat 渠道），
/// 返回夹具：网关端口、Db（可查日志）、网关密钥。
struct Fixture {
    port: u16,
    db: Db,
    key: String,
    /// 持有 stop 信号 + 服务器任务：drop 时随测试进程结束
    _keepalive: (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ),
}

async fn fixture(priority_a: i64, priority_b: i64, status_a: u16, status_b: u16) -> Fixture {

    let port_a = spawn_mock(status_a, "A").await;
    let port_b = spawn_mock(status_b, "B").await;

    // 主库与日志管道必须指向同一文件，否则 logs_recent 查不到写入侧
    let (db, main_path) = {
        let dir = std::env::temp_dir().join(format!(
            "jai-m2-main-{}-{}",
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
    // 渠道 A / B 均 openai_compat，同名模型 "m-test"
    let prow = |name: &str, prio: i64, port: u16| store::ProviderRow {
        id: format!("p-{name}"),
        name: name.into(),
        base_url: format!("http://127.0.0.1:{port}/v1"),
        family: "openai_compat".into(),
        enabled: true,
        priority: prio,
        weight: 1,
        extra_headers: None,
        api_key: Some(format!("sk-{name}")),
        website: None,
        last_ok_at: None,
        last_err_at: None,
        last_err_msg: None,
        created_at: now,
        updated_at: now,
    };
    db.with(|c| {
        store::provider_insert(c, &prow("A", priority_a, port_a))?;
        store::provider_insert(c, &prow("B", priority_b, port_b))?;
        store::model_upsert(c, "p-A", "m-test", Some(128000), 4096)?;
        store::model_upsert(c, "p-B", "m-test", Some(128000), 4096)?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();

    // 写密钥环凭据（mock）
    

    // 网关密钥
    let key = "sk-jai-integration-test-0000000000000000";
    db.with(|c| {
        store::gw_key_rotate(c, key, Some("test"))?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();

    // 起网关
    let ctx = GatewayCtx::new(db.clone(), logs);
    let app = server::build_router(ctx);
    let (listener, port) = server::bind_with_fallback("127.0.0.1", 0).unwrap();
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let guard = tokio::spawn(async move {
        let _ = server::run_until_shutdown(listener, app, stop_rx).await;
    });

    Fixture {
        port,
        db,
        key: key.to_string(),
        _keepalive: (stop_tx, guard),
    }
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
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        (status, body)
    }

    /// 等待后台日志管道落库后取最近 N 条
    async fn recent_logs(&self, n: i64) -> Vec<gateway_core::store::logs::LogRowView> {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        gateway_core::store::logs::logs_recent(&self.db, n).unwrap()
    }
}

// ---------------------------------------------------------------- 用例

fn chat_body(stream: bool) -> Value {
    json!({
        "model": "m-test",
        "stream": stream,
        "messages": [{"role":"user","content":"hi"}]
    })
}

/// 验收 1：一级渠道恒 500 → 请求成功，命中二级渠道
#[tokio::test(flavor = "multi_thread")]
async fn primary_500_fails_over_to_secondary() {
    let fx = fixture(1, 2, 500, 200).await;
    let (status, body) = fx.post_chat(chat_body(false)).await;
    assert_eq!(status, 200, "故障转移后应成功");
    assert_eq!(body["choices"][0]["message"]["content"], "from-B");

    // 日志记录二级渠道命中（provider_id = p-B）
    let rows = fx.recent_logs(20).await;
    let ok_row = rows
        .iter()
        .find(|r| r.http_status == 200)
        .expect("有成功日志");
    assert_eq!(ok_row.provider_id.as_deref(), Some("p-B"));
    assert_eq!(ok_row.route_mode, "passthrough");
}

/// 验收 2：反转 priority 后行为随之反转（B 高优先但挂掉 → 低优先 A 兜底）
#[tokio::test(flavor = "multi_thread")]
async fn priority_reversal_flips_winner() {
    let fx = fixture(2, 1, 200, 500).await;
    let (status, body) = fx.post_chat(chat_body(false)).await;
    assert_eq!(status, 200);
    assert_eq!(body["choices"][0]["message"]["content"], "from-A");

    let rows = fx.recent_logs(20).await;
    let ok_row = rows
        .iter()
        .find(|r| r.http_status == 200)
        .expect("有成功日志");
    assert_eq!(ok_row.provider_id.as_deref(), Some("p-A"));
}

/// 验收 3：确定性 400 不触发切换，原样返回 400；B 渠道不应被尝试
#[tokio::test(flavor = "multi_thread")]
async fn deterministic_400_stops_without_failover() {
    let fx = fixture(1, 2, 400, 200).await; // A 400、B 正常 → 不切换！
    let (status, body) = fx.post_chat(chat_body(false)).await;
    assert_eq!(status, 400, "确定性错误不切换渠道");
    assert!(body.get("error").is_some(), "应返回 OpenAI 错误形状");

    let rows = fx.recent_logs(20).await;
    assert!(
        !rows.iter().any(|r| r.provider_id.as_deref() == Some("p-B")),
        "B 渠道不应被尝试"
    );
    // 且没有任何成功日志
    assert!(!rows.iter().any(|r| r.http_status == 200), "不应有成功记录");
}

/// 补充：流式请求走同一故障转移语义
#[tokio::test(flavor = "multi_thread")]
async fn stream_request_fails_over_before_first_byte() {
    let fx = fixture(1, 2, 500, 200).await;
    let (status, body) = fx.post_chat(chat_body(true)).await;
    assert_eq!(status, 200);
    assert!(body["choices"][0]["message"]["content"].is_string());
}
