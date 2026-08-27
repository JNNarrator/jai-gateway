//! SQLite 存储层：连接初始化、迁移执行器。
//!
//! 设计权威：docs/design/storage-schema.md（v1 已定稿）。
//! 迁移机制：`PRAGMA user_version` 逐版本事务推进，零外部依赖（§7）。

pub mod migrations;

use rusqlite::Connection;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// 打开数据库并应用全部待执行迁移。幂等：重复调用无副作用（M0 验收标准 2）。
///
/// `journal_mode=WAL` / `foreign_keys=ON` / `synchronous=NORMAL`
/// 为 storage §3 定案的连接初始化 PRAGMA 三件套。WAL 对内存库无效，
/// 故仅对文件库设置；`:memory:` 跳过（测试路径）。
pub fn open_and_migrate(path: &str) -> Result<Connection, StoreError> {
    let conn = Connection::open(path)?;
    if path != ":memory:" {
        conn.pragma_update(None, "journal_mode", "WAL")?;
    }
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(conn)
}

/// 在既有连接上执行迁移（供测试注入内存库）。
pub fn migrate(conn: &Connection) -> Result<(), StoreError> {
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
        tracing_hint_applied(name);
    }
    Ok(())
}

fn tracing_hint_applied(name: &str) {
    // M0 阶段先复用 eprintln；M1 引入 tracing 后替换为结构化日志。
    eprintln!("[store] migration applied: {name}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::OptionalExtension;

    fn table_names(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    /// M0 验收标准 2：user_version 重复执行不重复建表、不重复写入。
    #[test]
    fn migrations_are_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let tables_first = table_names(&conn);

        // 再次执行（模拟应用重启）必须零变化且无错误
        migrate(&conn).unwrap();
        let tables_second = table_names(&conn);
        assert_eq!(tables_first, tables_second);

        let v: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v as usize, migrations::MIGRATIONS.len());
    }

    #[test]
    fn schema_contains_all_six_tables() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let tables = table_names(&conn);
        for expected in [
            "meta",
            "providers",
            "models",
            "gateway_keys",
            "request_logs",
            "tool_id_map",
        ] {
            assert!(tables.contains(&expected.to_string()), "缺表: {expected}");
        }
    }

    #[test]
    fn providers_check_constraint_enforced() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let now = 1_700_000_000_000i64;
        let r = conn.execute(
            "INSERT INTO providers(id,name,base_url,family,keyring_ref,created_at,updated_at)
             VALUES('p1','x','https://a','bogus_family','ref',?1,?1)",
            [now],
        );
        assert!(r.is_err(), "非法 family 必须被 CHECK 拦截");
    }

    #[test]
    fn models_cascade_on_provider_delete() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let now = 1_700_000_000_000i64;
        conn.execute(
            "INSERT INTO providers(id,name,base_url,family,keyring_ref,created_at,updated_at)
             VALUES('p1','官方','https://api.openai.com/v1','openai_compat','jai/provider/p1',?1,?1)",
            [now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO models(id,provider_id,model_name) VALUES('m1','p1','gpt-4o')",
            [],
        )
        .unwrap();

        conn.execute("DELETE FROM providers WHERE id='p1'", [])
            .unwrap();
        let gone: Option<String> = conn
            .query_row("SELECT id FROM models WHERE id='m1'", [], |r| r.get(0))
            .optional()
            .unwrap();
        assert!(gone.is_none(), "CASCADE 未生效");
    }

    #[test]
    fn request_logs_allow_dangling_provider() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let r = conn.execute(
            "INSERT INTO request_logs(ts,inbound_family,route_mode,model_name,provider_id,
                                       http_status,duration_ms)
             VALUES(1,'openai','passthrough','gpt-4o','deleted-provider',200,5)",
            [],
        );
        assert!(r.is_ok(), "request_logs 必须允许悬空 provider_id");
    }
}
