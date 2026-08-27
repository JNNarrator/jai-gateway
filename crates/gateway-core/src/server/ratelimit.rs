//! 鉴权失败限速（roadmap M2）：同源 10 次/分钟失败 → 封禁该源 5 分钟。
//!
//! 防御目标：网关密钥被暴力猜测 / 被错误客户端高频撞击。因为网关只监听
//! 回环地址，这里的「源」就是 TCP 对端 IP；对回环场景基本收敛为一个源，
//! 主要防的是本地失陷进程的爆破尝试。
//!
//! 纯内存实现（重启即清零），上限封顶避免内存失控。

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Mutex;

/// 窗口内允许的最大失败次数
pub const MAX_FAILS_PER_WINDOW: usize = 10;
/// 失败计数窗口：1 分钟
pub const FAIL_WINDOW_MS: i64 = 60_000;
/// 封禁时长：5 分钟
pub const BAN_MS: i64 = 300_000;

#[derive(Default)]
pub struct AuthRateLimiter {
    /// ip → 该窗口内的失败时间戳（保持升序）
    fails: Mutex<HashMap<IpAddr, VecDeque<i64>>>,
    /// ip → 封禁到期时间戳
    banned: Mutex<HashMap<IpAddr, i64>>,
}

pub enum BanStatus {
    /// 放行
    Allowed,
    /// 当前被封禁，剩余毫秒
    Banned(i64),
}

impl AuthRateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 查询状态（幂等，会惰性清理过期封禁）。
    pub fn status(&self, ip: IpAddr, now: i64) -> BanStatus {
        let mut banned = self.banned.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(&until) = banned.get(&ip) {
            if until > now {
                return BanStatus::Banned(until - now);
            }
            banned.remove(&ip);
        }
        BanStatus::Allowed
    }

    /// 记录一次鉴权失败。返回是否因此触发封禁。
    pub fn record_failure(&self, ip: IpAddr, now: i64) -> bool {
        {
            let mut fails = self.fails.lock().unwrap_or_else(|p| p.into_inner());
            let q = fails.entry(ip).or_default();
            // 清掉窗口外的旧记录
            while let Some(&t) = q.front() {
                if now - t >= FAIL_WINDOW_MS {
                    q.pop_front();
                } else {
                    break;
                }
            }
            q.push_back(now);
            if q.len() < MAX_FAILS_PER_WINDOW {
                return false;
            }
        }
        // 达到阈值 → 封禁（覆盖已有封禁，从本轮起算）
        self.banned
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(ip, now + BAN_MS);
        // 顺带清理该源的计数（封禁期间不再累积）
        self.fails
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&ip);
        true
    }

    /// 封禁到期后由上层调用，重置该源状态。
    pub fn release(&self, ip: IpAddr) {
        self.banned
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&ip);
        self.fails
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&ip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
    }

    #[test]
    fn allows_under_threshold() {
        let l = AuthRateLimiter::new();
        for i in 0..MAX_FAILS_PER_WINDOW - 1 {
            assert!(!l.record_failure(ip(), 1000 + i as i64));
            assert!(matches!(l.status(ip(), 1000 + i as i64), BanStatus::Allowed));
        }
    }

    #[test]
    fn bans_at_threshold_and_expires() {
        let l = AuthRateLimiter::new();
        let mut banned = false;
        for i in 0..MAX_FAILS_PER_WINDOW {
            banned |= l.record_failure(ip(), 2000 + i as i64);
        }
        assert!(banned, "第 10 次失败应触发封禁");
        assert!(matches!(l.status(ip(), 2000 + MAX_FAILS_PER_WINDOW as i64),
                         BanStatus::Banned(ms) if ms <= BAN_MS && ms > BAN_MS - 10_000));

        // 封禁期内仍报 Banned
        assert!(matches!(l.status(ip(), 2000 + 60_000), BanStatus::Banned(_)));
        // 过期后放行（封禁自第 10 次失败时刻 2000+9 起算 BAN_MS）
        let last_fail = 2000 + (MAX_FAILS_PER_WINDOW as i64 - 1);
        assert!(matches!(
            l.status(ip(), last_fail + BAN_MS + 1),
            BanStatus::Allowed
        ));
    }

    #[test]
    fn window_slides_old_failures_away() {
        let l = AuthRateLimiter::new();
        // 每分钟 9 次，连续 3 分钟 → 永不触发（老记录被窗口滑走）
        for m in 0..3 {
            for i in 0..MAX_FAILS_PER_WINDOW - 1 {
                let t = 10_000 + m as i64 * FAIL_WINDOW_MS + i as i64;
                assert!(!l.record_failure(ip(), t), "m={m} i={i} 不应封禁");
            }
        }
    }

    #[test]
    fn different_sources_independent() {
        let l = AuthRateLimiter::new();
        let other = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9));
        for i in 0..MAX_FAILS_PER_WINDOW {
            l.record_failure(ip(), 100 + i as i64);
        }
        assert!(matches!(l.status(ip(), 200), BanStatus::Banned(_)));
        assert!(matches!(l.status(other, 200), BanStatus::Allowed));
    }
}