//! 配置同步（roadmap M7）：WebDAV 手动推/拉，last-write-wins。
//!
//! 设计：
//! - 配置文件固定为 `jai-config.json`（可由目录字段拼接）。
//! - 推：构建导出 JSON → PUT 覆盖远端；推送前本地留存一份 last snapshot。
//! - 拉：GET 远端 → 交给 `store::import::apply_import` 落库（覆盖式 last-write-wins）。
//! - 凭据：username/password 由调用方从 UI/钥匙串提供，不落 meta。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::store::StoreError;

pub const CONFIG_FILE_NAME: &str = "jai-config.json";
const SNAPSHOT_META_KEY: &str = "webdav_last_snapshot";

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WebDavConfig {
    pub url: String,
    pub username: String,
    /// 远端目录（可空，默认根目录）
    pub directory: String,
    /// 变更/定时自动推送总开关（默认关）
    pub auto_push_enabled: bool,
    /// 定时推送间隔分钟数（30/60/360；默认 60）
    pub auto_push_interval_min: u32,
}

pub const AUTO_PUSH_INTERVAL_DEFAULT: u32 = 60;
pub const AUTO_PUSH_INTERVAL_ALLOWED: [u32; 3] = [30, 60, 360];

impl WebDavConfig {
    pub fn config_url(&self) -> String {
        let base = self.url.trim_end_matches('/');
        let dir = self.directory.trim_matches('/');
        if dir.is_empty() {
            format!("{base}/{CONFIG_FILE_NAME}")
        } else {
            format!("{base}/{dir}/{CONFIG_FILE_NAME}")
        }
    }

    /// 非法值回退默认 60 分钟。
    pub fn normalized_interval(&self) -> u32 {
        if AUTO_PUSH_INTERVAL_ALLOWED.contains(&self.auto_push_interval_min) {
            self.auto_push_interval_min
        } else {
            AUTO_PUSH_INTERVAL_DEFAULT
        }
    }
}

/// 推送前本地快照的 meta key。
pub fn snapshot_meta_key() -> &'static str {
    SNAPSHOT_META_KEY
}

/// 读取 WebDAV 连接配置（密码不入库，由钥匙串单独保存）。
pub fn config_get(c: &Connection) -> Result<Option<WebDavConfig>, StoreError> {
    let Some(url) = crate::store::meta_get(c, "webdav_url")? else {
        return Ok(None);
    };
    Ok(Some(WebDavConfig {
        url,
        username: crate::store::meta_get(c, "webdav_username")?.unwrap_or_default(),
        directory: crate::store::meta_get(c, "webdav_directory")?.unwrap_or_default(),
        auto_push_enabled: crate::store::meta_get(c, "webdav_auto_push_enabled")?
            .map(|v| v == "1")
            .unwrap_or(false),
        auto_push_interval_min: crate::store::meta_get(c, "webdav_auto_push_interval_min")?
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(AUTO_PUSH_INTERVAL_DEFAULT),
    }))
}

/// 保存 WebDAV 连接配置。
pub fn config_set(c: &Connection, cfg: &WebDavConfig) -> Result<(), StoreError> {
    crate::store::meta_set(c, "webdav_url", &cfg.url)?;
    crate::store::meta_set(c, "webdav_username", &cfg.username)?;
    crate::store::meta_set(c, "webdav_directory", &cfg.directory)?;
    crate::store::meta_set(
        c,
        "webdav_auto_push_enabled",
        if cfg.auto_push_enabled { "1" } else { "0" },
    )?;
    crate::store::meta_set(
        c,
        "webdav_auto_push_interval_min",
        &cfg.normalized_interval().to_string(),
    )?;
    Ok(())
}

/// 读取推送前本地快照（用于误操作回退）。
pub fn snapshot_get(c: &Connection) -> Result<Option<String>, StoreError> {
    crate::store::meta_get(c, SNAPSHOT_META_KEY)
}

/// 保存推送前本地快照。
pub fn snapshot_put(c: &Connection, text: &str) -> Result<(), StoreError> {
    crate::store::meta_set(c, SNAPSHOT_META_KEY, text)
}

/// WebDAV PUT：用 Basic Auth 覆盖远端配置。
pub async fn push(
    http: &reqwest::Client,
    cfg: &WebDavConfig,
    password: &str,
    body: String,
) -> Result<(), String> {
    let url = cfg.config_url();
    let resp = http
        .put(&url)
        .basic_auth(&cfg.username, Some(password))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("WebDAV 推送请求失败: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let hint = match status.as_u16() {
            401 | 403 => "（认证失败：检查 WebDAV 用户名/密码）",
            404 => "（目标目录不存在，请先在远端创建该目录）",
            _ => "",
        };
        return Err(format!("WebDAV 推送失败 HTTP {status}{hint}: {text}"));
    }
    Ok(())
}

/// WebDAV GET：拉取远端配置文本。
pub async fn pull(
    http: &reqwest::Client,
    cfg: &WebDavConfig,
    password: &str,
) -> Result<String, String> {
    let url = cfg.config_url();
    let resp = http
        .get(&url)
        .basic_auth(&cfg.username, Some(password))
        .send()
        .await
        .map_err(|e| format!("WebDAV 拉取请求失败: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let hint = match status.as_u16() {
            401 | 403 => "（认证失败：检查 WebDAV 用户名/密码，或在设置中重新保存）",
            404 => "（远端尚无配置文件：请先在任一设备执行「推送」，或检查目录是否正确）",
            _ => "",
        };
        return Err(format!("WebDAV 拉取失败 HTTP {status}{hint}: {text}"));
    }
    resp.text()
        .await
        .map_err(|e| format!("WebDAV 响应读取失败: {e}"))
}

/// 校验 WebDAV 连接与凭据。
///
/// 用带认证的 `PROPFIND Depth:0` 探测，而非 OPTIONS——DUFS 等服务器对
/// OPTIONS 匿名放行（实测返回 200），用它测连接会把无效凭据误报为
/// 「连接成功」；PROPFIND 需真实认证，401/403 即凭据错误。
pub async fn probe(
    http: &reqwest::Client,
    cfg: &WebDavConfig,
    password: &str,
) -> Result<String, String> {
    let base = cfg.url.trim_end_matches('/');
    let resp = http
        .request(
            reqwest::Method::from_bytes(b"PROPFIND").expect("PROPFIND 合法方法"),
            base,
        )
        .basic_auth(&cfg.username, Some(password))
        .header("Depth", "0")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("连接失败: {e}"))?;
    let status = resp.status().as_u16();
    match status {
        200..=299 => Ok("连接成功".into()),
        401 | 403 => Err(format!(
            "认证失败（HTTP {status}）：用户名或密码不正确，请在设置中更新"
        )),
        404 => Err("路径不存在（HTTP 404）：请检查 WebDAV 根地址与目录设置".into()),
        _ => Err(format!("连接异常（HTTP {status}）")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::open_and_migrate;

    #[test]
    fn config_roundtrip_via_meta() {
        let c = open_and_migrate(":memory:").unwrap();
        let cfg = WebDavConfig {
            url: "https://dav.example.com/".into(),
            username: "u".into(),
            directory: "jai".into(),
            auto_push_enabled: true,
            auto_push_interval_min: 30,
        };
        config_set(&c, &cfg).unwrap();
        assert_eq!(config_get(&c).unwrap(), Some(cfg));
        snapshot_put(&c, "{}").unwrap();
        assert_eq!(snapshot_get(&c).unwrap().as_deref(), Some("{}"));
    }

    #[test]
    fn config_defaults_and_interval_normalization() {
        let c = open_and_migrate(":memory:").unwrap();
        // 未配置时：无连接配置，但间隔常量默认 60
        assert_eq!(config_get(&c).unwrap(), None);
        assert_eq!(AUTO_PUSH_INTERVAL_DEFAULT, 60);
        // 写入非法间隔 → 读出回落 60
        let cfg = WebDavConfig {
            url: "https://dav.example.com/".into(),
            username: "u".into(),
            directory: String::new(),
            auto_push_enabled: false,
            auto_push_interval_min: 7,
        };
        config_set(&c, &cfg).unwrap();
        let read = config_get(&c).unwrap().unwrap();
        assert!(!read.auto_push_enabled);
        assert_eq!(read.auto_push_interval_min, AUTO_PUSH_INTERVAL_DEFAULT);
    }

    #[test]
    fn config_url_joins_directory() {
        let cfg = WebDavConfig {
            url: "https://dav.example.com/remote.php/dav/files/u".into(),
            username: "u".into(),
            directory: "jai/backups".into(),
            auto_push_enabled: false,
            auto_push_interval_min: AUTO_PUSH_INTERVAL_DEFAULT,
        };
        assert_eq!(
            cfg.config_url(),
            "https://dav.example.com/remote.php/dav/files/u/jai/backups/jai-config.json"
        );
    }

    #[test]
    fn config_url_without_directory() {
        let cfg = WebDavConfig {
            url: "https://dav.example.com/".into(),
            username: String::new(),
            directory: String::new(),
            auto_push_enabled: false,
            auto_push_interval_min: AUTO_PUSH_INTERVAL_DEFAULT,
        };
        assert_eq!(cfg.config_url(), "https://dav.example.com/jai-config.json");
    }
}
