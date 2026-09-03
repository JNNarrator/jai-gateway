//! MCP（Model Context Protocol）客户端 —— 工具发现 / 调用。
//!
//! 当前支持：
//! - `stdio`：子进程按 MCP stdio 传输约定走换行分隔 JSON-RPC 2.0
//!   （进程复用池：懒初始化 + 进程复用 + 空闲超时回收 + 崩溃自动重建）
//! - `http` / `sse`：统一按 Streamable HTTP 风格向 `url` POST JSON-RPC
//!
//! 说明：这是轻量客户端，覆盖 Codex/Claude 类 tool calling 需要的
//! `initialize`、`tools/list`、`tools/call` 子集，不承载完整 MCP 生命周期。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::process::Stdio;
use std::sync::Mutex as StdMutex;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, Mutex};

use crate::store::McpServerRow;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

/// 解析 McpServerRow 中 JSON 对象字符串的 env；非法或空返回空 map。
fn server_env(server: &McpServerRow) -> std::collections::HashMap<String, String> {
    server
        .env
        .as_deref()
        .and_then(|s| serde_json::from_str::<std::collections::HashMap<String, String>>(s).ok())
        .unwrap_or_default()
}

/// 向一个 MCP Server 发起 `tools/list`。
pub async fn list_tools(server: &McpServerRow) -> Result<Vec<McpTool>, String> {
    match server.kind.as_str() {
        "stdio" => {
            let cmd = server.command.as_deref().ok_or("stdio MCP 缺少 command")?;
            let args: Vec<String> = server
                .args
                .as_deref()
                .map(|s| serde_json::from_str::<Vec<String>>(s).unwrap_or_default())
                .unwrap_or_default();
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let result = pooled_stdio_jsonrpc(
                cmd,
                &args,
                &server_env(server),
                "tools/list",
                &json!({}),
                &tx,
            )
            .await?;
            parse_tools_list(&result)
        }
        "http" | "sse" => {
            let url = server.url.as_deref().ok_or("http/sse MCP 缺少 url")?;
            let result = run_http_jsonrpc(url, "tools/list", &json!({})).await?;
            parse_tools_list(&result)
        }
        other => Err(format!("不支持的 MCP 类型: {other}")),
    }
}

/// 向一个 MCP Server 发起 `tools/call`。
pub async fn call_tool(
    server: &McpServerRow,
    name: &str,
    arguments: Value,
) -> Result<Value, String> {
    let params = json!({ "name": name, "arguments": arguments });
    match server.kind.as_str() {
        "stdio" => {
            let cmd = server.command.as_deref().ok_or("stdio MCP 缺少 command")?;
            let args: Vec<String> = server
                .args
                .as_deref()
                .map(|s| serde_json::from_str::<Vec<String>>(s).unwrap_or_default())
                .unwrap_or_default();
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let result =
                pooled_stdio_jsonrpc(cmd, &args, &server_env(server), "tools/call", &params, &tx)
                    .await?;
            Ok(unwrap_result(result))
        }
        "http" | "sse" => {
            let url = server.url.as_deref().ok_or("http/sse MCP 缺少 url")?;
            let result = run_http_jsonrpc(url, "tools/call", &params).await?;
            Ok(unwrap_result(result))
        }
        other => Err(format!("不支持的 MCP 类型: {other}")),
    }
}

/// 从 JSON-RPC 响应中取出 `result`；若没有 result（异常响应）则原样返回。
fn unwrap_result(v: Value) -> Value {
    v.get("result").cloned().unwrap_or(v)
}

fn parse_tools_list(v: &Value) -> Result<Vec<McpTool>, String> {
    let result = v.get("result").ok_or("MCP 响应缺少 result")?;
    let tools = result
        .get("tools")
        .and_then(Value::as_array)
        .ok_or("MCP result 缺少 tools")?;
    tools
        .iter()
        .map(|t| {
            Ok(McpTool {
                name: t
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or("MCP tool 缺少 name")?
                    .to_string(),
                description: t
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                input_schema: t
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type":"object"})),
            })
        })
        .collect()
}

// ────────────────────────── stdio 进程池 ──────────────────────────
//
// 背景：MCP 工具代理实测 stdio 冷启动 ~198ms/次（超标），故做进程复用：
// 每个 (cmd, args, env) 内容指纹维护一个可复用 stdio 进程：
// 首次调用懒 spawn + initialize，后续 tools/list、tools/call 复用已初始化
// 进程（JSON-RPC id 递增）；空闲超时回收；进程崩溃/无响应自动重建。
// 同一连接内请求用 tokio Mutex 串行化（单进程顺序执行），不同 server
// 键不同、独立连接，互不阻塞。

/// stdio 连接池默认参数（测试可用环境变量覆写为短值）。
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(120);

fn env_duration_ms(name: &str, default: Duration) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(default)
}

fn pool_idle_timeout() -> Duration {
    env_duration_ms("JAI_MCP_POOL_IDLE_MS", DEFAULT_IDLE_TIMEOUT)
}

fn pool_call_timeout() -> Duration {
    env_duration_ms("JAI_MCP_POOL_CALL_TIMEOUT_MS", DEFAULT_CALL_TIMEOUT)
}

struct StdioPool {
    entries: StdMutex<HashMap<u64, Arc<StdioConn>>>,
}

impl StdioPool {
    fn get_or_insert(&self, key: u64, cmd: &str) -> Arc<StdioConn> {
        let mut map = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(conn) = map.get(&key) {
            if conn.cmd == cmd {
                return conn.clone();
            }
            // 键哈希碰撞（理论可能）：丢弃旧连接，另起一条
            map.remove(&key);
        }
        let conn = Arc::new(StdioConn {
            cmd: cmd.to_string(),
            inner: Mutex::new(ConnState::Uninit),
        });
        map.insert(key, conn.clone());
        conn
    }

    fn evict_if_same(&self, key: u64, conn: &Arc<StdioConn>) {
        let mut map = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(c) = map.get(&key) {
            if Arc::ptr_eq(c, conn) {
                map.remove(&key);
            }
        }
    }
}

fn pool() -> &'static StdioPool {
    static POOL: OnceLock<StdioPool> = OnceLock::new();
    static REAPER: OnceLock<()> = OnceLock::new();
    let p = POOL.get_or_init(|| StdioPool {
        entries: StdMutex::new(HashMap::new()),
    });
    // 后台空闲回收：仅挂在有 tokio runtime 的进程（桌面网关常驻；
    // server 停用/删除后不再被访问，访问时回收覆盖不到，需要周期清扫）
    if REAPER.get().is_none()
        && tokio::runtime::Handle::try_current().is_ok()
        && REAPER.set(()).is_ok()
    {
        tokio::spawn(async {
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            tick.tick().await; // 跳过 interval 的首个立即 tick
            loop {
                tick.tick().await;
                reap_idle_conns().await;
            }
        });
    }
    p
}

/// 周期清扫空闲超时的连接（阈值与访问时回收一致，取自 `JAI_MCP_POOL_IDLE_MS`）。
async fn reap_idle_conns() {
    let idle = pool_idle_timeout();
    let expired: Vec<(u64, Arc<StdioConn>)> = {
        let map = pool().entries.lock().unwrap_or_else(|p| p.into_inner());
        map.iter()
            .filter(|(_, c)| {
                // try_lock：连接正被占用则本轮跳过，下轮再试
                c.inner
                    .try_lock()
                    .map(|g| matches!(&*g, ConnState::Ready(r) if r.last_used.elapsed() >= idle))
                    .unwrap_or(false)
            })
            .map(|(k, c)| (*k, c.clone()))
            .collect()
    };
    for (key, conn) in expired {
        if let Ok(mut g) = conn.inner.try_lock() {
            if matches!(&*g, ConnState::Ready(r) if r.last_used.elapsed() >= idle) {
                // 置 Dead：ReadyConn 被 drop → start_kill 终止子进程
                *g = ConnState::Dead;
                pool().evict_if_same(key, &conn);
            }
        }
    }
}

/// 连接键：cmd/args + 排序后的 env。env 值只参与哈希、不落池（可能含密钥）。
fn pool_key(cmd: &str, args: &[String], env: &HashMap<String, String>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    cmd.hash(&mut h);
    for a in args {
        a.hash(&mut h);
    }
    let mut kv: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    kv.sort_unstable();
    for (k, v) in kv {
        k.hash(&mut h);
        v.hash(&mut h);
    }
    h.finish()
}

struct StdioConn {
    cmd: String, // 与 get_or_insert 校验配套，防哈希碰撞串线
    inner: Mutex<ConnState>,
}

/// 单连接状态机：Uninit（懒初始化）→ Ready（已初始化可复用）→ Dead（重建）。
enum ConnState {
    Uninit,
    Ready(Box<ReadyConn>),
    Dead,
}

struct ReadyConn {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64, // initialize 占 id=1，首个业务请求从 2 起递增
    last_used: Instant,
}

impl Drop for ReadyConn {
    fn drop(&mut self) {
        // 任何未写回 Ready 的路径（超时、调用方中途取消等）都终止子进程，
        // 避免孤儿进程占资源；正常路径先写回 Ready 再走 drop（此时已 kill 或复用）。
        let _ = self.child.start_kill();
    }
}

/// 传输级错误（写失败/EOF/进程退出）与业务错误（服务端 error 帧）分开：
/// 前者连接不可用需重建，后者连接健康可继续复用。
enum XchgErr {
    Transport(String),
    Rpc(String),
}

/// spawn 子进程并完成 initialize 握手，返回可复用的 Ready 连接。
async fn spawn_ready(
    cmd: &str,
    args: &[String],
    env: &HashMap<String, String>,
) -> Result<ReadyConn, String> {
    let mut command = tokio::process::Command::new(cmd);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (k, v) in env {
        command.env(k, v);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("启动 MCP stdio 进程失败: {e}"))?;
    let stdin = match child.stdin.take() {
        Some(s) => s,
        None => {
            let _ = child.kill().await;
            return Err("MCP stdin 不可用".into());
        }
    };
    let stdout = match child.stdout.take() {
        Some(o) => o,
        None => {
            let _ = child.kill().await;
            return Err("MCP stdout 不可用".into());
        }
    };
    let mut ready = ReadyConn {
        child,
        stdin,
        stdout: BufReader::new(stdout),
        next_id: 2,
        last_used: Instant::now(),
    };

    let initialize = json!({
        "jsonrpc":"2.0",
        "id":1,
        "method":"initialize",
        "params":{
            "protocolVersion":"2024-11-05",
            "capabilities":{},
            "clientInfo":{"name":"jai-gateway","version":env!("CARGO_PKG_VERSION")}
        }
    });
    let init = tokio::time::timeout(pool_call_timeout(), async {
        ready
            .stdin
            .write_all(format!("{initialize}\n").as_bytes())
            .await
            .map_err(|e| format!("写入 MCP initialize 失败: {e}"))?;
        ready.stdin.flush().await.map_err(|e| e.to_string())?;

        let mut line = String::new();
        let bytes = ready
            .stdout
            .read_line(&mut line)
            .await
            .map_err(|e| format!("读取 MCP stdout 失败: {e}"))?;
        if bytes == 0 {
            return Err("MCP initialize 无响应".into());
        }
        let resp: Value =
            serde_json::from_str(&line).map_err(|e| format!("MCP 响应 JSON 失败: {e}"))?;
        if let Some(err) = resp.get("error") {
            return Err(format!("MCP initialize 错误: {err}"));
        }

        // initialized notification（忽略响应）
        let notif = json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}});
        ready
            .stdin
            .write_all(format!("{notif}\n").as_bytes())
            .await
            .map_err(|e| format!("写入 MCP initialized 失败: {e}"))?;
        ready.stdin.flush().await.map_err(|e| e.to_string())?;
        Ok::<_, String>(())
    })
    .await;

    match init {
        Ok(Ok(())) => Ok(ready),
        Ok(Err(e)) => {
            let _ = ready.child.kill().await;
            Err(e)
        }
        Err(_) => {
            let _ = ready.child.kill().await;
            Err("MCP initialize 超时".into())
        }
    }
}

/// 在已初始化连接上执行一次 JSON-RPC 请求，读取携带本次 id 的响应帧
/// （通知行及非 JSON 噪音跳过）。
async fn exchange(ready: &mut ReadyConn, method: &str, params: &Value) -> Result<Value, XchgErr> {
    let id = ready.next_id;
    ready.next_id += 1;
    let request = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
    ready
        .stdin
        .write_all(format!("{request}\n").as_bytes())
        .await
        .map_err(|e| XchgErr::Transport(format!("写入 MCP 请求失败: {e}")))?;
    ready
        .stdin
        .flush()
        .await
        .map_err(|e| XchgErr::Transport(e.to_string()))?;

    loop {
        let mut line = String::new();
        let bytes = ready
            .stdout
            .read_line(&mut line)
            .await
            .map_err(|e| XchgErr::Transport(format!("读取 MCP 响应失败: {e}")))?;
        if bytes == 0 {
            return Err(XchgErr::Transport(
                "MCP 工具请求无响应（进程已退出）".into(),
            ));
        }
        let frame: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue, // 非 JSON 行（横幅/噪音）忽略
        };
        if frame.get("id").and_then(Value::as_i64) != Some(id) {
            continue; // 通知行或他次请求的迟到帧，不属于本次
        }
        if let Some(err) = frame.get("error") {
            return Err(XchgErr::Rpc(format!("MCP {method} 错误: {err}")));
        }
        return Ok(frame);
    }
}

/// 通过进程池执行一次 stdio MCP 请求。`tx` 为每次调用脉冲（预留异步挂载点）。
async fn pooled_stdio_jsonrpc(
    cmd: &str,
    args: &[String],
    env: &HashMap<String, String>,
    method: &str,
    params: &Value,
    tx: &mpsc::UnboundedSender<()>,
) -> Result<Value, String> {
    let key = pool_key(cmd, args, env);
    let idle = pool_idle_timeout();
    let call_timeout = pool_call_timeout();

    let conn = pool().get_or_insert(key, cmd);
    let mut guard = conn.inner.lock().await;

    // 取出当前状态；无论结果如何最终都会写回（Ready 保留 / Dead 重建）。
    // 单遍即可：崩溃/空闲重建都在获取阶段完成，不需要重试循环。
    let state = std::mem::replace(&mut *guard, ConnState::Dead);

    let mut ready = match state {
        ConnState::Ready(mut r) => {
            let exited = r
                .child
                .try_wait()
                .map_err(|e| format!("检测 MCP 进程状态失败: {e}"))?
                .is_some();
            if exited || r.last_used.elapsed() >= idle {
                // 进程已退出（崩溃后自动重建）或空闲超时（回收重建）
                let _ = r.child.kill().await;
                Box::new(spawn_ready(cmd, args, env).await?)
            } else {
                r
            }
        }
        ConnState::Uninit | ConnState::Dead => Box::new(spawn_ready(cmd, args, env).await?),
    };

    // 每次调用脉冲（预留异步挂载点：统计/钩子可挂这里）
    let _ = tx.send(());

    let result = tokio::time::timeout(call_timeout, exchange(&mut ready, method, params)).await;
    match result {
        Ok(Ok(frame)) => {
            ready.last_used = Instant::now();
            *guard = ConnState::Ready(ready);
            Ok(frame)
        }
        Ok(Err(XchgErr::Rpc(e))) => {
            // 服务端业务错误：连接健康，保留复用
            ready.last_used = Instant::now();
            *guard = ConnState::Ready(ready);
            Err(e)
        }
        Ok(Err(XchgErr::Transport(e))) => {
            // 传输失败：连接不可用，杀进程废弃，下次调用自动重建
            let _ = ready.child.kill().await;
            *guard = ConnState::Dead;
            pool().evict_if_same(key, &conn);
            Err(e)
        }
        Err(_) => {
            // 超时：连接处于不可知状态，杀进程废弃，下次调用自动重建
            let _ = ready.child.kill().await;
            *guard = ConnState::Dead;
            pool().evict_if_same(key, &conn);
            Err(format!(
                "MCP {method} 超时（{}ms）",
                call_timeout.as_millis()
            ))
        }
    }
}

async fn run_http_jsonrpc(url: &str, method: &str, params: &Value) -> Result<Value, String> {
    let body = json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("MCP HTTP 请求失败: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("MCP HTTP {status}: {text}"));
    }
    let text = resp
        .text()
        .await
        .map_err(|e| format!("MCP HTTP 响应读取失败: {e}"))?;
    // Streamable HTTP 可能返回 SSE；这里只处理 JSON 主体
    let text = if text.starts_with("data:") {
        text.lines()
            .find(|l| l.starts_with("data:"))
            .map(|l| l.trim_start_matches("data:").trim().to_string())
            .unwrap_or(text)
    } else {
        text
    };
    let v: Value =
        serde_json::from_str(&text).map_err(|e| format!("MCP HTTP 响应 JSON 失败: {e}: {text}"))?;
    if let Some(err) = v.get("error") {
        return Err(format!("MCP {method} 错误: {err}"));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tools_list_extracts_tools() {
        let v = json!({
            "jsonrpc":"2.0",
            "id":2,
            "result":{"tools":[
                {"name":"read_file","description":"Read a file","inputSchema":{"type":"object","properties":{}}}
            ]}
        });
        let tools = parse_tools_list(&v).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "read_file");
    }

    #[test]
    fn server_env_parses_json_object() {
        let row = McpServerRow {
            id: "m1".into(),
            name: "netcatty".into(),
            kind: "stdio".into(),
            command: Some("/bin/echo".into()),
            args: None,
            url: None,
            env: Some(r#"{"NETCATTY_EXTERNAL_MCP_DISCOVERY_FILE":"/tmp/discovery.json"}"#.into()),
            proxy_allowed: false,
            enabled: true,
            created_at: 0,
            updated_at: 0,
        };
        let env = server_env(&row);
        assert_eq!(env.len(), 1);
        assert_eq!(
            env.get("NETCATTY_EXTERNAL_MCP_DISCOVERY_FILE")
                .map(String::as_str),
            Some("/tmp/discovery.json")
        );
        // 非法 env 字符串回落为空 map，不 panic
        let bad = McpServerRow {
            env: Some("not-json".into()),
            ..row.clone()
        };
        assert!(server_env(&bad).is_empty());
        let none = McpServerRow { env: None, ..row };
        assert!(server_env(&none).is_empty());
    }
}
