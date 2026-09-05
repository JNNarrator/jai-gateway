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
    /// 配置文件完整远端地址（目录各路径段按 URL 语义百分号编码，
    /// 空格/中文等不再拼出非法 URL）。
    pub fn config_url(&self) -> String {
        join_remote_file(&self.url, &self.directory, CONFIG_FILE_NAME)
    }

    /// 非法值回退默认 60 分钟。
    pub fn normalized_interval(&self) -> u32 {
        if AUTO_PUSH_INTERVAL_ALLOWED.contains(&self.auto_push_interval_min) {
            self.auto_push_interval_min
        } else {
            AUTO_PUSH_INTERVAL_DEFAULT
        }
    }

    /// 配置文件所在远端目录（不含文件名；路径段已编码）。
    pub fn config_dir(&self) -> String {
        let base = self.url.trim_end_matches('/');
        let dir = self.directory.trim_matches('/');
        if dir.is_empty() {
            base.to_string()
        } else {
            join_path(base, dir)
        }
    }

    /// 覆盖前留存远端旧版的备份地址（与配置文件同目录，时间戳后缀）。
    pub fn backup_url(&self, now_ms: i64) -> String {
        join_remote_file(
            &self.url,
            &self.directory,
            &format!("jai-config.{now_ms}.json"),
        )
    }
}

/// 把目录拼到 base 后（各路径段按 URL 语义编码）。base 无法解析（如
/// 非 http(s) 前缀）时回退裸字符串拼接，保持旧行为不抛错。
fn join_path(base: &str, dir: &str) -> String {
    let base = base.trim_end_matches('/');
    let mut u = match url::Url::parse(base) {
        Ok(u) => u,
        Err(_) => return format!("{base}/{dir}"),
    };
    if u.cannot_be_a_base() {
        return format!("{base}/{dir}");
    }
    {
        let mut segs = u.path_segments_mut().expect("cannot_be_a_base 已排除");
        segs.pop_if_empty();
        for s in dir.split('/').filter(|s| !s.is_empty()) {
            segs.push(s); // url crate 自动对每段做百分号编码
        }
    }
    u.to_string()
}

/// 远端配置文件地址 = base + 目录段 + 文件名。
fn join_remote_file(base: &str, dir: &str, file: &str) -> String {
    let base = base.trim_end_matches('/');
    let dir = dir.trim_matches('/');
    if dir.is_empty() {
        join_path(base, file)
    } else {
        format!("{}/{}", join_path(base, dir), file)
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

/// 解析备份文件名 `jai-config.<unix_ms>.json` 的时间戳；不匹配返回 None。
pub fn backup_timestamp(name: &str) -> Option<i64> {
    let stem = name.strip_prefix("jai-config.")?;
    let digits = stem.strip_suffix(".json")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<i64>().ok()
}

/// 远端备份保留份数（推送后滚动清理时保留最近 N 份时间戳备份）。
pub const BACKUP_KEEP: usize = 10;

/// 备份保留策略：给定远端目录下全部文件名（含当前配置），返回应删除的最老备份
/// 文件名列表。只识别 `jai-config.<digits>.json` 形态，当前配置文件与无关文件
/// 永不列入；不足 `keep` 份时不删任何备份。
pub fn backup_evict_candidates(names: &[String], keep: usize) -> Vec<String> {
    let mut cands: Vec<(i64, String)> = names
        .iter()
        .filter_map(|n| backup_timestamp(n).map(|ts| (ts, n.clone())))
        .collect();
    cands.sort_by_key(|(ts, _)| *ts);
    cands
        .iter()
        .take(cands.len().saturating_sub(keep))
        .map(|(_, n)| n.clone())
        .collect()
}

/// 远端备份信息（PROPFIND 列表结果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    /// 文件名（如 `jai-config.123.json`；当前配置为 `jai-config.json`）
    pub name: String,
    /// 完整远端路径（GET/DELETE 用）
    pub href: String,
    /// 字节大小（服务器未返回时为 None）
    pub size: Option<u64>,
}

/// 备份文件名 → 其完整远端地址。仅接受 `jai-config.<digits>.json` 形态。
pub fn backup_href(cfg: &WebDavConfig, name: &str) -> Result<String, String> {
    if backup_timestamp(name).is_none() {
        return Err(format!("非法备份文件名: {name}"));
    }
    Ok(format!("{}/{}", cfg.config_dir(), name))
}

/// PROPFIND（Depth:1）列出远端目录下全部条目，仅保留当前配置与时间戳备份。
pub async fn list_backups(
    http: &reqwest::Client,
    cfg: &WebDavConfig,
    password: &str,
) -> Result<Vec<BackupInfo>, String> {
    let dir_url = cfg.config_dir();
    let resp = http
        .request(
            reqwest::Method::from_bytes(b"PROPFIND").expect("PROPFIND 合法"),
            &dir_url,
        )
        .basic_auth(&cfg.username, Some(password))
        .header("Depth", "1")
        .header("Content-Type", "application/xml")
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("WebDAV 备份列表请求失败: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let hint = match status.as_u16() {
            401 | 403 => "（认证失败：检查 WebDAV 用户名/密码）",
            404 => "（目录不存在：请检查根地址与目录设置）",
            _ => "",
        };
        return Err(format!("WebDAV 备份列表失败 HTTP {status}{hint}: {text}"));
    }
    let xml = resp
        .text()
        .await
        .map_err(|e| format!("WebDAV 备份列表响应读取失败: {e}"))?;
    let mut out: Vec<BackupInfo> = parse_multistatus(&xml)?
        .into_iter()
        .filter(|b| b.name == CONFIG_FILE_NAME || backup_timestamp(&b.name).is_some())
        .collect();
    out.sort_by_key(|b| backup_timestamp(&b.name).unwrap_or(i64::MAX));
    Ok(out)
}

/// GET 指定备份的配置文本。
pub async fn fetch_backup(
    http: &reqwest::Client,
    cfg: &WebDavConfig,
    password: &str,
    name: &str,
) -> Result<String, String> {
    let url = backup_href(cfg, name)?;
    let resp = http
        .get(&url)
        .basic_auth(&cfg.username, Some(password))
        .send()
        .await
        .map_err(|e| format!("WebDAV 备份读取请求失败: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let hint = match status.as_u16() {
            401 | 403 => "（认证失败：检查 WebDAV 用户名/密码）",
            404 => "（备份不存在：可能已被清理）",
            _ => "",
        };
        return Err(format!("WebDAV 备份读取失败 HTTP {status}{hint}: {text}"));
    }
    resp.text()
        .await
        .map_err(|e| format!("WebDAV 备份响应读取失败: {e}"))
}

/// DELETE 指定备份（仅允许时间戳备份名）。
pub async fn delete_backup(
    http: &reqwest::Client,
    cfg: &WebDavConfig,
    password: &str,
    name: &str,
) -> Result<(), String> {
    let url = backup_href(cfg, name)?;
    let resp = http
        .delete(&url)
        .basic_auth(&cfg.username, Some(password))
        .send()
        .await
        .map_err(|e| format!("WebDAV 备份删除请求失败: {e}"))?;
    if resp.status().as_u16() == 404 {
        return Ok(()); // 已不存在，幂等成功
    }
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let hint = match status.as_u16() {
            401 | 403 => "（认证失败：检查 WebDAV 用户名/密码）",
            _ => "",
        };
        return Err(format!("WebDAV 备份删除失败 HTTP {status}{hint}: {text}"));
    }
    Ok(())
}

/// 解析 PROPFIND multistatus XML：抽取 href + getcontentlength。
fn parse_multistatus(xml: &str) -> Result<Vec<BackupInfo>, String> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.trim_text(true);
    let mut out = Vec::new();
    let mut cur_href: Option<String> = None;
    let mut cur_size: Option<u64> = None;
    let mut last_tag: Vec<u8> = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e)) => {
                last_tag = e.name().as_ref().to_vec();
                if last_tag.ends_with(b"response") {
                    cur_href = None;
                    cur_size = None;
                }
            }
            Ok(quick_xml::events::Event::Empty(e)) => {
                last_tag = e.name().as_ref().to_vec();
                if last_tag.ends_with(b"href") {
                    cur_href = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key.as_ref().ends_with(b"href"))
                        .and_then(|a| a.unescape_value().ok())
                        .map(|s| s.trim().to_string());
                }
            }
            Ok(quick_xml::events::Event::Text(t)) => {
                let text = t.unescape().unwrap_or_default();
                let text = text.trim().to_string();
                if last_tag.ends_with(b"href") && !text.is_empty() {
                    cur_href = Some(text);
                } else if last_tag.ends_with(b"getcontentlength") {
                    cur_size = text.parse::<u64>().ok();
                }
            }
            Ok(quick_xml::events::Event::End(e)) => {
                let name = e.name();
                let name = name.as_ref();
                if name.ends_with(b"response") {
                    if let Some(href) = cur_href.take() {
                        let file = href.rsplit('/').next().unwrap_or(&href).to_string();
                        out.push(BackupInfo {
                            name: file,
                            href,
                            size: cur_size.take(),
                        });
                    }
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(e) => return Err(format!("PROPFIND 响应解析失败: {e}")),
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

/// 推送前差异明细（可视 diff，T5）：双向列出差异条目。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PushDiffDetail {
    pub remote_exists: bool,
    /// 远端有、本地没有的供应商（推送覆盖后将丢失）
    pub remote_only_providers: Vec<(String, String)>,
    /// 远端有、本地没有的模型（推送覆盖后将丢失）
    pub remote_only_models: Vec<(String, String)>,
    /// 本地有、远端没有的供应商（推送会新增到远端）
    pub local_only_providers: Vec<(String, String)>,
    /// 本地有、远端没有的模型（推送会新增到远端）
    pub local_only_models: Vec<(String, String)>,
}

impl PushDiffDetail {
    pub fn blocks(&self) -> bool {
        !self.remote_only_providers.is_empty() || !self.remote_only_models.is_empty()
    }
}

/// 推送前差异明细：远端独有（将丢失）与本地独有（将新增）逐条列出。
pub fn push_diff_detail(local: &str, remote: Option<&str>) -> Result<PushDiffDetail, String> {
    let Some(remote) = remote else {
        return Ok(PushDiffDetail {
            remote_exists: false,
            ..Default::default()
        });
    };
    let (lp, lm) = export_keys(local)?;
    let (rp, rm) = export_keys(remote)?;
    let mut remote_only_providers: Vec<_> = rp.difference(&lp).cloned().collect();
    let mut remote_only_models: Vec<_> = rm.difference(&lm).cloned().collect();
    let mut local_only_providers: Vec<_> = lp.difference(&rp).cloned().collect();
    let mut local_only_models: Vec<_> = lm.difference(&rm).cloned().collect();
    remote_only_providers.sort();
    remote_only_models.sort();
    local_only_providers.sort();
    local_only_models.sort();
    Ok(PushDiffDetail {
        remote_exists: true,
        remote_only_providers,
        remote_only_models,
        local_only_providers,
        local_only_models,
    })
}

/// 推送前差异预警结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PushDiff {
    pub remote_exists: bool,
    /// 远端有、本地没有的供应商数（推送覆盖后将丢失）
    pub remote_only_providers: usize,
    /// 远端有、本地没有的模型数（推送覆盖后将丢失）
    pub remote_only_models: usize,
}

impl PushDiff {
    /// 是否需要用户确认（远端存在本机没有的内容）。
    pub fn blocks(&self) -> bool {
        self.remote_only_providers > 0 || self.remote_only_models > 0
    }
}

/// 导出键集：供应商 (name, base_url) 与模型 (providerId, modelName)。
type ExportKeys = (
    std::collections::HashSet<(String, String)>,
    std::collections::HashSet<(String, String)>,
);

/// 提取导出 JSON 的供应商 (name, base_url) 与模型 (providerId, modelName) 键集。
fn export_keys(text: &str) -> Result<ExportKeys, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("配置 JSON 解析失败: {e}"))?;
    let mut providers = std::collections::HashSet::new();
    if let Some(arr) = v.get("providers").and_then(Value::as_array) {
        for p in arr {
            providers.insert((
                p.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                p.get("base_url")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ));
        }
    }
    let mut models = std::collections::HashSet::new();
    if let Some(arr) = v.get("models").and_then(Value::as_array) {
        for m in arr {
            models.insert((
                m.get("providerId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                m.get("modelName")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ));
        }
    }
    Ok((providers, models))
}

/// 推送前差异：远端有而本地没有的供应商/模型（推送覆盖会丢掉它们）。
pub fn push_diff(local: &str, remote: Option<&str>) -> Result<PushDiff, String> {
    let Some(remote) = remote else {
        return Ok(PushDiff {
            remote_exists: false,
            ..Default::default()
        });
    };
    let (lp, lm) = export_keys(local)?;
    let (rp, rm) = export_keys(remote)?;
    Ok(PushDiff {
        remote_exists: true,
        remote_only_providers: rp.difference(&lp).count(),
        remote_only_models: rm.difference(&lm).count(),
    })
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
    fn config_url_percent_encodes_directory_segments() {
        // 空格与中文目录：各路径段按 URL 语义百分号编码，不再拼出非法 URL
        let cfg = mk_cfg("https://dav.example.com/", "u", "jai 备份/子 目录");
        assert_eq!(
            cfg.config_url(),
            "https://dav.example.com/jai%20%E5%A4%87%E4%BB%BD/%E5%AD%90%20%E7%9B%AE%E5%BD%95/jai-config.json"
        );
        assert_eq!(
            cfg.backup_url(7),
            "https://dav.example.com/jai%20%E5%A4%87%E4%BB%BD/%E5%AD%90%20%E7%9B%AE%E5%BD%95/jai-config.7.json"
        );
        // 无法解析的 base 回退裸拼接（不抛错）
        let odd = mk_cfg("not a url", "u", "a b");
        assert_eq!(odd.config_url(), "not a url/a b/jai-config.json");
    }

    #[test]
    fn export_counts_parse() {
        let full = r#"{"format":"jai-export/v1","providers":[{"id":"a"}],"models":[{"id":"m"}]}"#;
        assert_eq!(export_counts(full).unwrap(), (1, 1));
        assert_eq!(export_counts("{}").unwrap(), (0, 0));
        assert!(export_counts("not json").is_err());
    }

    #[test]
    fn backup_timestamp_parse() {
        assert_eq!(backup_timestamp("jai-config.123.json"), Some(123));
        assert_eq!(backup_timestamp("jai-config.0007.json"), Some(7));
        assert_eq!(backup_timestamp("jai-config.json"), None); // 当前配置名
        assert_eq!(backup_timestamp("jai-config.x.json"), None);
        assert_eq!(backup_timestamp("other.json"), None);
        assert_eq!(backup_timestamp("jai-config.12.json.bak"), None);
    }

    #[test]
    fn backup_evict_keeps_newest_n() {
        let names: Vec<String> = (1..=12).map(|i| format!("jai-config.{i}.json")).collect();
        // 12 份保留 10 → 删最老 2 份
        let evict = backup_evict_candidates(&names, 10);
        assert_eq!(
            evict,
            vec![
                "jai-config.1.json".to_string(),
                "jai-config.2.json".to_string()
            ]
        );
        // 恰好 10 份 → 不删
        let names10: Vec<String> = (1..=10).map(|i| format!("jai-config.{i}.json")).collect();
        assert!(backup_evict_candidates(&names10, 10).is_empty());
        // 仅 2 份备份 + keep=1 → 删最老 1 份；当前配置与无关文件永不列入
        let mixed = vec![
            "jai-config.1.json".into(),
            "jai-config.2.json".into(),
            "jai-config.json".into(),
            "readme.txt".into(),
        ];
        assert_eq!(
            backup_evict_candidates(&mixed, 1),
            vec!["jai-config.1.json".to_string()]
        );
        // keep=0 语义：全部备份都可删（当前配置文件不在候选内）
        assert_eq!(backup_evict_candidates(&names, 0).len(), 12);
    }

    #[test]
    fn parse_multistatus_extracts_href_and_size() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dav/files/u/jai/jai-config.json</D:href>
    <D:propstat>
      <D:prop><D:getcontentlength>1200</D:getcontentlength></D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/files/u/jai/jai-config.111.json</D:href>
    <D:propstat>
      <D:prop><D:getcontentlength>900</D:getcontentlength></D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/files/u/jai/readme.txt</D:href>
    <D:propstat>
      <D:prop><D:getcontentlength>10</D:getcontentlength></D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
</D:multistatus>"#;
        let items = parse_multistatus(xml).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].name, "jai-config.json");
        assert_eq!(items[0].size, Some(1200));
        assert_eq!(items[1].name, "jai-config.111.json");
        assert_eq!(items[1].href, "/dav/files/u/jai/jai-config.111.json");
        assert_eq!(items[2].name, "readme.txt");
        // 垃圾输入：不 panic，解析结果为空（quick-xml 对不完整 XML 宽容）
        assert_eq!(parse_multistatus("<unclosed").unwrap(), Vec::new());
    }

    #[test]
    fn backup_href_validates_name() {
        let cfg = mk_cfg("https://dav.example.com/", "u", "jai/backups");
        assert_eq!(
            backup_href(&cfg, "jai-config.5.json").unwrap(),
            "https://dav.example.com/jai/backups/jai-config.5.json"
        );
        // 当前配置名 / 路径穿越 / 无关文件一律拒绝
        assert!(backup_href(&cfg, "jai-config.json").is_err());
        assert!(backup_href(&cfg, "../evil.json").is_err());
        assert!(backup_href(&cfg, "readme.txt").is_err());
    }

    #[test]
    fn push_diff_detail_lists_both_sides() {
        let local = r#"{"format":"jai-export/v1","providers":[{"id":"p1","name":"A","base_url":"https://a/v1"},{"id":"p3","name":"C","base_url":"https://c/v1"}],"models":[{"id":"m1","providerId":"p1","modelName":"gpt-4o"}]}"#;
        let remote = r#"{"format":"jai-export/v1","providers":[{"id":"p1","name":"A","base_url":"https://a/v1"},{"id":"p2","name":"B","base_url":"https://b/v1"}],"models":[{"id":"m1","providerId":"p1","modelName":"gpt-4o"},{"id":"m2","providerId":"p2","modelName":"claude-3"}]}"#;
        let d = push_diff_detail(local, Some(remote)).unwrap();
        assert!(d.remote_exists);
        // 远端独有：供应商 B、模型 claude-3（会被覆盖丢失）→ 阻断
        assert_eq!(
            d.remote_only_providers,
            vec![("B".to_string(), "https://b/v1".to_string())]
        );
        assert_eq!(
            d.remote_only_models,
            vec![("p2".to_string(), "claude-3".to_string())]
        );
        assert!(d.blocks());
        // 本地独有：供应商 C（推送会新增到远端）
        assert_eq!(
            d.local_only_providers,
            vec![("C".to_string(), "https://c/v1".to_string())]
        );
        assert!(d.local_only_models.is_empty());
        // 无远端：不阻断
        let none = push_diff_detail(local, None).unwrap();
        assert!(!none.remote_exists);
        assert!(!none.blocks());
        // 等价：两侧都空
        let eq = push_diff_detail(local, Some(local)).unwrap();
        assert!(eq.remote_only_providers.is_empty());
        assert!(eq.local_only_models.is_empty());
        assert!(!eq.blocks());
    }

    #[test]
    fn push_diff_detects_remote_only_content() {
        let local = r#"{"format":"jai-export/v1","providers":[{"id":"p1","name":"A","base_url":"https://a/v1"}],"models":[{"id":"m1","providerId":"p1","modelName":"gpt-4o"}]}"#;
        let remote = r#"{"format":"jai-export/v1","providers":[{"id":"p1","name":"A","base_url":"https://a/v1"},{"id":"p2","name":"B","base_url":"https://b/v1"}],"models":[{"id":"m1","providerId":"p1","modelName":"gpt-4o"},{"id":"m2","providerId":"p2","modelName":"claude-3"}]}"#;
        // 远端多出 1 供应商 + 1 模型 → 阻断
        let diff = push_diff(local, Some(remote)).unwrap();
        assert!(diff.remote_exists);
        assert_eq!(diff.remote_only_providers, 1);
        assert_eq!(diff.remote_only_models, 1);
        assert!(diff.blocks());
        // 远端不存在 → 不阻断
        let none = push_diff(local, None).unwrap();
        assert!(!none.remote_exists);
        assert!(!none.blocks());
        // 本地与远端等价 → 不阻断
        let eq = push_diff(local, Some(local)).unwrap();
        assert_eq!(eq.remote_only_providers, 0);
        assert!(!eq.blocks());
        // 本地 ⊇ 远端 → 不阻断
        let local_full = r#"{"format":"jai-export/v1","providers":[{"id":"p1","name":"A","base_url":"https://a/v1"},{"id":"p2","name":"B","base_url":"https://b/v1"}],"models":[{"id":"m1","providerId":"p1","modelName":"gpt-4o"},{"id":"m2","providerId":"p2","modelName":"claude-3"}]}"#;
        let less = push_diff(local_full, Some(remote)).unwrap();
        assert!(!less.blocks(), "本地更全应放行: {less:?}");
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
