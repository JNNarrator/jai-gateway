//! 保活清理任务（storage §5：「保留策略」+ `tool_id_map` TTL）。
//!
//! roadmap M2 验收 5：保留裁剪任务可单测（mock clock —— 本模块所有函数
//! 都接受显式 `now_ms`，调用方（常驻 timer）传入真实时钟）。

use rusqlite::Connection;

use super::{Db, StoreError};

/// 默认保留窗口：30 天（毫秒）。
pub const DEFAULT_RETENTION_DAYS: i64 = 30;
/// 日志行数硬上限：5 万行。
pub const DEFAULT_LOG_ROW_CAP: i64 = 50_000;

/// 一次清理的结果统计（UI/日志展示用）。
#[derive(Debug, Default, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionStats {
    /// 按时间窗口删除的日志行数
    pub logs_expired: u64,
    /// 超 5 万行上限裁掉的最旧日志行数
    pub logs_capped: u64,
    /// tool_id_map 过期条目数
    pub tool_ids_expired: u64,
}

/// 执行一轮保留清理。`now_ms` 显式注入以便测试。
///
/// 1. `request_logs`：删除 `ts < now - retention_days` 的行；
/// 2. 剩下仍超 `row_cap` 时，按 id（插入序）裁掉最旧的超出部分；
/// 3. `tool_id_map`：删除 `expires_at < now` 的行。
pub fn run_retention(
    c: &Connection,
    now_ms: i64,
    retention_days: i64,
    row_cap: i64,
) -> Result<RetentionStats, StoreError> {
    let retention_days = retention_days.max(1);
    // 防误配置为 0/负值：至少保留 1 条
    let row_cap = row_cap.max(1);
    let cutoff = now_ms.saturating_sub(retention_days * 86_400_000);

    let mut stats = RetentionStats {
        logs_expired: c.execute(
            "DELETE FROM request_logs WHERE ts < ?1",
            rusqlite::params![cutoff],
        )? as u64,
        ..RetentionStats::default()
    };

    // 2) 行数上限裁剪（保留最新的 row_cap 条）
    let total: i64 = c.query_row("SELECT COUNT(*) FROM request_logs", [], |r| r.get(0))?;
    let over = total.saturating_sub(row_cap);
    if over > 0 {
        stats.logs_capped = c.execute(
            "DELETE FROM request_logs WHERE id IN (
                 SELECT id FROM request_logs ORDER BY id ASC LIMIT ?1
             )",
            rusqlite::params![over],
        )? as u64;
    }

    // 3) tool_id_map TTL
    stats.tool_ids_expired = c.execute(
        "DELETE FROM tool_id_map WHERE expires_at < ?1",
        rusqlite::params![now_ms],
    )? as u64;

    Ok(stats)
}

/// 启动常驻清理循环（roadmap M2「保活 timer」，默认每日一次）。
///
/// 独立后台任务，失败不中断循环（打日志继续下一轮），绝不影响网关主路径。
/// 返回 JoinHandle 供调用方持有/观察。
pub fn spawn_retention_loop(
    db: Db,
    interval: std::time::Duration,
    retention_days: i64,
    row_cap: i64,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("retention runtime build failed");
        runtime.block_on(async move {
            loop {
                tokio::time::sleep(interval).await;
                let db2 = db.clone();
                let days = retention_days;
                let cap = row_cap;
                let res = tokio::task::spawn_blocking(move || {
                    let now = super::now_ms();
                    db2.with(|c| run_retention(c, now, days, cap))
                })
                .await;
                match res {
                    Ok(Ok(stats)) => {
                        if stats.logs_expired > 0 || stats.logs_capped > 0 || stats.tool_ids_expired > 0
                        {
                            eprintln!(
                                "[retention] 清理完成: logs_expired={} logs_capped={} tool_ids_expired={}",
                                stats.logs_expired, stats.logs_capped, stats.tool_ids_expired
                            );
                        }
                    }
                    Ok(Err(e)) => eprintln!("[retention] 清理失败(下轮重试): {e}"),
                    Err(e) => eprintln!("[retention] 任务 join 失败: {e}"),
                }
            }
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{now_ms, open_and_migrate};

    fn insert_log(c: &Connection, ts: i64) {
        c.execute(
            "INSERT INTO request_logs(ts,inbound_family,route_mode,model_name,http_status,duration_ms)
             VALUES (?1,'openai','passthrough','m',200,1)",
            rusqlite::params![ts],
        )
        .unwrap();
    }

    fn insert_tool_id(c: &Connection, expires_at: i64) {
        c.execute(
            "INSERT INTO tool_id_map(outbound_id,canonical_id,created_at,expires_at)
             VALUES (?1,'canon',1000,?2)",
            rusqlite::params![format!("out-{expires_at}"), expires_at],
        )
        .unwrap();
    }

    #[test]
    fn expires_old_logs_and_tool_ids() {
        let conn = open_and_migrate(":memory:").unwrap();
        let now = now_ms();
        insert_log(&conn, now - 40 * 86_400_000); // 40 天前
        insert_log(&conn, now - 10 * 86_400_000); // 10 天前
        insert_tool_id(&conn, now - 1000); // 已过期
        insert_tool_id(&conn, now + 86_400_000); // 未过期

        let stats = run_retention(&conn, now, DEFAULT_RETENTION_DAYS, DEFAULT_LOG_ROW_CAP).unwrap();
        assert_eq!(stats.logs_expired, 1);
        assert_eq!(stats.tool_ids_expired, 1);
        assert_eq!(stats.logs_capped, 0);

        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM request_logs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 1);
        let tool_left: i64 = conn
            .query_row("SELECT COUNT(*) FROM tool_id_map", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tool_left, 1);
    }

    #[test]
    fn caps_rows_beyond_limit_keeping_newest() {
        let conn = open_and_migrate(":memory:").unwrap();
        let now = now_ms();
        // 全部在窗口内，但超过 row_cap=5
        for i in 0..8 {
            insert_log(&conn, now - 1000 + i);
        }
        let stats = run_retention(&conn, now, DEFAULT_RETENTION_DAYS, 5).unwrap();
        assert_eq!(stats.logs_capped, 3);
        let rows: Vec<i64> = {
            let mut stmt = conn
                .prepare("SELECT id FROM request_logs ORDER BY id ASC")
                .unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(rows.len(), 5);
        // 保留最新 5 条（id 最大者仍在）
        assert_eq!(*rows.last().unwrap(), *rows.iter().max().unwrap());
    }

    #[test]
    fn zero_work_when_nothing_to_do() {
        let conn = open_and_migrate(":memory:").unwrap();
        let now = now_ms();
        let stats = run_retention(&conn, now, DEFAULT_RETENTION_DAYS, DEFAULT_LOG_ROW_CAP).unwrap();
        assert_eq!(stats.logs_expired, 0);
        assert_eq!(stats.logs_capped, 0);
        assert_eq!(stats.tool_ids_expired, 0);
    }
}
