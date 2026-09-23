//! M13 集成测试：网关密钥白/黑名单（D9-T6b）。
//!
//! 背景：多密钥（D9-T6a）之后「一把密钥发给一个客户端」是常态，但每把密钥能碰到的
//! **渠道 / 模型**完全一样 —— 发给 CI 流水线的那把同样能调最贵的模型。T6b 让密钥
//! 自带规则（迁移 0013）。
//!
//! 用例（每条都端到端走完整 HTTP 链路，只测函数会漏掉接线回归）：
//! 1. `deny` 某渠道 → 该渠道的上游**零请求**（不是「试了再失败」，是根本不发）
//! 2. 候选全被过滤 → **403 `model_not_allowed`**（不是 404 `model_not_found`）
//! 3. `deny` 某模型 → 同名模型在所有渠道上一起被挡
//! 4. `allow` 非空 ⇒ 白名单（未列出的渠道一律不可用）
//! 5. `/v1/models` 按密钥过滤（客户端看不到自己调不通的模型）
//! 6. 向后兼容：没配规则的密钥行为与改造前**完全一致**
//! 7. 保存规则后立刻生效（缓存失效接线）

use axum::body::Body;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use gateway_core::server::{self, security, GatewayCtx};
use gateway_core::store::keyrules::KeyRules;
use gateway_core::store::{self, Db};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// 没配规则的密钥（对照组）
const KEY_OPEN: &str = "sk-jai-OPEN000000000000000000000000";
/// 被规则限制的密钥
const KEY_LIMITED: &str = "sk-jai-LIMITED0000000000000000000000";

const MODEL: &str = "m-test";

// ---------------------------------------------------------------- mock 上游

struct Upstream {
    port: u16,
    hits: Arc<AtomicUsize>,
}

/// 起一个 chat completions mock，并统计**收到的请求数** ——
/// 「被规则挡掉的渠道一个请求都不该发出去」只能靠计数证明。
async fn spawn_mock(tag: &'static str) -> Upstream {
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let h = h.clone();
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                Response::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "id": tag, "object": "chat.completion",
                            "choices": [{"index": 0,
                                "message": {"role": "assistant", "content": format!("from-{tag}")},
                                "finish_reason": "stop"}],
                            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
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
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Upstream { port, hits }
}

// ---------------------------------------------------------------- 夹具

struct Fixture {
    port: u16,
    db: Db,
    a: Upstream,
    b: Upstream,
    /// 网关那一份规则缓存（桌面端由 AppCore 持有，保存规则后 invalidate）
    rules: Arc<security::KeyRulesCache>,
    limited_id: String,
    _keepalive: (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ),
}

/// 两个 openai_compat 渠道（A 优先）+ 一把无规则密钥 + 一把待限制密钥。
async fn fixture() -> Fixture {
    let a = spawn_mock("A").await;
    let b = spawn_mock("B").await;

    let (db, main_path) = {
        let dir = std::env::temp_dir().join(format!(
            "jai-m13kr-{}-{}",
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
        store::provider_insert(c, &prow("A", 10, a.port))?;
        store::provider_insert(c, &prow("B", 20, b.port))?;
        store::model_upsert(c, "p-A", MODEL, Some(128_000), 4096, None, None)?;
        store::model_upsert(c, "p-B", MODEL, Some(128_000), 4096, None, None)?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();

    let limited_id = db
        .with(|c| {
            store::gw_key_create(c, KEY_OPEN, Some("无规则"))?;
            let k = store::gw_key_create(c, KEY_LIMITED, Some("受限"))?;
            Ok::<_, store::StoreError>(k.id)
        })
        .unwrap();

    let ctx = GatewayCtx::new(db.clone(), logs);
    let rules = ctx.rules.clone();
    let app = server::build_router(ctx);
    let (listener, port) = server::bind_with_fallback("127.0.0.1", 0).unwrap();
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(async move {
        let _ = server::run_until_shutdown(listener, app, stop_rx).await;
    });

    Fixture {
        port,
        db,
        a,
        b,
        rules,
        limited_id,
        _keepalive: (stop_tx, handle),
    }
}

impl Fixture {
    /// 写规则 + **失效缓存** —— 这正是桌面端 `gateway_key_rules_set` 做的事
    /// （保存后立刻 `invalidate`，用户不必等 5s TTL）。
    fn set_rules(&self, key_id: &str, f: impl FnOnce(&mut KeyRules)) {
        patch_rules(&self.db, key_id, f);
        self.rules.invalidate(Some(key_id));
    }
}

/// 覆盖式写入某把密钥的规则（`f` 在已读出的规则上改，等价于 UI 的「读-改-写」）。
/// **不失效缓存** —— 只有专门测缓存的用例才该用它。
fn patch_rules(db: &Db, key_id: &str, f: impl FnOnce(&mut KeyRules)) {
    db.with(|c| {
        let mut r = store::keyrules::key_rules_get(c, key_id)?;
        f(&mut r);
        store::keyrules::key_rules_set(c, key_id, &r)
    })
    .unwrap();
}

fn set(items: &[&str]) -> std::collections::BTreeSet<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

// ---------------------------------------------------------------- HTTP 助手

async fn chat(port: u16, token: &str, model: &str) -> (u16, Value) {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .bearer_auth(token)
        .json(&json!({"model": model, "messages": [{"role": "user", "content": "hi"}]}))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    (status, body)
}

async fn models(port: u16, token: &str) -> Vec<String> {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/v1/models"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    body["data"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| r["id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// 请求体里 mock 返回的 id 就是渠道标记（`from-A` / `from-B`）。
fn served_by(body: &Value) -> String {
    body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------- 用例

/// 向后兼容：没配规则的密钥，行为与改造前完全一致（A 优先命中）。
#[tokio::test(flavor = "multi_thread")]
async fn key_without_rules_behaves_as_before() {
    let f = fixture().await;
    let (status, body) = chat(f.port, KEY_OPEN, MODEL).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(served_by(&body), "from-A");
    assert_eq!(f.a.hits.load(Ordering::SeqCst), 1);
    assert_eq!(f.b.hits.load(Ordering::SeqCst), 0, "A 成功就不该碰 B");
    assert_eq!(
        models(f.port, KEY_OPEN).await,
        vec![format!("A/{MODEL}"), format!("B/{MODEL}")],
        "无规则 ⇒ /v1/models 不受影响"
    );
}

/// `deny` 某渠道 → 该渠道的上游**零请求**（不是「试了再失败」，是根本不发）。
#[tokio::test(flavor = "multi_thread")]
async fn provider_deny_never_touches_that_upstream() {
    let f = fixture().await;
    f.set_rules(&f.limited_id, |r| {
        r.provider_deny = set(&["p-A"]);
    });

    let (status, body) = chat(f.port, KEY_LIMITED, MODEL).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(served_by(&body), "from-B", "A 被 deny ⇒ 落到 B");
    assert_eq!(
        f.a.hits.load(Ordering::SeqCst),
        0,
        "被规则挡掉的渠道一个请求都不该发出去"
    );
    assert_eq!(f.b.hits.load(Ordering::SeqCst), 1);

    // 同一时刻另一把无规则密钥不受影响
    let (status, body) = chat(f.port, KEY_OPEN, MODEL).await;
    assert_eq!(status, 200);
    assert_eq!(served_by(&body), "from-A");
}

/// 候选全被过滤 → **403 `model_not_allowed`**，且上游零请求。
///
/// 这条是验收的硬要求：必须是 403 而不是 404 `model_not_found` ——
/// 后者会让用户以为「模型没了」，而实际上换把密钥就能用。
#[tokio::test(flavor = "multi_thread")]
async fn all_candidates_filtered_returns_403_not_404() {
    let f = fixture().await;
    f.set_rules(&f.limited_id, |r| {
        r.provider_deny = set(&["p-A", "p-B"]);
    });

    let (status, body) = chat(f.port, KEY_LIMITED, MODEL).await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["error"]["code"], "model_not_allowed");
    assert_eq!(body["error"]["type"], "permission_error");
    // 报错要说清是「密钥规则」而不是「模型不存在」，否则用户会去查错方向
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("规则"),
        "错误信息必须点明是密钥规则导致: {body}"
    );
    assert_eq!(f.a.hits.load(Ordering::SeqCst), 0);
    assert_eq!(f.b.hits.load(Ordering::SeqCst), 0);
}

/// 模型规则按 `model_name` 生效：同名模型挂在两个渠道上 ⇒ 一起被挡。
#[tokio::test(flavor = "multi_thread")]
async fn model_deny_blocks_the_model_on_every_provider() {
    let f = fixture().await;
    f.set_rules(&f.limited_id, |r| {
        r.model_deny = set(&[MODEL]);
    });

    let (status, body) = chat(f.port, KEY_LIMITED, MODEL).await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["error"]["code"], "model_not_allowed");
    assert_eq!(f.a.hits.load(Ordering::SeqCst), 0);
    assert_eq!(f.b.hits.load(Ordering::SeqCst), 0);
}

/// `allow` 列表非空 ⇒ 白名单：未列出的渠道一律不可用（这里只放 B）。
#[tokio::test(flavor = "multi_thread")]
async fn non_empty_allow_list_acts_as_whitelist() {
    let f = fixture().await;
    f.set_rules(&f.limited_id, |r| {
        r.provider_allow = set(&["p-B"]);
    });

    let (status, body) = chat(f.port, KEY_LIMITED, MODEL).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(served_by(&body), "from-B", "白名单外（A）必须被跳过");
    assert_eq!(f.a.hits.load(Ordering::SeqCst), 0);

    // 白名单里放一个**不提供该模型**的渠道 ⇒ 等于没有可用候选 ⇒ 403
    f.set_rules(&f.limited_id, |r| {
        r.provider_allow = set(&["p-A", "p-B"]);
        r.provider_deny = set(&["p-A", "p-B"]);
    });
    let (status, body) = chat(f.port, KEY_LIMITED, MODEL).await;
    assert_eq!(status, 403, "deny 优先于 allow: {body}");
}

/// `/v1/models` 也按密钥过滤 —— 否则客户端会看到一堆自己调不通的模型。
#[tokio::test(flavor = "multi_thread")]
async fn models_list_is_filtered_per_key() {
    let f = fixture().await;
    f.set_rules(&f.limited_id, |r| {
        r.provider_deny = set(&["p-B"]);
    });
    assert_eq!(
        models(f.port, KEY_LIMITED).await,
        vec![format!("A/{MODEL}")],
        "被 deny 的渠道不该出现在该密钥的模型列表里"
    );

    // 模型轴同样生效：deny 掉模型 ⇒ 两个渠道的该模型都不出现
    f.set_rules(&f.limited_id, |r| {
        r.provider_deny.clear();
        r.model_deny = set(&[MODEL]);
    });
    assert!(
        models(f.port, KEY_LIMITED).await.is_empty(),
        "模型被 deny ⇒ 列表为空"
    );

    // 对照：无规则密钥照旧
    assert_eq!(models(f.port, KEY_OPEN).await.len(), 2);
}

/// 规则保存后立刻生效（桌面端 `gateway_key_rules_set` 会 invalidate 缓存）。
///
/// 同时把 5s TTL 的语义钉住：不失效的话，缓存里的旧规则在 TTL 内仍然说了算 ——
/// 所以「保存即生效」这条必须靠 invalidate，而不是靠 TTL 到期。
#[tokio::test(flavor = "multi_thread")]
async fn saved_rules_take_effect_after_cache_invalidation() {
    let f = fixture().await;

    // 先打一次请求把「无规则」灌进缓存
    let (status, _) = chat(f.port, KEY_LIMITED, MODEL).await;
    assert_eq!(status, 200);

    // 这里刻意用**不失效**缓存的写入路径（`patch_rules`）—— 见下面对 TTL 的断言
    patch_rules(&f.db, &f.limited_id, |r| {
        r.provider_deny = set(&["p-A", "p-B"]);
    });
    // 未失效：TTL 内的缓存仍说「不限制」（这正是需要 invalidate 的原因）
    let (status, _) = chat(f.port, KEY_LIMITED, MODEL).await;
    assert_eq!(status, 200, "TTL 未到 ⇒ 仍走缓存");

    f.rules.invalidate(Some(&f.limited_id));
    let (status, body) = chat(f.port, KEY_LIMITED, MODEL).await;
    assert_eq!(status, 403, "失效后必须立刻按新规则拒绝: {body}");
}
