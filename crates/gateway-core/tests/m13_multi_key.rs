//! M13 集成测试：网关多密钥（D9-T6a）。
//!
//! 背景：此前「密钥」实质只有一把 —— `gw_key_active` 只取最新一条，鉴权也只比那一把。
//! 于是「给 A 客户端一把、给 B 客户端一把，A 泄露了单独吊销 A」做不到：只能轮换，
//! 而那会把所有人一起踢掉。
//!
//! 用例：
//! 1. 创建 3 把 → `gw_keys_active` 返回 3；**三把都能鉴权通过**（旧密钥不被踢）
//! 2. 吊销其中 1 把 → 该密钥 401，其余两把正常（HTTP 端到端，走完整中间件）
//! 3. 轮换 → 新密钥可用、全部旧密钥立即失效
//! 4. 全吊销 → 鉴权一律失败（`ensure_gateway_key` 的自举只发生在桌面端启动路径）
//! 5. 吊销幂等：重复吊销不改时间戳
//!
//! 同时覆盖 `security::authenticate` 直接调用与**完整 HTTP 链路**两条路径 ——
//! 后者才能证明中间件真的接上了多密钥（只测函数会漏掉接线回归）。

use axum::http::{HeaderMap, HeaderValue};
use gateway_core::server::{self, security, GatewayCtx};
use gateway_core::store::{self, Db};
use serde_json::Value;

const KEY_A: &str = "sk-jai-AAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const KEY_B: &str = "sk-jai-BBBBBBBBBBBBBBBBBBBBBBBBBBBB";
const KEY_C: &str = "sk-jai-CCCCCCCCCCCCCCCCCCCCCCCCCCCC";

fn db() -> Db {
    Db::in_memory().unwrap()
}

fn auth_headers(token: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
    );
    h
}

async fn authed(db: &Db, token: &str) -> bool {
    security::authenticate(db, &auth_headers(token))
        .await
        .is_ok()
}

/// 起一个最小网关（只有鉴权与 /v1/models，不需要任何渠道）。
async fn spawn_gateway(db: Db) -> (u16, tokio::sync::watch::Sender<bool>) {
    let dir = std::env::temp_dir().join(format!(
        "jai-m13mk-{}-{}",
        std::process::id(),
        rand::random::<u32>()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("main.db");
    let (logs, _task) = gateway_core::store::logs::spawn_logger(path.to_str().unwrap()).unwrap();
    let ctx = GatewayCtx::new(db, logs);
    let app = server::build_router(ctx);
    let (listener, port) = server::bind_with_fallback("127.0.0.1", 0).unwrap();
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let _ = server::run_until_shutdown(listener, app, stop_rx).await;
    });
    (port, stop_tx)
}

async fn get_models(port: u16, token: &str) -> (u16, Value) {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/v1/models"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    (status, body)
}

// ---------------------------------------------------------------- 用例

/// 创建 3 把：都在列表里，且三把都能鉴权（新建不会踢掉旧的）。
#[tokio::test(flavor = "multi_thread")]
async fn creating_keys_keeps_the_old_ones_working() {
    let db = db();
    db.with(|c| {
        store::gw_key_create(c, KEY_A, Some("笔记本"))?;
        store::gw_key_create(c, KEY_B, Some("台式机"))?;
        store::gw_key_create(c, KEY_C, None)?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();

    let rows = db.with(store::gw_keys_active).unwrap();
    assert_eq!(rows.len(), 3, "三把都该是活跃的");
    // 新建在前
    assert_eq!(rows[0].key, KEY_C);
    assert_eq!(rows[2].key, KEY_A);
    // label 承担「发给谁」的备注；没填就是 None
    assert_eq!(rows[0].label, None);
    assert_eq!(rows[1].label.as_deref(), Some("台式机"));

    for k in [KEY_A, KEY_B, KEY_C] {
        assert!(authed(&db, k).await, "{k} 应能通过鉴权");
    }
    assert!(!authed(&db, "sk-jai-not-a-real-key-0000000000").await);
}

/// 端到端：吊销一把后，那把 401、其余两把 200（走完整中间件）。
#[tokio::test(flavor = "multi_thread")]
async fn revoking_one_key_only_kills_that_key() {
    let db = db();
    let id_b = db
        .with(|c| {
            store::gw_key_create(c, KEY_A, Some("A"))?;
            let b = store::gw_key_create(c, KEY_B, Some("B"))?;
            store::gw_key_create(c, KEY_C, Some("C"))?;
            Ok::<_, store::StoreError>(b.id)
        })
        .unwrap();

    let (port, _stop) = spawn_gateway(db.clone()).await;

    // 吊销前：三把都通
    for k in [KEY_A, KEY_B, KEY_C] {
        assert_eq!(get_models(port, k).await.0, 200, "{k} 吊销前应 200");
    }

    let changed = db.with(|c| store::gw_key_revoke(c, &id_b)).unwrap();
    assert!(changed, "首次吊销应改动一行");

    let (s_b, _) = get_models(port, KEY_B).await;
    assert_eq!(s_b, 401, "被吊销的密钥必须 401");
    for k in [KEY_A, KEY_C] {
        assert_eq!(get_models(port, k).await.0, 200, "{k} 不该受影响");
    }
    // 列表里也不该再有它
    let rows = db.with(store::gw_keys_active).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(!rows.iter().any(|r| r.id == id_b));
}

/// 轮换 = 创建新 + 吊销全部旧的：新密钥立即可用，旧的全部失效。
#[tokio::test(flavor = "multi_thread")]
async fn rotate_invalidates_every_old_key() {
    let db = db();
    db.with(|c| {
        store::gw_key_create(c, KEY_A, None)?;
        store::gw_key_create(c, KEY_B, None)?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();

    const NEW: &str = "sk-jai-NEWNEWNEWNEWNEWNEWNEWNEWNEW";
    db.with(|c| store::gw_key_rotate(c, NEW, Some("轮换")))
        .unwrap();

    assert!(authed(&db, NEW).await);
    for old in [KEY_A, KEY_B] {
        assert!(!authed(&db, old).await, "轮换后 {old} 必须失效");
    }
    assert_eq!(db.with(store::gw_keys_active).unwrap().len(), 1);
}

/// 全吊销 → 鉴权一律失败（自举只发生在桌面端启动路径，不在 store 层偷偷补一把）。
#[tokio::test(flavor = "multi_thread")]
async fn revoking_everything_locks_the_gateway() {
    let db = db();
    let ids = db
        .with(|c| {
            let a = store::gw_key_create(c, KEY_A, None)?;
            let b = store::gw_key_create(c, KEY_B, None)?;
            Ok::<_, store::StoreError>(vec![a.id, b.id])
        })
        .unwrap();

    for id in &ids {
        assert!(db.with(|c| store::gw_key_revoke(c, id)).unwrap());
    }
    assert!(db.with(store::gw_keys_active).unwrap().is_empty());
    assert!(db.with(store::gw_key_active).unwrap().is_none());
    for k in [KEY_A, KEY_B] {
        assert!(!authed(&db, k).await, "全吊销后 {k} 必须失效");
    }
}

/// 吊销幂等：重复吊销返回 false 且不改 `revoked_at`（审计时间要稳定）。
#[tokio::test(flavor = "multi_thread")]
async fn revoke_is_idempotent_and_keeps_the_first_timestamp() {
    let db = db();
    let id = db
        .with(|c| store::gw_key_create(c, KEY_A, None).map(|r| r.id))
        .unwrap();

    assert!(db.with(|c| store::gw_key_revoke(c, &id)).unwrap());
    let first = db
        .with(|c| {
            Ok::<_, store::StoreError>(c.query_row(
                "SELECT revoked_at FROM gateway_keys WHERE id=?1",
                [&id],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .unwrap();

    // 等 5ms 再吊销一次：若实现写成「无条件 UPDATE」，时间戳会变
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    assert!(
        !db.with(|c| store::gw_key_revoke(c, &id)).unwrap(),
        "重复吊销应返回 false"
    );
    let again = db
        .with(|c| {
            Ok::<_, store::StoreError>(c.query_row(
                "SELECT revoked_at FROM gateway_keys WHERE id=?1",
                [&id],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .unwrap();
    assert_eq!(first, again, "重复吊销不该改时间戳");
}

/// 未填 / 空白的 label 归一成 NULL（避免列表里出现空字符串标签）。
#[tokio::test(flavor = "multi_thread")]
async fn blank_label_is_stored_as_null() {
    let db = db();
    let rows = db
        .with(|c| {
            store::gw_key_create(c, KEY_A, Some("   "))?;
            store::gw_key_create(c, KEY_B, None)?;
            store::gw_keys_active(c)
        })
        .unwrap();
    assert!(rows.iter().all(|r| r.label.is_none()), "{rows:?}");
}

/// `gw_key_active` 保留「最新一把」语义（导出 / WebDAV 导入 / 首次自举在用）。
#[tokio::test(flavor = "multi_thread")]
async fn gw_key_active_still_returns_the_newest() {
    let db = db();
    db.with(|c| {
        store::gw_key_create(c, KEY_A, None)?;
        store::gw_key_create(c, KEY_B, None)?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();
    assert_eq!(db.with(store::gw_key_active).unwrap().unwrap().key, KEY_B);

    // 把最新那把吊销 → 回退到上一把（而不是 None）
    let newest = db.with(store::gw_keys_active).unwrap()[0].id.clone();
    db.with(|c| store::gw_key_revoke(c, &newest)).unwrap();
    assert_eq!(db.with(store::gw_key_active).unwrap().unwrap().key, KEY_A);
}
