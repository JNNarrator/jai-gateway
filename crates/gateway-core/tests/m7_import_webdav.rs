//! M7 集成测试：配置导入 + WebDAV 手动推/拉（roadmap M7 验收）。
//!
//! 拓扑：本地 DB → 导出 JSON → WebDAV mock（PUT/GET）→ 拉取 → 导入到新 DB。

use axum::body::Body;
use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, put};
use axum::Router;
use gateway_core::store::{self, Db};
use gateway_core::sync::{self, WebDavConfig};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

// ---------------------------------------------------------------- mock WebDAV

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
