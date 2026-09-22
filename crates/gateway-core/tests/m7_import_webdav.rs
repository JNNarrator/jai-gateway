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

/// 真机复现（2026-09-22）：服务没就绪时，nginx 用自己的 HTML 404 页回应**所有**方法
/// （`GET`/`PUT`/`PROPFIND` 全是 404），而不是 WebDAV 的 XML 错误。
const NGINX_404_HTML: &str = "<!DOCTYPE html>\n<html>\n<head>\n<title>Not Found</title>\n\
<style>\n    body {\n        width: 35em;\n        margin: 0 auto;\n        \
font-family: Tahoma, Verdana, Arial, sans-serif;\n    }\n</style>\n</head>\n\
<body>\n<h1>The page you requested was not found.</h1>\n<p>Sorry, the page you are \
looking for is currently unavailable.<br/>\nPlease try again later.</p>\n</body>\n</html>\n";

fn html_404() -> Response {
    Response::builder()
        .status(404)
        .header("content-type", "text/html; charset=utf-8")
        .body(Body::from(NGINX_404_HTML))
        .unwrap()
}

/// 所有方法一律 HTML 404（服务未就绪 / 根地址打错）。
async fn spawn_mock_html_404_all() -> u16 {
    let app = Router::new().fallback(any(|| async { html_404() }));
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr.port()
}

/// GET 是「干净」的 404（远端确实还没有配置文件），PUT 时服务刚好没起来（HTML 404）
/// —— 正是真机上「推送失败」的形态。
async fn spawn_mock_get404_put_html404() -> u16 {
    let app = Router::new().route(
        "/jai-config.json",
        get(|| async { Response::builder().status(404).body(Body::empty()).unwrap() })
            .put(|| async { html_404() }),
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

/// 进程级环境变量守卫：drop 时还原（含 panic 路径）。
/// 教训来自 bug 清单 6：env 清理绝不能写在断言之后，否则一次 panic 会污染后续用例。
struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, val: &str) -> Self {
        let prev = std::env::var(key).ok();
        std::env::set_var(key, val);
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

/// 回归（bug 清单 14）：远端 `jai-config.json` 若被历史自引用快照撑大，
/// 拉取必须直接给出可操作错误，而不是把几百 MB 读进内存再解析。
#[tokio::test(flavor = "multi_thread")]
async fn pull_rejects_oversized_remote_config() {
    let (port, remote) = spawn_dav_mock().await;
    // 阈值压到 4KB，避免测试真去构造 32MB 字符串
    let _guard = EnvGuard::set("JAI_REMOTE_CONFIG_MAX_BYTES", "4096");
    remote
        .lock()
        .unwrap()
        .insert("/jai-config.json".into(), "x".repeat(8 * 1024));

    let client = reqwest::Client::new();
    let cfg = WebDavConfig {
        url: format!("http://127.0.0.1:{port}"),
        username: "u".into(),
        directory: String::new(),
        auto_push_enabled: false,
        auto_push_interval_min: 60,
        auto_pull_enabled: false,
        auto_pull_interval_min: 60,
    };

    let e = sync::try_pull(&client, &cfg, "pw").await.unwrap_err();
    assert!(e.contains("异常过大"), "应报体积异常：{e}");
    assert!(e.contains("bug 清单 14"), "应指向根因条目：{e}");

    // 正常体积仍然通过（护栏不能误伤）
    remote.lock().unwrap().insert(
        "/jai-config.json".into(),
        r#"{"format":"jai-export/v1","exportedAt":1}"#.into(),
    );
    assert!(sync::try_pull(&client, &cfg, "pw").await.unwrap().is_some());
}

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
        auto_pull_interval_min: 60,
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
        auto_pull_interval_min: 60,
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
        auto_pull_interval_min: 60,
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
        auto_pull_interval_min: 60,
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
        auto_pull_interval_min: 60,
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
        auto_pull_interval_min: 60,
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
        auto_pull_interval_min: 60,
    };
    let err = sync::probe(&client, &cfg, "pw").await.unwrap_err();
    assert!(err.contains("路径不存在"), "{err}");
}

/// 真机回归（2026-09-22）：推送撞上服务器未就绪的 HTML 404 时，提示必须指向
/// 「地址/服务」而不是「目标目录不存在」（DUFS 对 PUT 到不存在的目录返回 201）。
#[tokio::test(flavor = "multi_thread")]
async fn push_404_web_page_reports_endpoint_not_missing_directory() {
    let port = spawn_mock_get404_put_html404().await;
    let client = reqwest::Client::new();
    let err = sync::push(&client, &cfg_for(port), "pw", "{}".into())
        .await
        .unwrap_err();
    assert!(err.contains("不是可用的 WebDAV 端点"), "{err}");
    assert!(!err.contains("目标目录不存在"), "{err}");
    assert!(err.contains("HTTP 404"), "{err}");
    // 整页 HTML 不再原样进消息：空白被折叠、正文被截断
    assert!(!err.contains('\n'), "错误正文应折叠空白：{err}");
    assert!(err.ends_with('…'), "超长正文应带省略号：{err}");
    assert!(
        err.chars().count() < 400,
        "错误消息过长（{} 字符）：{err}",
        err.chars().count()
    );
}

/// 反向控制：空正文的 404 是 WebDAV 在说「文件/目录不存在」，原措辞必须保留。
#[tokio::test(flavor = "multi_thread")]
async fn push_404_without_body_keeps_directory_hint() {
    let app = Router::new().fallback(any(|| async {
        Response::builder().status(404).body(Body::empty()).unwrap()
    }));
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::new();
    let err = sync::push(&client, &cfg_for(addr.port()), "pw", "{}".into())
        .await
        .unwrap_err();
    assert!(err.contains("目标目录不存在"), "{err}");
}

/// 拉取：网页 404 不能被当成「远端尚无配置文件」（自动拉取会据此静默放弃）。
#[tokio::test(flavor = "multi_thread")]
async fn pull_html_404_is_not_missing_remote_file() {
    let port = spawn_mock_html_404_all().await;
    let client = reqwest::Client::new();
    let err = sync::pull(&client, &cfg_for(port), "pw").await.unwrap_err();
    assert!(err.contains("不是可用的 WebDAV 端点"), "{err}");
    assert!(!err.contains("远端尚无"), "{err}");
}

/// 关键决策的反向控制：`try_pull` 对「网页 404」**不做 fail-closed**（返回 `Ok(None)`）。
/// 理由见 `try_pull` 的注释：健康的 WebDAV 也会用 HTML 404 表示「文件不存在」
/// （nginx dav_module、反向代理 `error_page 404 /404.html`），若在这里报错，
/// `push` 的第 1 步就会中止，**首次推送永远建不出远端文件**。
/// 这条用例钉住「分类只用来选措辞、不改控制流」。
#[tokio::test(flavor = "multi_thread")]
async fn try_pull_html_404_is_not_fail_closed() {
    let port = spawn_mock_html_404_all().await;
    let client = reqwest::Client::new();
    let got = sync::try_pull(&client, &cfg_for(port), "pw").await;
    assert!(
        matches!(got, Ok(None)),
        "网页 404 不得 fail-closed（否则首次推送无法创建远端文件）：{got:?}"
    );
}

/// 测连接：同样是 404，网页版要指向端点、空正文版才指向路径。
#[tokio::test(flavor = "multi_thread")]
async fn probe_html_404_points_at_endpoint_not_path() {
    let port = spawn_mock_html_404_all().await;
    let client = reqwest::Client::new();
    let err = sync::probe(&client, &cfg_for(port), "pw")
        .await
        .unwrap_err();
    assert!(err.contains("不是可用的 WebDAV 端点"), "{err}");
    assert!(!err.contains("路径不存在"), "{err}");
}

/// 删除备份：服务没就绪时不能谎报「幂等成功」（推送后的滚动清理会以为已删掉）。
#[tokio::test(flavor = "multi_thread")]
async fn delete_backup_html_404_is_not_silent_success() {
    let port = spawn_mock_html_404_all().await;
    let client = reqwest::Client::new();
    let err = sync::delete_backup(&client, &cfg_for(port), "pw", "jai-config.100.json")
        .await
        .unwrap_err();
    assert!(err.contains("不是可用的 WebDAV 端点"), "{err}");
}

/// 推送前留存备份这一处（`push` 的第 1 步）此前没有用例覆盖：远端已有旧配置 ⇒
/// 先 GET 到旧版，再 PUT `jai-config.<ts>.json`，而这一步撞上 HTML 404（服务没就绪）。
/// 提示必须指向端点，且仍然中止推送（防覆盖丢失）。
#[tokio::test(flavor = "multi_thread")]
async fn push_backup_put_html_404_reports_endpoint() {
    let app = Router::new().fallback(|uri: Uri, method: Method| async move {
        match (method.as_str(), uri.path()) {
            ("GET", "/jai-config.json") => Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"format":"jai-export/v1","providers":[{"id":"p1"}],"models":[]}"#,
                ))
                .unwrap(),
            // 备份 PUT 落在 HTML 404 上（`/jai-config.<ts>.json` 走这里）
            _ => html_404(),
        }
    });
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::new();
    let err = sync::push(&client, &cfg_for(addr.port()), "pw", "{}".into())
        .await
        .unwrap_err();
    assert!(err.contains("备份远端旧配置失败"), "{err}");
    assert!(err.contains("不是可用的 WebDAV 端点"), "{err}");
    assert!(err.contains("已中止推送"), "仍须中止，防覆盖丢失：{err}");
}

/// 备份列表 / 备份读取的 404 同样要分「网页」与「WebDAV 说不存在」两种。
#[tokio::test(flavor = "multi_thread")]
async fn backup_list_and_fetch_html_404_report_endpoint() {
    let port = spawn_mock_html_404_all().await;
    let client = reqwest::Client::new();
    let cfg = cfg_for(port);

    let err = sync::list_backups(&client, &cfg, "pw").await.unwrap_err();
    assert!(err.contains("不是可用的 WebDAV 端点"), "{err}");
    assert!(!err.contains("目录不存在"), "{err}");

    let err = sync::fetch_backup(&client, &cfg, "pw", "jai-config.100.json")
        .await
        .unwrap_err();
    assert!(err.contains("不是可用的 WebDAV 端点"), "{err}");
    assert!(!err.contains("备份不存在"), "{err}");
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
        auto_pull_interval_min: 60,
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

// ---------------------------------------------------------------- bug 19：换台电脑

/// 造一台「老机器 A」的完整导出物：带 key 的供应商 + 模型 + 网关 Key + A 机调度偏好。
///
/// **刻意注入**四个 `webdav_auto_*`（模拟 v0.2.3 及更早推上去的远端内容——
/// 也就是升级前真实存在于用户远端的那种 payload）：只有 payload 里带着它们，
/// 才真正压住导入侧；若只依赖本版本的导出器（已剔除这四个键），
/// 导入侧即使退化回「照单全收」也测不出来。
async fn machine_a_export(port: u16) -> String {
    let a = Db::in_memory().unwrap();
    a.with_any(|c| {
        // A 机本机偏好：自动推送开、自动拉取关（默认值）、间隔 30
        sync::config_set(
            c,
            &WebDavConfig {
                url: format!("http://127.0.0.1:{port}"),
                username: "u".into(),
                directory: String::new(),
                auto_push_enabled: true,
                auto_push_interval_min: 30,
                auto_pull_enabled: false,
                auto_pull_interval_min: 30,
            },
        )
        .map_err(|e| e.to_string())?;
        store::meta_set(c, "webdav_password", "pw").map_err(|e| e.to_string())?;
        store::import::apply_import(
            c,
            &json!({
                "format": "jai-export/v1",
                "gateway_key": "sk-jai-from-a",
                "meta": [],
                "providers": [{
                    "id": "pa", "name": "A机供应商", "base_url": "https://api.a.test/v1",
                    "family": "openai_compat", "enabled": true, "priority": 50,
                    "extra_headers": null, "api_key": "sk-upstream-from-a"
                }],
                "models": [{
                    "id": "ma", "providerId": "pa", "modelName": "gpt-4o",
                    "upstreamModelId": null, "contextWindow": 128000,
                    "maxOutputTokens": 4096, "enabled": true
                }]
            })
            .to_string(),
            false,
        )?;
        Ok::<(), String>(())
    })
    .unwrap();

    let built = a
        .with_any(|c| store::export::build_export_json(c).map_err(|e| e.to_string()))
        .unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&built).unwrap();
    // 旧版形态：这四个键曾被导出、并随 payload 旅行到对方机器
    let meta = v.get_mut("meta").unwrap().as_array_mut().unwrap();
    meta.push(json!(["webdav_auto_push_enabled", "1"]));
    meta.push(json!(["webdav_auto_push_interval_min", "30"]));
    meta.push(json!(["webdav_auto_pull_enabled", "0"]));
    // 拉取间隔也注入，且**故意取 360**：B 机本机值是 30，若导入侧把本机调度偏好
    // 照单全收，B 的 30 会被改成 360，下面的断言立刻变红（可反证）。
    meta.push(json!(["webdav_auto_pull_interval_min", "360"]));
    v.to_string()
}

/// 导出侧：本机调度偏好不得随配置离开这台机器（半升级状态下保护旧版本的关键）。
#[test]
fn export_omits_machine_local_switches() {
    let db = Db::in_memory().unwrap();
    db.with_any(|c| {
        sync::config_set(
            c,
            &WebDavConfig {
                url: "https://dav.example.com".into(),
                username: "u".into(),
                directory: "jai".into(),
                auto_push_enabled: true,
                auto_push_interval_min: 30,
                auto_pull_enabled: true,
                auto_pull_interval_min: 30,
            },
        )
        .map_err(|e| e.to_string())?;
        store::meta_set(c, "webdav_password", "pw").map_err(|e| e.to_string())
    })
    .unwrap();
    let text = db
        .with_any(|c| store::export::build_export_json(c).map_err(|e| e.to_string()))
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let keys: Vec<String> = v["meta"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|kv| kv.get(0).and_then(|k| k.as_str()).map(str::to_string))
        .collect();
    for k in sync::MACHINE_LOCAL_META_KEYS {
        assert!(
            !keys.iter().any(|x| x == k),
            "{k} 属本机调度偏好，不该出现在导出物里（会被对方机器导入并覆盖其开关）"
        );
    }
    // 共享的连接配置与凭据仍必须随行
    for k in [
        "webdav_url",
        "webdav_username",
        "webdav_directory",
        "webdav_password",
    ] {
        assert!(keys.iter().any(|x| x == k), "{k} 应随配置同步");
    }
}

fn cfg_for(port: u16) -> WebDavConfig {
    WebDavConfig {
        url: format!("http://127.0.0.1:{port}"),
        username: "u".into(),
        directory: String::new(),
        auto_push_enabled: true,
        auto_push_interval_min: 30,
        auto_pull_enabled: false,
        auto_pull_interval_min: 30,
    }
}

/// 「换台电脑」的第一步：新机器拉取后**数据必须到位**（供应商/上游密钥/模型/网关 Key）。
#[tokio::test(flavor = "multi_thread")]
async fn second_machine_pull_lands_data_and_keys() {
    let (port, _remote) = spawn_dav_mock().await;
    let export = machine_a_export(port).await;
    let client = reqwest::Client::new();
    sync::push(&client, &cfg_for(port), "pw", export)
        .await
        .unwrap();

    // B 机：全新库
    let b = Db::in_memory().unwrap();
    let text = sync::pull(&client, &cfg_for(port), "pw").await.unwrap();
    let report = b
        .with_any(|c| store::import::apply_import(c, &text, false))
        .unwrap();
    assert_eq!(report.providers_imported, 1, "新机器应拿到供应商");
    assert_eq!(report.models_imported, 1, "新机器应拿到模型");

    let (name, key) = b
        .with_any(|c| {
            let p = store::provider_list(c)
                .map_err(|e| e.to_string())?
                .remove(0);
            Ok::<_, String>((p.name, p.api_key))
        })
        .unwrap();
    assert_eq!(name, "A机供应商");
    assert_eq!(
        key.as_deref(),
        Some("sk-upstream-from-a"),
        "上游 API Key 必须随配置到达新机器（否则新机器上所有请求 401 = 用不了）"
    );
    let gw = b
        .with_any(|c| store::gw_key_active(c).map_err(|e| e.to_string()))
        .unwrap();
    assert_eq!(
        gw.map(|k| k.key),
        Some("sk-jai-from-a".to_string()),
        "网关 Key 必须随配置到达新机器"
    );
}

/// bug 19 守门人：**拉取不得改掉本机的自动同步开关**。
///
/// 用户实报「换台电脑就没成功过」的机制：
/// A 机 `auto_pull_enabled=0`（默认关）→ 导出物里带着这个 0 →
/// B 机（新电脑）用户打开「自动拉取」想让本机接收 A 的更新 → 点一次「拉取」→
/// 导入白名单把 A 的 0 照单收下 → **自动拉取被这次拉取自己关掉**，
/// 此后 B 再也不自动拉，用户看到的就是「怎么都同步不过来」。
/// 同一机制还会把 B 的 `auto_push_enabled` 翻成 A 的值，让新机器反过来覆盖远端。
#[tokio::test(flavor = "multi_thread")]
async fn pull_must_not_clobber_local_auto_switches() {
    let (port, _remote) = spawn_dav_mock().await;
    let export = machine_a_export(port).await;
    let client = reqwest::Client::new();
    sync::push(&client, &cfg_for(port), "pw", export)
        .await
        .unwrap();

    // B 机：用户已按自己的意愿设定本机偏好 —— 自动拉取开、自动推送关、
    // 推送间隔 360、拉取间隔 30（推送/拉取两个间隔刻意不同，且都不同于 A 机）
    let b = Db::in_memory().unwrap();
    b.with_any(|c| {
        sync::config_set(
            c,
            &WebDavConfig {
                url: format!("http://127.0.0.1:{port}"),
                username: "u".into(),
                directory: String::new(),
                auto_push_enabled: false,
                auto_push_interval_min: 360,
                auto_pull_enabled: true,
                auto_pull_interval_min: 30,
            },
        )
        .map_err(|e| e.to_string())
    })
    .unwrap();

    // B 机拉取
    let text = sync::pull(&client, &cfg_for(port), "pw").await.unwrap();
    b.with_any(|c| store::import::apply_import(c, &text, false))
        .unwrap();

    // 前半段：数据确实到位（否则是另一个问题，不该被这条断言掩盖）
    assert_eq!(
        b.with_any(|c| Ok::<_, String>(store::provider_list(c).unwrap().len()))
            .unwrap(),
        1
    );

    // 后半段：本机调度偏好不被远端覆盖
    let after = b
        .with_any(|c| sync::config_get(c).map_err(|e| e.to_string()))
        .unwrap()
        .unwrap();
    assert!(
        after.auto_pull_enabled,
        "拉取不得关掉本机的自动拉取（远端 auto_pull=0 不该被导入）"
    );
    assert!(
        !after.auto_push_enabled,
        "拉取不得打开本机的自动推送（否则新机器会反过来覆盖远端）"
    );
    assert_eq!(
        after.auto_push_interval_min, 360,
        "拉取不得改掉本机的自动推送间隔"
    );
    assert_eq!(
        after.auto_pull_interval_min, 30,
        "拉取不得改掉本机的自动拉取间隔（A 机 payload 里注入的是 360，不该被导入）"
    );
}
