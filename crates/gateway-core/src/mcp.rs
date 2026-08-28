//! MCP（Model Context Protocol）客户端 —— 工具发现 / 调用。
//!
//! 当前支持：
//! - `stdio`：spawn 子进程，按 MCP stdio 传输约定走换行分隔 JSON-RPC 2.0
//! - `http` / `sse`：统一按 Streamable HTTP 风格向 `url` POST JSON-RPC
//!
//! 说明：这是轻量客户端，覆盖 Codex/Claude 类 tool calling 需要的
//! `initialize`、`tools/list`、`tools/call` 子集，不承载完整 MCP 生命周期。

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::store::McpServerRow;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: Value,
}

/// 已发现工具 + 所属 MCP Server，供网关自动合并/执行。
#[derive(Debug, Clone)]
pub struct McpServerTool {
    pub server: McpServerRow,
    pub tool: McpTool,
}

/// MCP 工具列表缓存 TTL：避免每个请求都去 spawn/请求一次 MCP Server。
const MCP_TOOL_CACHE_TTL: Duration = Duration::from_secs(30);

struct CachedTools {
    at: Instant,
    tools: Vec<McpTool>,
}

static TOOL_CACHE: OnceLock<Mutex<HashMap<String, CachedTools>>> = OnceLock::new();

fn tool_cache_get(server_id: &str) -> Option<Vec<McpTool>> {
    let cache = TOOL_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let guard = cache.lock().unwrap_or_else(|p| p.into_inner());
    let entry = guard.get(server_id)?;
    if entry.at.elapsed() > MCP_TOOL_CACHE_TTL {
        return None;
    }
    Some(entry.tools.clone())
}

fn tool_cache_put(server_id: &str, tools: Vec<McpTool>) {
    let cache = TOOL_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(|p| p.into_inner());
    guard.insert(
        server_id.to_string(),
        CachedTools {
            at: Instant::now(),
            tools,
        },
    );
}

/// 收集所有启用 MCP Server 的工具列表。
///
/// 单个 Server 的 `tools/list` 失败不会拖垮整个请求：跳过该 Server 并打印警告。
/// 返回的 `McpServerTool` 已按 Server 展开；同名工具去重由调用方决定。
pub async fn collect_enabled_tools(db: &crate::store::Db) -> Vec<McpServerTool> {
    let servers = match tokio::task::spawn_blocking({
        let db = db.clone();
        move || db.with(crate::store::mcp_list).map_err(|e| e.to_string())
    })
    .await
    {
        Ok(Ok(list)) => list,
        Ok(Err(e)) => {
            eprintln!("[mcp] 读取 MCP Server 列表失败: {e}");
            return Vec::new();
        }
        Err(e) => {
            eprintln!("[mcp] MCP Server 列表任务失败: {e}");
            return Vec::new();
        }
    };

    let mut out = Vec::new();
    for server in servers.into_iter().filter(|s| s.enabled) {
        // 缓存命中：直接用上次成功拉到的工具列表，避免每个请求都启动 MCP 子进程/网络请求
        if let Some(tools) = tool_cache_get(&server.id) {
            for tool in tools {
                out.push(McpServerTool {
                    server: server.clone(),
                    tool,
                });
            }
            continue;
        }

        match tokio::time::timeout(Duration::from_secs(10), list_tools(&server)).await {
            Ok(Ok(tools)) => {
                tool_cache_put(&server.id, tools.clone());
                for tool in tools {
                    out.push(McpServerTool {
                        server: server.clone(),
                        tool,
                    });
                }
            }
            Ok(Err(e)) => eprintln!("[mcp] Server {} tools/list 失败: {e}", server.name),
            Err(_) => eprintln!("[mcp] Server {} tools/list 超时(10s)", server.name),
        }
    }
    out
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
            let result = run_stdio_jsonrpc(cmd, &args, "tools/list", &json!({}), &tx).await?;
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
            let result = run_stdio_jsonrpc(cmd, &args, "tools/call", &params, &tx).await?;
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

async fn run_stdio_jsonrpc(
    cmd: &str,
    args: &[String],
    method: &str,
    params: &Value,
    _tx: &tokio::sync::mpsc::UnboundedSender<()>,
) -> Result<Value, String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let mut child = tokio::process::Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("启动 MCP stdio 进程失败: {e}"))?;

    let stdin = child.stdin.as_mut().ok_or("MCP stdin 不可用")?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or("MCP stdout 不可用")?);

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
    stdin
        .write_all(format!("{initialize}\n").as_bytes())
        .await
        .map_err(|e| format!("写入 MCP initialize 失败: {e}"))?;
    stdin.flush().await.map_err(|e| e.to_string())?;

    let mut line = String::new();
    let bytes = stdout
        .read_line(&mut line)
        .await
        .map_err(|e| format!("读取 MCP stdout 失败: {e}"))?;
    if bytes == 0 {
        return Err("MCP initialize 无响应".into());
    }
    let init_resp: Value =
        serde_json::from_str(&line).map_err(|e| format!("MCP 响应 JSON 失败: {e}"))?;
    if let Some(err) = init_resp.get("error") {
        return Err(format!("MCP initialize 错误: {err}"));
    }

    // initialized notification（忽略响应）
    let notif = json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}});
    stdin
        .write_all(format!("{notif}\n").as_bytes())
        .await
        .map_err(|e| format!("写入 MCP initialized 失败: {e}"))?;
    stdin.flush().await.map_err(|e| e.to_string())?;

    let request = json!({"jsonrpc":"2.0","id":2,"method":method,"params":params});
    stdin
        .write_all(format!("{request}\n").as_bytes())
        .await
        .map_err(|e| format!("写入 MCP 请求失败: {e}"))?;
    stdin.flush().await.map_err(|e| e.to_string())?;

    let mut resp_line = String::new();
    let bytes = stdout
        .read_line(&mut resp_line)
        .await
        .map_err(|e| format!("读取 MCP 响应失败: {e}"))?;
    if bytes == 0 {
        return Err("MCP 工具请求无响应".into());
    }
    let _ = child.kill().await;
    let resp: Value =
        serde_json::from_str(&resp_line).map_err(|e| format!("MCP 响应 JSON 失败: {e}"))?;
    if let Some(err) = resp.get("error") {
        return Err(format!("MCP {method} 错误: {err}"));
    }
    Ok(resp)
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
    fn tool_cache_returns_recent_tools() {
        tool_cache_put(
            "server-1",
            vec![McpTool {
                name: "cached_tool".into(),
                description: Some("desc".into()),
                input_schema: json!({"type":"object"}),
            }],
        );
        let tools = tool_cache_get("server-1").expect("缓存应命中");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "cached_tool");
    }

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
}
