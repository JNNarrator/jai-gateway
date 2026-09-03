//! `/mcp` 元数据 MCP Server —— 把网关登记的 MCP Server / Skill 台账以 MCP 协议暴露。
//!
//! 设计约定（与用户确认的方案）：
//! - **只读信息台**：静态工具返回"信息"，不代替 Agent 执行任何 MCP 工具；
//!   网关不再注入对话链路，执行面归客户端（dsh / Claude Code 等）。
//! - **代理执行（M1 起）**：`proxy_allowed=1` 的 MCP Server 的工具会以
//!   `<server>__<tool>` 命名动态暴露，`tools/call` 显式转发到真实 Server 执行。
//!   选择权始终在 Agent：工具出现在其工具列表里、由它主动调用，网关只做转发。
//! - 传输：Streamable HTTP（单端点 `POST /mcp`，换行内 JSON-RPC 2.0）。
//!   仅覆盖 `initialize` / `notifications/initialized` / `ping` /
//!   `tools/list` / `tools/call` 子集。
//! - 鉴权复用网关安全中间件（Authorization: Bearer / x-api-key，常量时间比对）。
//! - env 只回键名不回值：环境变量值属供应商侧敏感信息，不通过元数据接口扩散。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use super::proxy::GatewayCtx;
use crate::store::McpServerRow;

/// MCP 实现版本与协议版本（客户端据此协商）。
const PROTOCOL_VERSION: &str = "2025-03-26";
const SERVER_NAME: &str = "jai-gateway-registry";
const SERVER_VERSION: &str = "0.1.0";

/// 动态工具列表 TTL：避免每次 tools/list 都去 spawn MCP 子进程拉工具。
const PROXY_TOOLS_TTL: Duration = Duration::from_secs(30);

/// 进程内动态工具列表缓存（server 工具聚合）。`proxy_allowed` 配置变化由
/// 签名比对感知：签名 = 可代理 server 的 (name, kind, command, url) 连接串。
static PROXY_TOOLS_CACHE: Mutex<Option<ProxyToolsCache>> = Mutex::new(None);

struct ProxyToolsCache {
    /// 签名（可代理 server 连接形态的 md5/摘要），变化即失效
    signature: u64,
    /// 聚合后的动态工具（name 已带 `<server>__` 前缀）
    tools: Vec<Value>,
    built_at: Instant,
}

/// 快速签名：djb2 哈希串接，足够感知配置变化（非密码学用途）。
fn proxy_signature(servers: &[McpServerRow]) -> u64 {
    let mut h: u64 = 5381;
    for s in servers {
        for part in std::iter::once(&s.name)
            .chain(std::iter::once(&s.kind))
            .chain(s.command.iter())
            .chain(s.url.iter())
        {
            for b in part.bytes() {
                h = h.wrapping_mul(33).wrapping_add(b as u64);
            }
        }
    }
    h
}

/// 取可代理（enabled=1 且 proxy_allowed=1）的 MCP Server 列表。
fn proxy_servers(ctx: &GatewayCtx) -> Vec<McpServerRow> {
    let list = ctx
        .db
        .with_any(|c| crate::store::mcp_list(c).map_err(|e| e.to_string()));
    match list {
        Ok(rows) => rows
            .into_iter()
            .filter(|s| s.enabled && s.proxy_allowed)
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// 聚合可代理 Server 的动态工具（带 `<server>__` 前缀与来源标注）。
/// 单个 server 拉取失败跳过，不阻塞整体。
async fn build_proxy_tools(servers: &[McpServerRow]) -> Vec<Value> {
    let mut specs: Vec<Value> = Vec::new();
    for s in servers {
        let tools = match crate::mcp::list_tools(s).await {
            Ok(t) => t,
            Err(e) => {
                eprintln!("[registry] {0} list_tools 失败，跳过: {e}", s.name);
                continue;
            }
        };
        for t in tools {
            let name = format!("{}__{}", s.name, t.name);
            specs.push(json!({
                "name": name,
                "description": format!("[proxy: {}] {}", s.name, t.description.as_deref().unwrap_or("")),
                "inputSchema": t.input_schema,
            }));
        }
    }
    specs
}

/// 动态工具列表（带 30s TTL 缓存；签名变化立即失效）。
async fn dynamic_tool_specs(ctx: &GatewayCtx) -> Vec<Value> {
    let servers = proxy_servers(ctx);
    let signature = proxy_signature(&servers);
    {
        let guard = PROXY_TOOLS_CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(cache) = guard.as_ref() {
            if cache.signature == signature
                && cache.built_at.elapsed() < PROXY_TOOLS_TTL
            {
                return cache.tools.clone();
            }
        }
    }
    let specs = build_proxy_tools(&servers).await;
    if let Ok(mut guard) = PROXY_TOOLS_CACHE.lock() {
        *guard = Some(ProxyToolsCache {
            signature,
            tools: specs.clone(),
            built_at: Instant::now(),
        });
    }
    specs
}

// ---------------------------------------------------------------- 工具定义

fn tool_specs() -> Vec<Value> {
    vec![
        json!({
            "name": "list_mcp_servers",
            "description": "列出网关登记的全部 MCP Server（名称、传输类型、启停状态、工具数量概览）。用于发现 Agent 可以连接哪些 MCP Server。",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "get_mcp_server_detail",
            "description": "返回某个 MCP Server 的完整登记信息：command/args/url、env 键名（不含值）与连接方式说明。用于照着台账把该 Server 添加到 Agent 的配置里。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "MCP Server 名称（list_mcp_servers 返回的 name）"}
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "get_tool_schemas",
            "description": "返回某个 MCP Server 下全部工具的 JSON Schema 定义。用于了解该 Server 能做什么、调用参数是什么。注意：本工具只返回定义，不执行工具。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "MCP Server 名称"}
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "list_skills",
            "description": "列出网关登记的全部技能（Skill）及启停状态。技能是可复用的提示词/工作流说明，需要时用 get_skill_detail 获取全文。",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "get_skill_detail",
            "description": "返回某个技能的完整内容（prompt 全文）、用途描述与使用建议。当用户任务与某个技能的适用场景匹配时应获取并遵循它。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "技能名称（list_skills 返回的 name）"}
                },
                "required": ["name"]
            }
        }),
    ]
}

// ---------------------------------------------------------------- 工具实现

fn server_env_keys(env_json: Option<&str>) -> Vec<String> {
    env_json
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v.as_object().map(|m| m.keys().cloned().collect()))
        .unwrap_or_default()
}

/// stdio 命令的 args JSON 数组字符串 → Vec<String>。
fn server_args(args_json: Option<&str>) -> Vec<String> {
    args_json
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default()
}

async fn tool_list_mcp_servers(ctx: &GatewayCtx) -> Value {
    let servers = tokio::task::spawn_blocking({
        let db = ctx.db.clone();
        move || db.with_any(|c| crate::store::mcp_list(c).map_err(|e| e.to_string()))
    })
    .await
    .unwrap_or_else(|e| Err(format!("任务失败: {e}")));

    let list = match servers {
        Ok(list) => list,
        Err(e) => return json!({"error": format!("读取 MCP Server 列表失败: {e}")}),
    };

    let items: Vec<Value> = list
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "kind": s.kind,
                "enabled": s.enabled,
                "url": s.url,
                "command": s.command,
            })
        })
        .collect();
    json!({
        "servers": items,
        "note": "以上为网关登记台账。要实际调用某个 Server 的工具，请在你的客户端配置中添加该 Server（用 get_mcp_server_detail 获取配置）。"
    })
}

async fn tool_server_detail(ctx: &GatewayCtx, name: &str) -> Value {
    let servers = tokio::task::spawn_blocking({
        let db = ctx.db.clone();
        move || db.with_any(|c| crate::store::mcp_list(c).map_err(|e| e.to_string()))
    })
    .await
    .unwrap_or_else(|e| Err(format!("任务失败: {e}")));

    let list = match servers {
        Ok(list) => list,
        Err(e) => return json!({"error": format!("读取 MCP Server 列表失败: {e}")}),
    };

    let Some(s) = list.iter().find(|s| s.name == name) else {
        return json!({
            "error": format!("未找到名为「{name}」的 MCP Server"),
            "hint": "用 list_mcp_servers 查看现有登记"
        });
    };

    let connect = match s.kind.as_str() {
        "stdio" => json!({
            "transport": "stdio",
            "command": s.command,
            "args": server_args(s.args.as_deref()),
            "envKeys": server_env_keys(s.env.as_deref()),
        }),
        _ => json!({
            "transport": s.kind,
            "url": s.url,
            "envKeys": server_env_keys(s.env.as_deref()),
        }),
    };
    json!({
        "name": s.name,
        "kind": s.kind,
        "enabled": s.enabled,
        "connect": connect,
        "note": "env 仅返回键名；实际值在客户端/供应商侧配置。"
    })
}

async fn tool_tool_schemas(ctx: &GatewayCtx, name: &str) -> Value {
    let servers = tokio::task::spawn_blocking({
        let db = ctx.db.clone();
        move || db.with_any(|c| crate::store::mcp_list(c).map_err(|e| e.to_string()))
    })
    .await
    .unwrap_or_else(|e| Err(format!("任务失败: {e}")));

    let list = match servers {
        Ok(list) => list,
        Err(e) => return json!({"error": format!("读取 MCP Server 列表失败: {e}")}),
    };

    let Some(s) = list.iter().find(|s| s.name == name) else {
        return json!({
            "error": format!("未找到名为「{name}」的 MCP Server"),
            "hint": "用 list_mcp_servers 查看现有登记"
        });
    };
    if !s.enabled {
        return json!({"error": format!("MCP Server「{name}」未启用"), "tools": []});
    }

    // 复用 mcp.rs 的工具发现（10s 超时），只读不执行
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        crate::mcp::list_tools(s),
    )
    .await
    {
        Ok(Ok(tools)) => json!({
            "server": s.name,
            "tools": tools,
            "note": "工具定义仅供参考；本接口不执行工具。"
        }),
        Ok(Err(e)) => json!({"error": format!("tools/list 失败: {e}")}),
        Err(_) => json!({"error": "tools/list 超时(10s)"}),
    }
}

async fn tool_list_skills(ctx: &GatewayCtx) -> Value {
    let skills = tokio::task::spawn_blocking({
        let db = ctx.db.clone();
        move || db.with_any(|c| crate::store::skill_list(c).map_err(|e| e.to_string()))
    })
    .await
    .unwrap_or_else(|e| Err(format!("任务失败: {e}")));

    let list = match skills {
        Ok(list) => list,
        Err(e) => return json!({"error": format!("读取技能列表失败: {e}")}),
    };
    let items: Vec<Value> = list
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "description": s.description,
                "enabled": s.enabled,
            })
        })
        .collect();
    json!({"skills": items})
}

async fn tool_skill_detail(ctx: &GatewayCtx, name: &str) -> Value {
    let skills = tokio::task::spawn_blocking({
        let db = ctx.db.clone();
        move || db.with_any(|c| crate::store::skill_list(c).map_err(|e| e.to_string()))
    })
    .await
    .unwrap_or_else(|e| Err(format!("任务失败: {e}")));

    let list = match skills {
        Ok(list) => list,
        Err(e) => return json!({"error": format!("读取技能列表失败: {e}")}),
    };
    let Some(s) = list.iter().find(|s| s.name == name) else {
        return json!({
            "error": format!("未找到名为「{name}」的技能"),
            "hint": "用 list_skills 查看现有登记"
        });
    };
    json!({
        "name": s.name,
        "description": s.description,
        "enabled": s.enabled,
        "content": s.content,
    })
}

/// 解析 `server__tool` 命名（按第一个双下划线分割；server 名本身可含 `_`）。
fn parse_proxy_name(name: &str) -> Option<(&str, &str)> {
    let (server, tool) = name.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server, tool))
}

/// 代理转发：`<server>__<tool>` → 真实 MCP Server 执行。
async fn proxy_call_tool(ctx: &GatewayCtx, server_name: &str, tool_name: &str, args: &Value) -> Result<Value, String> {
    let servers = proxy_servers(ctx);
    let Some(server) = servers.iter().find(|s| s.name == server_name) else {
        return Err(format!(
            "Server「{server_name}」不存在或未开启代理执行（proxy_allowed）"
        ));
    };
    let result = crate::mcp::call_tool(server, tool_name, args.clone())
        .await
        .map_err(|e| format!("[proxy: {server_name}] {tool_name} 调用失败: {e}"))?;
    // 附来源标注，便于审计与 Agent 理解结果出处
    Ok(json!({
        "source": format!("jai-gateway-proxy/{server_name}"),
        "result": result,
    }))
}

async fn dispatch_tool(ctx: &GatewayCtx, name: &str, args: &Value) -> Result<Value, String> {
    // 代理工具优先：命中 `server__tool` 且 server 可代理才转发
    if let Some((server, tool)) = parse_proxy_name(name) {
        // 若与静态工具重名（如 "list_mcp_servers" 不含 __，不会走到这），
        // 直接按代理语义处理；server 校验在 proxy_call_tool 内完成。
        return proxy_call_tool(ctx, server, tool, args).await;
    }
    match name {
        "list_mcp_servers" => Ok(tool_list_mcp_servers(ctx).await),
        "get_mcp_server_detail" => {
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or("缺少参数 name")?;
            Ok(tool_server_detail(ctx, name).await)
        }
        "get_tool_schemas" => {
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or("缺少参数 name")?;
            Ok(tool_tool_schemas(ctx, name).await)
        }
        "list_skills" => Ok(tool_list_skills(ctx).await),
        "get_skill_detail" => {
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or("缺少参数 name")?;
            Ok(tool_skill_detail(ctx, name).await)
        }
        other => Err(format!("未知工具: {other}")),
    }
}

// ---------------------------------------------------------------- JSON-RPC 端点

fn rpc_result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message}
    })
}

async fn handle_rpc(ctx: &GatewayCtx, v: &Value) -> Option<Value> {
    let method = v.get("method").and_then(Value::as_str)?;
    let id = v.get("id").cloned().unwrap_or(Value::Null);
    let params = v.get("params").cloned().unwrap_or(json!({}));

    let result = match method {
        "initialize" => json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION,
                "title": "JAI Gateway Registry",
                "description": "网关登记的 MCP Server 与 Skill 台账；开启代理执行的 Server 工具以 <server>__<tool> 暴露并可转发调用"
            }
        }),
        "ping" => json!({}),
        "tools/list" => {
            // 静态只读工具 + 动态代理工具（可代理 server 的工具聚合，30s TTL）
            let mut tools = tool_specs();
            tools.extend(dynamic_tool_specs(ctx).await);
            json!({"tools": tools})
        }
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match dispatch_tool(ctx, name, &args).await {
                // MCP tools/call 结果包裹在 content 数组里
                Ok(payload) => json!({
                    "content": [{"type": "text", "text": payload.to_string()}],
                    "isError": false
                }),
                Err(e) => json!({
                    "content": [{"type": "text", "text": e}],
                    "isError": true
                }),
            }
        }
        // notifications/* 等无响应方法由调用方过滤；此处仅兜底
        _ => {
            return if id.is_null() {
                None // notification：无 id，不回包
            } else {
                Some(rpc_error(id, -32601, &format!("未知方法: {method}")))
            };
        }
    };
    if id.is_null() {
        None // notification（如 notifications/initialized）
    } else {
        Some(rpc_result(id, result))
    }
}

/// `POST /mcp`：Streamable HTTP 风格的 JSON-RPC 端点。
pub async fn mcp_endpoint(State(ctx): State<GatewayCtx>, Json(body): Json<Value>) -> Response {
    if let Some(batch) = body.as_array() {
        // 批量请求（少见但协议允许）
        let mut outs = Vec::new();
        for item in batch {
            if let Some(resp) = handle_rpc(&ctx, item).await {
                outs.push(resp);
            }
        }
        if outs.is_empty() {
            // 纯通知批次：202 Accepted 无正文
            return StatusCode::ACCEPTED.into_response();
        }
        return (StatusCode::OK, Json(json!(outs))).into_response();
    }

    match handle_rpc(&ctx, &body).await {
        Some(resp) => (StatusCode::OK, Json(resp)).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{self, Db};
    use std::sync::Once;

    static INIT: Once = Once::new();
    fn test_ctx() -> GatewayCtx {
        INIT.call_once(|| {
            crate::vault::testing::set_mock_default();
        });
        // 每个测试独立 db 文件，避免并行测试间建表冲突
        let dir = std::env::temp_dir().join(format!(
            "jai-mcp-reg-{}-{}",
            std::process::id(),
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.db");
        let db = Db::open(path.to_str().unwrap()).unwrap();
        let (logs, _t) = crate::store::logs::spawn_logger(path.to_str().unwrap()).unwrap();
        GatewayCtx::new(db.clone(), logs)
    }

    fn seed(ctx: &GatewayCtx) {
        ctx.db
            .with(|c| {
                let now = store::now_ms();
                store::mcp_insert(
                    c,
                    &store::McpServerRow {
                        id: "s1".into(),
                        name: "netcatty".into(),
                        kind: "stdio".into(),
                        command: Some("/usr/local/bin/nct-mcp".into()),
                        args: Some(r#"["--verbose"]"#.into()),
                        url: None,
                        env: Some(r#"{"NETCATTY_TOKEN":"secret-value","HOME":"/x"}"#.into()),
                        proxy_allowed: true,
                        enabled: true,
                        created_at: now,
                        updated_at: now,
                    },
                )
                .unwrap();
                store::mcp_insert(
                    c,
                    &store::McpServerRow {
                        id: "s2".into(),
                        name: "websearch".into(),
                        kind: "http".into(),
                        command: None,
                        args: None,
                        url: Some("https://mcp.example.com/mcp".into()),
                        env: None,
                        proxy_allowed: false,
                        enabled: false,
                        created_at: now,
                        updated_at: now,
                    },
                )
                .unwrap();
                store::skill_insert(
                    c,
                    &store::SkillRow {
                        id: "k1".into(),
                        name: "code-review".into(),
                        description: "代码评审".into(),
                        content: "按提交做评审。".into(),
                        enabled: true,
                        created_at: now,
                        updated_at: now,
                    },
                )
                .unwrap();
                Ok::<_, store::StoreError>(())
            })
            .unwrap();
    }

    fn rpc(method: &str, params: Value, id: Value) -> Value {
        json!({"jsonrpc": "2.0", "method": method, "params": params, "id": id})
    }

    #[tokio::test]
    async fn initialize_returns_protocol_info() {
        let ctx = test_ctx();
        let resp = handle_rpc(&ctx, &rpc("initialize", json!({}), json!(1)))
            .await
            .unwrap();
        assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(resp["result"]["serverInfo"]["name"], SERVER_NAME);
    }

    #[tokio::test]
    async fn notification_returns_none() {
        let ctx = test_ctx();
        assert!(handle_rpc(
            &ctx,
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .await
        .is_none());
    }

    #[tokio::test]
    async fn tools_list_returns_five_specs() {
        let ctx = test_ctx();
        let resp = handle_rpc(&ctx, &rpc("tools/list", json!({}), json!(2)))
            .await
            .unwrap();
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 5);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"list_mcp_servers"));
        assert!(names.contains(&"get_skill_detail"));
    }

    #[tokio::test]
    async fn list_servers_and_detail_env_keys_only() {
        let ctx = test_ctx();
        seed(&ctx);

        let resp = handle_rpc(
            &ctx,
            &rpc(
                "tools/call",
                json!({"name":"list_mcp_servers","arguments":{}}),
                json!(3),
            ),
        )
        .await
        .unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["servers"].as_array().unwrap().len(), 2);

        let resp = handle_rpc(
            &ctx,
            &rpc(
                "tools/call",
                json!({"name":"get_mcp_server_detail","arguments":{"name":"netcatty"}}),
                json!(4),
            ),
        )
        .await
        .unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["connect"]["command"], "/usr/local/bin/nct-mcp");
        assert_eq!(payload["connect"]["args"][0], "--verbose");
        // env 只回键名，绝不回值
        let keys = payload["connect"]["envKeys"].as_array().unwrap();
        assert_eq!(keys.len(), 2);
        let dumped = payload.to_string();
        assert!(!dumped.contains("secret-value"), "env 值不得泄漏: {dumped}");
    }

    #[tokio::test]
    async fn skill_flow_and_missing_name() {
        let ctx = test_ctx();
        seed(&ctx);

        let resp = handle_rpc(
            &ctx,
            &rpc(
                "tools/call",
                json!({"name":"get_skill_detail","arguments":{"name":"code-review"}}),
                json!(5),
            ),
        )
        .await
        .unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["content"], "按提交做评审。");

        // 缺参数 → isError
        let resp = handle_rpc(
            &ctx,
            &rpc(
                "tools/call",
                json!({"name":"get_skill_detail","arguments":{}}),
                json!(6),
            ),
        )
        .await
        .unwrap();
        assert_eq!(resp["result"]["isError"], true);

        // 未知方法 → -32601
        let resp = handle_rpc(&ctx, &rpc("no/such", json!({}), json!(7)))
            .await
            .unwrap();
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[test]
    fn proxy_name_split_handles_underscores_in_server() {
        // server 名内含 `_` 也能正确切分（按第一个 `__` 拆）
        assert_eq!(parse_proxy_name("netcatty__get_environment"),
            Some(("netcatty", "get_environment")));
        assert_eq!(parse_proxy_name("my_server__do_thing"),
            Some(("my_server", "do_thing")));
        // 缺 tool / 缺 server / 无分隔符 → 不是代理名
        assert_eq!(parse_proxy_name("netcatty__"), None);
        assert_eq!(parse_proxy_name("__tool"), None);
        assert_eq!(parse_proxy_name("list_mcp_servers"), None);
    }

    #[test]
    fn proxy_signature_stable_and_sensitive_to_config() {
        let row = store::McpServerRow {
            id: "s".into(), name: "a".into(), kind: "stdio".into(),
            command: Some("/bin/x".into()), args: None, url: None, env: None,
            proxy_allowed: true, enabled: true, created_at: 0, updated_at: 0,
        };
        let rows = [row.clone()];
        let a = proxy_signature(&rows);
        let b = proxy_signature(&rows);
        assert_eq!(a, b, "相同配置签名应一致");
        let mut changed = row;
        changed.command = Some("/bin/y".into());
        assert_ne!(a, proxy_signature(std::slice::from_ref(&changed)), "配置变化签名应改变");
    }

    #[tokio::test]
    async fn proxy_servers_filters_enabled_and_allowed() {
        let ctx = test_ctx();
        seed(&ctx);
        let servers = proxy_servers(&ctx);
        // netcatty：enabled=1 && proxy_allowed=1；websearch：enabled=0 → 被过滤
        let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["netcatty"]);
    }

    #[tokio::test]
    async fn proxy_call_to_ineligible_server_errors_without_network() {
        let ctx = test_ctx();
        seed(&ctx);
        // websearch 未 enabled=1，代理校验应直接拒绝，不发起网络请求
        let err = proxy_call_tool(&ctx, "websearch", "any", &json!({}))
            .await
            .unwrap_err();
        assert!(err.contains("websearch"), "报错应点名 server: {err}");
        assert!(err.contains("proxy_allowed"), "报错应提示未开启代理: {err}");
    }

    #[tokio::test]
    async fn tools_list_with_unreachable_proxy_still_has_static_five() {
        // netcatty 可代理但命令在本机不存在 → list_tools 失败应被跳过，
        // 动态聚合不阻塞，静态 5 个工具仍在。
        let ctx = test_ctx();
        seed(&ctx);
        let resp = handle_rpc(&ctx, &rpc("tools/list", json!({}), json!(8)))
            .await
            .unwrap();
        let tools = resp["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"list_mcp_servers"));
        // 没有可用代理 target 时，不应混入任何 `server__tool` 动态项
        assert!(!names.iter().any(|n| n.contains("__")), "不应有动态代理工具: {names:?}");
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_error() {
        let ctx = test_ctx();
        let err = dispatch_tool(&ctx, "no_such_tool", &json!({})).await.unwrap_err();
        assert!(err.contains("未知工具"), "未知静态工具应报错: {err}");
    }
}
