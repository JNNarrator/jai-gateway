//! SQLite 存储层：连接初始化、迁移执行器、CRUD 仓储。
//!
//! 设计权威：docs/design/storage-schema.md（v1 已定稿）。
//! 迁移机制：`PRAGMA user_version` 逐版本事务推进，零外部依赖（§7）。

pub mod export;
pub mod import;
pub mod logs;
pub mod migrations;
pub mod retention;
pub mod snapshot;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

// ================================================================ 连接与迁移

/// 打开数据库并应用全部待执行迁移。幂等：重复调用无副作用（M0 验收标准 2）。
///
/// `journal_mode=WAL` / `foreign_keys=ON` / `synchronous=NORMAL` + `busy_timeout`
/// 为 storage §3 定案的连接初始化 PRAGMA 组合（§5-5 DB 忙碌降级）。
/// WAL 对内存库无效，仅对文件库设置；`:memory:` 跳过（测试路径）。
pub fn open_and_migrate(path: &str) -> Result<Connection, StoreError> {
    let conn = Connection::open(path)?;
    if path != ":memory:" {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.busy_timeout(std::time::Duration::from_millis(2500))?;
    }
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(conn)
}

/// 在既有连接上执行迁移（供测试注入内存库）。
pub fn migrate(conn: &Connection) -> Result<(), StoreError> {
    // 0003 需要重建 providers 表；迁移期临时关闭外键，跑完恢复
    conn.pragma_update(None, "foreign_keys", "OFF")?;
    let result = (|| -> Result<(), StoreError> {
        let mut current: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (idx, (name, sql)) in migrations::MIGRATIONS.iter().enumerate() {
            let version = (idx + 1) as u32;
            if version <= current {
                continue;
            }
            conn.execute_batch(&format!(
                "BEGIN;\n{sql}\nPRAGMA user_version = {version};\nCOMMIT;"
            ))?;
            current = version;
            eprintln!("[store] migration applied: {name}");
        }
        Ok(())
    })();
    conn.pragma_update(None, "foreign_keys", "ON")?;
    result
}

// ================================================================ 共享句柄

/// 可跨任务共享的数据库句柄。单写连接 + Mutex 串行化；
/// 高频日志写入走独立的 logger 连接（见 logs::spawn_logger，WAL 允许读写并发）。
#[derive(Clone)]
pub struct Db(pub Arc<Mutex<Connection>>);

impl Db {
    pub fn open(path: &str) -> Result<Self, StoreError> {
        Ok(Db(Arc::new(Mutex::new(open_and_migrate(path)?))))
    }

    pub fn in_memory() -> Result<Self, StoreError> {
        Ok(Db(Arc::new(Mutex::new(open_and_migrate(":memory:")?))))
    }

    /// 在锁内执行同步闭包。本地 SQLite 操作极短；调用方负责放入 spawn_blocking。
    pub fn with<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let guard = self.0.lock().unwrap_or_else(|p| p.into_inner());
        f(&guard)
    }

    /// 闭包可返回任意错误类型（IPC 层常把多步操作折叠成 String）。
    pub fn with_any<T, E>(&self, f: impl FnOnce(&Connection) -> Result<T, E>) -> Result<T, E> {
        let guard = self.0.lock().unwrap_or_else(|p| p.into_inner());
        f(&guard)
    }
}

// ================================================================ 行模型

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRow {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub family: String,
    pub enabled: bool,
    pub priority: i64,
    pub extra_headers: Option<String>,
    /// keyring 引用：服务端内部使用；序列化到 UI 的 DTO 必须剔除
    #[serde(skip_serializing)]
    pub keyring_ref: String,
    pub last_ok_at: Option<i64>,
    pub last_err_at: Option<i64>,
    pub last_err_msg: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRow {
    pub id: String,
    pub provider_id: String,
    pub model_name: String,
    pub upstream_model_id: Option<String>,
    pub context_window: Option<i64>,
    pub max_output_tokens: i64,
    pub enabled: bool,
}

// ================================================================ providers

const PROVIDER_COLS: &str = "id,name,base_url,family,enabled,priority,extra_headers,keyring_ref,last_ok_at,last_err_at,last_err_msg,created_at,updated_at";

fn row_to_provider(r: &rusqlite::Row) -> rusqlite::Result<ProviderRow> {
    Ok(ProviderRow {
        id: r.get(0)?,
        name: r.get(1)?,
        base_url: r.get(2)?,
        family: r.get(3)?,
        enabled: r.get::<_, i64>(4)? != 0,
        priority: r.get(5)?,
        extra_headers: r.get(6)?,
        keyring_ref: r.get(7)?,
        last_ok_at: r.get(8)?,
        last_err_at: r.get(9)?,
        last_err_msg: r.get(10)?,
        created_at: r.get(11)?,
        updated_at: r.get(12)?,
    })
}

fn row_to_model(r: &rusqlite::Row) -> rusqlite::Result<ModelRow> {
    Ok(ModelRow {
        id: r.get(0)?,
        provider_id: r.get(1)?,
        model_name: r.get(2)?,
        upstream_model_id: r.get(3)?,
        context_window: r.get(4)?,
        max_output_tokens: r.get(5)?,
        enabled: r.get::<_, i64>(6)? != 0,
    })
}

pub fn provider_insert(c: &Connection, p: &ProviderRow) -> Result<(), StoreError> {
    c.execute(
        &format!(
            "INSERT INTO providers({PROVIDER_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)"
        ),
        params![
            p.id,
            p.name,
            p.base_url,
            p.family,
            p.enabled as i64,
            p.priority,
            p.extra_headers,
            p.keyring_ref,
            p.last_ok_at,
            p.last_err_at,
            p.last_err_msg,
            p.created_at,
            p.updated_at
        ],
    )?;
    Ok(())
}

pub fn provider_get(c: &Connection, id: &str) -> Result<Option<ProviderRow>, StoreError> {
    let sql = format!("SELECT {PROVIDER_COLS} FROM providers WHERE id = ?1");
    Ok(c.query_row(&sql, [id], row_to_provider).optional()?)
}

/// 按 (name, base_url) 查找现有供应商，供导入去重使用。
pub fn provider_get_by_name_base(
    c: &Connection,
    name: &str,
    base_url: &str,
) -> Result<Option<ProviderRow>, StoreError> {
    let sql = format!("SELECT {PROVIDER_COLS} FROM providers WHERE name = ?1 AND base_url = ?2");
    Ok(c.query_row(&sql, [name, base_url], row_to_provider)
        .optional()?)
}

pub fn provider_list(c: &Connection) -> Result<Vec<ProviderRow>, StoreError> {
    let sql =
        format!("SELECT {PROVIDER_COLS} FROM providers ORDER BY priority ASC, created_at ASC");
    let mut stmt = c.prepare(&sql)?;
    let rows = stmt
        .query_map([], row_to_provider)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 更新非 None 字段，updated_at 必更。extra_headers 用 Some(Option<&str>)，
/// 内层 None 表示显式清空。
pub fn provider_update_fields(
    c: &Connection,
    id: &str,
    name: Option<&str>,
    base_url: Option<&str>,
    priority: Option<i64>,
    extra_headers: Option<Option<&str>>,
) -> Result<(), StoreError> {
    let now = now_ms();
    if let Some(n) = name {
        c.execute(
            "UPDATE providers SET name=?1, updated_at=?2 WHERE id=?3",
            params![n, now, id],
        )?;
    }
    if let Some(u) = base_url {
        c.execute(
            "UPDATE providers SET base_url=?1, updated_at=?2 WHERE id=?3",
            params![u, now, id],
        )?;
    }
    if let Some(pr) = priority {
        c.execute(
            "UPDATE providers SET priority=?1, updated_at=?2 WHERE id=?3",
            params![pr, now, id],
        )?;
    }
    if let Some(eh) = extra_headers {
        c.execute(
            "UPDATE providers SET extra_headers=?1, updated_at=?2 WHERE id=?3",
            params![eh, now, id],
        )?;
    }
    Ok(())
}

pub fn provider_set_enabled(c: &Connection, id: &str, enabled: bool) -> Result<(), StoreError> {
    c.execute(
        "UPDATE providers SET enabled=?1, updated_at=?2 WHERE id=?3",
        params![enabled as i64, now_ms(), id],
    )?;
    Ok(())
}

pub fn provider_mark_ok(c: &Connection, id: &str) -> Result<(), StoreError> {
    c.execute(
        "UPDATE providers SET last_ok_at=?1, last_err_at=NULL, last_err_msg=NULL WHERE id=?2",
        params![now_ms(), id],
    )?;
    Ok(())
}

pub fn provider_mark_err(c: &Connection, id: &str, msg: &str) -> Result<(), StoreError> {
    let truncated: String = msg.chars().take(300).collect();
    c.execute(
        "UPDATE providers SET last_err_at=?1, last_err_msg=?2 WHERE id=?3",
        params![now_ms(), truncated, id],
    )?;
    Ok(())
}

/// 删除供应商行（models 由 CASCADE 清理）。返回受影响行数。
pub fn provider_delete(c: &Connection, id: &str) -> Result<usize, StoreError> {
    Ok(c.execute("DELETE FROM providers WHERE id=?1", [id])?)
}

// ================================================================ models

const MODEL_COLS: &str =
    "id,provider_id,model_name,upstream_model_id,context_window,max_output_tokens,enabled";

pub fn model_upsert(
    c: &Connection,
    provider_id: &str,
    model_name: &str,
    context_window: Option<i64>,
    max_output_tokens: i64,
) -> Result<(), StoreError> {
    let id = uuid::Uuid::now_v7().to_string();
    c.execute(
        &format!(
            "INSERT INTO models({MODEL_COLS}) VALUES (?1,?2,?3,NULL,?4,?5,1)
             ON CONFLICT(provider_id, model_name) DO UPDATE SET
               context_window=excluded.context_window,
               max_output_tokens=excluded.max_output_tokens"
        ),
        params![
            id,
            provider_id,
            model_name,
            context_window,
            max_output_tokens
        ],
    )?;
    Ok(())
}

pub fn model_get(c: &Connection, model_id: &str) -> Result<Option<ModelRow>, StoreError> {
    let sql = format!("SELECT {MODEL_COLS} FROM models WHERE id=?1");
    Ok(c.query_row(&sql, [model_id], row_to_model).optional()?)
}

pub fn model_list_by_provider(
    c: &Connection,
    provider_id: &str,
) -> Result<Vec<ModelRow>, StoreError> {
    let sql = format!("SELECT {MODEL_COLS} FROM models WHERE provider_id=?1 ORDER BY model_name");
    let mut stmt = c.prepare(&sql)?;
    let rows = stmt
        .query_map([provider_id], row_to_model)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 按 (provider_id, model_name) 查模型行（导入后按需禁用）。
pub fn model_get_by_provider_name(
    c: &Connection,
    provider_id: &str,
    model_name: &str,
) -> Result<Option<ModelRow>, StoreError> {
    let sql = format!("SELECT {MODEL_COLS} FROM models WHERE provider_id=?1 AND model_name=?2");
    Ok(c.query_row(&sql, [provider_id, model_name], row_to_model)
        .optional()?)
}

pub fn model_update_limits(
    c: &Connection,
    model_id: &str,
    context_window: Option<i64>,
    max_output_tokens: i64,
) -> Result<(), StoreError> {
    c.execute(
        "UPDATE models SET context_window=?1, max_output_tokens=?2 WHERE id=?3",
        params![context_window, max_output_tokens, model_id],
    )?;
    Ok(())
}

pub fn model_toggle(c: &Connection, model_id: &str, enabled: bool) -> Result<(), StoreError> {
    c.execute(
        "UPDATE models SET enabled=?1 WHERE id=?2",
        params![enabled as i64, model_id],
    )?;
    Ok(())
}

/// 路由候选查询 —— storage §3 定案 SQL。按 (priority, rowid) 序逐渠道尝试。
#[derive(Debug, Clone)]
pub struct RouteCandidate {
    pub provider_id: String,
    pub provider_name: String,
    pub base_url: String,
    pub family: String,
    pub extra_headers: Option<String>,
    pub keyring_ref: String,
    pub upstream_model_id: Option<String>,
    pub max_output_tokens: i64,
}

pub fn route_candidates(
    c: &Connection,
    model_name: &str,
) -> Result<Vec<RouteCandidate>, StoreError> {
    let sql = "SELECT p.id, p.name, p.base_url, p.family, p.extra_headers, p.keyring_ref,
                m.upstream_model_id, m.max_output_tokens
         FROM models m JOIN providers p ON p.id = m.provider_id
         WHERE m.model_name = ?1 AND m.enabled = 1 AND p.enabled = 1
         ORDER BY p.priority ASC, m.rowid ASC";
    let mut stmt = c.prepare(sql)?;
    let rows = stmt
        .query_map([model_name], |r| {
            Ok(RouteCandidate {
                provider_id: r.get(0)?,
                provider_name: r.get(1)?,
                base_url: r.get(2)?,
                family: r.get(3)?,
                extra_headers: r.get(4)?,
                keyring_ref: r.get(5)?,
                upstream_model_id: r.get(6)?,
                max_output_tokens: r.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ================================================================ gateway_keys

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayKeyRow {
    pub id: String,
    /// 明文存储是已拍板决策（storage §6）；此字段仅供网关认证与一次性回显使用，
    /// 列表 DTO 序列化时被 skip 掉，永远不出 `gateway_key_list/reveal` 之外的接口。
    #[serde(skip_serializing)]
    pub key: String,
    pub prefix: String,
    pub label: Option<String>,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
    pub last_used_at: Option<i64>,
}

pub fn gw_key_active(c: &Connection) -> Result<Option<GatewayKeyRow>, StoreError> {
    let sql = "SELECT id,key,prefix,label,created_at,revoked_at,last_used_at
               FROM gateway_keys WHERE revoked_at IS NULL ORDER BY created_at DESC LIMIT 1";
    Ok(c.query_row(sql, [], |r| {
        Ok(GatewayKeyRow {
            id: r.get(0)?,
            key: r.get(1)?,
            prefix: r.get(2)?,
            label: r.get(3)?,
            created_at: r.get(4)?,
            revoked_at: r.get(5)?,
            last_used_at: r.get(6)?,
        })
    })
    .optional()?)
}

/// 创建新网关密钥并吊销旧密钥（storage §6-2 轮换语义：吊销+新建，保留审计痕迹）。
pub fn gw_key_rotate(
    c: &Connection,
    new_key: &str,
    label: Option<&str>,
) -> Result<GatewayKeyRow, StoreError> {
    let now = now_ms();
    c.execute(
        "UPDATE gateway_keys SET revoked_at=?1 WHERE revoked_at IS NULL",
        [now],
    )?;
    let row = GatewayKeyRow {
        id: uuid::Uuid::now_v7().to_string(),
        key: new_key.to_string(),
        prefix: new_key.chars().take(14).collect(),
        label: label.map(str::to_string),
        created_at: now,
        revoked_at: None,
        last_used_at: None,
    };
    c.execute(
        "INSERT INTO gateway_keys(id,key,prefix,label,created_at,revoked_at,last_used_at)
         VALUES (?1,?2,?3,?4,?5,NULL,NULL)",
        params![row.id, row.key, row.prefix, row.label, row.created_at],
    )?;
    Ok(row)
}

/// 认证命中后节流更新 last_used_at（至多每 60s 一次，避免写放大）。
pub fn gw_key_touch(c: &Connection, id: &str) -> Result<(), StoreError> {
    c.execute(
        "UPDATE gateway_keys SET last_used_at=?1
         WHERE id=?2 AND (last_used_at IS NULL OR ?1 - last_used_at >= 60000)",
        params![now_ms(), id],
    )?;
    Ok(())
}

// ================================================================ MCP / Skills（MCP & skill 管理）

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerRow {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub command: Option<String>,
    /// JSON 数组字符串
    pub args: Option<String>,
    pub url: Option<String>,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content: String,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

fn row_to_mcp_server(r: &rusqlite::Row) -> rusqlite::Result<McpServerRow> {
    Ok(McpServerRow {
        id: r.get(0)?,
        name: r.get(1)?,
        kind: r.get(2)?,
        command: r.get(3)?,
        args: r.get(4)?,
        url: r.get(5)?,
        enabled: r.get::<_, i64>(6)? != 0,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
    })
}

fn row_to_skill(r: &rusqlite::Row) -> rusqlite::Result<SkillRow> {
    Ok(SkillRow {
        id: r.get(0)?,
        name: r.get(1)?,
        description: r.get(2)?,
        content: r.get(3)?,
        enabled: r.get::<_, i64>(4)? != 0,
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
    })
}

pub fn mcp_list(c: &Connection) -> Result<Vec<McpServerRow>, StoreError> {
    let mut stmt = c.prepare(
        "SELECT id,name,kind,command,args,url,enabled,created_at,updated_at
         FROM mcp_servers ORDER BY name",
    )?;
    let rows = stmt
        .query_map([], row_to_mcp_server)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn mcp_insert(c: &Connection, row: &McpServerRow) -> Result<(), StoreError> {
    c.execute(
        "INSERT INTO mcp_servers(id,name,kind,command,args,url,enabled,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            row.id,
            row.name,
            row.kind,
            row.command,
            row.args,
            row.url,
            row.enabled as i64,
            row.created_at,
            row.updated_at,
        ],
    )?;
    Ok(())
}

pub fn mcp_update(
    c: &Connection,
    id: &str,
    name: &str,
    kind: &str,
    command: Option<&str>,
    args: Option<&str>,
    url: Option<&str>,
) -> Result<(), StoreError> {
    c.execute(
        "UPDATE mcp_servers SET name=?1, kind=?2, command=?3, args=?4, url=?5, updated_at=?6
         WHERE id=?7",
        params![name, kind, command, args, url, now_ms(), id],
    )?;
    Ok(())
}

pub fn mcp_set_enabled(c: &Connection, id: &str, enabled: bool) -> Result<(), StoreError> {
    c.execute(
        "UPDATE mcp_servers SET enabled=?1, updated_at=?2 WHERE id=?3",
        params![enabled as i64, now_ms(), id],
    )?;
    Ok(())
}

pub fn mcp_delete(c: &Connection, id: &str) -> Result<usize, StoreError> {
    Ok(c.execute("DELETE FROM mcp_servers WHERE id=?1", [id])?)
}

pub fn skill_list(c: &Connection) -> Result<Vec<SkillRow>, StoreError> {
    let mut stmt = c.prepare(
        "SELECT id,name,description,content,enabled,created_at,updated_at
         FROM skills ORDER BY name",
    )?;
    let rows = stmt
        .query_map([], row_to_skill)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn skill_insert(c: &Connection, row: &SkillRow) -> Result<(), StoreError> {
    c.execute(
        "INSERT INTO skills(id,name,description,content,enabled,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            row.id,
            row.name,
            row.description,
            row.content,
            row.enabled as i64,
            row.created_at,
            row.updated_at,
        ],
    )?;
    Ok(())
}

pub fn skill_update(
    c: &Connection,
    id: &str,
    name: &str,
    description: &str,
    content: &str,
) -> Result<(), StoreError> {
    c.execute(
        "UPDATE skills SET name=?1, description=?2, content=?3, updated_at=?4 WHERE id=?5",
        params![name, description, content, now_ms(), id],
    )?;
    Ok(())
}

pub fn skill_set_enabled(c: &Connection, id: &str, enabled: bool) -> Result<(), StoreError> {
    c.execute(
        "UPDATE skills SET enabled=?1, updated_at=?2 WHERE id=?3",
        params![enabled as i64, now_ms(), id],
    )?;
    Ok(())
}

pub fn skill_delete(c: &Connection, id: &str) -> Result<usize, StoreError> {
    Ok(c.execute("DELETE FROM skills WHERE id=?1", [id])?)
}

// ================================================================ meta KV

pub fn meta_get(c: &Connection, key: &str) -> Result<Option<String>, StoreError> {
    Ok(
        c.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
            .optional()?,
    )
}

pub fn meta_set(c: &Connection, key: &str, value_json: &str) -> Result<(), StoreError> {
    c.execute(
        "INSERT INTO meta(key,value) VALUES(?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value_json],
    )?;
    Ok(())
}

// ================================================================ tool_id_map（协议 IR §5-B）

/// 超长 tool_use id 映射 TTL（storage §5：7 天滚动过期）。
pub const TOOL_ID_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// 写入/更新 outbound_id → canonical_id 映射。
pub fn tool_id_put(
    c: &Connection,
    outbound_id: &str,
    canonical_id: &str,
) -> Result<(), StoreError> {
    let now = now_ms();
    c.execute(
        "INSERT INTO tool_id_map(outbound_id, canonical_id, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(outbound_id) DO UPDATE SET
           canonical_id=excluded.canonical_id,
           created_at=excluded.created_at,
           expires_at=excluded.expires_at",
        params![outbound_id, canonical_id, now, now + TOOL_ID_TTL_MS],
    )?;
    Ok(())
}

/// 查超长工具 id 映射（未过期才有效）。
pub fn tool_id_get(c: &Connection, outbound_id: &str) -> Result<Option<String>, StoreError> {
    Ok(c.query_row(
        "SELECT canonical_id FROM tool_id_map
         WHERE outbound_id=?1 AND expires_at>?2",
        params![outbound_id, now_ms()],
        |r| r.get(0),
    )
    .optional()?)
}

// ================================================================ utils

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
