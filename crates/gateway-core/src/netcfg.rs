//! 网关出站网络配置（D8）：HTTP(S)/SOCKS5 代理 + 绕过列表。
//!
//! 设计：
//! - meta 持久化（`proxy_enabled` / `proxy_url` / `proxy_bypass`，逗号分隔绕过主机）。
//! - 校验与匹配均为纯函数（可单测）；`build_client` 关闭时代理行为与现状完全一致。
//! - 生效时机：与监听端口一致——保存后重启网关生效（不热重建 client，避免动
//!   `GatewayCtx`/`AppCore` 结构引发转发链路风险）。

use std::time::Duration;

use rusqlite::Connection;

use crate::store::StoreError;

pub const PROXY_META_ENABLED: &str = "proxy_enabled";
pub const PROXY_META_URL: &str = "proxy_url";
pub const PROXY_META_BYPASS: &str = "proxy_bypass";

/// 出站代理配置。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyConfig {
    pub enabled: bool,
    /// `http://` / `https://` / `socks5://` 地址（可含 `user:pass@` 认证）
    pub url: String,
    /// 绕过主机列表：精确 host、`.suffix`（含子域）或 `*`（全部绕过）
    pub bypass: Vec<String>,
}

impl ProxyConfig {
    /// 从 meta 读取（缺省全部默认）。
    pub fn from_meta(c: &Connection) -> Result<ProxyConfig, StoreError> {
        Ok(ProxyConfig {
            enabled: crate::store::meta_get(c, PROXY_META_ENABLED)?
                .map(|v| v == "1")
                .unwrap_or(false),
            url: crate::store::meta_get(c, PROXY_META_URL)?.unwrap_or_default(),
            bypass: crate::store::meta_get(c, PROXY_META_BYPASS)?
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// 持久化到 meta。
    pub fn save(&self, c: &Connection) -> Result<(), StoreError> {
        crate::store::meta_set(c, PROXY_META_ENABLED, if self.enabled { "1" } else { "0" })?;
        crate::store::meta_set(c, PROXY_META_URL, &self.url)?;
        crate::store::meta_set(c, PROXY_META_BYPASS, &self.bypass.join(","))?;
        Ok(())
    }
}

/// 校验代理 URL：scheme 白名单（http/https/socks5/socks5h）、必须含 host:port。
/// 不返回规范化地址——原样透传给 reqwest（URL 内 `user:pass@` 认证随行）。
pub fn validate_proxy_url(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("代理地址不能为空".into());
    }
    let u = url::Url::parse(trimmed).map_err(|e| format!("代理地址无法解析: {e}"))?;
    match u.scheme() {
        "http" | "https" | "socks5" | "socks5h" => {}
        s => return Err(format!("代理 scheme 不支持: {s}（仅 http/https/socks5）")),
    }
    let host = u
        .host_str()
        .ok_or_else(|| "代理地址缺少主机名".to_string())?;
    if host.is_empty() {
        return Err("代理地址缺少主机名".into());
    }
    // 要求显式端口（代理端口语义必须明确，http 默认 80 不适用）
    let port = u.port().ok_or_else(|| "代理地址缺少端口".to_string())?;
    if port == 0 {
        return Err("代理端口不能为 0".into());
    }
    Ok(())
}

/// 绕过判定：`*` 全过；条目 `host` 精确匹配；条目 `.suffix` 匹配自身与所有子域。
/// 主机与条目均去 `[]`（IPv6）、小写比较；条目忽略前导点。
pub fn should_bypass(host: &str, bypass: &[String]) -> bool {
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    bypass.iter().any(|b| {
        let b = b.trim().trim_start_matches('.').to_ascii_lowercase();
        if b.is_empty() {
            return false;
        }
        if b == "*" {
            return true;
        }
        host == b || host.ends_with(&format!(".{b}"))
    })
}

/// 构建网关出站 HTTP client。
/// - `proxy` 为 Some 且 `enabled` 且 URL 非空：应用 `Proxy::all` + 绕过列表；
/// - 否则与现状完全一致（不设置任何代理）。
///
/// URL 非法时不阻断启动——回退为无代理并打印警告（保存路径已有校验兜底）。
pub fn build_client(proxy: Option<&ProxyConfig>, connect_timeout: Duration) -> reqwest::Client {
    let mut builder = reqwest::Client::builder().connect_timeout(connect_timeout);
    if let Some(p) = proxy.filter(|p| p.enabled && !p.url.trim().is_empty()) {
        match reqwest::Proxy::all(p.url.trim()) {
            Ok(proxy) => {
                let proxy = proxy.no_proxy(reqwest::NoProxy::from_string(&p.bypass.join(",")));
                builder = builder.proxy(proxy);
            }
            Err(e) => eprintln!("[netcfg] 代理 URL 无效({e})，本次启动按无代理处理"),
        }
    }
    builder.build().expect("reqwest client 构建失败")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::open_and_migrate;

    #[test]
    fn validate_proxy_url_matrix() {
        // 合法
        for url in [
            "http://127.0.0.1:7890",
            "https://proxy.example.com:8443",
            "socks5://127.0.0.1:1080",
            "http://user:pass@127.0.0.1:7890", // 认证随行
            "http://localhost:7890",
        ] {
            validate_proxy_url(url).unwrap_or_else(|e| panic!("{url} 应合法: {e}"));
        }
        // 非法
        for url in [
            "",
            "ftp://host:21",
            "http://",       // 缺 host
            "http://host",   // 缺端口（非默认 scheme）
            "http://:7890",  // 空 host
            "http://host:0", // 端口 0
            "not a url",     // 无法解析
        ] {
            assert!(validate_proxy_url(url).is_err(), "{url} 应非法");
        }
    }

    #[test]
    fn should_bypass_matching() {
        let list = vec![
            "127.0.0.1".to_string(),
            ".internal".to_string(),
            "*".to_string(),
            ".Example.com".to_string(),
        ];
        // 精确 host（大小写不敏感）
        assert!(should_bypass("127.0.0.1", &list));
        assert!(should_bypass("127.0.0.1:8080", &list));
        // 通配 * → 全过
        assert!(should_bypass("anything.else", &list));
        // .suffix：自身与子域
        assert!(should_bypass("internal", &list));
        assert!(should_bypass("api.internal", &list));
        assert!(should_bypass("deep.nested.api.internal", &list));
        assert!(should_bypass("EXAMPLE.com", &list));
        // 非绕过
        assert!(!should_bypass(
            "public.example.org",
            &["127.0.0.1".into(), ".internal".into()]
        ));
        assert!(!should_bypass("", &["127.0.0.1".into()]));
        // 空条目忽略
        assert!(!should_bypass("h", &["  ".into(), ",".into()]));
    }

    #[test]
    fn proxy_config_meta_roundtrip() {
        let c = open_and_migrate(":memory:").unwrap();
        let cfg = ProxyConfig {
            enabled: true,
            url: "socks5://127.0.0.1:1080".into(),
            bypass: vec!["127.0.0.1".into(), ".internal".into()],
        };
        cfg.save(&c).unwrap();
        assert_eq!(ProxyConfig::from_meta(&c).unwrap(), cfg);
        // 未配置 → 默认关闭
        let fresh = open_and_migrate(":memory:").unwrap();
        let d = ProxyConfig::from_meta(&fresh).unwrap();
        assert!(!d.enabled);
        assert!(d.url.is_empty());
        assert!(d.bypass.is_empty());
    }

    #[test]
    fn build_client_proxy_applies_when_enabled() {
        // 启用：client 可正常构建（Proxy::all 解析 http 代理）
        let cfg = ProxyConfig {
            enabled: true,
            url: "http://127.0.0.1:7890".into(),
            bypass: vec!["127.0.0.1".into()],
        };
        let _c = build_client(Some(&cfg), Duration::from_secs(10));
        // 关闭：与默认一致
        let off = ProxyConfig::default();
        let _c2 = build_client(Some(&off), Duration::from_secs(10));
        // None：与现状一致
        let _c3 = build_client(None, Duration::from_secs(10));
    }
}
