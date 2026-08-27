//! 请求日志异步管道 —— storage §5：永不反压 HTTP 层。
//!
//! 结构：handler 侧 `LogHandle::emit`（非阻塞 try_send）→ 后台任务攒批
//! （≥64 行或 ≥500ms）→ 独立连接批量 INSERT。洪峰时丢弃并计数告警。

use rusqlite::{params_from_iter, Connection};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use super::StoreError;

const CHANNEL_CAP: usize = 1024;
const BATCH_MAX_ROWS: usize = 64;
const BATCH_FLUSH_INTERVAL_MS: u64 = 500;

/// 一条请求的元数据日志（不含 prompt/响应明文 —— 需求硬约束）。
#[derive(Debug, Clone)]
pub struct LogEvent {
    pub ts: i64,
    /// 'openai' | 'anthropic' | 'responses'
    pub inbound_family: String,
    /// 'passthrough' | 'converted'
    pub route_mode: &'static str,
    pub model_name: String,
    pub provider_id: Option<String>,
    pub upstream_model_id: Option<String>,
    pub http_status: i64,
    pub stop_reason: Option<String>,
    pub usage_input: Option<i64>,
    pub usage_output: Option<i64>,
    pub usage_cache_read: Option<i64>,
    pub usage_cache_write: Option<i64>,
    pub duration_ms: i64,
    pub is_stream: bool,
    pub tool_calls: i64,
    pub error_kind: Option<String>,
    /// 截断至 300 字符；只允许错误摘要，禁止携带请求内容
    pub error_summary: Option<String>,
}

/// 17 列的动态参数行，与 INSERT 列序一一对应。
fn event_params(
    e: &LogEvent,
) -> Vec<Box<dyn rusqlite::types::ToSql>> {
    vec![
        Box::new(e.ts),
        Box::new(e.inbound_family.clone()),
        Box::new(e.route_mode.to_string()),
        Box::new(e.model_name.clone()),
        Box::new(e.provider_id.clone()),
        Box::new(e.upstream_model_id.clone()),
        Box::new(e.http_status),
        Box::new(e.stop_reason.clone()),
        Box::new(e.usage_input),
        Box::new(e.usage_output),
        Box::new(e.usage_cache_read),
        Box::new(e.usage_cache_write),
        Box::new(e.duration_ms),
        Box::new(e.is_stream as i64),
        Box::new(e.tool_calls),
        Box::new(e.error_kind.clone()),
        Box::new(e
            .error_summary
            .as_deref()
            .map(|s| s.chars().take(300).collect::<String>())),
    ]
}

const INSERT_SQL: &str = "INSERT INTO request_logs(ts,inbound_family,route_mode,model_name,\
     provider_id,upstream_model_id,http_status,stop_reason,\
     usage_input,usage_output,usage_cache_read,usage_cache_write,\
     duration_ms,is_stream,tool_calls,error_kind,error_summary)\
     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)";

#[derive(Clone)]
pub struct LogHandle {
    tx: mpsc::Sender<LogEvent>,
    dropped: Arc<AtomicU64>,
    enabled: Arc<AtomicBool>,
}

impl LogHandle {
    /// 非阻塞投递；通道满即丢弃并计数（洪峰语义）。
    /// 日志开关关闭时直接丢弃（设置页「记录日志」开关，roadmap M2）。
    pub fn emit(&self, ev: LogEvent) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        if self.tx.try_send(ev).is_err() {
            let n = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 100 == 1 {
                eprintln!("[logs] 洪峰丢弃计数: {n}");
            }
        }
    }

    pub fn dropped_total(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// 设置日志开关（不影响已入队事件）。
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
}

fn open_logger_conn(path: &str) -> Result<Connection, StoreError> {
    // 迁移已由主连接完成；第二条连接 + busy_timeout（WAL 允许并发读写）
    let conn = Connection::open(path)?;
    if path != ":memory:" {
        conn.busy_timeout(std::time::Duration::from_millis(2500))?;
    }
    Ok(conn)
}

/// 启动后台写日志任务。同步函数（启动路径调用），内部不做 async 文件 IO。
pub fn spawn_logger(db_path: &str) -> Result<(LogHandle, tokio::task::JoinHandle<()>), StoreError> {
    let (tx, mut rx) = mpsc::channel::<LogEvent>(CHANNEL_CAP);
    let dropped = Arc::new(AtomicU64::new(0));

    let conn = Arc::new(Mutex::new(open_logger_conn(db_path)?));
    let drops = dropped.clone();

    let task = tokio::spawn(async move {
        let mut batch: Vec<LogEvent> = Vec::with_capacity(BATCH_MAX_ROWS);
        loop {
            // 有积压则立即冲刷；否则等事件或定时间隔
            if batch.len() < BATCH_MAX_ROWS {
                tokio::select! {
                    ev = rx.recv() => match ev {
                        Some(ev) => { batch.push(ev); continue; }
                        None => break,
                    },
                    _ = tokio::time::sleep(std::time::Duration::from_millis(BATCH_FLUSH_INTERVAL_MS)) => {}
                }
            }

            if batch.is_empty() {
                continue;
            }

            // LogEvent 是 Send 的；Box<dyn ToSql> 不是 —— 参数行在阻塞线程内构造
            let rows_src = std::mem::take(&mut batch);
            let row_count = rows_src.len();
            let c = conn.clone();
            let res = tokio::task::spawn_blocking(move || {
                let guard = c.lock().unwrap_or_else(|p| p.into_inner());
                let mut stmt = guard.prepare_cached(INSERT_SQL)?;
                for ev in &rows_src {
                    stmt.execute(params_from_iter(event_params(ev)))?;
                }
                Ok::<_, rusqlite::Error>(())
            })
            .await;

            match res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    eprintln!("[logs] 写入失败(丢 {row_count} 条): {e}")
                }
                Err(e) => eprintln!("[logs] 写入任务 join 失败: {e}"),
            }
            let _ = &drops;
        }
        if !batch.is_empty() {
            eprintln!("[logs] 退出时未冲刷批次 {} 条", batch.len());
        }
    });

    Ok((
        LogHandle {
            tx,
            dropped,
            enabled: Arc::new(AtomicBool::new(true)),
        },
        task,
    ))
}

// ================================================================ 查询

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRowView {
    pub id: i64,
    pub ts: i64,
    pub inbound_family: String,
    pub route_mode: String,
    pub model_name: String,
    pub provider_id: Option<String>,
    pub http_status: i64,
    pub duration_ms: i64,
    pub is_stream: bool,
    pub usage_input: Option<i64>,
    pub usage_output: Option<i64>,
    pub error_kind: Option<String>,
    pub error_summary: Option<String>,
}

/// 最近 N 条日志（UI 只读页数据源）。
pub fn logs_recent(db: &super::Db, limit: i64) -> Result<Vec<LogRowView>, StoreError> {
    db.with(|c| {
        let mut stmt = c.prepare(
            "SELECT id,ts,inbound_family,route_mode,model_name,provider_id,http_status,\
             duration_ms,is_stream,usage_input,usage_output,error_kind,error_summary \
             FROM request_logs ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit.clamp(1, 1000)], |r| {
                Ok(LogRowView {
                    id: r.get(0)?,
                    ts: r.get(1)?,
                    inbound_family: r.get(2)?,
                    route_mode: r.get(3)?,
                    model_name: r.get(4)?,
                    provider_id: r.get(5)?,
                    http_status: r.get(6)?,
                    duration_ms: r.get(7)?,
                    is_stream: r.get::<_, i64>(8)? != 0,
                    usage_input: r.get(9)?,
                    usage_output: r.get(10)?,
                    error_kind: r.get(11)?,
                    error_summary: r.get(12)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Db;

    fn ev(i: u32) -> LogEvent {
        LogEvent {
            ts: 1000 + i as i64,
            inbound_family: "openai".into(),
            route_mode: "passthrough",
            model_name: "gpt-4o".into(),
            provider_id: Some("p1".into()),
            upstream_model_id: None,
            http_status: 200,
            stop_reason: Some("stop".into()),
            usage_input: Some(11 * i as i64),
            usage_output: Some(7 * i as i64),
            usage_cache_read: None,
            usage_cache_write: None,
            duration_ms: 50 + i as i64,
            is_stream: true,
            tool_calls: 0,
            error_kind: None,
            error_summary: None,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pipeline_persists_batch_and_order() {
        let dir =
            std::env::temp_dir().join(format!("jai-logs-test-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");

        let db = Db::open(path.to_str().unwrap()).unwrap();
        let (handle, task) = spawn_logger(path.to_str().unwrap()).unwrap();

        for i in 0..10u32 {
            handle.emit(ev(i));
        }

        tokio::time::sleep(std::time::Duration::from_millis(900)).await;
        let dropped = handle.dropped_total();
        drop(handle);
        let _ = task.await;

        let rows = logs_recent(&db, 100).unwrap();
        assert_eq!(rows.len(), 10, "全部事件都应落库");
        assert!(rows[0].id > rows[rows.len() - 1].id, "按 id 倒序");
        assert_eq!(rows[0].model_name, "gpt-4o");
        assert_eq!(dropped, 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn rand_suffix() -> String {
        use rand::Rng;
        let n: u32 = rand::thread_rng().gen_range(0..u32::MAX);
        format!("{n:x}")
    }
}
