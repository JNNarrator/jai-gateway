//! 配置同步（roadmap M7）：WebDAV 手动推/拉，last-write-wins。
//!
//! 设计：
//! - 配置文件固定为 `jai-config.json`（可由目录字段拼接）。
//! - 推：构建导出 JSON → PUT 覆盖远端；推送前本地留存一份 last snapshot。
//! - 拉：GET 远端 → 交给 `store::import::apply_import` 落库（覆盖式 last-write-wins）。
//! - 凭据：username/password 由调用方从 UI/钥匙串提供，不落 meta。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::store::StoreError;

pub const CONFIG_FILE_NAME: &str = "jai-config.json";
const SNAPSHOT_META_KEY: &str = "webdav_last_snapshot";
/// 最近一次成功推/拉的远端 exportedAt（last-write-wins 时间戳基线）。
const LAST_SYNC_META_KEY: &str = "webdav_last_sync_exported_at";

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
    /// 定时自动拉取总开关（默认关；与自动推送共用间隔，按 exportedAt 时间戳 last-write-wins）
    #[serde(default)]
    pub auto_pull_enabled: bool,
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

    /// 配置文件所在远端目录（不含文件名）。
    fn config_dir(&self) -> String {
        let base = self.url.trim_end_matches('/');
        let dir = self.directory.trim_matches('/');
        if dir.is_empty() {
            base.to_string()
        } else {
            format!("{base}/{dir}")
        }
    }

    /// 覆盖前留存远端旧版的备份地址（与配置文件同目录，时间戳后缀）。
    pub fn backup_url(&self, now_ms: i64) -> String {
        format!("{}/jai-config.{now_ms}.json", self.config_dir())
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
        auto_pull_enabled: crate::store::meta_get(c, "webdav_auto_pull_enabled")?
            .map(|v| v == "1")
            .unwrap_or(false),
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
    crate::store::meta_set(
        c,
        "webdav_auto_pull_enabled",
        if cfg.auto_pull_enabled { "1" } else { "0" },
    )?;
    Ok(())
}

/// 最近一次成功推/拉的远端 exportedAt（无记录为 None）。
pub fn last_sync_get(c: &Connection) -> Result<Option<i64>, StoreError> {
    Ok(crate::store::meta_get(c, LAST_SYNC_META_KEY)?.and_then(|v| v.parse::<i64>().ok()))
}

/// 记录最近一次成功推/拉的远端 exportedAt。
pub fn last_sync_put(c: &Connection, exported_at_ms: i64) -> Result<(), StoreError> {
    crate::store::meta_set(c, LAST_SYNC_META_KEY, &exported_at_ms.to_string())
}

/// 读取推送前本地快照（用于误操作回退）。
pub fn snapshot_get(c: &Connection) -> Result<Option<String>, StoreError> {
    crate::store::meta_get(c, SNAPSHOT_META_KEY)
}

/// 保存推送前本地快照。
pub fn snapshot_put(c: &Connection, text: &str) -> Result<(), StoreError> {
    crate::store::meta_set(c, SNAPSHOT_META_KEY, text)
}

/// 覆盖式推送：先尝试读取远端旧版并留存时间戳备份，再 PUT 覆盖主文件。
///
/// 2026-09 数据丢失修复：此前直接 PUT 覆盖——另一台设备（空配置 + 自动推送）
/// 会把远端完整备份覆盖成空文件；现在每次覆盖前先把旧版复制到同目录
/// `jai-config.<unix_ms>.json`，远端备份永不丢失。备份失败即中止，不盲覆盖。
pub async fn push(
    http: &reqwest::Client,
    cfg: &WebDavConfig,
    password: &str,
    body: String,
) -> Result<(), String> {
    // 1) 远端旧版先留存备份（首次推送 / 远端无文件时跳过）
    match try_pull(http, cfg, password).await {
        Ok(Some(old)) if !old.trim().is_empty() && old.trim() != "{}" => {
            let backup_url = cfg.backup_url(crate::store::now_ms());
            let resp = http
                .put(&backup_url)
                .basic_auth(&cfg.username, Some(password))
                .header("Content-Type", "application/json")
                .body(old)
                .send()
                .await
                .map_err(|e| format!("WebDAV 备份请求失败: {e}"))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                let hint = match status.as_u16() {
                    401 | 403 => "（认证失败：检查 WebDAV 用户名/密码）",
                    404 => "（目标目录不存在，请先在远端创建该目录）",
                    _ => "",
                };
                return Err(format!(
                    "WebDAV 备份远端旧配置失败 HTTP {status}{hint}: {text}（已中止推送，防止覆盖丢失）"
                ));
            }
        }
        Ok(_) => {}
        Err(e) => return Err(e),
    }

    // 2) 覆盖主文件
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

/// GET 远端配置文本；远端文件不存在（HTTP 404）返回 `Ok(None)`。
pub async fn try_pull(
    http: &reqwest::Client,
    cfg: &WebDavConfig,
    password: &str,
) -> Result<Option<String>, String> {
    let url = cfg.config_url();
    let resp = http
        .get(&url)
        .basic_auth(&cfg.username, Some(password))
        .send()
        .await
        .map_err(|e| format!("WebDAV 拉取请求失败: {e}"))?;
    if resp.status().as_u16() == 404 {
        return Ok(None);
    }
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let hint = match status.as_u16() {
            401 | 403 => "（认证失败：检查 WebDAV 用户名/密码，或在设置中重新保存）",
            _ => "",
        };
        return Err(format!("WebDAV 拉取失败 HTTP {status}{hint}: {text}"));
    }
    resp.text()
        .await
        .map(Some)
        .map_err(|e| format!("WebDAV 响应读取失败: {e}"))
}

/// WebDAV GET：拉取远端配置文本（404 转「远端尚无配置文件」提示）。
pub async fn pull(
    http: &reqwest::Client,
    cfg: &WebDavConfig,
    password: &str,
) -> Result<String, String> {
    match try_pull(http, cfg, password).await? {
        Some(text) => Ok(text),
        None => Err(
            "WebDAV 拉取失败 HTTP 404（远端尚无配置文件：请先在任一设备执行「推送」，或检查目录是否正确）"
                .to_string(),
        ),
    }
}

/// 解析导出 JSON 中的供应商/模型数量（护栏比对与预览共用）。
pub fn export_counts(text: &str) -> Result<(usize, usize), String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("配置 JSON 解析失败: {e}"))?;
    let providers = v
        .get("providers")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    let models = v
        .get("models")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    Ok((providers, models))
}

/// 自动推送护栏：本地为空（0 供应商或 0 模型）而远端有内容时返回跳过原因。
///
/// 2026-09 数据丢失修复：空配置的自动推送会覆盖远端完整备份；护栏让自动推送
/// 在这种情形下让位，改由用户显式手动推送。远端不存在或远端也为空时放行。
/// 解析失败一律放行（不因护栏误伤正常推送）。
pub fn should_protect_remote(local: &str, remote: Option<&str>) -> Option<String> {
    let remote = remote?;
    let (lp, lm) = export_counts(local).ok()?;
    let (rp, rm) = export_counts(remote).ok()?;
    if (lp == 0 && rp > 0) || (lm == 0 && rm > 0) {
        return Some(format!(
            "自动推送已跳过：本地为空（{lp} 个供应商/{lm} 个模型），远端有 {rp} 个供应商/{rm} 个模型；为避免覆盖远端备份，请手动「推送」"
        ));
    }
    None
}

/// 读取导出 JSON 的 `exportedAt`（毫秒时间戳；缺失/非法返回 None）。
pub fn exported_at(text: &str) -> Option<i64> {
    serde_json::from_str::<Value>(text)
        .ok()?
        .get("exportedAt")
        .and_then(Value::as_i64)
}

/// 定时自动拉取判定（last-write-wins 按 exportedAt 时间戳 + 空远端防护）：
/// 仅当远端非空（有供应商或模型）、比上次成功同步更新、且内容与本地不同时才应拉取。
/// 空远端不自动拉取——防止远端空配置静默清空本机（与推送护栏对称）。
pub fn should_pull(local: &str, remote: &str, last_sync_at: Option<i64>) -> bool {
    let Ok((rp, rm)) = export_counts(remote) else {
        return false;
    };
    if rp + rm == 0 {
        return false; // 空远端不自动拉取
    }
    let Some(ts) = exported_at(remote) else {
        return false; // 无法确认远端时间戳 → 不自动拉取（手动拉取可用）
    };
    if last_sync_at.is_some_and(|t| ts <= t) {
        return false; // 不比上次成功同步更新 → 本地不落后
    }
    if local == remote {
        return false; // 内容一致无需拉
    }
    true
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

    fn mk_cfg(url: &str, username: &str, directory: &str) -> WebDavConfig {
        WebDavConfig {
            url: url.into(),
            username: username.into(),
            directory: directory.into(),
            auto_push_enabled: false,
            auto_push_interval_min: AUTO_PUSH_INTERVAL_DEFAULT,
            auto_pull_enabled: false,
        }
    }

    #[test]
    fn config_roundtrip_via_meta() {
        let c = open_and_migrate(":memory:").unwrap();
        let mut cfg = mk_cfg("https://dav.example.com/", "u", "jai");
        cfg.auto_push_enabled = true;
        cfg.auto_push_interval_min = 30;
        cfg.auto_pull_enabled = true;
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
        // 写入非法间隔 → 读出回落 60；auto_pull 未写 → 默认关
        let mut cfg = mk_cfg("https://dav.example.com/", "u", "");
        cfg.auto_push_interval_min = 7;
        config_set(&c, &cfg).unwrap();
        let read = config_get(&c).unwrap().unwrap();
        assert!(!read.auto_push_enabled);
        assert!(!read.auto_pull_enabled);
        assert_eq!(read.auto_push_interval_min, AUTO_PUSH_INTERVAL_DEFAULT);
    }

    #[test]
    fn config_url_joins_directory() {
        let cfg = mk_cfg(
            "https://dav.example.com/remote.php/dav/files/u",
            "u",
            "jai/backups",
        );
        assert_eq!(
            cfg.config_url(),
            "https://dav.example.com/remote.php/dav/files/u/jai/backups/jai-config.json"
        );
    }

    #[test]
    fn config_url_without_directory() {
        let cfg = mk_cfg("https://dav.example.com/", "", "");
        assert_eq!(cfg.config_url(), "https://dav.example.com/jai-config.json");
    }

    #[test]
    fn backup_url_joins_directory_and_timestamp() {
        let cfg = mk_cfg(
            "https://dav.example.com/remote.php/dav/files/u",
            "u",
            "jai/backups",
        );
        assert_eq!(
            cfg.backup_url(1234),
            "https://dav.example.com/remote.php/dav/files/u/jai/backups/jai-config.1234.json"
        );
        let root = mk_cfg("https://dav.example.com/", "", "");
        assert_eq!(
            root.backup_url(9),
            "https://dav.example.com/jai-config.9.json"
        );
    }

    #[test]
    fn export_counts_parse() {
        let full = r#"{"format":"jai-export/v1","providers":[{"id":"a"}],"models":[{"id":"m"}]}"#;
        assert_eq!(export_counts(full).unwrap(), (1, 1));
        assert_eq!(export_counts("{}").unwrap(), (0, 0));
        assert!(export_counts("not json").is_err());
    }

    #[test]
    fn auto_push_guard_protects_richer_remote() {
        let full = r#"{"providers":[{"id":"a"}],"models":[{"id":"m"}]}"#;
        let empty = r#"{"providers":[],"models":[]}"#;
        // 本地空 + 远端有内容 → 跳过并给出原因
        let reason = should_protect_remote(empty, Some(full)).expect("应拦截空配置覆盖");
        assert!(reason.contains("自动推送已跳过"), "{reason}");
        // 远端无文件 → 放行（首次推送）
        assert!(should_protect_remote(empty, None).is_none());
        // 远端也为空 → 放行
        assert!(should_protect_remote(empty, Some("{}")).is_none());
        // 本地与远端等价 → 放行
        assert!(should_protect_remote(full, Some(full)).is_none());
        // 本地比远端更全 → 放行
        assert!(should_protect_remote(full, Some(empty)).is_none());
        // 本地解析失败 → 放行（不误伤）
        assert!(should_protect_remote("bad json", Some(full)).is_none());
    }

    #[test]
    fn last_sync_roundtrip_via_meta() {
        let c = open_and_migrate(":memory:").unwrap();
        assert_eq!(last_sync_get(&c).unwrap(), None);
        last_sync_put(&c, 1_788_452_907_000).unwrap();
        assert_eq!(last_sync_get(&c).unwrap(), Some(1_788_452_907_000));
    }

    #[test]
    fn exported_at_parse() {
        assert_eq!(
            exported_at(r#"{"exportedAt":12345,"providers":[]}"#),
            Some(12345)
        );
        assert_eq!(exported_at(r#"{"providers":[]}"#), None);
        assert_eq!(exported_at("not json"), None);
    }

    #[test]
    fn auto_pull_should_pull_judgement() {
        let local = r#"{"format":"jai-export/v1","exportedAt":100,"providers":[{"id":"a"}],"models":[{"id":"m"}]}"#;
        let remote_newer = r#"{"format":"jai-export/v1","exportedAt":200,"providers":[{"id":"a"},{"id":"b"}],"models":[{"id":"m"}]}"#;
        let remote_older = r#"{"format":"jai-export/v1","exportedAt":50,"providers":[{"id":"a"}],"models":[{"id":"m"}]}"#;
        let remote_empty =
            r#"{"format":"jai-export/v1","exportedAt":300,"providers":[],"models":[]}"#;
        let remote_no_ts =
            r#"{"format":"jai-export/v1","providers":[{"id":"a"}],"models":[{"id":"m"}]}"#;

        // 远端更新且非空 → 拉
        assert!(should_pull(local, remote_newer, Some(100)));
        // 无 last_sync 记录 → 只要远端非空且时间戳可读即拉
        assert!(should_pull(local, remote_newer, None));
        // 远端不比 last_sync 新 → 不拉
        assert!(!should_pull(local, remote_newer, Some(200)));
        // 远端比本地旧 → 不拉（本地为主）
        assert!(!should_pull(local, remote_older, Some(100)));
        // 空远端 → 不拉（防清空本地）
        assert!(!should_pull(local, remote_empty, Some(100)));
        // 远端无时间戳 → 不拉（无法判定新旧）
        assert!(!should_pull(local, remote_no_ts, Some(100)));
        // 内容一致 → 不拉
        assert!(!should_pull(local, local, Some(100)));
        // 远端非 JSON → 不拉
        assert!(!should_pull(local, "not json", Some(100)));
    }
}
