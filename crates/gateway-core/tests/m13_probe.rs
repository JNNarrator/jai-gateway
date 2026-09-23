//! M13 集成测试：渠道草稿探测（D9-T1）。
//!
//! 用本地假上游（axum fallback 接任意路径）直接打 [`gateway_core::probe::probe_draft`]：
//! 探测是纯函数式 API（给一个 `reqwest::Client`），不需要起整个网关。
//!
//! 用例：
//! 1. 200 + 合法 body → `Passed`
//! 2. **200 + HTML 拦截页 → `Failed{Protocol}`**（这条最重要：防「假绿」）
//! 3. 401 → `Failed{Authentication}`
//! 4. 404 + 模型语义 → `Failed{Model}`
//! 5. 响应慢于 `per_probe_timeout` → `Failed{Timeout}`
//! 6. 连接拒绝 → `Failed{Network}`
//! 7. 云元数据地址 → `Failed{UrlBlocked}`，且**一个请求都不发**
//! 8. `127.0.0.1` + `allow_loopback=false` → `Failed{UrlBlocked}`；`=true` → 正常探测
//! 9. 未填模型 → `Skipped{NoModel}`
//! 10. 未知协议族 → `Failed{Protocol}`
//!
//! 注意：出站 client 必须 `.no_proxy()` —— 本机若开系统代理（Clash 等），
//! 打本地 mock 的请求会被代理接走并回 502（见 docs/bug和优化清单.md）。

use axum::body::Body;
use axum::response::Response;
use axum::Router;
use gateway_core::probe::{probe_draft, ProbeCategory, ProbeConfig, ProbeStatus, ProbeTarget};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Copy)]
enum Mode {
    /// 200 + 合法 chat 响应
    Ok,
    /// 200 + HTML 拦截页（假绿陷阱）
    Html,
    /// 401 错误体
    Unauthorized,
    /// 404 + 模型语义
    ModelNotFound,
    /// 200 但慢 1s
    Slow,
}

async fn spawn_mock(mode: Mode) -> (u16, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let app = Router::new().fallback({
        let hits = hits.clone();
        move || {
            let hits = hits.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                match mode {
                    Mode::Ok => json_response(
                        200,
                        json!({
                            "id": "c1",
                            "object": "chat.completion",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "pong"},
                                "finish_reason": "stop"
                            }]
                        })
                        .to_string(),
                    ),
                    Mode::Html => Response::builder()
                        .status(200)
                        .header("content-type", "text/html")
                        .body(Body::from("<html><body>Access denied</body></html>"))
                        .unwrap(),
                    Mode::Unauthorized => json_response(
                        401,
                        json!({"error": {"message": "invalid api key", "type": "auth_error"}})
                            .to_string(),
                    ),
                    Mode::ModelNotFound => json_response(
                        404,
                        json!({"error": {"message": "The model `gpt-9` does not exist"}})
                            .to_string(),
                    ),
                    Mode::Slow => {
                        tokio::time::sleep(Duration::from_millis(1000)).await;
                        json_response(200, json!({"choices": []}).to_string())
                    }
                }
            }
        }
    });
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr.port(), hits)
}

fn json_response(status: u16, body: String) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap()
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

fn target(base_url: String, model: &str) -> ProbeTarget {
    ProbeTarget {
        family: "openai_compat".into(),
        base_url,
        api_key: Some("sk-test-abcdefghijklmnop".into()),
        model: model.into(),
        extra_headers: None,
    }
}

/// 允许访问本机（测试里的 mock 都在 127.0.0.1）
fn loopback_cfg() -> ProbeConfig {
    ProbeConfig {
        allow_loopback: true,
        ..ProbeConfig::default()
    }
}

/// 主探测（非信息性）那一行
fn primary(report: &gateway_core::probe::DraftProbeReport) -> &gateway_core::probe::ProbeOutcome {
    report
        .results
        .iter()
        .find(|o| !o.informational)
        .expect("报告里应有主探测行")
}

// ---------------------------------------------------------------- 用例

#[tokio::test(flavor = "multi_thread")]
async fn valid_json_200_is_passed() {
    let (port, hits) = spawn_mock(Mode::Ok).await;
    let report = probe_draft(
        &client(),
        &target(format!("http://127.0.0.1:{port}/v1"), "gpt-x"),
        &loopback_cfg(),
    )
    .await;

    let p = primary(&report);
    assert_eq!(p.status, ProbeStatus::Passed, "{:?}", p.message);
    assert_eq!(p.category, None);
    assert_eq!(p.endpoint, "chat_completions");
    assert!(p.cost_possible, "真的发了推理请求");
    assert!(hits.load(Ordering::SeqCst) >= 1);
    // openai_compat 会额外探一次 /responses（信息性）：mock 回的是 chat 形状，
    // 缺 output/status → 判 Protocol 失败，但**不影响**主探测结论
    assert!(report.results.iter().any(|o| o.informational));
    assert!(!report.fingerprint.is_empty());
    assert!(!report.run_id.is_empty());
}

/// 最重要的一条：200 + HTML 拦截页必须判失败，否则用户拿到的是「假绿」。
#[tokio::test(flavor = "multi_thread")]
async fn html_intercept_page_is_protocol_failure() {
    let (port, _hits) = spawn_mock(Mode::Html).await;
    let report = probe_draft(
        &client(),
        &target(format!("http://127.0.0.1:{port}/v1"), "gpt-x"),
        &loopback_cfg(),
    )
    .await;

    let p = primary(&report);
    assert_eq!(p.status, ProbeStatus::Failed);
    assert_eq!(p.category, Some(ProbeCategory::Protocol));
    assert!(p.message.contains("不是合法 JSON"), "{}", p.message);
}

#[tokio::test(flavor = "multi_thread")]
async fn unauthorized_is_authentication_failure() {
    let (port, _hits) = spawn_mock(Mode::Unauthorized).await;
    let report = probe_draft(
        &client(),
        &target(format!("http://127.0.0.1:{port}/v1"), "gpt-x"),
        &loopback_cfg(),
    )
    .await;

    let p = primary(&report);
    assert_eq!(p.status, ProbeStatus::Failed);
    assert_eq!(p.category, Some(ProbeCategory::Authentication));
}

#[tokio::test(flavor = "multi_thread")]
async fn model_not_found_is_model_failure() {
    let (port, _hits) = spawn_mock(Mode::ModelNotFound).await;
    let report = probe_draft(
        &client(),
        &target(format!("http://127.0.0.1:{port}/v1"), "gpt-x"),
        &loopback_cfg(),
    )
    .await;

    let p = primary(&report);
    assert_eq!(p.status, ProbeStatus::Failed);
    assert_eq!(p.category, Some(ProbeCategory::Model));
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_upstream_times_out() {
    let (port, _hits) = spawn_mock(Mode::Slow).await;
    let cfg = ProbeConfig {
        per_probe_timeout: Duration::from_millis(200),
        ..loopback_cfg()
    };
    let report = probe_draft(
        &client(),
        &target(format!("http://127.0.0.1:{port}/v1"), "gpt-x"),
        &cfg,
    )
    .await;

    let p = primary(&report);
    assert_eq!(p.status, ProbeStatus::Failed);
    assert_eq!(p.category, Some(ProbeCategory::Timeout), "{}", p.message);
}

#[tokio::test(flavor = "multi_thread")]
async fn connection_refused_is_network_failure() {
    // 占一个端口再放掉 → 该端口上确定没有监听者
    let port = {
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        l.local_addr().unwrap().port()
    };
    let report = probe_draft(
        &client(),
        &target(format!("http://127.0.0.1:{port}/v1"), "gpt-x"),
        &loopback_cfg(),
    )
    .await;

    let p = primary(&report);
    assert_eq!(p.status, ProbeStatus::Failed);
    assert_eq!(p.category, Some(ProbeCategory::Network), "{}", p.message);
}

/// SSRF：云元数据地址必须被拦，且**一个请求都不发**。
#[tokio::test(flavor = "multi_thread")]
async fn cloud_metadata_address_is_blocked_without_any_request() {
    let (port, hits) = spawn_mock(Mode::Ok).await;
    // 同时验证：即便 allow_loopback=true 也不放行 link-local
    for cfg in [ProbeConfig::default(), loopback_cfg()] {
        let report = probe_draft(
            &client(),
            &target("http://169.254.169.254/latest/meta-data/".into(), "gpt-x"),
            &cfg,
        )
        .await;
        let p = primary(&report);
        assert_eq!(p.status, ProbeStatus::Failed);
        assert_eq!(p.category, Some(ProbeCategory::UrlBlocked), "{}", p.message);
        assert_eq!(p.latency_ms, 0, "被拦的地址不该产生任何网络往返");
        assert!(!p.cost_possible);
    }
    // 上面的探测与这个 mock 无关，所以它不该被打过
    assert_eq!(hits.load(Ordering::SeqCst), 0, "被拦地址不该发出请求");
    let _ = port;
}

/// 本机地址：默认拦，显式勾选后放行（Ollama / LM Studio 场景）。
#[tokio::test(flavor = "multi_thread")]
async fn loopback_needs_explicit_opt_in() {
    let (port, hits) = spawn_mock(Mode::Ok).await;
    let t = target(format!("http://127.0.0.1:{port}/v1"), "gpt-x");

    // 未勾选 → 拦，且不发请求
    let report = probe_draft(&client(), &t, &ProbeConfig::default()).await;
    let p = primary(&report);
    assert_eq!(p.category, Some(ProbeCategory::UrlBlocked), "{}", p.message);
    assert_eq!(hits.load(Ordering::SeqCst), 0);

    // 勾选 → 正常探测
    let report = probe_draft(&client(), &t, &loopback_cfg()).await;
    assert_eq!(primary(&report).status, ProbeStatus::Passed);
    assert!(hits.load(Ordering::SeqCst) >= 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_model_is_skipped() {
    let (port, hits) = spawn_mock(Mode::Ok).await;
    let report = probe_draft(
        &client(),
        &target(format!("http://127.0.0.1:{port}/v1"), ""),
        &loopback_cfg(),
    )
    .await;

    assert_eq!(report.results.len(), 1, "没模型只出一行");
    let p = &report.results[0];
    assert_eq!(p.status, ProbeStatus::Skipped);
    assert_eq!(p.category, Some(ProbeCategory::NoModel));
    assert_eq!(p.latency_ms, 0);
    assert_eq!(hits.load(Ordering::SeqCst), 0, "跳过时不该发请求");
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_family_is_protocol_failure() {
    let (port, hits) = spawn_mock(Mode::Ok).await;
    let mut t = target(format!("http://127.0.0.1:{port}/v1"), "gpt-x");
    t.family = "made_up_family".into();
    let report = probe_draft(&client(), &t, &loopback_cfg()).await;

    assert_eq!(report.results.len(), 1);
    let p = &report.results[0];
    assert_eq!(p.status, ProbeStatus::Failed);
    assert_eq!(p.category, Some(ProbeCategory::Protocol));
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

/// 探测结论里不能出现明文 key（上游回显请求时会把它带回来）。
#[tokio::test(flavor = "multi_thread")]
async fn messages_never_contain_the_api_key() {
    let (port, _hits) = spawn_mock(Mode::Unauthorized).await;
    let t = target(format!("http://127.0.0.1:{port}/v1"), "gpt-x");
    let key = t.api_key.clone().unwrap();
    let report = probe_draft(&client(), &t, &loopback_cfg()).await;

    for o in &report.results {
        assert!(!o.message.contains(&key), "{}", o.message);
    }
}
