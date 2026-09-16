//! `/mcp` 代理转发的**等待预算**回归（bug 9 网关侧）。
//!
//! 背景（2026-09-15 dsh 实测）：dsh 客户端单次 MCP 调用默认 **60s 硬中止**
//! （错误码 -32001）。旧实现网关等 120s，比客户端更能等 —— 结果网关等满
//! 60.007s 才拿到上游结果，客户端 60.000s 已放弃，**差 7ms 输掉竞速**：
//! agent 只看到 `MCP error -32001: Request timed out`，同批并行的其它工具
//! 调用还一起被作废。
//!
//! 现在网关按 `JAI_MCP_PROXY_CALL_TIMEOUT_MS`（默认 55s）提前失败，并给出
//! **可执行的工具级错误**（isError=true + 长命令改走「先启动后轮询」的指引）。
//!
//! ⚠ 本文件只应保留这一个测试：`JAI_*` 是进程级环境变量，多测试会互相污染
//! （见 bug 6 的失败级联教训）。

use std::time::{Duration, Instant};

use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use serde_json::{json, Value};

const FAKE: &str = env!("CARGO_BIN_EXE_mcp_fake_server");

/// 进程级环境变量守卫：drop 时还原（含 panic 路径）。
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

async fn fixture() -> (u16, String) {
    let dir = std::env::temp_dir().join(format!(
        "jai-mcp-proxy-timeout-{}-{}",
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
                // 唯一 env 参与池键：本文件只有这一个测试，但仍显式隔离，
                // 避免将来加测试时跨 runtime 复用连接（见 bug 6 的教训）。
                env: Some(format!(r#"{{"FAKE_TEST_ID":"{}"}}"#, rand::random::<u32>())),
                enabled: true,
                proxy_allowed: true,
                created_at: now,
                updated_at: now,
            },
        )?;
        Ok::<_, store::StoreError>(())
    })
    .unwrap();

    let key = "sk-jai-mcp-timeout-test-000000000000000";
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
    // 保活到进程结束（测试进程退出即释放）
    std::mem::forget((stop_tx, guard));
    (port, key.to_string())
}

async fn call(port: u16, key: &str, name: &str, args: Value) -> Value {
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/mcp");
    let resp = client
        .post(&url)
        .header("host", format!("127.0.0.1:{port}"))
        .bearer_auth(key)
        .json(&json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":name,"arguments":args}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    resp.json::<Value>().await.unwrap()
}

/// bug 9 网关侧回归：上游工具比预算慢时，网关必须**提前**返回工具级失败
/// （而不是让客户端等到自己的 60s 硬中止），且错误文案可执行。
#[tokio::test]
async fn proxy_call_timeout_fails_fast_with_actionable_error() {
    let (port, key) = fixture().await;

    // 预热：首次调用含「spawn + initialize 握手」，先让它落在默认预算里，
    // 否则 150ms 的小预算会被冷启动吃掉（这本身就是我们要避免的假阳性）。
    let warm = call(port, &key, "extern__echo", json!({"text": "warm"})).await;
    assert_eq!(warm["result"]["isError"], false, "预热调用应成功: {warm}");

    let _guard = EnvGuard::set("JAI_MCP_PROXY_CALL_TIMEOUT_MS", "200");
    let started = Instant::now();
    // 假 server 的 sleep 工具睡 3s，远大于 200ms 预算
    let resp = call(port, &key, "extern__sleep", json!({"ms": 3000})).await;
    let elapsed = started.elapsed();

    let result = &resp["result"];
    assert_eq!(
        result["isError"], true,
        "网关主动放弃等待必须作为工具级失败返回（不能是客户端超时）: {result}"
    );
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("未返回"),
        "文案应说明「网关已主动放弃本次等待」: {text}"
    );
    assert!(
        text.contains("terminal_start") && text.contains("terminal_poll"),
        "应给出可执行指引（长命令改走先启动后轮询）: {text}"
    );
    assert!(
        text.contains("JAI_MCP_PROXY_CALL_TIMEOUT_MS"),
        "应提示调参入口: {text}"
    );
    assert!(
        elapsed < Duration::from_millis(2000),
        "网关应在预算内提前失败，而不是等上游 3s（elapsed={elapsed:?}）: {result}"
    );
}
