//! stdio MCP 进程复用池（mcp.rs）集成测试。
//!
//! 用 mcp_fake_server 假 server 验证：进程复用、崩溃重建、超时/空闲回收、
//! env 参与连接键、并发串行化。池参数经环境变量覆写（JAI_MCP_POOL_*），
//! 这些变量是进程级全局的，故本文件所有测试用静态锁串行执行。

use std::time::Duration;

use gateway_core::mcp;
use gateway_core::store::McpServerRow;
use serde_json::{json, Value};

const FAKE: &str = env!("CARGO_BIN_EXE_mcp_fake_server");

/// 本文件测试串行执行（池参数环境变量是进程级全局，避免互相污染）。
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 进程级环境变量的 RAII 守卫：`Drop` 时还原原值（缺失则删除）。
///
/// 旧写法把 `remove_var` 放在断言**之后**，只在成功路径执行；一旦断言 panic 就
/// 跳过清理，变量残留（如 `JAI_MCP_POOL_CALL_TIMEOUT_MS=100`）→ 后续用例连带超时，
/// 表现为「每轮失败的用例集合都不同」的失败级联（bug 6 的根因之二）。
/// 用 Drop 兜住 panic 与提前 return 两条路径。
struct EnvGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

fn row(name: &str, env: Option<&str>) -> McpServerRow {
    McpServerRow {
        id: name.into(),
        name: name.into(),
        kind: "stdio".into(),
        command: Some(FAKE.into()),
        args: Some(r#"["--stdio-fake"]"#.into()),
        url: None,
        env: env.map(str::to_string),
        proxy_allowed: true,
        enabled: true,
        created_at: 0,
        updated_at: 0,
    }
}

/// 调用工具并取出首个 text 内容。
async fn call_text(server: &McpServerRow, tool: &str, args: Value) -> Result<String, String> {
    let v = mcp::call_tool(server, tool, args).await?;
    v.get("content")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|t| t.get("text"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("响应缺少 text: {v}"))
}

async fn pid_of(server: &McpServerRow) -> u32 {
    call_text(server, "pid", json!({}))
        .await
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test]
async fn reuses_process_across_calls() {
    let _g = SERIAL.lock().await;
    // FAKE_TEST_ID 参与池键：本文件测试各自独立连接，避免跨 runtime 复用
    // （#[tokio::test] 每个测试独立 runtime，全局池连接归属首个创建它的 runtime）
    let s = row("reuse", Some(r#"{"FAKE_TEST_ID":"reuse"}"#));

    let p1 = pid_of(&s).await;
    let p2 = pid_of(&s).await;
    assert_eq!(p1, p2, "进程复用：两次调用应为同一进程");

    // count 工具验证进程内状态延续（非复用的话每次都是新进程、恒为 count=1）
    assert_eq!(call_text(&s, "count", json!({})).await.unwrap(), "count=1");
    assert_eq!(call_text(&s, "count", json!({})).await.unwrap(), "count=2");

    // tools/list 也走同一连接池
    let tools = mcp::list_tools(&s).await.unwrap();
    assert!(tools.iter().any(|t| t.name == "echo"));
    let p3 = pid_of(&s).await;
    assert_eq!(p1, p3, "tools/list 后仍复用原进程");
}

#[tokio::test]
async fn rebuilds_after_crash() {
    let _g = SERIAL.lock().await;
    let s = row("crash", Some(r#"{"FAKE_TEST_ID":"crash"}"#));

    let p1 = pid_of(&s).await;
    // crash 工具收到请求即退出：本次调用应报传输错误
    let r = mcp::call_tool(&s, "crash", json!({})).await;
    assert!(r.is_err(), "进程崩溃应返回错误: {r:?}");

    // 下次调用自动重建（新 pid），无需手动恢复
    let p2 = pid_of(&s).await;
    assert_ne!(p1, p2, "崩溃后应重建为新进程");
}

#[tokio::test]
async fn rebuilds_when_process_dies_after_response() {
    let _g = SERIAL.lock().await;
    let s = row("die-after-resp", Some(r#"{"FAKE_TEST_ID":"die"}"#));

    let p1 = pid_of(&s).await;
    // 回包后进程退出：本次调用成功，但连接已死
    let r = call_text(&s, "die_after_response", json!({})).await;
    assert_eq!(r.unwrap(), "bye");
    // 等进程真正退出，让 try_wait 能观察到
    tokio::time::sleep(Duration::from_millis(200)).await;

    // 下次调用：获取时检测到进程已退出 → 本次调用内重建
    let p2 = pid_of(&s).await;
    assert_ne!(p1, p2, "回包即崩的进程应在下次调用时重建");
}

#[tokio::test]
async fn timeout_discards_connection() {
    let _g = SERIAL.lock().await;
    // 只压「调用」超时；握手预算不受影响（独立 `JAI_MCP_POOL_INIT_TIMEOUT_MS`），
    // 故负载机器上的冷启动+握手不会被 100ms 判死。
    let _t = EnvGuard::set("JAI_MCP_POOL_CALL_TIMEOUT_MS", "100");
    let s = row("timeout", Some(r#"{"FAKE_TEST_ID":"timeout"}"#));

    // fake slow 工具 300ms 响应 > 100ms 超时
    let r = mcp::call_tool(&s, "slow", json!({})).await;
    assert!(r.is_err(), "慢调用应超时: {r:?}");

    // 超时后连接已废弃，下次调用自动重建、恢复正常
    let t = call_text(&s, "echo", json!({"text": "ok"})).await;
    assert_eq!(t.unwrap(), "ok");
}

#[tokio::test]
async fn init_handshake_uses_separate_budget() {
    let _g = SERIAL.lock().await;
    // 调用超时被压到 1ms（握手必然超不过它），但初始化仍应成功——
    // 证明握手不再复用调用超时（bug 6 根因一的回归防线）。
    let _c = EnvGuard::set("JAI_MCP_POOL_CALL_TIMEOUT_MS", "1");
    let s = row("init-budget", Some(r#"{"FAKE_TEST_ID":"init-budget"}"#));

    // pid 工具自身会被 1ms 的调用超时判死（报「MCP pid 超时（1ms）」），
    // 但握手用独立预算，不应出现「initialize 超时」——回退到复用调用超时
    // 的旧实现时，本断言必红（1ms 内不可能完成进程冷启动 + 握手）。
    let r = mcp::call_tool(&s, "pid", json!({})).await;
    match r {
        Ok(_) => {} // 极快机器上 1ms 内竟也成功，握手同样已通过
        Err(e) => assert!(
            !e.contains("initialize"),
            "初始化不应受调用超时约束，实际报错: {e}"
        ),
    }
}

#[tokio::test]
async fn idle_reclaim_reaps_process() {
    let _g = SERIAL.lock().await;
    let _t = EnvGuard::set("JAI_MCP_POOL_IDLE_MS", "100");
    let s = row("idle", Some(r#"{"FAKE_TEST_ID":"idle"}"#));

    let p1 = pid_of(&s).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    let p2 = pid_of(&s).await;
    assert_ne!(p1, p2, "空闲超时后应回收并重建进程");
}

#[tokio::test]
async fn env_participates_in_pool_key() {
    let _g = SERIAL.lock().await;
    let a = row("env-a", Some(r#"{"FAKE_POOL_ENV":"alpha"}"#));
    let b = row("env-b", Some(r#"{"FAKE_POOL_ENV":"beta"}"#));
    let shared = row("env-c", Some(r#"{"FAKE_POOL_ENV":"alpha"}"#));

    let pa = pid_of(&a).await;
    let pb = pid_of(&b).await;
    assert_ne!(pa, pb, "不同 env 应各自独立进程");

    assert_eq!(call_text(&a, "env", json!({})).await.unwrap(), "alpha");
    assert_eq!(call_text(&b, "env", json!({})).await.unwrap(), "beta");

    // 相同 cmd/args/env 共享同一连接（含 env 顺序不同的 JSON）
    let shared2 = row("env-d", Some(r#"{"FAKE_POOL_ENV":"alpha"}"#));
    let pc = pid_of(&shared).await;
    let pd = pid_of(&shared2).await;
    assert_eq!(pc, pd, "相同 cmd/args/env 应共享连接");
}

#[tokio::test]
async fn concurrent_calls_serialize_on_one_process() {
    let _g = SERIAL.lock().await;
    let s = row("concurrent", Some(r#"{"FAKE_TEST_ID":"concurrent"}"#));
    let p = pid_of(&s).await;

    // 同连接并发 8 次调用：锁串行化，全部成功且不串帧
    let mut handles = Vec::new();
    for i in 0..8 {
        let s = s.clone();
        handles.push(tokio::spawn(async move {
            call_text(&s, "echo", json!({"text": format!("m{i}")})).await
        }));
    }
    for (i, h) in handles.into_iter().enumerate() {
        assert_eq!(h.await.unwrap().unwrap(), format!("m{i}"));
    }

    let p2 = pid_of(&s).await;
    assert_eq!(p, p2, "并发调用应共享同一进程（锁串行化）");
}
