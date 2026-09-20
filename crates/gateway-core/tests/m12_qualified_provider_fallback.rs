//! M12 集成测试：限定名 `供应商/模型` 的**优先**语义（`proxy.rs` dispatch 路由段）。
//!
//! 背景（2026-09-20）：旧实现是 `candidates.retain(|c| c.provider_name == …)`，
//! 即「限定名 = 只用这一家」—— 等于把故障转移**彻底关掉**。而 JAI `/v1/models`
//! 推荐的正是限定名（Reasonix 就是这么选的），于是该上游一抖动就**没有任何退路**
//! （实测该上游 502 占 9.78%，当天 20%）。
//!
//! 现语义：指定供应商**优先**，其余同模型候选保留为**后备**；且**健康优先于指定** ——
//! 指定渠道已知不健康时不去抢健康备渠道的位置，否则每次都要先撞一次已知失败。
//!
//! 拓扑（注意 **A 的优先级刻意更差**，否则「没实现优先」也能碰巧选对，断言会失效）：
//!   mock A（限定名指定的那家，priority 200） ←─┐
//!                                              ├─ JAI 网关（真实 build_router）
//!   mock B（另一家，priority 100）          ←─┘
//!
//! 用例：
//! 1. A 恒 500 → 限定名 `A/m-q` 仍成功，落到 B（优先不等于不转移）
//! 2. 同一夹具再发一次 → A 已被标记不健康 → 直接走 B，**A 不再被尝试**（健康优先于指定）
//! 3. A 与 B 都 200 → 用 **A**（优先压过更优的 priority），B 一次都不被碰
//! 4. 限定名的供应商名对不上任何渠道 → **404**（保留旧语义：打错名不该静默换家）

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use serde_json::{json, Value};

// ---------------------------------------------------------------- mock 上游

/// 计数 mock 上游：固定返回 (status, tag)，并统计收到的请求数。
async fn spawn_counting_mock(status: u16, tag: &'static str) -> (u16, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let h = h.clone();
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                let body = if (400..500).contains(&status) {
                    json!({"error": {"message": format!("mock {status} from {tag}"),
                                     "type": "invalid_request_error", "code": null}})
                } else if status >= 500 {
                    json!({"error": {"message": format!("mock {status} from {tag}"),
                                     "type": "api_error", "code": null}})
                } else {
                    json!({"id": tag, "object": "chat.completion",
                        "choices": [{"index": 0,
                            "message": {"role": "assistant", "content": format!("from-{tag}")},
                            "finish_reason": "stop"}],
                        "usage": {"prompt_tokens": 1, "completion_tokens": 1}})
                };
                Response::builder()
                    .status(status)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (port, hits)
}

// ---------------------------------------------------------------- 夹具

/// 两家供应商 + 同名模型 `m-q`。**A 的 priority 刻意比 B 差**（200 vs 100），
/// 这样「限定名优先」若没生效，请求必然落到 B，用例会立刻变红。
struct Fixture {
    port: u16,
    key: String,
    _keepalive: (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ),
}

async fn fixture(port_a: u16, port_b: u16) -> Fixture {
    let (db, main_path) = {
        let dir = std::env::temp_dir().join(format!(
            "jai-m12-main-{}-{}",
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
        max_tools: None,
        reasoning_effort_levels: None,
        created_at: now,
        updated_at: now,
    };
    db.with(|c| {
        // A 是「限定名指定的那家」，但 priority 更差（200 > 100）
        store::provider_insert(c, &prow("A", 200, port_a))?;
        store::provider_insert(c, &prow("B", 100, port_b))?;
        store::model_upsert(c, "p-A", "m-q", Some(128000), 4096, None, None)?;
        store::model_upsert(c, "p-B", "m-q", Some(128000), 4096, None, None)?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();

    let key = "sk-jai-integration-test-qualified-name";
    db.with(|c| {
        store::gw_key_rotate(c, key, Some("test"))?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();

    let ctx = GatewayCtx::new(db.clone(), logs);
    // 显式关代理：本机若开系统代理，打本地 mock 的请求会被代理接走并回 HTTP 502，
    // 失败形态就不是我们要验的那条路径了（详见 m11 的同名说明）。
    let mut ctx = ctx;
    ctx.http = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
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

impl Fixture {
    /// 用**指定 model 名**发一次 chat 请求，返回 (状态码, 上游回的内容或错误摘要)
    async fn chat(&self, model: &str) -> (u16, String) {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let url = format!("http://127.0.0.1:{}/v1/chat/completions", self.port);
        let resp = client
            .post(&url)
            .bearer_auth(&self.key)
            .json(&json!({"model": model, "stream": false,
                          "messages": [{"role": "user", "content": "hi"}]}))
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        let text = body["choices"][0]["message"]["content"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                body["error"]["message"]
                    .as_str()
                    .unwrap_or("<无内容>")
                    .to_string()
            });
        (status, text)
    }
}

/// 等后台日志/健康标记落库，避免断言撞竞态。
async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
}

// ---------------------------------------------------------------- 用例

/// 用例 1+2：指定渠道挂了 → 回退到备渠道；且备渠道随即被健康逻辑接管。
#[tokio::test(flavor = "multi_thread")]
async fn qualified_name_falls_back_when_named_provider_down() {
    let (port_a, hits_a) = spawn_counting_mock(500, "A").await;
    let (port_b, hits_b) = spawn_counting_mock(200, "B").await;
    let fx = fixture(port_a, port_b).await;

    // 第 1 次：A 健康且被指定 → 先撞 A（500）→ 转移到 B
    let (st, text) = fx.chat("A/m-q").await;
    assert_eq!(
        st, 200,
        "指定渠道 500 后应转移到备渠道成功，实际 {st}: {text}"
    );
    assert_eq!(text, "from-B", "应落到备渠道 B");
    assert_eq!(hits_a.load(Ordering::SeqCst), 1, "A 应被尝试恰好 1 次");
    assert_eq!(hits_b.load(Ordering::SeqCst), 1, "B 应被尝试 1 次");

    settle().await;

    // 第 2 次：A 已被标记不健康（冷却窗 5 分钟）→ 健康优先于指定，B 直接接管
    let (st2, text2) = fx.chat("A/m-q").await;
    assert_eq!(st2, 200, "第二次仍应成功，实际 {st2}: {text2}");
    assert_eq!(text2, "from-B", "第二次应直接走 B");
    assert_eq!(
        hits_a.load(Ordering::SeqCst),
        1,
        "A 已知不健康，不该再被撞一次（健康优先于指定）"
    );
    assert_eq!(hits_b.load(Ordering::SeqCst), 2, "B 应再被调用一次");
}

/// 用例 3：指定渠道可用时，优先压过**更优的 priority**（否则本用例会拿到 from-B）。
#[tokio::test(flavor = "multi_thread")]
async fn qualified_name_wins_over_better_priority() {
    let (port_a, hits_a) = spawn_counting_mock(200, "A").await;
    let (port_b, hits_b) = spawn_counting_mock(200, "B").await;
    let fx = fixture(port_a, port_b).await;

    let (st, text) = fx.chat("A/m-q").await;
    assert_eq!(st, 200);
    assert_eq!(
        text, "from-A",
        "限定名指定的 A（priority 200）应压过 B（priority 100）"
    );
    assert_eq!(hits_a.load(Ordering::SeqCst), 1);
    assert_eq!(
        hits_b.load(Ordering::SeqCst),
        0,
        "指定渠道可用时不该碰备渠道"
    );
}

/// 用例 4：供应商名对不上任何渠道 → 仍 404（**保留旧语义**）。
///
/// 没有这条，把实现写成「无条件保留其它候选」会让打错供应商名的请求静默换家出答案，
/// 比报错难排查得多。
#[tokio::test(flavor = "multi_thread")]
async fn unknown_provider_in_qualified_name_still_404() {
    let (port_a, hits_a) = spawn_counting_mock(200, "A").await;
    let (port_b, hits_b) = spawn_counting_mock(200, "B").await;
    let fx = fixture(port_a, port_b).await;

    let (st, _text) = fx.chat("NoSuchProvider/m-q").await;
    assert_eq!(st, 404, "供应商名对不上应 404，而不是静默换家");
    assert_eq!(hits_a.load(Ordering::SeqCst), 0);
    assert_eq!(hits_b.load(Ordering::SeqCst), 0);
}

/// 用例 5：裸模型名（无限定）行为不变 —— 按 priority 选 B，不被限定名逻辑影响。
#[tokio::test(flavor = "multi_thread")]
async fn bare_model_name_keeps_priority_routing() {
    let (port_a, hits_a) = spawn_counting_mock(200, "A").await;
    let (port_b, _hits_b) = spawn_counting_mock(200, "B").await;
    let fx = fixture(port_a, port_b).await;

    let (st, text) = fx.chat("m-q").await;
    assert_eq!(st, 200);
    assert_eq!(
        text, "from-B",
        "裸模型名不受限定名优先逻辑影响，应按 priority 走 B(100)"
    );
    assert_eq!(
        hits_a.load(Ordering::SeqCst),
        0,
        "裸名不该因为 A 是某处的限定名而被优先"
    );
}
