//! `/mcp` 元数据 MCP Server —— 把网关登记的 MCP Server / Skill 台账以 MCP 协议暴露。
//!
//! 设计约定（与用户确认的方案）：
//! - **只读信息台**：所有工具返回"信息"，不代替 Agent 执行任何 MCP 工具；
//!   网关不再注入对话链路，执行面归客户端（dsh / Claude Code 等）。
//! - 传输：Streamable HTTP（单端点 `POST /mcp`，换行内 JSON-RPC 2.0）。
//!   仅覆盖 `initialize` / `notifications/initialized` / `ping` /
//!   `tools/list` / `tools/call` 子集。
//! - 鉴权复用网关安全中间件（Authorization: Bearer / x-api-key，常量时间比对）。
//! - env 只回键名不回值：环境变量值属供应商侧敏感信息，不通过元数据接口扩散。

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use super::proxy::GatewayCtx;

/// MCP 实现版本与协议版本（客户端据此协商）。
const PROTOCOL_VERSION: &str = "2025-03-26";
const SERVER_NAME: &str = "jai-gateway-registry";
const SERVER_VERSION: &str = "0.1.0";

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

async fn dispatch_tool(ctx: &GatewayCtx, name: &str, args: &Value) -> Result<Value, String> {
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
                "description": "网关登记的 MCP Server 与 Skill 台账（只读元数据，不执行工具）"
            }
        }),
        "ping" => json!({}),
        "tools/list" => json!({"tools": tool_specs()}),
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
            }
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
        ctx.db.with(|c| {
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
        let names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"list_mcp_servers"));
        assert!(names.contains(&"get_skill_detail"));
    }

    #[tokio::test]
    async fn list_servers_and_detail_env_keys_only() {
        let ctx = test_ctx();
        seed(&ctx);

        let resp = handle_rpc(&ctx, &rpc("tools/call", json!({"name":"list_mcp_servers","arguments":{}}), json!(3)))
            .await
            .unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["servers"].as_array().unwrap().len(), 2);

        let resp = handle_rpc(
            &ctx,
            &rpc("tools/call", json!({"name":"get_mcp_server_detail","arguments":{"name":"netcatty"}}), json!(4)),
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
            &rpc("tools/call", json!({"name":"get_skill_detail","arguments":{"name":"code-review"}}), json!(5)),
        )
        .await
        .unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["content"], "按提交做评审。");

        // 缺参数 → isError
        let resp = handle_rpc(
            &ctx,
            &rpc("tools/call", json!({"name":"get_skill_detail","arguments":{}}), json!(6)),
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
}
