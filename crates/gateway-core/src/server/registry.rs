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
use crate::store::{McpServerRow, SkillRow};

/// MCP 实现版本与协议版本（客户端据此协商）。
const PROTOCOL_VERSION: &str = "2025-03-26";
const SERVER_NAME: &str = "jai-gateway-registry";
const SERVER_VERSION: &str = "0.1.0";

/// 动态工具列表 TTL：避免每次 tools/list 都去 spawn MCP 子进程拉工具。
const PROXY_TOOLS_TTL: Duration = Duration::from_secs(30);

/// 技能工具名前缀。
const SKILL_TOOL_PREFIX: &str = "skill__";
/// MCP 规范的工具名契约：`[A-Za-z0-9_-]{1,64}`（客户端可能整份 tools/list 校验）。
const TOOL_NAME_MAX: usize = 64;
const TOOL_NAME_HASH_LEN: usize = 6;

fn is_tool_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// 把任意字符串压成合法工具名片段（非法字符 → `_`，并按需截断）。
fn sanitize_tool_segment(s: &str, max: usize) -> String {
    s.chars()
        .map(|c| if is_tool_name_char(c) { c } else { '_' })
        .take(max)
        .collect()
}

/// 取名字摘要前 `n` 位十六进制（n 为偶数）。
fn name_hash(name: &str, n: usize) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(name.as_bytes());
    let mut out = String::with_capacity(n);
    for b in d.iter() {
        if out.len() >= n {
            break;
        }
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// 技能 → 工具名映射（确定性、可逆；与输入顺序无关，按技能名排序分配）。
///
/// 规则：
/// - 技能名**本身**已是合法工具名片段且未被占用 → `skill__<原名>`（向后兼容，历史会话里
///   模型见过的 `skill__code-review` 仍然有效）；
/// - 否则（含中文/空格/斜杠/超长/重名）→ `skill__<sanitized>_<hash6>`：
///   非法字符替换为 `_` 并附 6 位名字摘要，保证**合法且唯一**；真实名字始终出现在
///   工具 description 里，模型仍能看到可读名。
///
/// 背景（bug 清单 15）：此前直接用 `skill__<原名>`，技能名未做任何校验，
/// 一个名为 `probe 带空格/斜杠 的技能` 的技能会广告出违反 MCP 名契约的工具名，
/// 严格校验的客户端可能拒绝整份 tools/list（连累全部代理工具）。
fn skill_tool_names(list: &[SkillRow]) -> Vec<(String, String)> {
    let suffix_max = TOOL_NAME_MAX - SKILL_TOOL_PREFIX.len();
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut ordered: Vec<&SkillRow> = list.iter().collect();
    ordered.sort_by(|a, b| a.name.cmp(&b.name));

    let mut out = Vec::with_capacity(ordered.len());
    for s in ordered {
        let mut n = TOOL_NAME_HASH_LEN;
        let mut seg = loop {
            // 合法原名优先（仅当确实等于原名时才算"原名"）
            let clean = sanitize_tool_segment(&s.name, suffix_max.saturating_sub(n + 1));
            let cand = if clean == s.name && s.name.len() <= suffix_max && !used.contains(&clean) {
                clean
            } else {
                format!("{}_{}", clean, name_hash(&s.name, n))
            };
            if !used.contains(&cand) {
                break cand;
            }
            // 摘要碰撞（极罕见）：加长摘要重试
            n += 2;
            if n > 32 {
                break format!(
                    "{}_{}",
                    sanitize_tool_segment(&s.name, 8),
                    name_hash(&s.name, 32)
                );
            }
        };
        seg.truncate(TOOL_NAME_MAX - SKILL_TOOL_PREFIX.len());
        used.insert(seg.clone());
        out.push((format!("{SKILL_TOOL_PREFIX}{seg}"), s.name.clone()));
    }
    out
}

/// 由（可能被编码过的）工具名反解出技能名。
/// 先按映射表精确匹配；再回退「原样名字」形式（兼容历史会话/手工调用）。
fn resolve_skill_name(map: &[(String, String)], tool_name: &str) -> Option<String> {
    if let Some((_, skill)) = map.iter().find(|(t, _)| t == tool_name) {
        return Some(skill.clone());
    }
    tool_name
        .strip_prefix(SKILL_TOOL_PREFIX)
        .map(|raw| raw.to_string())
}

/// Skill 全文投递单次上限（超过截断并在结果尾部注明）。
const SKILL_MAX_BYTES: usize = 32 * 1024;

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
            if cache.signature == signature && cache.built_at.elapsed() < PROXY_TOOLS_TTL {
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

/// 动态 Skill 工具（`skill__<name>`，仅返回 enabled=1 的）。skills 数量小、
/// 变更少，每次直接查本地 SQLite 即可，无需缓存。
async fn dynamic_skill_specs(ctx: &GatewayCtx) -> Vec<Value> {
    let skills = tokio::task::spawn_blocking({
        let db = ctx.db.clone();
        move || db.with_any(|c| crate::store::skill_list(c).map_err(|e| e.to_string()))
    })
    .await
    .unwrap_or_else(|e| Err(format!("任务失败: {e}")));

    let list = match skills {
        Ok(list) => list,
        Err(e) => {
            eprintln!("[registry] skill_list 失败: {e}");
            return Vec::new();
        }
    };
    // 工具名一律走编码映射：合法原名保持原样（向后兼容），非法/重名则附名字摘要，
    // 保证广告出去的工具名始终满足 MCP 名契约（bug 清单 15）。
    let names = skill_tool_names(&list);
    list.iter()
        .filter(|s| s.enabled)
        .filter_map(|s| {
            let tool = names
                .iter()
                .find(|(_, skill)| skill == &s.name)
                .map(|(t, _)| t.clone())?;
            Some(json!({
                "name": tool,
                "description": format!("加载名为「{}」的技能全文并遵循（{}）", s.name, s.description),
                "inputSchema": {"type": "object", "properties": {}}
            }))
        })
        .collect()
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

async fn tool_list_mcp_servers(ctx: &GatewayCtx) -> Result<Value, String> {
    let servers = tokio::task::spawn_blocking({
        let db = ctx.db.clone();
        move || db.with_any(|c| crate::store::mcp_list(c).map_err(|e| e.to_string()))
    })
    .await
    .unwrap_or_else(|e| Err(format!("任务失败: {e}")));

    let list = servers.map_err(|e| format!("读取 MCP Server 列表失败: {e}"))?;

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
    Ok(json!({
        "servers": items,
        "note": "以上为网关登记台账。要实际调用某个 Server 的工具，请在你的客户端配置中添加该 Server（用 get_mcp_server_detail 获取配置）。"
    }))
}

async fn tool_server_detail(ctx: &GatewayCtx, name: &str) -> Result<Value, String> {
    let servers = tokio::task::spawn_blocking({
        let db = ctx.db.clone();
        move || db.with_any(|c| crate::store::mcp_list(c).map_err(|e| e.to_string()))
    })
    .await
    .unwrap_or_else(|e| Err(format!("任务失败: {e}")));

    let list = servers.map_err(|e| format!("读取 MCP Server 列表失败: {e}"))?;

    let Some(s) = list.iter().find(|s| s.name == name) else {
        return Err(format!(
            "未找到名为「{name}」的 MCP Server；用 list_mcp_servers 查看现有登记"
        ));
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
    Ok(json!({
        "name": s.name,
        "kind": s.kind,
        "enabled": s.enabled,
        "connect": connect,
        "note": "env 仅返回键名；实际值在客户端/供应商侧配置。"
    }))
}

async fn tool_tool_schemas(ctx: &GatewayCtx, name: &str) -> Result<Value, String> {
    let servers = tokio::task::spawn_blocking({
        let db = ctx.db.clone();
        move || db.with_any(|c| crate::store::mcp_list(c).map_err(|e| e.to_string()))
    })
    .await
    .unwrap_or_else(|e| Err(format!("任务失败: {e}")));

    let list = servers.map_err(|e| format!("读取 MCP Server 列表失败: {e}"))?;

    let Some(s) = list.iter().find(|s| s.name == name) else {
        return Err(format!(
            "未找到名为「{name}」的 MCP Server；用 list_mcp_servers 查看现有登记"
        ));
    };
    if !s.enabled {
        return Err(format!("MCP Server「{name}」未启用，暂不查询其工具"));
    }

    // 复用 mcp.rs 的工具发现（10s 超时），只读不执行
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        crate::mcp::list_tools(s),
    )
    .await
    {
        Ok(Ok(tools)) => Ok(json!({
            "server": s.name,
            "tools": tools,
            "note": "工具定义仅供参考；本接口不执行工具。"
        })),
        Ok(Err(e)) => Err(format!("tools/list 失败: {e}")),
        Err(_) => Err("tools/list 超时(10s)".to_string()),
    }
}

async fn tool_list_skills(ctx: &GatewayCtx) -> Result<Value, String> {
    let list = load_skills(ctx)
        .await
        .map_err(|e| format!("读取技能列表失败: {e}"))?;
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
    Ok(json!({"skills": items}))
}

/// 台账查询：返回技能全文。
///
/// bug 清单 16：**与投递口径一致**——未启用（enabled=0）的技能不再返回全文，
/// 避免 `enabled` 这道闸门被旁路（投递拒绝、台账却照给，语义上是同一件事）。
async fn tool_skill_detail(ctx: &GatewayCtx, name: &str) -> Result<Value, String> {
    let list = load_skills(ctx).await?;
    let Some(s) = list.iter().find(|s| s.name == name) else {
        return Err(format!(
            "未找到名为「{name}」的技能；用 list_skills 查看现有登记"
        ));
    };
    if !s.enabled {
        return Err(format!(
            "技能「{name}」未启用（enabled=0）；需要时先在 JAI「技能」页启用"
        ));
    }
    Ok(json!({
        "name": s.name,
        "description": s.description,
        "enabled": s.enabled,
        "content": s.content,
    }))
}

/// 投递 Skill 全文（`skill__<name>` 工具）。复用 `skill_list` 查找；
/// 只投递给已启用（enabled=1）的技能，超长按 `SKILL_MAX_BYTES` 截断并注明。
async fn deliver_skill(ctx: &GatewayCtx, name: &str) -> Result<Value, String> {
    let skills = load_skills(ctx).await?;

    let Some(s) = skills.iter().find(|s| s.name == name) else {
        return Err(format!(
            "未找到名为「{name}」的技能，用 list_skills 查看现有登记"
        ));
    };
    if !s.enabled {
        return Err(format!("技能「{name}」未启用（enabled=0），暂不投递"));
    }

    let content = &s.content;
    let truncated = content.len() > SKILL_MAX_BYTES;
    let text = if truncated {
        // 裁到不超过上限的字符边界，避免把 UTF-8 多字节字符切断
        let mut t = content[..content.floor_char_boundary(SKILL_MAX_BYTES)].to_string();
        t.push_str("\n\n……（技能全文超出 32KB，已截断）");
        t
    } else {
        content.clone()
    };

    Ok(json!({
        "skill": s.name,
        "description": s.description,
        "truncated": truncated,
        "content": text,
    }))
}

/// 解析 `server__tool` 命名（按第一个双下划线分割；server 名本身可含 `_`）。
fn parse_proxy_name(name: &str) -> Option<(&str, &str)> {
    let (server, tool) = name.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server, tool))
}

/// 代理转发单次调用的等待预算。
///
/// 取「略低于常见 MCP 客户端的单次调用预算」：dsh 客户端默认 **60s 硬中止**
/// （MCP 错误码 -32001），若网关比客户端更能等，agent 只会拿到一个毫无信息量
/// 的客户端超时，同批并行的其它工具调用还会一起被作废
/// （2026-09-15 实测：网关等满 60.007s 才拿到上游结果，客户端 60.000s 已放弃，
/// 差 7ms 输掉竞速 → `MCP error -32001: Request timed out`）。
/// 这里提前失败，让 agent 拿到一条**可执行的工具级错误**而不是客户端超时。
/// 需要更长预算时用 `JAI_MCP_PROXY_CALL_TIMEOUT_MS` 覆写。
const DEFAULT_PROXY_CALL_TIMEOUT: Duration = Duration::from_secs(55);

fn proxy_call_timeout() -> Duration {
    crate::mcp::env_duration_ms("JAI_MCP_PROXY_CALL_TIMEOUT_MS", DEFAULT_PROXY_CALL_TIMEOUT)
}

/// 预算的人类可读形式：整秒用 `55s`，否则用毫秒（测试会压到几百毫秒）。
fn fmt_budget(d: Duration) -> String {
    if d.as_secs() >= 1 && d.subsec_millis() == 0 {
        format!("{}s", d.as_secs())
    } else {
        format!("{}ms", d.as_millis())
    }
}

/// `tools/call` 的返回形态：
/// - `Info`：网关自产信息（静态台账 / 技能投递）→ 序列化成单个 text 块；
/// - `Proxied`：代理转发的上游 MCP 结果 → 按 MCP 语义**原样透传**
///   （content 逐块保留 / isError 原样冒泡 / structuredContent 保留）。
///
/// 代理结果绝不能像早期实现那样「包成 {source,result} 再 to_string()」：那会
/// ① 把上游工具级失败（isError=true）压成外层 isError=false，客户端按外层判定
///    → agent 把失败当成功；② 丢掉 content 结构（image / resource_link）与
///    structuredContent，客户端再也做不了图片投影。
#[derive(Debug)]
enum ToolOutcome {
    Info(Value),
    Proxied { server: String, result: Value },
}

/// 代理转发：`<server>__<tool>` → 真实 MCP Server 执行。
async fn proxy_call_tool(
    ctx: &GatewayCtx,
    server_name: &str,
    tool_name: &str,
    args: &Value,
) -> Result<ToolOutcome, String> {
    let servers = proxy_servers(ctx);
    let Some(server) = servers.iter().find(|s| s.name == server_name) else {
        return Err(format!(
            "Server「{server_name}」不存在或未开启代理执行（proxy_allowed）"
        ));
    };
    let budget = proxy_call_timeout();
    let start = Instant::now();
    let result = match tokio::time::timeout(
        budget,
        crate::mcp::call_tool(server, tool_name, args.clone()),
    )
    .await
    {
        Ok(r) => r.map_err(|e| format!("[proxy: {server_name}] {tool_name} 调用失败: {e}")),
        Err(_) => Err(format!(
            "[proxy: {server_name}] {tool_name} 超过 {} 未返回，网关已主动放弃本次等待。\
             长时命令请改用「先启动后轮询」类工具（如 terminal_start + terminal_poll）；\
             确需更长等待可调 JAI_MCP_PROXY_CALL_TIMEOUT_MS。",
            fmt_budget(budget)
        )),
    };
    let duration_ms = start.elapsed().as_millis() as i64;

    // 审计落库（独立 proxy_call_logs 表；失败也要记，便于排查）。
    // 注意：status 只反映「转发层是否拿到结果」；上游工具级失败（isError=true）
    // 仍是 ok —— 那是 MCP 语义下的成功调用，不是网关错误。
    let audit = result
        .as_ref()
        .map(|_| ("ok", None))
        .unwrap_or_else(|e| ("error", Some(e.as_str())));
    let kind = server.kind.clone();
    let db = ctx.db.clone();
    let (status, err) = audit;
    let err_owned = err.map(str::to_string);
    let log_server = server_name.to_string();
    let log_tool = tool_name.to_string();
    tokio::task::spawn_blocking(move || {
        let _ = db.with_any(|c| {
            crate::store::proxy_call_log(
                c,
                &log_server,
                &log_tool,
                &kind,
                status,
                duration_ms,
                err_owned.as_deref(),
            )
        });
    });

    Ok(ToolOutcome::Proxied {
        server: server_name.to_string(),
        result: result?,
    })
}

/// 代理结果 → `tools/call` 的 MCP 结果形状（原样透传 + 来源标注）。
fn proxy_result_payload(server: &str, upstream: Value) -> Value {
    let mut content = upstream
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if content.is_empty() {
        // 上游没给 content（异常形状）：退化成整包 JSON 文本，至少不丢信息
        content.push(json!({"type": "text", "text": upstream.to_string()}));
    }
    let mut out = json!({
        "content": content,
        // 关键：上游工具级失败必须冒泡，否则客户端会把失败当成功
        "isError": upstream.get("isError").and_then(Value::as_bool).unwrap_or(false),
        // 非 MCP 标准字段：客户端会忽略，仅供审计/排查定位来源
        "source": format!("jai-gateway-proxy/{server}"),
    });
    if let Some(sc) = upstream.get("structuredContent") {
        out["structuredContent"] = sc.clone();
    }
    out
}

async fn dispatch_tool(ctx: &GatewayCtx, name: &str, args: &Value) -> Result<ToolOutcome, String> {
    // Skill 投递优先：`skill__<...>` —— 必须先于 proxy 判断，
    // 否则 `skill__code-review` 会被 parse_proxy_name 拆成 server="skill"。
    if name.starts_with(SKILL_TOOL_PREFIX) {
        let skills = load_skills(ctx).await?;
        let map = skill_tool_names(&skills);
        let Some(real) = resolve_skill_name(&map, name) else {
            return Err(format!("未找到工具 {name}；用 list_skills 查看现有技能"));
        };
        return deliver_skill(ctx, &real).await.map(ToolOutcome::Info);
    }
    // 代理工具：命中 `server__tool` 且 server 可代理才转发
    if let Some((server, tool)) = parse_proxy_name(name) {
        // 若与静态工具重名（如 "list_mcp_servers" 不含 __，不会走到这），
        // 直接按代理语义处理；server 校验在 proxy_call_tool 内完成。
        return proxy_call_tool(ctx, server, tool, args).await;
    }
    // 台账类工具：失败一律走 Err（→ isError=true），不再把错误塞进成功载荷
    // （bug 清单 17：与 v0.2.0 bug 10 的「失败不得被拉平成成功」哲学统一）。
    match name {
        "list_mcp_servers" => Ok(ToolOutcome::Info(tool_list_mcp_servers(ctx).await?)),
        "get_mcp_server_detail" => {
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or("缺少参数 name")?;
            Ok(ToolOutcome::Info(tool_server_detail(ctx, name).await?))
        }
        "get_tool_schemas" => {
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or("缺少参数 name")?;
            Ok(ToolOutcome::Info(tool_tool_schemas(ctx, name).await?))
        }
        "list_skills" => Ok(ToolOutcome::Info(tool_list_skills(ctx).await?)),
        "get_skill_detail" => {
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or("缺少参数 name")?;
            Ok(ToolOutcome::Info(tool_skill_detail(ctx, name).await?))
        }
        other => Err(format!("未知工具: {other}")),
    }
}

/// 读技能列表（唯一入口：错误统一转 String）。
async fn load_skills(ctx: &GatewayCtx) -> Result<Vec<SkillRow>, String> {
    let db = ctx.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with_any(|c| crate::store::skill_list(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(|e| format!("任务失败: {e}"))?
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
            // + 动态 Skill 工具（enabled=1 的 skill__<name>）
            let mut tools = tool_specs();
            tools.extend(dynamic_tool_specs(ctx).await);
            tools.extend(dynamic_skill_specs(ctx).await);
            json!({"tools": tools})
        }
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match dispatch_tool(ctx, name, &args).await {
                // 网关自产信息（静态台账 / 技能全文）：序列化进单个 text 块
                Ok(ToolOutcome::Info(payload)) => json!({
                    "content": [{"type": "text", "text": payload.to_string()}],
                    "isError": false
                }),
                // 代理转发：上游 MCP 结果原样透传（含 isError，失败必须让 agent 看见）
                Ok(ToolOutcome::Proxied { server, result }) => {
                    proxy_result_payload(&server, result)
                }
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

    /// 测试辅助：断言结果是「网关自产信息」（静态台账 / 技能投递）并取出 JSON。
    /// 代理转发路径返回 `ToolOutcome::Proxied`，需用 `proxy_result_payload` 单独断言。
    fn info(v: Result<ToolOutcome, String>) -> Value {
        match v.expect("dispatch_tool 失败") {
            ToolOutcome::Info(v) => v,
            ToolOutcome::Proxied { server, .. } => {
                panic!("期望 Info，实际是 Proxied（server={server}）")
            }
        }
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
        assert_eq!(
            parse_proxy_name("netcatty__get_environment"),
            Some(("netcatty", "get_environment"))
        );
        assert_eq!(
            parse_proxy_name("my_server__do_thing"),
            Some(("my_server", "do_thing"))
        );
        // 缺 tool / 缺 server / 无分隔符 → 不是代理名
        assert_eq!(parse_proxy_name("netcatty__"), None);
        assert_eq!(parse_proxy_name("__tool"), None);
        assert_eq!(parse_proxy_name("list_mcp_servers"), None);
    }

    #[test]
    fn proxy_signature_stable_and_sensitive_to_config() {
        let row = store::McpServerRow {
            id: "s".into(),
            name: "a".into(),
            kind: "stdio".into(),
            command: Some("/bin/x".into()),
            args: None,
            url: None,
            env: None,
            proxy_allowed: true,
            enabled: true,
            created_at: 0,
            updated_at: 0,
        };
        let rows = [row.clone()];
        let a = proxy_signature(&rows);
        let b = proxy_signature(&rows);
        assert_eq!(a, b, "相同配置签名应一致");
        let mut changed = row;
        changed.command = Some("/bin/y".into());
        assert_ne!(
            a,
            proxy_signature(std::slice::from_ref(&changed)),
            "配置变化签名应改变"
        );
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
        // enabled=1 的 skill 会以 skill__<name> 暴露（M2 正常行为）
        assert!(
            names.contains(&"skill__code-review"),
            "应暴露已启用 skill: {names:?}"
        );
        // 没有可用代理 target 时，不应混入任何 `server__tool` 动态代理项
        let proxied: Vec<&str> = names
            .iter()
            .copied()
            .filter(|n| n.contains("__") && !n.starts_with("skill__"))
            .collect();
        assert!(
            proxied.is_empty(),
            "有不可达代理 target，不应出现 server__tool: {proxied:?}"
        );
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_error() {
        let ctx = test_ctx();
        let err = dispatch_tool(&ctx, "no_such_tool", &json!({}))
            .await
            .unwrap_err();
        assert!(err.contains("未知工具"), "未知静态工具应报错: {err}");
    }

    #[tokio::test]
    async fn skills_list_exposes_enabled_as_skill_tools() {
        let ctx = test_ctx();
        seed(&ctx);
        let resp = handle_rpc(&ctx, &rpc("tools/list", json!({}), json!(9)))
            .await
            .unwrap();
        let tools = resp["result"]["tools"].as_array().unwrap();
        let skill_tools: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .filter(|n| n.starts_with("skill__"))
            .collect();
        // code-review enabled=1 → 暴露；其他 skill 不存在 → 仅此一个
        assert_eq!(skill_tools, vec!["skill__code-review"]);
    }

    #[tokio::test]
    async fn skill_delivery_returns_full_content() {
        let ctx = test_ctx();
        seed(&ctx);
        let payload = info(dispatch_tool(&ctx, "skill__code-review", &json!({})).await);
        assert_eq!(payload["skill"], "code-review");
        assert_eq!(payload["truncated"], false);
        assert_eq!(payload["content"], "按提交做评审。");
    }

    #[tokio::test]
    async fn skill_delivery_rejects_disabled_or_missing() {
        let ctx = test_ctx();
        seed(&ctx);
        // 先插入一个未启用的 skill
        let now = store::now_ms();
        ctx.db
            .with(|c| {
                store::skill_insert(
                    c,
                    &store::SkillRow {
                        id: "k2".into(),
                        name: "disabled-skill".into(),
                        description: "未启用".into(),
                        content: "不该被投递".into(),
                        enabled: false,
                        created_at: now,
                        updated_at: now,
                    },
                )
                .unwrap();
                Ok::<_, store::StoreError>(())
            })
            .unwrap();

        let err = dispatch_tool(&ctx, "skill__disabled-skill", &json!({}))
            .await
            .unwrap_err();
        assert!(err.contains("未启用"), "未启用 skill 应拒绝: {err}");

        let err = dispatch_tool(&ctx, "skill__no-such", &json!({}))
            .await
            .unwrap_err();
        assert!(err.contains("未找到"), "不存在的 skill 应报错: {err}");
    }

    #[tokio::test]
    async fn skill_delivery_truncates_over_32kb() {
        let ctx = test_ctx();
        // 插入一个超大 skill（> 32KB）
        let long = "长文内容-".repeat(5000); // ~50000 字节
        assert!(long.len() > SKILL_MAX_BYTES);
        let now = store::now_ms();
        ctx.db
            .with(|c| {
                store::skill_insert(
                    c,
                    &store::SkillRow {
                        id: "k-big".into(),
                        name: "big-skill".into(),
                        description: "超长技能".into(),
                        content: long.clone(),
                        enabled: true,
                        created_at: now,
                        updated_at: now,
                    },
                )
                .unwrap();
                Ok::<_, store::StoreError>(())
            })
            .unwrap();

        let payload = info(dispatch_tool(&ctx, "skill__big-skill", &json!({})).await);
        assert_eq!(payload["truncated"], true);
        let text = payload["content"].as_str().unwrap();
        assert!(
            text.len() <= SKILL_MAX_BYTES + 64,
            "截断后仍超限: {}",
            text.len()
        );
        assert!(text.contains("已截断"), "应注明截断: {text}");
        // 截断处不把多字节 UTF-8 切断：content 前缀（去掉尾部标记行）须为合法 UTF-8
        if let Some(pos) = text.find("……（技能全文超出") {
            let prefix = &text[..pos];
            assert!(
                prefix.len() <= SKILL_MAX_BYTES,
                "截断边界超过上限: {}",
                prefix.len()
            );
            assert!(prefix.is_char_boundary(prefix.len()), "截断落在字符中间");
        }
    }
}
