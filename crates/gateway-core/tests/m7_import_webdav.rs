//! M7 集成测试：配置导入 + WebDAV 手动推/拉（roadmap M7 验收）。
//!
//! 拓扑：本地 DB → 导出 JSON → WebDAV mock（PUT/GET）→ 拉取 → 导入到新 DB。

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Method, Uri};
use axum::response::Response;
use axum::routing::{any, get, put};
use axum::Router;
use gateway_core::store::{self, Db};
use gateway_core::sync::{self, WebDavConfig};
use serde_json::json;
use std::collections::HashMap;
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

/// 有状态 mock：按路径存取（GET 200/404，PUT 201），支持配置文件与时间戳备份
/// 共存的真实 WebDAV 行为。
async fn spawn_dav_mock() -> (u16, Arc<Mutex<HashMap<String, String>>>) {
    #[derive(Clone)]
    struct DavState(Arc<Mutex<HashMap<String, String>>>);
    let state = DavState(Arc::new(Mutex::new(HashMap::new())));
    let state2 = state.clone();
    let app = Router::new()
        .fallback(
            move |State(st): State<DavState>, uri: Uri, method: Method, body: String| async move {
                let path = uri.path().to_string();
                let mut map = st.0.lock().unwrap();
                match method.as_str() {
                    "GET" => match map.get(&path).cloned() {
                        Some(b) => Response::builder()
                            .status(200)
                            .header("content-type", "application/json")
                            .body(Body::from(b))
                            .unwrap(),
                        None => Response::builder().status(404).body(Body::empty()).unwrap(),
                    },
                    "PUT" => {
                        map.insert(path, body);
                        Response::builder().status(201).body(Body::empty()).unwrap()
                    }
                    "PROPFIND" => {
                        // Depth:1 列表：全部条目（含目录内备份），multistatus XML
                        let mut xml = String::from(
                            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<D:multistatus xmlns:D=\"DAV:\">\n",
                        );
                        let mut keys: Vec<&String> = map.keys().collect();
                        keys.sort();
                        for k in keys {
                            let size = map.get(k).map(|v| v.len()).unwrap_or(0);
                            xml.push_str(&format!(
                                "  <D:response>\n    <D:href>{k}</D:href>\n    \
                                 <D:propstat>\n      <D:prop><D:getcontentlength>{size}</D:getcontentlength></D:prop>\n      \
                                 <D:status>HTTP/1.1 200 OK</D:status>\n    </D:propstat>\n  </D:response>\n"
                            ));
                        }
                        xml.push_str("</D:multistatus>");
                        Response::builder()
                            .status(207)
                            .header("content-type", "application/xml")
                            .body(Body::from(xml))
                            .unwrap()
                    }
                    "DELETE" => {
                        if map.remove(&path).is_some() {
                            Response::builder().status(204).body(Body::empty()).unwrap()
                        } else {
                            Response::builder().status(404).body(Body::empty()).unwrap()
                        }
                    }
                    _ => Response::builder().status(404).body(Body::empty()).unwrap(),
                }
            },
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
        auto_pull_enabled: false,
    };
    let client = reqwest::Client::new();
    let payload = r#"{"format":"jai-export/v1","providers":[]}"#.to_string();

    sync::push(&client, &cfg, "pw", payload.clone())
        .await
        .unwrap();
    assert_eq!(
        remote
            .lock()
            .unwrap()
            .get("/jai-config.json")
            .map(String::as_str),
        Some(payload.as_str())
    );

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
    remote
        .lock()
        .unwrap()
        .insert("/jai-config.json".to_string(), export.to_string());

    let cfg = WebDavConfig {
        url: format!("http://127.0.0.1:{port}"),
        username: "u".into(),
        directory: String::new(),
        auto_push_enabled: false,
        auto_push_interval_min: 60,
        auto_pull_enabled: false,
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
        auto_pull_enabled: false,
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
        auto_pull_enabled: false,
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
        auto_pull_enabled: false,
    };

    let e = sync::pull(&client, &cfg, "pw").await.unwrap_err();
    assert!(e.contains("HTTP 404") && e.contains("远端尚无"), "{e}");
}

/// 数据丢失回归：远端已有完整配置时再推送，旧版必须留存为时间戳备份，
/// 主文件才被新内容覆盖（2026-09 备份消失事件根因修复）。
#[tokio::test(flavor = "multi_thread")]
async fn push_backs_up_existing_remote_before_overwrite() {
    let (port, remote) = spawn_dav_mock().await;
    let full =
        r#"{"format":"jai-export/v1","providers":[{"id":"p1","name":"A"}],"models":[{"id":"m1"}]}"#
            .to_string();
    remote
        .lock()
        .unwrap()
        .insert("/jai-config.json".to_string(), full.clone());

    let cfg = WebDavConfig {
        url: format!("http://127.0.0.1:{port}"),
        username: "u".into(),
        directory: String::new(),
        auto_push_enabled: false,
        auto_push_interval_min: 60,
        auto_pull_enabled: false,
    };
    let client = reqwest::Client::new();
    let empty = r#"{"format":"jai-export/v1","providers":[],"models":[]}"#.to_string();
    sync::push(&client, &cfg, "pw", empty.clone())
        .await
        .unwrap();

    let map = remote.lock().unwrap();
    // 主文件被新内容覆盖
    assert_eq!(
        map.get("/jai-config.json").map(String::as_str),
        Some(empty.as_str())
    );
    // 旧版必须留存在同目录时间戳备份中（永不丢失）
    let backups: Vec<String> = map
        .keys()
        .filter(|k| {
            k.starts_with("/jai-config.") && k.ends_with(".json") && *k != "/jai-config.json"
        })
        .cloned()
        .collect();
    assert_eq!(backups.len(), 1, "应留存一份远端旧版备份: {map:?}");
    assert_eq!(
        map.get(backups[0].as_str()).map(String::as_str),
        Some(full.as_str())
    );
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
        auto_pull_enabled: false,
    };
    let err = sync::probe(&client, &cfg, "pw").await.unwrap_err();
    assert!(err.contains("路径不存在"), "{err}");
}

/// T2 回归：远端备份列表（PROPFIND）过滤/排序、读取、删除与防误删。
#[tokio::test(flavor = "multi_thread")]
async fn webdav_backups_list_restore_delete_roundtrip() {
    let (port, remote) = spawn_dav_mock().await;
    let cfg = WebDavConfig {
        url: format!("http://127.0.0.1:{port}"),
        username: "u".into(),
        directory: String::new(),
        auto_push_enabled: false,
        auto_push_interval_min: 60,
        auto_pull_enabled: false,
    };
    {
        let mut m = remote.lock().unwrap();
        m.insert(
            "/jai-config.json".into(),
            r#"{"providers":[{"id":"cur"}]}"#.into(),
        );
        m.insert(
            "/jai-config.100.json".into(),
            r#"{"providers":[{"id":"old"}]}"#.into(),
        );
        m.insert(
            "/jai-config.200.json".into(),
            r#"{"providers":[{"id":"older"}]}"#.into(),
        );
        m.insert("/readme.txt".into(), "x".into());
    }
    let client = reqwest::Client::new();

    // 列表：只保留当前配置 + 时间戳备份，无关文件剔除，按时间戳升序（当前配置排最后）
    let items = sync::list_backups(&client, &cfg, "pw").await.unwrap();
    let names: Vec<String> = items.iter().map(|b| b.name.clone()).collect();
    assert_eq!(
        names,
        vec![
            "jai-config.100.json",
            "jai-config.200.json",
            "jai-config.json"
        ]
    );
    let b100 = items
        .iter()
        .find(|b| b.name == "jai-config.100.json")
        .unwrap();
    assert_eq!(
        b100.size,
        Some(r#"{"providers":[{"id":"old"}]}"#.len() as u64)
    );

    // 读取备份内容
    let text = sync::fetch_backup(&client, &cfg, "pw", "jai-config.100.json")
        .await
        .unwrap();
    assert!(text.contains("\"id\":\"old\""), "{text}");
    // 当前配置名不可作为备份读取
    assert!(sync::fetch_backup(&client, &cfg, "pw", "jai-config.json")
        .await
        .is_err());

    // 删除备份；当前配置拒绝删除；404 幂等
    sync::delete_backup(&client, &cfg, "pw", "jai-config.100.json")
        .await
        .unwrap();
    assert!(remote.lock().unwrap().get("/jai-config.100.json").is_none());
    assert!(sync::delete_backup(&client, &cfg, "pw", "jai-config.json")
        .await
        .is_err());
    sync::delete_backup(&client, &cfg, "pw", "jai-config.100.json")
        .await
        .unwrap(); // 已删 → 幂等成功
    assert!(remote.lock().unwrap().contains_key("/jai-config.json"));
}
