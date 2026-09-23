//! 出站 URL 守卫（D9-T1）：给「用户填什么就打什么」的入口加一道 SSRF 校验。
//!
//! 为什么需要它：`providers.base_url` 此前**零校验**（`provider_create` / `provider_update`
//! 直接落库）。渠道草稿测试是第一个由用户提供 URL、由网关**主动发起请求**的入口，
//! 如果不校验，这个便利就变成一个 SSRF 面：填 `http://169.254.169.254/latest/meta-data/`
//! 就能让网关去读云元数据。
//!
//! 范围（刻意收敛，见 D2）：
//! - **做**：草稿探测前校验；`provider_create` / `provider_update` 保存时校验（一次性，
//!   不做每请求的热路径校验 —— 否则「已存渠道突然被拦」会变成可用性事故）。
//! - **不做**：热路径（`try_candidate`）每请求校验。
//!
//! 判定口径：
//! - 只允许 `http` / `https`；
//! - 不允许 URL 里带用户名 / 密码（避免凭据被写进日志或与鉴权头语义打架）；
//! - 字面量 IP 按地址族判定（见 [`is_blocked_ip`]）；
//! - 主机名 `localhost` / `*.localhost` / `*.local` / `*.internal` 按「本机」处理；
//! - `allow_loopback = true` 时放行 loopback（Ollama / LM Studio / vLLM 等本机部署），
//!   但**任何情况下都不放行** link-local（含云元数据 `169.254.169.254`）、私网段、
//!   组播、保留段与未指定地址。
//!
//! ⚠️ **已知局限**：这里只校验 URL 的**字面**主机（IP 或特殊主机名），**不做 DNS 解析后的
//! 复查**。所以「域名解析到 127.0.0.1」这类间接指向（含 DNS rebinding）不在拦截范围内。
//! 之所以敢这样收敛：`base_url` 由本机用户自己填写，威胁模型是「用户被误导去填一个危险
//! 地址」而不是「远端攻击者控制目标主机名」；真要挡住后者需要「解析 → 逐 IP 校验 → 用
//! 已校验的 IP 建连」的完整链路，成本远高于本迭代的收益。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use url::Url;

/// 校验出站 URL 是否允许访问。通过时返回规范化后的 [`Url`]，否则返回中文原因。
pub fn validate_outbound_url(raw: &str, allow_loopback: bool) -> Result<Url, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Base URL 为空".into());
    }
    let url = Url::parse(trimmed).map_err(|e| format!("URL 解析失败: {e}"))?;

    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!("不支持的协议 {other}（只允许 http / https）"));
        }
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URL 不允许带用户名 / 密码".into());
    }
    // 用 `url.host()` 而不是 `host_str()`：前者已经把主机归一化成
    // `Ipv4` / `Ipv6` / `Domain` 三态，于是十进制（`http://2130706433/`）、
    // 十六进制（`http://0x7f.0.0.1/`）、八进制（`http://0177.0.0.1/`）、
    // 省略段（`http://127.1/`）这些**字面量混淆写法**都被 url crate 归一化掉了，
    // 无法绕过下面的地址族判定；而 `host_str()` 会把 IPv6 带上方括号（`"[::1]"`），
    // 拿去 parse 成 IpAddr 会直接失败（漏判）。
    let host = match url.host() {
        Some(h) => h.to_owned(),
        None => return Err("URL 缺少主机名".into()),
    };

    let blocked = match &host {
        url::Host::Ipv4(v4) => Some(IpAddr::V4(*v4)),
        url::Host::Ipv6(v6) => Some(IpAddr::V6(*v6)),
        url::Host::Domain(d) => {
            // 域名位置也可能写 IP（url 一般已归一化，这里兜底）
            if let Ok(ip) = d.parse::<IpAddr>() {
                Some(ip)
            } else if is_local_hostname(d) && !allow_loopback {
                return Err(format!(
                    "主机名 {d} 指向本机或内网，如需探测本机部署请勾选「允许访问本机地址」"
                ));
            } else {
                None
            }
        }
    };
    if let Some(ip) = blocked {
        if is_blocked_ip(&ip, allow_loopback) {
            return Err(blocked_reason(&ip));
        }
    }
    Ok(url)
}

/// 是否是被禁止的目标 IP。
///
/// `allow_loopback = true` 只放行 loopback（`127/8`、`::1`、`::ffff:127.x`），
/// link-local / 私网 / 组播 / 保留段一律仍然禁止。
pub fn is_blocked_ip(ip: &IpAddr, allow_loopback: bool) -> bool {
    match ip {
        IpAddr::V4(v4) => ipv4_blocked(v4, allow_loopback),
        IpAddr::V6(v6) => {
            // IPv4-mapped（`::ffff:127.0.0.1`）是最常见的绕过手法：先还原成 IPv4 判定。
            if let Some(v4) = v6.to_ipv4_mapped() {
                return ipv4_blocked(&v4, allow_loopback);
            }
            ipv6_blocked(v6, allow_loopback)
        }
    }
}

fn ipv4_blocked(ip: &Ipv4Addr, allow_loopback: bool) -> bool {
    if ip.is_loopback() {
        return !allow_loopback;
    }
    // 0.0.0.0 与 255.255.255.255
    if ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() {
        return true;
    }
    let o = ip.octets();
    match o[0] {
        // 0.0.0.0/8「本网络」：在 Linux 上 0.0.0.0 会被当成 127.0.0.1 路由
        0 => true,
        // 10.0.0.0/8 私网
        10 => true,
        // 100.64.0.0/10 CGNAT（运营商级 NAT，常被云厂商用作内网）
        100 => (64..=127).contains(&o[1]),
        // 127.0.0.0/8 已在上面的 is_loopback 处理
        127 => true,
        // 169.254.0.0/16 link-local —— **云元数据端点 169.254.169.254 在这里**
        169 => o[1] == 254,
        // 172.16.0.0/12 私网
        172 => (16..=31).contains(&o[1]),
        // 192.0.2.0/24 TEST-NET-1（文档用，不可路由）
        192 if o[1] == 0 && o[2] == 2 => true,
        // 192.168.0.0/16 私网
        192 => o[1] == 168,
        // 198.18.0.0/15 基准测试用
        198 => o[1] == 18 || o[1] == 19,
        // 224.0.0.0/4 组播 + 240.0.0.0/4 保留（含广播地址）
        224..=255 => true,
        _ => false,
    }
}

fn ipv6_blocked(ip: &Ipv6Addr, allow_loopback: bool) -> bool {
    if ip.is_loopback() {
        return !allow_loopback;
    }
    // :: 未指定地址
    if ip.is_unspecified() {
        return true;
    }
    // ff00::/8 组播
    if ip.is_multicast() {
        return true;
    }
    let seg = ip.segments();
    // fc00::/7 唯一本地地址（ULA）
    if (seg[0] & 0xfe00) == 0xfc00 {
        return true;
    }
    // fe80::/10 link-local
    if (seg[0] & 0xffc0) == 0xfe80 {
        return true;
    }
    false
}

/// 主机名是否「明确指向本机 / 内网」。
///
/// 只认这几个约定俗成的特例，**不做 DNS 解析**（见模块头的已知局限）。
fn is_local_hostname(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    h == "localhost"
        || h.ends_with(".localhost")
        || h.ends_with(".local")
        || h.ends_with(".internal")
}

fn blocked_reason(ip: &IpAddr) -> String {
    let what = match ip {
        IpAddr::V4(v4) => {
            if v4.is_loopback() {
                "本机地址"
            } else if v4.octets()[0] == 169 && v4.octets()[1] == 254 {
                "链路本地地址（含云元数据端点，任何情况下都不放行）"
            } else {
                "内网 / 保留地址"
            }
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                "本机地址"
            } else if (v6.segments()[0] & 0xffc0) == 0xfe80 {
                "链路本地地址（任何情况下都不放行）"
            } else {
                "内网 / 保留地址"
            }
        }
    };
    format!("目标 {ip} 属于{what}，已拦截")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked(s: &str, allow_loopback: bool) -> bool {
        is_blocked_ip(&s.parse().unwrap(), allow_loopback)
    }

    #[test]
    fn ipv4_private_and_reserved_are_blocked() {
        for ip in [
            "0.0.0.0",
            "0.1.2.3",
            "10.0.0.1",
            "10.255.255.255",
            "100.64.0.1",
            "100.127.255.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.2.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.19.255.255",
            "224.0.0.1",
            "239.255.255.255",
            "240.0.0.1",
            "255.255.255.255",
        ] {
            assert!(blocked(ip, true), "{ip} 必须被拦（即使 allow_loopback）");
        }
    }

    #[test]
    fn ipv4_link_local_never_allowed() {
        // 云元数据端点是重中之重：任何开关都不放行
        for ip in ["169.254.169.254", "169.254.0.1", "169.254.255.255"] {
            assert!(blocked(ip, false), "{ip} 必须被拦");
            assert!(blocked(ip, true), "{ip} 即使 allow_loopback 也必须被拦");
        }
        // 相邻但不同段的不该误伤
        assert!(!blocked("169.253.0.1", true));
        assert!(!blocked("169.255.0.1", true));
    }

    #[test]
    fn ipv4_loopback_only_when_allowed() {
        for ip in ["127.0.0.1", "127.1.2.3", "127.255.255.255"] {
            assert!(blocked(ip, false), "{ip} 未勾选本机地址时必须被拦");
            assert!(!blocked(ip, true), "{ip} 勾选后应放行");
        }
    }

    #[test]
    fn ipv4_public_is_allowed() {
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "172.32.0.1",
            "100.128.0.1",
            "192.0.3.1",
        ] {
            assert!(!blocked(ip, false), "{ip} 是公网地址，不该被拦");
        }
    }

    #[test]
    fn ipv6_special_ranges_are_blocked() {
        for ip in [
            "::",           // 未指定
            "fc00::1",      // ULA
            "fd12:3456::1", // ULA
            "fe80::1",      // link-local
            "febf::1",      // link-local 上界
            "ff02::1",      // 组播
        ] {
            assert!(blocked(ip, false), "{ip} 必须被拦");
            assert!(blocked(ip, true), "{ip} 即使 allow_loopback 也必须被拦");
        }
        // 公网 IPv6 放行
        assert!(!blocked("2606:4700:4700::1111", false));
        // fec0::/10 是已废弃的 site-local，不在本迭代拦截清单里（不误报为 link-local）
        assert!(!blocked("fec0::1", false));
    }

    #[test]
    fn ipv6_loopback_only_when_allowed() {
        assert!(blocked("::1", false));
        assert!(!blocked("::1", true));
    }

    #[test]
    fn ipv4_mapped_ipv6_is_judged_as_ipv4() {
        // 最常见的绕过手法：`::ffff:127.0.0.1` 与 `127.0.0.1` 必须同判
        assert!(blocked("::ffff:127.0.0.1", false));
        assert!(!blocked("::ffff:127.0.0.1", true));
        assert!(blocked("::ffff:169.254.169.254", false));
        assert!(blocked("::ffff:169.254.169.254", true), "元数据端点不放行");
        assert!(blocked("::ffff:10.0.0.1", true));
        assert!(!blocked("::ffff:1.1.1.1", false));
    }

    #[test]
    fn validate_accepts_normal_https() {
        let u = validate_outbound_url("https://api.openai.com/v1", false).unwrap();
        assert_eq!(u.host_str(), Some("api.openai.com"));
        // 前后空白容忍（表单里常带）
        let u = validate_outbound_url("  https://api.x.com  ", false).unwrap();
        assert_eq!(u.host_str(), Some("api.x.com"));
        // 公网 IP 也放行
        assert!(validate_outbound_url("http://1.1.1.1:8080/v1", false).is_ok());
    }

    #[test]
    fn validate_rejects_bad_scheme_and_missing_host() {
        for raw in ["ftp://api.x.com", "file:///etc/passwd", "ws://api.x.com"] {
            let err = validate_outbound_url(raw, true).unwrap_err();
            assert!(err.contains("不支持的协议"), "{raw} → {err}");
        }
        assert!(validate_outbound_url("", false)
            .unwrap_err()
            .contains("为空"));
        assert!(validate_outbound_url("api.openai.com", false).is_err());
    }

    #[test]
    fn validate_rejects_userinfo() {
        let err = validate_outbound_url("https://user:pass@api.x.com/v1", false).unwrap_err();
        assert!(err.contains("用户名"), "{err}");
        let err = validate_outbound_url("https://user@api.x.com/v1", false).unwrap_err();
        assert!(err.contains("用户名"), "{err}");
    }

    #[test]
    fn validate_rejects_loopback_and_metadata_literal() {
        // 未勾选 → 拦
        assert!(validate_outbound_url("http://127.0.0.1:11434/v1", false).is_err());
        assert!(validate_outbound_url("http://[::1]:11434/v1", false).is_err());
        // 勾选 → 放行本机（Ollama / LM Studio）
        assert!(validate_outbound_url("http://127.0.0.1:11434/v1", true).is_ok());
        assert!(validate_outbound_url("http://[::1]:11434/v1", true).is_ok());
        // 云元数据：勾不勾都拦
        for allow in [false, true] {
            let err = validate_outbound_url("http://169.254.169.254/latest/meta-data/", allow)
                .unwrap_err();
            assert!(err.contains("链路本地"), "{err}");
        }
        // IPv4-mapped 写法同样拦
        assert!(validate_outbound_url("http://[::ffff:127.0.0.1]:8080/v1", false).is_err());
    }

    #[test]
    fn validate_local_hostnames_follow_loopback_switch() {
        for host in [
            "localhost",
            "LocalHost",
            "ollama.localhost",
            "nas.local",
            "metadata.internal",
        ] {
            assert!(
                validate_outbound_url(&format!("http://{host}:8080/v1"), false).is_err(),
                "{host} 未勾选本机地址时应被拦"
            );
            assert!(
                validate_outbound_url(&format!("http://{host}:8080/v1"), true).is_ok(),
                "{host} 勾选后应放行"
            );
        }
        // 普通域名不受影响（哪怕含 local 字样但不是后缀）
        assert!(validate_outbound_url("https://localify.com/v1", false).is_ok());
        assert!(validate_outbound_url("https://mylocal.com/v1", false).is_ok());
    }

    #[test]
    fn obfuscated_ipv4_literals_are_normalized_then_blocked() {
        // 十进制 / 十六进制 / 八进制 / 省略段 都指向 127.0.0.1。
        // url crate 会把它们归一化成 Ipv4，所以我们用 `url.host()` 判定时能拦住；
        // 这条测试就是为了防止将来有人改回 `host_str()` + 字符串比较而静默失守。
        for raw in [
            "http://2130706433/",
            "http://0x7f.0.0.1/",
            "http://0177.0.0.1/",
            "http://127.1/",
            "http://127.0.0.1./",
        ] {
            assert!(
                validate_outbound_url(raw, false).is_err(),
                "{raw} 归一化后是 127.0.0.1，必须被拦"
            );
        }
        // 元数据端点的混淆写法同样要拦
        assert!(validate_outbound_url("http://0xa9fea9fe/", true).is_err());
    }

    #[test]
    fn blocked_reason_is_human_readable() {
        let err = validate_outbound_url("http://10.0.0.5/v1", false).unwrap_err();
        assert!(err.contains("10.0.0.5"), "{err}");
        assert!(err.contains("内网"), "{err}");
    }
}
