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
    pub weight: i64,
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

const PROVIDER_COLS: &str = "id,name,base_url,family,enabled,priority,weight,extra_headers,keyring_ref,last_ok_at,last_err_at,last_err_msg,created_at,updated_at";

fn row_to_provider(r: &rusqlite::Row) -> rusqlite::Result<ProviderRow> {
    Ok(ProviderRow {
        id: r.get(0)?,
        name: r.get(1)?,
        base_url: r.get(2)?,
        family: r.get(3)?,
        enabled: r.get::<_, i64>(4)? != 0,
        priority: r.get(5)?,
        weight: r.get(6)?,
        extra_headers: r.get(7)?,
        keyring_ref: r.get(8)?,
        last_ok_at: r.get(9)?,
        last_err_at: r.get(10)?,
        last_err_msg: r.get(11)?,
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
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
            "INSERT INTO providers({PROVIDER_COLS}) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)"
        ),
        params![
            p.id,
            p.name,
            p.base_url,
            p.family,
            p.enabled as i64,
            p.priority,
            p.weight,
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
    weight: Option<i64>,
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
    if let Some(w) = weight {
        c.execute(
            "UPDATE providers SET weight=?1, updated_at=?2 WHERE id=?3",
            params![w, now, id],
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

/// 模型别名/映射：设置该模型发给上游时使用的真实模型 id。
/// `None` 表示清空映射（上游使用同名模型）。
pub fn model_set_upstream(
    c: &Connection,
    model_id: &str,
    upstream_model_id: Option<&str>,
) -> Result<(), StoreError> {
    c.execute(
        "UPDATE models SET upstream_model_id=?1 WHERE id=?2",
        params![upstream_model_id, model_id],
    )?;
    Ok(())
}

/// 路由候选查询 —— storage §3 定案 SQL。按 (priority, rowid) 序逐渠道尝试。
#[derive(Debug, Clone)]
pub struct RouteCandidate {
    pub provider_id: String,
    pub provider_name: String,
    pub priority: i64,
    pub base_url: String,
    pub family: String,
    pub extra_headers: Option<String>,
    pub keyring_ref: String,
    pub upstream_model_id: Option<String>,
    pub max_output_tokens: i64,
    /// 高级路由：供应商权重（同 priority 内加权随机打散）
    pub weight: i64,
    /// 健康信息：用于健康感知排序（最近失败/最近成功）
    pub last_ok_at: Option<i64>,
    pub last_err_at: Option<i64>,
}

pub fn route_candidates(
    c: &Connection,
    model_name: &str,
) -> Result<Vec<RouteCandidate>, StoreError> {
    let sql = "SELECT p.id, p.name, p.priority, p.base_url, p.family, p.extra_headers,
                p.keyring_ref, m.upstream_model_id, m.max_output_tokens, p.weight,
                p.last_ok_at, p.last_err_at
         FROM models m JOIN providers p ON p.id = m.provider_id
         WHERE m.model_name = ?1 AND m.enabled = 1 AND p.enabled = 1
         ORDER BY p.priority ASC, m.rowid ASC";
    let mut stmt = c.prepare(sql)?;
    let rows = stmt
        .query_map([model_name], |r| {
            Ok(RouteCandidate {
                provider_id: r.get(0)?,
                provider_name: r.get(1)?,
                priority: r.get(2)?,
                base_url: r.get(3)?,
                family: r.get(4)?,
                extra_headers: r.get(5)?,
                keyring_ref: r.get(6)?,
                upstream_model_id: r.get(7)?,
                max_output_tokens: r.get(8)?,
                weight: r.get(9)?,
                last_ok_at: r.get(10)?,
                last_err_at: r.get(11)?,
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
    /// JSON 对象字符串（环境变量注入），如 {"KEY":"value"}
    pub env: Option<String>,
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
        env: r.get(6)?,
        enabled: r.get::<_, i64>(7)? != 0,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
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
        "SELECT id,name,kind,command,args,url,env,enabled,created_at,updated_at
         FROM mcp_servers ORDER BY name",
    )?;
    let rows = stmt
        .query_map([], row_to_mcp_server)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn mcp_insert(c: &Connection, row: &McpServerRow) -> Result<(), StoreError> {
    c.execute(
        "INSERT INTO mcp_servers(id,name,kind,command,args,url,env,enabled,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            row.id,
            row.name,
            row.kind,
            row.command,
            row.args,
            row.url,
            row.env,
            row.enabled as i64,
            row.created_at,
            row.updated_at,
        ],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)] // MCP 更新字段较多，保持与表列一一对应
pub fn mcp_update(
    c: &Connection,
    id: &str,
    name: &str,
    kind: &str,
    command: Option<&str>,
    args: Option<&str>,
    url: Option<&str>,
    env: Option<&str>,
) -> Result<(), StoreError> {
    c.execute(
        "UPDATE mcp_servers SET name=?1, kind=?2, command=?3, args=?4, url=?5, env=?6, updated_at=?7
         WHERE id=?8",
        params![name, kind, command, args, url, env, now_ms(), id],
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

// ================================================================ MCP 导入解析

/// 从导入配置解析出的单个 MCP Server 条目。
/// `skip_reason` 为 Some 时表示该条目不合法，应跳过（不会写入）。
#[derive(Debug, Clone)]
pub struct ParsedMcpServer {
    pub name: String,
    pub kind: String,
    pub command: Option<String>,
    pub args: Option<String>,
    pub url: Option<String>,
    pub env: Option<String>,
    pub skip_reason: Option<String>,
}

/// 导入 MCP 配置，自动识别三种格式：
/// 1. Codex CLI 命令行：`codex mcp add <名称> --env K=V -- <命令> [参数...]`
/// 2. `{"mcpServers": {...}}` JSON（Claude Code / Claude Desktop；也兼容无包装的裸对象）
/// 3. Codex `config.toml` 片段：`[mcp_servers.<名称>]`
pub fn parse_mcp_import(text: &str) -> Result<Vec<ParsedMcpServer>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("导入内容为空".into());
    }
    // 容忍粘贴时带上的 shell 提示符（$ / >）
    let body = trimmed.trim_start_matches(['$', '>']).trim_start();
    if looks_like_mcp_add_cli(body) {
        return parse_mcp_add_cli(body);
    }
    if trimmed.starts_with('{') {
        return parse_mcp_servers_json(trimmed);
    }
    if trimmed.starts_with('[') || trimmed.contains("[mcp_servers.") {
        return parse_mcp_servers_toml(trimmed);
    }
    Err(
        "无法识别格式：支持 {\"mcpServers\":{...}} JSON、codex mcp add 命令行、\
         [mcp_servers.*] TOML 三种格式"
            .into(),
    )
}

/// 识别是否为 `codex mcp add ...` / `claude mcp add ...` 风格命令行。
fn looks_like_mcp_add_cli(text: &str) -> bool {
    let tokens = tokenize_cli(text);
    if tokens.windows(2).any(|w| w[0] == "mcp" && w[1] == "add") {
        return matches!(
            tokens.first().map(String::as_str),
            Some("codex") | Some("claude") | Some("mcp")
        );
    }
    false
}

/// 解析 `codex mcp add` 命令行为单个条目（`claude mcp add` 同构，一并兼容）。
/// 语法：`[客户端] mcp add <名称> [--env K=V]... [--url URL] -- <命令> [参数...]`；
/// 无 `--` 时，`名称` 后的第一个位置参数为命令（或 URL），其余为参数。
pub fn parse_mcp_add_cli(text: &str) -> Result<Vec<ParsedMcpServer>, String> {
    let mut tokens = tokenize_cli(text);
    // 去掉客户端前缀
    if matches!(
        tokens.first().map(String::as_str),
        Some("codex") | Some("claude")
    ) {
        tokens.remove(0);
    }
    if tokens.first().map(String::as_str) == Some("mcp") {
        tokens.remove(0);
    }
    if tokens.first().map(String::as_str) != Some("add") {
        return Err("命令行缺少 mcp add 子命令".into());
    }
    tokens.remove(0);

    let mut env_pairs: Vec<(String, String)> = Vec::new();
    let mut url: Option<String> = None;
    let mut positionals: Vec<String> = Vec::new();
    let mut cmd_tokens: Vec<String> = Vec::new();
    let mut after_ddash = false;
    let mut malformed_env = false;
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i].clone();
        // 命令已开始（名称+命令两个位置参数已齐）或已过 `--`：后续 token 全部
        // 归入命令与参数，不再当旗标解析（否则 `-y` 之类参数会被误吞）
        let command_started = !after_ddash && positionals.len() >= 2;
        if after_ddash {
            cmd_tokens.push(t);
            i += 1;
            continue;
        }
        if command_started {
            if t == "--" {
                // clap 语义：`--` 是旗标终止符，本身不进参数
                i += 1;
                continue;
            }
            positionals.push(t);
            i += 1;
            continue;
        }
        if t == "--" {
            after_ddash = true;
            i += 1;
            continue;
        }
        // 带值旗标（值可能是独立 token，也可能用 = 内联）
        const VALUE_FLAGS: [&str; 8] = [
            "--env", "-e", "--url", "--transport", "-s", "--scope", "--profile", "--header",
        ];
        if VALUE_FLAGS.contains(&t.as_str()) {
            let val = tokens.get(i + 1).cloned();
            i += 2; // 消费旗标 + 值（值缺失时 val 为 None，走下方兜底）
            match (t.as_str(), val) {
                ("--env", Some(v)) | ("-e", Some(v)) => {
                    if let Some((k, value)) = v.split_once('=') {
                        env_pairs.push((k.to_string(), value.to_string()));
                    } else {
                        malformed_env = true;
                    }
                }
                ("--url", Some(v)) => url = Some(v),
                _ => {} // 其余旗标与网关无关，值一并丢弃
            }
            continue;
        }
        if let Some(v) = t.strip_prefix("--env=") {
            i += 1;
            if let Some((k, value)) = v.split_once('=') {
                env_pairs.push((k.to_string(), value.to_string()));
            } else {
                malformed_env = true;
            }
            continue;
        }
        if let Some(v) = t.strip_prefix("--url=") {
            url = Some(v.to_string());
            i += 1;
            continue;
        }
        if t.starts_with('-') && t.len() > 1 {
            // 未知开关（--verbose 等）：跳过旗标本身
            i += 1;
            continue;
        }
        positionals.push(t);
        i += 1;
    }

    let env = if env_pairs.is_empty() {
        None
    } else {
        let mut map = serde_json::Map::new();
        for (k, v) in env_pairs {
            map.insert(k, serde_json::Value::String(v));
        }
        Some(serde_json::Value::Object(map).to_string())
    };
    let skip_reason = |reason: String| ParsedMcpServer {
        name: positionals.first().cloned().unwrap_or_else(|| "(未命名)".into()),
        kind: "stdio".into(),
        command: None,
        args: None,
        url: None,
        env: env.clone(),
        skip_reason: Some(reason),
    };
    let name = match positionals.first() {
        Some(n) if !n.is_empty() => n.clone(),
        _ => return Err("命令行中未找到服务名称".into()),
    };
    if malformed_env {
        return Ok(vec![skip_reason("--env 需为 KEY=VALUE 形式".into())]);
    }

    // `--` 之后是完整命令；否则位置参数依次为 名称、命令（或 URL）、参数...
    let (kind, command, args, url) = if !cmd_tokens.is_empty() {
        let command = cmd_tokens[0].clone();
        let args = if cmd_tokens.len() > 1 {
            Some(serde_json::Value::Array(
                cmd_tokens[1..].iter().map(|s| serde_json::Value::String(s.clone())).collect(),
            ).to_string())
        } else {
            None
        };
        ("stdio".to_string(), Some(command), args, None)
    } else if let Some(u) = url {
        ("http".to_string(), None, None, Some(u))
    } else {
        match positionals.get(1) {
            Some(c) if c.starts_with("http://") || c.starts_with("https://") => {
                ("http".to_string(), None, None, Some(c.clone()))
            }
            Some(c) => {
                let args = if positionals.len() > 2 {
                    Some(serde_json::Value::Array(
                        positionals[2..]
                            .iter()
                            .map(|s| serde_json::Value::String(s.clone()))
                            .collect(),
                    ).to_string())
                } else {
                    None
                };
                ("stdio".to_string(), Some(c.clone()), args, None)
            }
            None => {
                return Ok(vec![skip_reason("stdio 类型缺少 command".into())]);
            }
        }
    };
    let skip_reason = if kind == "stdio" && command.is_none() {
        Some("stdio 类型缺少 command".to_string())
    } else {
        None
    };
    Ok(vec![ParsedMcpServer {
        name,
        kind,
        command,
        args,
        url,
        env,
        skip_reason,
    }])
}

/// 按 shell 引号规则把命令行拆成 token：单/双引号内保留字面量（含空格与
/// Windows 路径反斜杠），引号外按空白切分。
fn tokenize_cli(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_token = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' => {
                in_token = true;
                while let Some(ch) = chars.next() {
                    if ch == c {
                        break;
                    }
                    cur.push(ch);
                }
            }
            c if c.is_whitespace() => {
                if in_token {
                    tokens.push(std::mem::take(&mut cur));
                    in_token = false;
                }
            }
            c => {
                cur.push(c);
                in_token = true;
            }
        }
    }
    if in_token {
        tokens.push(cur);
    }
    tokens
}

/// 解析 Claude Code 风格 `mcpServers` JSON。
/// 顶层为 `{"mcpServers": {name: {...}}}`；也兼容无包装的裸对象
/// `{"name": {command/url/...}}`。每个条目支持 `type`（缺省 stdio）、
/// `command`/`args`/`env`（stdio）与 `url`（sse/http）。
/// 返回逐条解析结果，调用方决定 upsert 语义。
pub fn parse_mcp_servers_json(json_text: &str) -> Result<Vec<ParsedMcpServer>, String> {
    let parsed: serde_json::Value =
        serde_json::from_str(json_text).map_err(|e| format!("JSON 解析失败: {e}"))?;
    let servers = parsed
        .get("mcpServers")
        .and_then(serde_json::Value::as_object)
        .or_else(|| {
            // 裸对象：每个值都是含 command 或 url 的对象时，视作 server 映射本身
            let obj = parsed.as_object()?;
            (!obj.is_empty()
                && obj
                    .values()
                    .all(|v| v.is_object() && (v.get("command").is_some() || v.get("url").is_some())))
            .then_some(obj)
        })
        .ok_or_else(|| "缺少 mcpServers 对象".to_string())?;
    if servers.is_empty() {
        return Err("mcpServers 为空".into());
    }
    Ok(servers
        .iter()
        .map(|(name, conf)| json_entry_to_parsed(name, conf))
        .collect())
}

/// 单个 JSON 条目 → ParsedMcpServer（JSON 与 TOML 两条路径共用）。
fn json_entry_to_parsed(name: &str, conf: &serde_json::Value) -> ParsedMcpServer {
    let mut kind = conf
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("stdio")
        .to_string();
    // 兼容旧格式：无 type 但有 url 视为 http
    if kind == "stdio"
        && conf
            .get("url")
            .and_then(serde_json::Value::as_str)
            .is_some()
    {
        kind = "http".to_string();
    }
    if !matches!(kind.as_str(), "stdio" | "sse" | "http") {
        return ParsedMcpServer {
            name: name.to_string(),
            kind: kind.clone(),
            command: None,
            args: None,
            url: None,
            env: None,
            skip_reason: Some(format!("不支持的 type={kind}")),
        };
    }
    let command = conf
        .get("command")
        .and_then(serde_json::Value::as_str)
        .map(|s| s.to_string());
    let args = conf.get("args").map(|v| v.to_string());
    let url = conf
        .get("url")
        .and_then(serde_json::Value::as_str)
        .map(|s| s.to_string());
    let env = conf.get("env").map(|v| v.to_string());
    let skip_reason = if kind == "stdio" && command.is_none() {
        Some("stdio 类型缺少 command".to_string())
    } else if kind != "stdio" && url.is_none() {
        Some(format!("{kind} 类型缺少 url"))
    } else {
        None
    };
    ParsedMcpServer {
        name: name.to_string(),
        kind,
        command,
        args,
        url,
        env,
        skip_reason,
    }
}

/// 解析 Codex `config.toml` 片段：`[mcp_servers.<name>]` 表，
/// 支持 `command`/`args`/`env`（stdio）与 `url`（可带 `transport` 指明 sse/http）。
pub fn parse_mcp_servers_toml(text: &str) -> Result<Vec<ParsedMcpServer>, String> {
    let value: toml::Value = toml::from_str(text).map_err(|e| format!("TOML 解析失败: {e}"))?;
    let servers = value
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "缺少 [mcp_servers.*] 表".to_string())?;
    if servers.is_empty() {
        return Err("mcp_servers 为空".into());
    }
    let mut out = Vec::with_capacity(servers.len());
    for (name, conf) in servers {
        // TOML → JSON 后复用同一条目逻辑；同时把 url 条目的 transport 折算成 type
        let conf_json: serde_json::Value = match serde_json::to_value(conf) {
            Ok(v) => v,
            Err(e) => {
                out.push(ParsedMcpServer {
                    name: name.clone(),
                    kind: "stdio".into(),
                    command: None,
                    args: None,
                    url: None,
                    env: None,
                    skip_reason: Some(format!("条目无法解析: {e}")),
                });
                continue;
            }
        };
        let mut conf_json = conf_json;
        if conf_json.get("type").is_none() {
            if let Some(transport) = conf.get("transport").and_then(toml::Value::as_str) {
                let kind = match transport {
                    "sse" => "sse",
                    "streamable_http" | "streamable-http" | "http" => "http",
                    _ => "",
                };
                if !kind.is_empty() {
                    conf_json["type"] = serde_json::Value::String(kind.to_string());
                }
            }
        }
        out.push(json_entry_to_parsed(name, &conf_json));
    }
    Ok(out)
}

// ================================================================ utils

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
