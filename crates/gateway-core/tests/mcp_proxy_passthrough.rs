//! `/mcp` 代理转发的**结果透传**端到端回归（bug 10）。
//!
//! 拓扑：MCP 客户端 → 网关 `POST /mcp` → 假 stdio MCP Server（真实子进程，
//! `CARGO_BIN_EXE_mcp_fake_server`）。
//!
//! 早期实现把上游结果包成 `{source, result}` 再 `to_string()` 塞进单个 text 块，
//! 造成两个真实缺陷（dsh 侧实测）：
//! ① 上游**工具级失败**（`isError: true`）被外层 `"isError": false` 压平，
//!    客户端按外层判定 → agent 把失败当成功；
//! ② 上游 `content` 结构（image / resource_link）与 `structuredContent` 全丢，
//!    客户端再也做不了图片投影。
//!
//! 本文件不碰进程级环境变量（无 `JAI_*` 覆写），测试可并行。

use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use serde_json::{json, Value};

const FAKE: &str = env!("CARGO_BIN_EXE_mcp_fake_server");

struct Fixture {
    port: u16,
    key: String,
    _keepalive: (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ),
}

async fn fixture() -> Fixture {
    let dir = std::env::temp_dir().join(format!(
        "jai-mcp-proxy-e2e-{}-{}",
        std::process::id(),
        rand::random::<u32>()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let main_path = dir.join("main.db");
    let (db, main_path) = (Db::open(main_path.to_str().unwrap()).unwrap(), main_path);
    let (logs, _task) =
        gateway_core::store::logs::spawn_logger(main_path.to_str().unwrap()).unwrap();

    let now = store::now_ms();
    db.with(|c| {
        store::mcp_insert(
            c,
            &store::McpServerRow {
                id: "s-fake".into(),
                name: "extern".into(),
                kind: "stdio".into(),
                command: Some(FAKE.into()),
                args: Some(r#"["--stdio-fake"]"#.into()),
                url: None,
                // 唯一 env 参与 stdio 池键（mcp.rs::pool_key）：每个 fixture 各自独立
                // 子进程，避免「同一进程内多个 #[tokio::test] runtime 复用同一连接」
                // ——那会报 `A Tokio 1.x context was found, but it is being shutdown`
                // （生产无此问题：Tauri 命令与网关共用同一个 async_runtime）。
                env: Some(format!(r#"{{"FAKE_TEST_ID":"{}"}}"#, rand::random::<u32>())),
                enabled: true,
                // 显式开启代理执行：只有 proxy_allowed=1 才进动态工具列表/可转发
                proxy_allowed: true,
                created_at: now,
                updated_at: now,
            },
        )?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();

    let key = "sk-jai-mcp-proxy-test-0000000000000000";
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
        _keepalive: (stop_tx, guard),
    }
}

impl Fixture {
    async fn rpc(&self, body: Value) -> Value {
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{}/mcp", self.port);
        let resp = client
            .post(&url)
            .header("host", format!("127.0.0.1:{}", self.port))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200, "/mcp 应 200: {body}");
        resp.json().await.unwrap()
    }

    /// 调一次代理工具，返回 `tools/call` 的 result 对象。
    async fn call(&self, name: &str, args: Value) -> Value {
        let resp = self
            .rpc(json!({
                "jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":name,"arguments":args}
            }))
            .await;
        resp["result"].clone()
    }

    /// `tools/list` 里可见的工具名集合（验证 proxy_allowed 过滤 + 命名前缀）。
    async fn tool_names(&self) -> Vec<String> {
        let resp = self
            .rpc(json!({"jsonrpc":"2.0","id":9,"method":"tools/list","params":{}}))
            .await;
        resp["result"]["tools"]
            .as_array()
            .expect("tools/list 应返回数组")
            .iter()
            .filter_map(|t| t["name"].as_str().map(str::to_string))
            .collect()
    }
}

/// bug 10 主回归：上游工具级失败必须冒泡，且失败正文原样保留。
#[tokio::test(flavor = "multi_thread")]
async fn proxied_tool_failure_propagates_iserror() {
    let fx = fixture().await;
    let result = fx
        .call("extern__fail", json!({"text": "boom-from-fake"}))
        .await;

    assert_eq!(
        result["isError"], true,
        "上游 isError=true 必须冒泡（否则客户端把失败当成功）: {result}"
    );
    assert_eq!(result["content"][0]["type"], "text");
    assert_eq!(
        result["content"][0]["text"], "boom-from-fake",
        "失败正文不得被二次字符串化: {result}"
    );
    assert_eq!(result["source"], "jai-gateway-proxy/extern");
}

/// 成功路径同样原样透传：`content` 是真正的 MCP content 数组，
/// 而不是「整个上游结果序列化成的一个 text 块」。
#[tokio::test(flavor = "multi_thread")]
async fn proxied_tool_success_passes_content_through() {
    let fx = fixture().await;
    let result = fx
        .call("extern__echo", json!({"text": "JAI-PROXY-OK"}))
        .await;

    assert_eq!(result["isError"], false);
    let content = result["content"]
        .as_array()
        .unwrap_or_else(|| panic!("content 应为数组: {result}"));
    assert_eq!(
        content.len(),
        1,
        "透传不应额外插入/包裹块（否则 agent 看到的是嵌套 JSON）: {result}"
    );
    assert_eq!(content[0]["type"], "text");
    assert_eq!(
        content[0]["text"], "JAI-PROXY-OK",
        "文本应原样，而非 {{\"result\":...}} 这类二次编码: {result}"
    );
}

/// 动态工具有前缀与来源标注，且未开启代理的 server 不出现在列表里。
#[tokio::test(flavor = "multi_thread")]
async fn proxied_tools_are_listed_with_source_prefix() {
    let fx = fixture().await;
    let names = fx.tool_names().await;
    assert!(
        names.iter().any(|n| n == "extern__echo"),
        "开启代理的 server 工具应可见: {names:?}"
    );
    assert!(
        names.iter().all(|n| !n.starts_with("skill__")),
        "无技能时不应出现 skill__* 工具: {names:?}"
    );

    // 静态只读台账工具仍保持「JSON 文本」形态（不走代理透传）
    let detail = fx
        .call("get_mcp_server_detail", json!({"name": "extern"}))
        .await;
    let text = detail["content"][0]["text"].as_str().unwrap_or_default();
    let payload: Value = serde_json::from_str(text).expect("静态工具应返回 JSON 文本");
    assert_eq!(payload["connect"]["transport"], "stdio");
    assert_eq!(payload["connect"]["args"][0], "--stdio-fake");
}
