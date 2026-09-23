//! M13 集成测试：Retry-After 遵从（D9-T2）。
//!
//! 背景：上游 429/503 会给 `Retry-After`，但此前网关只是把它**回传给客户端**，
//! 自己切换候选时不等 —— 单轮遍历会把限流窗口二次打满（实测某上游 502 占 9.78%）。
//!
//! 用例：
//! 1. A 返回 429 + `Retry-After: 1` → B 的请求**至少晚 ~1s** 才到（真的等了）
//! 2. A 返回 429 + `Retry-After: 3600` → 等待被 CAP 截断在 5s（不挂住客户端）
//! 3. A 返回 401（认证错）→ **不等**（等多久都不会变好）
//! 4. 转换路径（Anthropic 上游）全失败 → 仍是 502（方言约束），但带上 `Retry-After`
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
use std::time::{Duration, Instant};

/// 每次被请求的时刻（按到达顺序）。
type Hits = Arc<Mutex<Vec<Instant>>>;

// ---------------------------------------------------------------- mock 上游

/// 起一个固定返回 `(status, body, retry-after?)` 的 mock 上游，并记录被调用时刻。
///
/// 同时注册两条路径：
/// - `/v1/chat/completions`：OpenAI 兼容（直通路径）
/// - `/v1/messages`：Anthropic（转换路径）
async fn spawn_mock(
    status: u16,
    retry_after: Option<&'static str>,
    tag: &'static str,
) -> (u16, Hits) {
    let hits: Hits = Arc::new(Mutex::new(Vec::new()));

    let handler = {
        let hits = hits.clone();
        move || {
            let hits = hits.clone();
            async move {
                hits.lock().unwrap().push(Instant::now());
                let body = if (200..300).contains(&status) {
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
                } else {
                    json!({"error": {
                        "message": format!("mock {status} from {tag}"),
                        "type": "invalid_request_error",
                        "code": null
                    }})
                };
                let mut b = Response::builder()
                    .status(status)
                    .header("content-type", "application/json");
                if let Some(ra) = retry_after {
                    b = b.header("retry-after", ra);
                }
                b.body(Body::from(body.to_string())).unwrap()
            }
        }
    };

    let app = Router::new()
        .route("/v1/chat/completions", post(handler.clone()))
        .route("/v1/messages", post(handler));
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

struct Fixture {
    port: u16,
    key: String,
    hits_a: Hits,
    hits_b: Hits,
    _db: Db,
    _keepalive: (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ),
}

/// 起一个完整 JAI 网关。
///
/// `family_b = None` 时只装 A 一个渠道（用于「全失败」用例）。
#[allow(clippy::too_many_arguments)]
async fn fixture(
    family_a: &'static str,
    family_b: Option<&'static str>,
    status_a: u16,
    retry_after_a: Option<&'static str>,
    status_b: u16,
) -> Fixture {
    let (port_a, hits_a) = spawn_mock(status_a, retry_after_a, "A").await;
    let (port_b, hits_b) = spawn_mock(status_b, None, "B").await;

    // 主库与日志管道必须指向同一文件，否则写入侧落不到读侧
    let (db, main_path) = {
        let dir = std::env::temp_dir().join(format!(
            "jai-m13-{}-{}",
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
    let prow = |id: &str, name: &str, prio: i64, port: u16, family: &str| store::ProviderRow {
        id: id.into(),
        name: name.into(),
        // Anthropic 上游的 base_url 不带 /v1（url_join 会补 /v1/messages）
        base_url: if family == "anthropic" {
            format!("http://127.0.0.1:{port}")
        } else {
            format!("http://127.0.0.1:{port}/v1")
        },
        family: family.into(),
        enabled: true,
        priority: prio,
        weight: 1,
        extra_headers: None,
        api_key: Some(format!("sk-{name}")),
        website: None,
        last_ok_at: None,
        last_err_at: None,
        last_err_msg: None,
        max_tools: None,
        reasoning_effort_levels: None,
        created_at: now,
        updated_at: now,
    };

    db.with(|c| {
        store::provider_insert(c, &prow("p-A", "A", 1, port_a, family_a))?;
        store::model_upsert(c, "p-A", "m-test", Some(128000), 4096, None, None)?;
        if let Some(fb) = family_b {
            store::provider_insert(c, &prow("p-B", "B", 2, port_b, fb))?;
            store::model_upsert(c, "p-B", "m-test", Some(128000), 4096, None, None)?;
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
        hits_a,
        hits_b,
        _db: db,
        _keepalive: (stop_tx, guard),
    }
}

impl Fixture {
    /// 返回 (状态码, 响应头, body)
    async fn post_chat(&self, body: Value) -> (u16, reqwest::header::HeaderMap, Value) {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let url = format!("http://127.0.0.1:{}/v1/chat/completions", self.port);
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

    /// A 的首个请求到 B 的首个请求之间的间隔（B 没被请求则为 None）
    fn gap_a_to_b(&self) -> Option<Duration> {
        let a = *self.hits_a.lock().unwrap().first()?;
        let b = *self.hits_b.lock().unwrap().first()?;
        Some(b.saturating_duration_since(a))
    }
}

fn chat_body() -> Value {
    json!({
        "model": "m-test",
        "stream": false,
        "messages": [{"role": "user", "content": "hi"}]
    })
}

// ---------------------------------------------------------------- 用例

/// T2-1：`Retry-After: 1` → 切换前真的等了约 1s（含 ±20% 抖动 → [800, 1200]ms）。
#[tokio::test(flavor = "multi_thread")]
async fn retry_after_is_honored_before_next_candidate() {
    let fx = fixture("openai_compat", Some("openai_compat"), 429, Some("1"), 200).await;
    let (status, _h, body) = fx.post_chat(chat_body()).await;

    assert_eq!(status, 200, "退避后应成功切到 B");
    assert_eq!(body["choices"][0]["message"]["content"], "from-B");

    let gap = fx.gap_a_to_b().expect("B 应被请求过");
    assert!(
        gap >= Duration::from_millis(700),
        "Retry-After: 1 应在切换前等待约 1s，实际只等了 {gap:?}"
    );
    assert!(
        gap <= Duration::from_millis(2000),
        "等待不应显著超过 1s + 20% 抖动，实际 {gap:?}"
    );
}

/// T2-2：`Retry-After: 3600` → 被 `RETRY_AFTER_CAP_MS`(5s) 截断，不挂住客户端。
#[tokio::test(flavor = "multi_thread")]
async fn retry_after_is_capped() {
    let fx = fixture(
        "openai_compat",
        Some("openai_compat"),
        429,
        Some("3600"),
        200,
    )
    .await;
    let (status, _h, _body) = fx.post_chat(chat_body()).await;
    assert_eq!(status, 200);

    let gap = fx.gap_a_to_b().expect("B 应被请求过");
    assert!(
        gap >= Duration::from_millis(3500),
        "应等到接近 CAP(5s) 再切换，实际 {gap:?}"
    );
    assert!(
        gap <= Duration::from_millis(7000),
        "CAP(5s) + 20% 抖动不应超过 6s，实际 {gap:?}"
    );
}

/// T2-3：认证错（401）不等 —— 等多久都不会变好，白等只是拖慢客户端。
#[tokio::test(flavor = "multi_thread")]
async fn auth_failure_does_not_wait() {
    // 即便上游同时给了 Retry-After，401 也不该等
    let fx = fixture("openai_compat", Some("openai_compat"), 401, Some("30"), 200).await;
    let (status, _h, _body) = fx.post_chat(chat_body()).await;
    assert_eq!(status, 200, "401 应立刻换到 B");

    let gap = fx.gap_a_to_b().expect("B 应被请求过");
    assert!(
        gap < Duration::from_millis(500),
        "认证错不应等待，实际等了 {gap:?}"
    );
}

/// T2-4：转换路径全失败 → 仍是 502（跨族必须按入站方言渲染，不能回传 Anthropic
/// 形状的 429），但退避头要带上，客户端才有退避依据。
#[tokio::test(flavor = "multi_thread")]
async fn converted_total_failure_keeps_502_but_forwards_retry_after() {
    let fx = fixture("anthropic", None, 429, Some("7"), 200).await;
    let (status, headers, body) = fx.post_chat(chat_body()).await;

    assert_eq!(status, 502, "跨族全失败仍是 502（方言约束）");
    assert!(body.get("error").is_some(), "应返回 OpenAI 错误形状");
    assert_eq!(
        headers.get("retry-after").and_then(|v| v.to_str().ok()),
        Some("7"),
        "上游的 Retry-After 不该在转换路径上丢失"
    );
}
