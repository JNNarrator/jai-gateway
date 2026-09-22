//! 直通路径的「零可见输出的截断轮」诊断（`empty_truncation_diagnostic` 的直通半边）。
//!
//! 拓扑：客户端(OpenAI chat) → JAI 网关 → mock OpenAI 上游，**同族字节直通**。
//!
//! 背景（2026-09-22 真机，基元律动/deepseek-flash，460k 上下文）：上游以
//! `finish_reason:"length"` 收尾，整轮只有 reasoning 增量、正文为空、无工具调用
//! （completion_tokens 只有 16/96/165/250）。客户端（PI-Desktop 的 agent 循环）据此判
//! silent turn → 追加 `<no_output_recovery>` 重跑一次 → 仍静默 → `EMPTY_MODEL_RESPONSE`。
//!
//! 转换路径由 `empty_truncation_diagnostic` 覆盖；直通路径是字节转发、没有 IR，此前完全
//! 没有可见输出信息 ⇒ 同样形状的轮次在那里仍是 `error_kind=NULL` 的干净 200。
//! 本文件钉住三件事：
//! 1. 直通流式下这种轮次被标成 `OutputTruncatedEmpty`，且摘要带上客户端声明的输出预算；
//! 2. **有正文**的截断、**有工具调用**的轮次一律不标（探针不得误判）；
//! 3. 探针只读不写：客户端收到的字节与上游发出的**完全一致**；
//! 4. 工具调用计数（`request_logs.tool_calls`）：直通流式此前硬编码 0，现按 id 去重计数，
//!    且**正文在前时也必须数到后面的工具调用**（探针不能提前收工）；
//! 5. 「有正文但撞上预算上限」单独标 `OutputBudgetClipped`，**没撞上不标**。

use axum::body::Body;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use serde_json::{json, Value};

mod common;

// ---------------------------------------------------------------- mock 上游

/// 只有推理、正文为空，以 `finish_reason:"length"` 收尾（真机形状）
const REASONING_ONLY_LENGTH: &str = "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"Hmm, no obvious shadow at the function top level.\"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"Wait - line 1478 is my new code.\"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"length\"}],\"usage\":{\"prompt_tokens\":460817,\"completion_tokens\":16}}\n\n\
data: [DONE]\n\n";

/// 有正文的截断：**正常**截断，不得被标成异常
const TEXT_LENGTH: &str = "data: {\"id\":\"c2\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"cut here\"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c2\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"length\"}],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":8}}\n\n\
data: [DONE]\n\n";

/// 只有工具调用、没有正文，**且被 length 截断**（工具参数写到一半就被切断，真机常见）。
///
/// 这一例刻意让 `finish_reason` 落在截断类上：若收尾是 `tool_calls`，判定根本不会看
/// 「有没有可见输出」（`empty_truncation_diagnostic` 只认 max_tokens/safety），
/// 用例会变成空洞通过。落在 `length` 上才能真的检验「工具调用算可见输出」。
const TOOL_CALLS_TRUNCATED: &str = "data: {\"id\":\"c3\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c3\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"p\\\":\\\"a.rs\\\"}\"}}]},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c3\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"length\"}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":7}}\n\n\
data: [DONE]\n\n";

/// 有正文、且**确实撞上预算上限**（`completion_tokens == max_tokens`）：P2 形状。
///
/// 真机形状（2026-09-22 10:44，基元律动/deepseek-flash，280k 上下文）：客户端声明
/// `max_tokens=8192`，上游 `finish_reason:"length"`，`completion_tokens` 恰好 8192 ——
/// 预算被真正用尽，用户拿到的是被掐短的答案。
const TEXT_CLIPPED_AT_BUDGET: &str = "data: {\"id\":\"c5\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"The fix is to move the let binding\"},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c5\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"length\"}],\"usage\":{\"prompt_tokens\":280726,\"completion_tokens\":8192}}\n\n\
data: [DONE]\n\n";

/// **正文在前、工具调用在后**，且两块**分两次**下发（`spawn_sse_mock` 逐块吐）：
/// 观测探针必须全程解析，见到正文不能提前收工 —— 提前收工会把这一轮的工具调用数成 0
/// （这正是「只判可见输出」那种实现会踩的坑）。分块是关键：整包一次到达时网关只看到一个
/// chunk，跨 chunk 的 bug 测不出来。
const TEXT_THEN_TOOL_CALLS_HEAD: &str = "data: {\"id\":\"c4\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"let me look\"},\"finish_reason\":null}]}\n\n";

const TEXT_THEN_TOOL_CALLS_TAIL: &str = "data: {\"id\":\"c4\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_a\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c4\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call_b\",\"type\":\"function\",\"function\":{\"name\":\"list_dir\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n\
data: {\"id\":\"c4\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":12}}\n\n\
data: [DONE]\n\n";

/// 把给定的 SSE 片段**逐块**吐出（每块之间 120ms），逼真流式。
///
/// 逐块是刻意的：整包一次到达时，网关的 `bytes_stream()` 只会 yield 一个 chunk，
/// 「跨 chunk 的状态机 bug」（例如见到正文就提前收工）会被完全掩盖。
async fn spawn_sse_mock(chunks: &'static [&'static str]) -> u16 {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || async move {
            let stream = futures_util::stream::unfold(0usize, move |i| async move {
                let piece = *chunks.get(i)?;
                tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                Some((
                    Ok::<_, std::io::Error>(axum::body::Bytes::from(piece)),
                    i + 1,
                ))
            });
            Response::builder()
                .status(200)
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(stream))
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

// ---------------------------------------------------------------- 夹具

struct Fixture {
    port: u16,
    db: Db,
    key: String,
    _keepalive: (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ),
}

async fn fixture(chunks: &'static [&'static str]) -> Fixture {
    let up_port = spawn_sse_mock(chunks).await;

    let (db, main_path) = {
        let dir = std::env::temp_dir().join(format!(
            "jai-trunc-{}-{}",
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
    db.with(|c| {
        store::provider_insert(
            c,
            &store::ProviderRow {
                id: "p-up".into(),
                name: "up".into(),
                base_url: format!("http://127.0.0.1:{up_port}/v1"),
                family: "openai_compat".into(),
                enabled: true,
                priority: 1,
                weight: 1,
                extra_headers: None,
                api_key: Some("sk-up".into()),
                website: None,
                last_ok_at: None,
                last_err_at: None,
                last_err_msg: None,
                max_tools: None,
                reasoning_effort_levels: None,
                created_at: now,
                updated_at: now,
            },
        )?;
        store::model_upsert(c, "p-up", "m-test", Some(512_000), 4096, None, None)?;
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
        db,
        key: key.to_string(),
        _keepalive: (stop_tx, guard),
    }
}

impl Fixture {
    /// 发一次直通流式 chat 请求，返回 (状态码, 原始响应体文本)
    async fn post_chat_stream(&self, body: Value) -> (u16, String) {
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
        (status, resp.text().await.unwrap_or_default())
    }

    async fn recent_logs(&self, n: i64) -> Vec<gateway_core::store::logs::LogRowView> {
        common::logs_settled(&self.db, n, std::time::Duration::from_secs(5)).await
    }
}

fn chat_body(stream: bool, max_tokens: Option<u64>) -> Value {
    let mut b = json!({
        "model": "m-test",
        "stream": stream,
        "messages": [{"role": "user", "content": "hi"}]
    });
    if let Some(m) = max_tokens {
        b["max_tokens"] = json!(m);
    }
    b
}

// ---------------------------------------------------------------- 用例

/// 直通流式：零可见输出的截断轮必须被标记，且摘要带上客户端声明的输出预算。
#[tokio::test(flavor = "multi_thread")]
async fn passthrough_reasoning_only_truncation_is_flagged() {
    let fx = fixture(&[REASONING_ONLY_LENGTH]).await;
    let (status, body) = fx.post_chat_stream(chat_body(true, Some(16))).await;
    assert_eq!(status, 200);

    // 直通契约：客户端收到的字节与上游发出的**完全一致**（探针只读不写）
    assert_eq!(body, REASONING_ONLY_LENGTH, "字节直通必须保持逐字节一致");

    let rows = fx.recent_logs(10).await;
    let row = rows
        .iter()
        .find(|r| r.http_status == 200 && r.is_stream)
        .expect("应有流式成功日志");
    assert_eq!(row.route_mode, "passthrough");
    assert_eq!(row.stop_reason.as_deref(), Some("max_tokens"));
    assert_eq!(row.tool_calls, 0, "这一轮没有工具调用");
    assert_eq!(
        row.error_kind.as_deref(),
        Some("OutputTruncatedEmpty"),
        "直通路径同样必须标出「零可见输出的截断轮」"
    );
    let summary = row.error_summary.as_deref().unwrap_or_default();
    assert!(
        summary.contains("没有任何可见输出"),
        "摘要要说清没有可见输出: {summary}"
    );
    assert!(
        summary.contains("max_output_tokens=16"),
        "摘要要带上客户端声明的输出预算（从 PeekRequest 读，网关不改写）: {summary}"
    );
}

/// 反向：有正文的截断是正常截断，不得被标成异常（否则日志被噪音淹没）。
#[tokio::test(flavor = "multi_thread")]
async fn passthrough_truncation_with_text_is_not_flagged() {
    let fx = fixture(&[TEXT_LENGTH]).await;
    let (status, body) = fx.post_chat_stream(chat_body(true, Some(4096))).await;
    assert_eq!(status, 200);
    assert_eq!(body, TEXT_LENGTH);

    let rows = fx.recent_logs(10).await;
    let row = rows
        .iter()
        .find(|r| r.http_status == 200 && r.is_stream)
        .expect("应有流式成功日志");
    assert_eq!(row.stop_reason.as_deref(), Some("max_tokens"));
    assert_eq!(row.tool_calls, 0);
    assert_eq!(row.error_kind, None, "有正文的截断不该带 error_kind");
}

/// 反向：**工具调用算可见输出** —— 被截断的工具调用轮次不得被标成「什么都没说」。
#[tokio::test(flavor = "multi_thread")]
async fn passthrough_truncated_tool_call_counts_as_visible_output() {
    let fx = fixture(&[TOOL_CALLS_TRUNCATED]).await;
    let (status, body) = fx.post_chat_stream(chat_body(true, None)).await;
    assert_eq!(status, 200);
    assert_eq!(body, TOOL_CALLS_TRUNCATED);

    let rows = fx.recent_logs(10).await;
    let row = rows
        .iter()
        .find(|r| r.http_status == 200 && r.is_stream)
        .expect("应有流式成功日志");
    assert_eq!(row.stop_reason.as_deref(), Some("max_tokens"));
    assert_eq!(
        row.error_kind, None,
        "工具调用是可见输出，即使被 length 截断也不该被标成「什么都没说」"
    );
    // 直通流式的 tool_calls 此前硬编码 0（「字节直通不解析 SSE 语义」）
    assert_eq!(
        row.tool_calls, 1,
        "直通流式必须按 id 去重数出工具调用（本用例只有 index 0 一个 id）"
    );
}

/// P2：直通路径下「有正文但撞上预算上限」必须被标成 `OutputBudgetClipped`。
#[tokio::test(flavor = "multi_thread")]
async fn passthrough_budget_clipped_with_text_is_flagged() {
    let fx = fixture(&[TEXT_CLIPPED_AT_BUDGET]).await;
    // 声明 8192，上游恰好产出 8192 → 撞上
    let (status, body) = fx.post_chat_stream(chat_body(true, Some(8192))).await;
    assert_eq!(status, 200);
    assert_eq!(body, TEXT_CLIPPED_AT_BUDGET);

    let rows = fx.recent_logs(10).await;
    let row = rows
        .iter()
        .find(|r| r.http_status == 200 && r.is_stream)
        .expect("应有流式成功日志");
    assert_eq!(row.route_mode, "passthrough");
    assert_eq!(row.stop_reason.as_deref(), Some("max_tokens"));
    assert_eq!(
        row.error_kind.as_deref(),
        Some("OutputBudgetClipped"),
        "有正文且撞上预算上限必须被标记"
    );
    let summary = row.error_summary.as_deref().unwrap_or_default();
    assert!(summary.contains("max_output_tokens=8192"), "{summary}");
    assert!(summary.contains("有可见输出"), "{summary}");
}

/// P2 反向：**没撞上**声明预算的截断不标 —— 否则客户端故意设小预算的正常场景、
/// 以及窗口边缘每一轮，都会被标成异常。
#[tokio::test(flavor = "multi_thread")]
async fn passthrough_clipped_below_budget_is_not_flagged() {
    let fx = fixture(&[TEXT_CLIPPED_AT_BUDGET]).await;
    // 上游只产出 8192，但客户端声明了远大的预算 → 没撞上，不标
    let (status, _) = fx.post_chat_stream(chat_body(true, Some(64_000))).await;
    assert_eq!(status, 200);

    let rows = fx.recent_logs(10).await;
    let row = rows
        .iter()
        .find(|r| r.http_status == 200 && r.is_stream)
        .expect("应有流式成功日志");
    assert_eq!(row.stop_reason.as_deref(), Some("max_tokens"));
    assert_eq!(
        row.error_kind, None,
        "产出没到声明预算就不该标成「被预算掐短」"
    );
}

/// 工具调用计数：**正文在前**的轮次也必须数到后面的工具调用。
///
/// 这是「探针不能提前收工」的守卫：若照「只判可见输出」的思路在 `seen_visible` 置位后
/// 就停止解析，这里会数成 0。
#[tokio::test(flavor = "multi_thread")]
async fn passthrough_counts_tool_calls_after_text() {
    let fx = fixture(&[TEXT_THEN_TOOL_CALLS_HEAD, TEXT_THEN_TOOL_CALLS_TAIL]).await;
    let (status, body) = fx.post_chat_stream(chat_body(true, None)).await;
    assert_eq!(status, 200);
    assert_eq!(
        body,
        format!("{TEXT_THEN_TOOL_CALLS_HEAD}{TEXT_THEN_TOOL_CALLS_TAIL}"),
        "字节直通必须保持逐字节一致（含跨 chunk 拼接）"
    );

    let rows = fx.recent_logs(10).await;
    let row = rows
        .iter()
        .find(|r| r.http_status == 200 && r.is_stream)
        .expect("应有流式成功日志");
    assert_eq!(row.route_mode, "passthrough");
    assert_eq!(row.stop_reason.as_deref(), Some("tool_use"));
    assert_eq!(
        row.tool_calls, 2,
        "正文在前时若见到正文就提前收工，这里会数成 0"
    );
    assert_eq!(row.error_kind, None);
}
