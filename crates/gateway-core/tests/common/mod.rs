//! 集成测试共用小工具（`tests/common/mod.rs` 不是独立测试目标，按需 `mod common;` 引入）。
#![allow(dead_code)]

use std::time::Duration;

use gateway_core::store::logs::{logs_recent, LogRowView};
use gateway_core::store::Db;

/// 等后台日志管道落库并**稳定下来**，返回最近 `limit` 条。
///
/// 起因（2026-09-22，v0.3.1 的 CI 实测）：此前各集成测试都写死
/// `sleep(700ms)` 后立刻 `logs_recent(...).find(...).unwrap()`。日志落库是**异步**的
/// （专用后台线程 + 批量写入），Windows runner 上偶发 700ms 还没落库 ⇒ `.find(...)`
/// 在 `None` 上 panic（红在 `m3_anthropic.rs:244`）。固定等待既慢（每次都白等 700ms）
/// 又不稳（负载高就超）。
///
/// 现在改成有界轮询：**行数连续两次相同**即认为本批已写完（正常情况约 100ms 返回）。
/// 超时**不 panic**：宁可把当前快照交给调用方（让真正的断言去判断对错），
/// 也不制造一个与产品行为无关的假失败；真的一条都没有时才报错。
pub async fn logs_settled(db: &Db, limit: i64, timeout: Duration) -> Vec<LogRowView> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut prev: Option<usize> = None;
    loop {
        let rows = logs_recent(db, limit).expect("logs_recent 查询失败");
        if !rows.is_empty() && prev == Some(rows.len()) {
            return rows;
        }
        if tokio::time::Instant::now() >= deadline {
            assert!(
                !rows.is_empty(),
                "等待日志落库超时（{timeout:?}）：{limit} 条里一条都没有（后台日志管道可能没启动）"
            );
            return rows;
        }
        prev = Some(rows.len());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
