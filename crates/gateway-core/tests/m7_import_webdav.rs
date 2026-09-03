//! M7 集成测试：配置导入 + WebDAV 手动推/拉（roadmap M7 验收）。
//!
//! 拓扑：本地 DB → 导出 JSON → WebDAV mock（PUT/GET）→ 拉取 → 导入到新 DB。

use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{any, get, put};
use axum::Router;
use gateway_core::store::{self, Db};
use gateway_core::sync::{self, WebDavConfig};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

// ---------------------------------------------------------------- mock WebDAV

/// 需要认证的 mock：Authorization 必须等于 `Basic dTpwdw==`（u:pw）。
/// GET/PUT/PROPFIND 校验，OPTIONS 匿名放行——复现 DUFS「OPTIONS 200、GET 401」行为。
async fn spawn_dav_mock_auth() -> u16 {
    fn basic_ok(h: &HeaderMap) -> bool {
        h.get("authorization").and_then(|v| v.to_str().ok()) == Some("Basic dTpwdw==")
    }
    let app = Router::new()
        .route(
            "/jai-config.json",
            get(move |h: HeaderMap| async move {
                if !basic_ok(&h) {
                    return Response::builder().status(401).body(Body::empty()).unwrap();
                }
                Response::builder()
                    .status(200)
                    .body(Body::from("{\"ok\":true}"))
                    .unwrap()
            }),
        )
        .route(
            "/jai-config.json",
            put(move |h: HeaderMap, _body: String| async move {
                if !basic_ok(&h) {
                    return Response::builder().status(401).body(Body::empty()).unwrap();
                }
                Response::builder().status(201).body(Body::empty()).unwrap()
            }),
        )
        .route(
            "/",
            any(move |h: HeaderMap| async move {
                // PROPFIND（probe 用）也要认证
                let status = if basic_ok(&h) { 207 } else { 401 };
                Response::builder()
                    .status(status)
                    .body(Body::empty())
                    .unwrap()
            }),
        );
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr.port()
}

/// 远端文件不存在的 mock：GET 一律 404。
async fn spawn_dav_mock_missing() -> u16 {
    let app = Router::new().route(
        "/jai-config.json",
        get(|| async { Response::builder().status(404).body(Body::empty()).unwrap() }),
    );
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr.port()
}

async fn spawn_dav_mock() -> (u16, Arc<Mutex<Option<String>>>) {
    #[derive(Clone)]
    struct DavState(Arc<Mutex<Option<String>>>);
    let state = DavState(Arc::new(Mutex::new(None)));
    let state2 = state.clone();
    let app = Router::new()
        .route(
            "/jai-config.json",
            get(move |State(st): State<DavState>| async move {
                let body = st.0.lock().unwrap().clone().unwrap_or_else(|| "{}".into());
                Response::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap()
            }),
        )
        .route(
            "/jai-config.json",
            put(move |State(st): State<DavState>, body: String| async move {
                *st.0.lock().unwrap() = Some(body);
                Response::builder().status(201).body(Body::empty()).unwrap()
            }),
        )
        .with_state(state2);
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr.port(), state.0)
}

// ---------------------------------------------------------------- 用例

#[tokio::test(flavor = "multi_thread")]
async fn import_creates_providers_and_models() {
    let db = Db::in_memory().unwrap();
    let text = r#"{
        "format":"jai-export/v1",
        "exportedAt":1,
        "meta":[],
        "providers":[
            {"id":"p1","name":"Imported","base_url":"https://api.example/v1","family":"openai_compat","enabled":true,"priority":10,"extra_headers":null}
        ],
        "models":[
            {"id":"m1","providerId":"p1","modelName":"gpt-4o","upstreamModelId":null,"contextWindow":128000,"maxOutputTokens":4096,"enabled":true}
        ]
    }"#;
    let report = db
        .with_any(|c| store::import::apply_import(c, text, false))
        .unwrap();
    assert_eq!(report.providers_imported, 1);
    assert_eq!(report.models_imported, 1);
    assert_eq!(report.missing_keys, vec!["Imported"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn webdav_push_pull_roundtrip() {
    let (port, remote) = spawn_dav_mock().await;
    let cfg = WebDavConfig {
        url: format!("http://127.0.0.1:{port}"),
        username: "u".into(),
        directory: String::new(),
        auto_push_enabled: false,
        auto_push_interval_min: 60,
    };
    let client = reqwest::Client::new();
    let payload = r#"{"format":"jai-export/v1","providers":[]}"#.to_string();

    sync::push(&client, &cfg, "pw", payload.clone())
        .await
        .unwrap();
    assert_eq!(remote.lock().unwrap().as_deref(), Some(payload.as_str()));

    let pulled = sync::pull(&client, &cfg, "pw").await.unwrap();
    assert_eq!(pulled, payload);
}

#[tokio::test(flavor = "multi_thread")]
async fn webdav_pull_imports_into_db() {
    let (port, remote) = spawn_dav_mock().await;
    let export = json!({
        "format":"jai-export/v1",
        "providers":[
            {"id":"p9","name":"WebDav","base_url":"https://dav.example/v1","family":"openai_compat","enabled":true,"priority":1,"extra_headers":null}
        ],
        "models":[{"id":"m9","providerId":"p9","modelName":"gpt-4o-mini","upstreamModelId":null,"contextWindow":128000,"maxOutputTokens":4096,"enabled":true}]
    });
    *remote.lock().unwrap() = Some(export.to_string());

    let cfg = WebDavConfig {
        url: format!("http://127.0.0.1:{port}"),
        username: "u".into(),
        directory: String::new(),
        auto_push_enabled: false,
        auto_push_interval_min: 60,
    };
    let client = reqwest::Client::new();
    let text = sync::pull(&client, &cfg, "pw").await.unwrap();
    let db = Db::in_memory().unwrap();
    let report = db
        .with_any(|c| store::import::apply_import(c, &text, false))
        .unwrap();
    assert_eq!(report.providers_imported, 1);
    assert_eq!(report.models_imported, 1);
    assert_eq!(report.missing_keys, vec!["WebDav"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn probe_verifies_credentials() {
    let port = spawn_dav_mock_auth().await;
    let client = reqwest::Client::new();
    let cfg = WebDavConfig {
        url: format!("http://127.0.0.1:{port}"),
        username: "u".into(),
        directory: String::new(),
        auto_push_enabled: false,
        auto_push_interval_min: 60,
    };

    // 错误凭据 → 认证失败（此前 OPTIONS 匿名放行会把这里误报成「连接成功」）
    let err = sync::probe(&client, &cfg, "wrong").await.unwrap_err();
    assert!(err.contains("认证失败"), "{err}");

    // 正确凭据 → 连接成功
    assert_eq!(sync::probe(&client, &cfg, "pw").await.unwrap(), "连接成功");
}

#[tokio::test(flavor = "multi_thread")]
async fn pull_push_error_hints_for_bad_credentials() {
    let port = spawn_dav_mock_auth().await;
    let client = reqwest::Client::new();
    let cfg = WebDavConfig {
        url: format!("http://127.0.0.1:{port}"),
        username: "u".into(),
        directory: String::new(),
        auto_push_enabled: false,
        auto_push_interval_min: 60,
    };

    let e = sync::pull(&client, &cfg, "wrong").await.unwrap_err();
    assert!(e.contains("认证失败") && e.contains("HTTP 401"), "{e}");
    let e = sync::push(&client, &cfg, "wrong", "{}".into())
        .await
        .unwrap_err();
    assert!(e.contains("认证失败") && e.contains("HTTP 401"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn pull_404_hints_missing_remote_file() {
    let port = spawn_dav_mock_missing().await;
    let client = reqwest::Client::new();
    let cfg = WebDavConfig {
        url: format!("http://127.0.0.1:{port}"),
        username: "u".into(),
        directory: String::new(),
        auto_push_enabled: false,
        auto_push_interval_min: 60,
    };

    let e = sync::pull(&client, &cfg, "pw").await.unwrap_err();
    assert!(e.contains("HTTP 404") && e.contains("远端尚无"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn probe_404_hints_bad_path() {
    let app = Router::new().route(
        "/somewhere-else",
        any(|| async { Response::builder().status(404).body(Body::empty()).unwrap() }),
    );
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::new();
    // 探测路径（无斜杠）不匹配 /somewhere-else → 404
    let cfg = WebDavConfig {
        url: format!("http://127.0.0.1:{}", addr.port()),
        username: "u".into(),
        directory: String::new(),
        auto_push_enabled: false,
        auto_push_interval_min: 60,
    };
    let err = sync::probe(&client, &cfg, "pw").await.unwrap_err();
    assert!(err.contains("路径不存在"), "{err}");
}
