//! JAI 桌面壳（Tauri 2）：网关监督进程 + 系统托盘 + IPC 命令。
//!
//! 稳定性基线落点：
//! - §5-2 超时三件套：随 M1 业务代理落地（gateway-core::server::proxy）
//! - §5-6 进程看门狗：本文件的 restart 循环
//! - 启动即应用 SQLite 迁移，失败即启动中止（storage §4 早拦截原则）
//!
//! M1 新增：providers/models/网关密钥/日志/导出/CORS 命令、
//! 密钥环生命周期（先写凭据后落库+回滚）、模型发现入库。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use gateway_core::codec::Family;
use gateway_core::discover::discover_models;
use gateway_core::netcfg::{self, ProxyConfig};
use gateway_core::server::{self, GatewayCtx};
use gateway_core::skills::SkillDraft;
use gateway_core::store::{
    self, import, logs, Db, GatewayKeyRow, McpServerRow, ModelRow, ProviderRow, SkillRow,
};
use gateway_core::sync::{self, WebDavConfig};
use gateway_core::vault;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_notification::NotificationExt;

// ---------------------------------------------------------------- 状态模型

#[derive(Debug, Clone, Serialize)]
pub struct GwStatus {
    pub running: bool,
    pub port: u16,
    pub restarts: u64,
}

struct SupervisorInner {
    stop_tx: tokio::sync::watch::Sender<bool>,
}

/// 托盘菜单句柄（运行态刷新文案/可用性用）
struct TrayHandles {
    status_item: MenuItem<tauri::Wry>,
    start_item: MenuItem<tauri::Wry>,
    stop_item: MenuItem<tauri::Wry>,
}

/// WebDAV 自动推送：变更/定时触发，本机为准直接覆盖远端。
#[derive(Clone)]
pub struct AutopushHub {
    /// 配置变更计数器（watch 通道天然合并突发变更）
    tx: tokio::sync::watch::Sender<u64>,
    /// 与手动推/拉互斥，避免并发写远端
    pub push_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    /// 手动拉取前后短暂置位，抑制变更触发的自动推送（防回声）
    pub suppress: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// 最近一次自动推送结果
    last: std::sync::Arc<tokio::sync::Mutex<Option<AutoPushStatus>>>,
    /// 最近一次自动拉取结果
    last_pull: std::sync::Arc<tokio::sync::Mutex<Option<AutoPushStatus>>>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoPushStatus {
    pub at_ms: u64,
    pub ok: bool,
    pub message: String,
}

impl AutopushHub {
    fn new() -> Self {
        let (tx, _rx) = tokio::sync::watch::channel(0);
        Self {
            tx,
            push_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            suppress: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            last: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
            last_pull: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// 供应商/模型配置发生增删改时调用；自动推送防抖后执行一次
    pub fn notify_change(&self) {
        let next = *self.tx.borrow() + 1;
        let _ = self.tx.send(next);
    }
}

/// IPC 命令共享的业务核心（数据库 / 日志句柄 / HTTP 客户端）
#[derive(Clone)]
pub struct AppCore {
    pub db: Db,
    pub logs: logs::LogHandle,
    pub http: reqwest::Client,
    pub db_path: String,
    pub autopush: AutopushHub,
}

struct GatewayState {
    preferred_port: u16,
    running: Arc<AtomicBool>,
    stop_flag: Arc<AtomicBool>,
    port: Arc<AtomicU16>,
    restarts: Arc<AtomicU64>,
    supervisor: Mutex<Option<SupervisorInner>>,
    tray: Mutex<Option<TrayHandles>>,
}

impl GatewayState {
    fn status(&self) -> GwStatus {
        GwStatus {
            running: self.running.load(Ordering::SeqCst),
            port: self.port.load(Ordering::SeqCst),
            restarts: self.restarts.load(Ordering::SeqCst),
        }
    }
}

// ---------------------------------------------------------------- DTO

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDto {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub family: String,
    pub enabled: bool,
    pub priority: i64,
    pub weight: i64,
    pub extra_headers: Option<String>,
    pub website: Option<String>,
    pub last_ok_at: Option<i64>,
    pub last_err_at: Option<i64>,
    pub last_err_msg: Option<String>,
    pub has_key: bool,
}

fn to_dto(p: ProviderRow) -> ProviderDto {
    ProviderDto {
        id: p.id.clone(),
        name: p.name,
        base_url: p.base_url,
        family: p.family,
        enabled: p.enabled,
        priority: p.priority,
        weight: p.weight,
        extra_headers: p.extra_headers,
        website: p.website,
        last_ok_at: p.last_ok_at,
        last_err_at: p.last_err_at,
        last_err_msg: p.last_err_msg,
        has_key: p.api_key.is_some(),
    }
}

// ---------------------------------------------------------------- 供应商命令

#[tauri::command]
async fn provider_list(core: State<'_, AppCore>) -> Result<Vec<ProviderDto>, String> {
    let db = core.db.clone();
    let rows = tokio::task::spawn_blocking(move || {
        db.with(store::provider_list).map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;

    Ok(rows.into_iter().map(to_dto).collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewProvider {
    pub name: String,
    pub base_url: String,
    pub family: String,
    #[serde(default = "default_priority")]
    pub priority: i64,
    #[serde(default = "default_weight")]
    pub weight: i64,
    #[serde(default)]
    pub extra_headers: Option<String>,
    pub api_key: String,
    /// 官网地址（可空）
    #[serde(default)]
    pub website: Option<String>,
}

fn default_priority() -> i64 {
    100
}

fn default_weight() -> i64 {
    1
}

#[tauri::command]
async fn provider_create(
    core: State<'_, AppCore>,
    input: NewProvider,
) -> Result<ProviderDto, String> {
    validate_family(&input.family)?;
    if input.api_key.trim().is_empty() {
        return Err("API Key 不能为空".into());
    }
    if input.name.trim().is_empty() {
        return Err("名称不能为空".into());
    }

    let id = uuid::Uuid::now_v7().to_string();
    let row = ProviderRow {
        id: id.clone(),
        name: input.name.trim().to_string(),
        base_url: normalize_base(&input.base_url),
        family: input.family,
        enabled: true,
        priority: input.priority,
        weight: input.weight,
        extra_headers: input.extra_headers.filter(|s| !s.trim().is_empty()),
        api_key: Some(input.api_key.trim().to_string()),
        website: input
            .website
            .map(|w| w.trim().to_string())
            .filter(|s| !s.is_empty()),
        last_ok_at: None,
        last_err_at: None,
        last_err_msg: None,
        created_at: store::now_ms(),
        updated_at: store::now_ms(),
    };
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || db.with(|c| store::provider_insert(c, &row)))
        .await
        .map_err(join_err)?
        .map_err(|e| format!("数据库写入失败: {e}"))?;

    let db2 = core.db.clone();
    let created = tokio::task::spawn_blocking(move || {
        db2.with(|c| store::provider_get(c, &id))
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "创建后读取失败".to_string())
    })
    .await
    .map_err(join_err)??;

    core.autopush.notify_change();
    Ok(to_dto(created))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProviderInput {
    pub id: String,
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub priority: Option<i64>,
    pub weight: Option<i64>,
    /// 外层 Some 表示要动这个字段；内层 None 表示清空
    pub extra_headers: Option<Option<String>>,
    /// Some(非空) 覆盖密钥；Some("")/None 不动
    pub api_key: Option<String>,
    /// Some 覆盖官网（空串清空）；None 不动
    pub website: Option<String>,
}

#[tauri::command]
async fn provider_update(
    core: State<'_, AppCore>,
    input: UpdateProviderInput,
) -> Result<(), String> {
    let new_key = input
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let new_website = input
        .website
        .as_deref()
        .map(str::trim)
        .map(|s| (!s.is_empty()).then(|| s.to_string()));

    let normalized = input.base_url.as_deref().map(normalize_base);
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| {
            if let Some(k) = &new_key {
                store::provider_set_api_key(c, &input.id, Some(k))?;
            }
            if let Some(w) = &new_website {
                store::provider_set_website(c, &input.id, w.as_deref())?;
            }
            store::provider_update_fields(
                c,
                &input.id,
                input.name.as_deref(),
                normalized.as_deref(),
                input.priority,
                input.weight,
                input.extra_headers.as_ref().map(|o| o.as_deref()),
            )
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;

    core.autopush.notify_change();
    Ok(())
}

#[tauri::command]
async fn provider_delete(core: State<'_, AppCore>, id: String) -> Result<(), String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::provider_delete(c, &id))
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;

    core.autopush.notify_change();
    Ok(())
}

#[tauri::command]
async fn provider_set_enabled(
    core: State<'_, AppCore>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::provider_set_enabled(c, &id, enabled))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;

    core.autopush.notify_change();
    Ok(())
}

/// 测试连接：跑一次模型发现。HTTP 200 即视为连通
/// （部分中转站隐藏 /models，0 个模型也算通；错误信息给出排查提示）。
#[tauri::command]
async fn provider_test(core: State<'_, AppCore>, id: String) -> Result<String, String> {
    let row = fetch_provider(&core.db, &id).await?;
    match probe_provider(&core, &row).await {
        Ok(n) => {
            let db = core.db.clone();
            tokio::task::spawn_blocking(move || {
                let _ = db.with(|c| store::provider_mark_ok(c, &id));
            });
            Ok(format!("连接成功 · 发现 {n} 个模型"))
        }
        Err(msg) => {
            let db = core.db.clone();
            let m2 = msg.clone();
            tokio::task::spawn_blocking(move || {
                let _ = db.with(|c| store::provider_mark_err(c, &id, &m2));
            });
            Err(msg)
        }
    }
}

// ---------------------------------------------------------------- 草稿测试

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTestDraftInput {
    pub base_url: String,
    pub family: String,
    pub api_key: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTestDraftResult {
    pub ok: bool,
    pub count: usize,
    pub model_names: Vec<String>,
}

/// 创建供应商弹框里的“测试连接”按钮：用草稿参数直接拉一次模型列表，
/// 不写库、不落凭据。
#[tauri::command]
async fn provider_test_draft(
    core: State<'_, AppCore>,
    input: ProviderTestDraftInput,
) -> Result<ProviderTestDraftResult, String> {
    let secret = if input.api_key.trim().is_empty() {
        None
    } else {
        Some(input.api_key.clone())
    };
    let models = discover_models(
        &core.http,
        &input.family,
        &normalize_base(&input.base_url),
        secret.as_deref(),
    )
    .await?;

    Ok(ProviderTestDraftResult {
        ok: true,
        count: models.len(),
        model_names: models.into_iter().map(|m| m.id).collect(),
    })
}

// ---------------------------------------------------------------- 模型命令

/// 自动发现 + 入库：已存在模型保留用户调过的默认值；
/// 新模型用快照值或保守缺省（128k / 4096）。
#[tauri::command]
async fn provider_discover_models(
    core: State<'_, AppCore>,
    id: String,
) -> Result<(usize, usize), String> {
    let row = fetch_provider(&core.db, &id).await?;

    let models = discover_models(
        &core.http,
        &row.family,
        &row.base_url,
        row.api_key.as_deref(),
    )
    .await?;

    let existing: std::collections::HashMap<String, ()> = {
        let db = core.db.clone();
        let pid = id.clone();
        tokio::task::spawn_blocking(move || {
            db.with(|c| store::model_list_by_provider(c, &pid))
                .map(|v| v.into_iter().map(|m| (m.model_name, ())).collect())
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(join_err)??
    };

    let mut added = 0usize;
    for m in &models {
        if existing.contains_key(&m.id) {
            continue; // 已有：绝不动用户配置过的默认值
        }
        let (ctx_w, out_w) = store::snapshot::lookup(&m.id).unwrap_or((128000, 4096));
        let db = core.db.clone();
        let pid = id.clone();
        let name = m.id.clone();
        tokio::task::spawn_blocking(move || {
            db.with(|c| store::model_upsert(c, &pid, &name, Some(ctx_w), out_w))
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(join_err)??;
        added += 1;
    }
    Ok((models.len(), added))
}

#[tauri::command]
async fn model_list(
    core: State<'_, AppCore>,
    provider_id: String,
) -> Result<Vec<ModelRow>, String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::model_list_by_provider(c, &provider_id))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelLimitsInput {
    pub model_id: String,
    /// null 表示回到「默认」
    pub context_window: Option<i64>,
    pub max_output_tokens: i64,
}

#[tauri::command]
async fn model_set_limits(core: State<'_, AppCore>, input: ModelLimitsInput) -> Result<(), String> {
    if input.max_output_tokens <= 0 {
        return Err("最大输出必须 > 0".into());
    }
    if let Some(w) = input.context_window {
        if w <= 0 {
            return Err("上下文窗口必须 > 0".into());
        }
    }
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| {
            store::model_update_limits(
                c,
                &input.model_id,
                input.context_window,
                input.max_output_tokens,
            )
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;

    core.autopush.notify_change();
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelAliasInput {
    pub model_id: String,
    /// null 表示清除映射，上游使用同名模型
    pub upstream_model_id: Option<String>,
}

#[tauri::command]
async fn model_set_alias(core: State<'_, AppCore>, input: ModelAliasInput) -> Result<(), String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| {
            store::model_set_upstream(c, &input.model_id, input.upstream_model_id.as_deref())
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;

    core.autopush.notify_change();
    Ok(())
}

#[tauri::command]
async fn model_toggle(
    core: State<'_, AppCore>,
    model_id: String,
    enabled: bool,
) -> Result<(), String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::model_toggle(c, &model_id, enabled))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;

    core.autopush.notify_change();
    Ok(())
}

// ---------------------------------------------------------------- 网关密钥

fn gen_gateway_key() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    let body: String = (0..28)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect();
    format!("sk-jai-{body}")
}

/// 首次启动自举一个活跃密钥（storage §6-3）。
fn ensure_gateway_key(core: &AppCore) -> Result<(), String> {
    core.db
        .with(|c| {
            if store::gw_key_active(c)?.is_none() {
                store::gw_key_rotate(c, &gen_gateway_key(), Some("初始"))?;
            }
            Ok::<_, store::StoreError>(())
        })
        .map_err(|e| e.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayKeyInfo {
    pub prefix: String,
    pub label: Option<String>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    /// 仅 reveal/regenerate 携带全文；info 恒为空串
    pub key: String,
}

fn key_info(k: GatewayKeyRow, with_full: bool) -> GatewayKeyInfo {
    GatewayKeyInfo {
        prefix: k.prefix,
        label: k.label,
        created_at: k.created_at,
        last_used_at: k.last_used_at,
        key: if with_full { k.key } else { String::new() },
    }
}

#[tauri::command]
async fn gateway_key_info(core: State<'_, AppCore>) -> Result<Option<GatewayKeyInfo>, String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with_any(|c| match store::gw_key_active(c) {
            Ok(k) => Ok(k.map(|row| key_info(row, false))),
            Err(e) => Err(e.to_string()),
        })
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
async fn gateway_key_reveal(core: State<'_, AppCore>) -> Result<GatewayKeyInfo, String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with_any(|c| match store::gw_key_active(c) {
            Ok(Some(k)) => Ok(key_info(k, true)),
            Ok(None) => Err("无活跃网关密钥".to_string()),
            Err(e) => Err(e.to_string()),
        })
    })
    .await
    .map_err(join_err)?
}

/// 轮换并返回新全量密钥（UI 弹窗一次性展示旧密钥即刻失效）。
#[tauri::command]
async fn gateway_key_regenerate(core: State<'_, AppCore>) -> Result<GatewayKeyInfo, String> {
    let new_key = gen_gateway_key();
    let nk = new_key.clone();
    let db = core.db.clone();
    let row = tokio::task::spawn_blocking(move || {
        db.with(|c| store::gw_key_rotate(c, &nk, Some("手动轮换")))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;
    // 新密钥随 WebDAV 同步：触发自动推送防抖通知（未配置 WebDAV 时无副作用）
    core.autopush.notify_change();
    Ok(GatewayKeyInfo {
        prefix: row.prefix,
        label: row.label,
        created_at: row.created_at,
        last_used_at: None,
        key: new_key,
    })
}

// ---------------------------------------------------------------- 日志 / 导出 / 设置

#[tauri::command]
async fn logs_recent(
    core: State<'_, AppCore>,
    limit: i64,
) -> Result<Vec<logs::LogRowView>, String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || logs::logs_recent(&db, limit).map_err(|e| e.to_string()))
        .await
        .map_err(join_err)?
}

#[tauri::command]
async fn stats_usage(
    core: State<'_, AppCore>,
    days: i64,
) -> Result<Vec<logs::UsageStatRow>, String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || logs::usage_stats(&db, days).map_err(|e| e.to_string()))
        .await
        .map_err(join_err)?
}

/// 导出 JSON（storage §8 语义：meta+providers+models，零敏感字段——
/// 构建逻辑在 gateway-core::store::export，保证单测覆盖「全文无敏感串」）。
#[tauri::command]
async fn export_config_json(core: State<'_, AppCore>) -> Result<String, String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with_any(|c| store::export::build_export_json(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)?
}

/// 导出到数据目录并返回文件路径（供“打开所在目录”使用）。
#[tauri::command]
async fn export_config_to_file(core: State<'_, AppCore>) -> Result<String, String> {
    let json = export_config_json(core.clone()).await?;
    let db_path = std::path::Path::new(&core.db_path);
    let dir = db_path.parent().unwrap_or(std::path::Path::new("."));
    let path = dir.join(format!("jai-export-{}.json", store::now_ms()));
    std::fs::write(&path, json).map_err(|e| format!("写入导出文件失败: {e}"))?;
    Ok(path.to_string_lossy().into_owned())
}

/// 在系统文件管理器中显示该文件（macOS Finder / Windows Explorer）。
#[tauri::command]
fn reveal_in_folder(path: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("打开 Finder 失败: {e}"))?;
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg("/select,")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("打开资源管理器失败: {e}"))?;
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = &path;
        Err("当前平台暂不支持打开所在目录".into())
    }
}

// ---------------------------------------------------------------- M7：导入 + WebDAV

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavConfigDto {
    pub url: String,
    pub username: String,
    pub directory: String,
    pub auto_push_enabled: bool,
    pub auto_push_interval_min: u32,
    /// 定时自动拉取总开关（与自动推送共用间隔，按 exportedAt 时间戳 last-write-wins）
    pub auto_pull_enabled: bool,
    /// 明文回显（0006 起密码入库并随同步携带，与网关 Key 同级安全模型）
    pub password: Option<String>,
}

impl From<WebDavConfig> for WebDavConfigDto {
    fn from(c: WebDavConfig) -> Self {
        Self {
            url: c.url,
            username: c.username,
            directory: c.directory,
            auto_push_enabled: c.auto_push_enabled,
            auto_push_interval_min: c.auto_push_interval_min,
            auto_pull_enabled: c.auto_pull_enabled,
            password: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavConfigInput {
    pub url: String,
    pub username: String,
    /// 仅 webdav_config_set 使用；测试连接不传，缺省视为根目录
    #[serde(default)]
    pub directory: String,
    /// Some(非空) 覆盖密码；None/空串保持原密码
    pub password: Option<String>,
    /// 自动推送开关；None 保持原值
    pub auto_push_enabled: Option<bool>,
    /// 自动推送间隔分钟；None 保持原值
    pub auto_push_interval_min: Option<u32>,
    /// 自动拉取开关；None 保持原值
    pub auto_pull_enabled: Option<bool>,
}

#[cfg(test)]
mod webdav_input_tests {
    use super::*;

    /// 前端「测试连接」只传 url/username/password，directory 缺省必须能反序列化。
    #[test]
    fn webdav_test_input_without_directory() {
        let v = serde_json::json!({
            "url": "http://jn_file.88933.vip/",
            "username": "jiangnan",
            "password": null
        });
        let input: WebDavConfigInput =
            serde_json::from_value(v).expect("缺 directory 也应反序列化成功");
        assert_eq!(input.directory, "");
        assert_eq!(input.url, "http://jn_file.88933.vip/");
    }
}

#[tauri::command]
async fn config_import(
    core: State<'_, AppCore>,
    text: String,
    strict: Option<bool>,
) -> Result<import::ImportReport, String> {
    let db = core.db.clone();
    let strict = strict.unwrap_or(false);
    let out = tokio::task::spawn_blocking(move || {
        db.with_any(|c| import::apply_import(c, &text, strict).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)??;

    core.autopush.notify_change();
    Ok(out)
}

#[tauri::command]
async fn webdav_config_get(core: State<'_, AppCore>) -> Result<Option<WebDavConfigDto>, String> {
    let db = core.db.clone();
    let (cfg, password): (Option<WebDavConfig>, Option<String>) =
        tokio::task::spawn_blocking(move || {
            db.with_any(|c| {
                let cfg = sync::config_get(c).map_err(|e| e.to_string())?;
                let password = store::meta_get(c, "webdav_password")
                    .map_err(|e| e.to_string())?
                    .filter(|s| !s.is_empty());
                Ok::<_, String>((cfg, password))
            })
        })
        .await
        .map_err(join_err)??;
    Ok(cfg.map(|c| {
        let mut dto = WebDavConfigDto::from(c);
        dto.password = password;
        dto
    }))
}

#[tauri::command]
async fn webdav_config_set(
    core: State<'_, AppCore>,
    input: WebDavConfigInput,
) -> Result<(), String> {
    // 密码明文入 meta（0006 起与网关 key/MCP env 同级安全模型），随导出同步
    if let Some(pw) = input.password.as_deref().filter(|s| !s.trim().is_empty()) {
        let db = core.db.clone();
        let pw = pw.trim().to_string();
        tokio::task::spawn_blocking(move || {
            db.with(|c| store::meta_set(c, "webdav_password", &pw))
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(join_err)??;
    }
    let db = core.db.clone();
    let old: Option<WebDavConfig> = tokio::task::spawn_blocking(move || {
        db.with_any(|c| sync::config_get(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)??;
    let old = old.unwrap_or_default();
    let cfg = WebDavConfig {
        url: normalize_base(&input.url),
        username: input.username.trim().to_string(),
        directory: input.directory.trim().to_string(),
        auto_push_enabled: input.auto_push_enabled.unwrap_or(old.auto_push_enabled),
        auto_push_interval_min: input
            .auto_push_interval_min
            .unwrap_or(old.auto_push_interval_min),
        auto_pull_enabled: input.auto_pull_enabled.unwrap_or(old.auto_pull_enabled),
    };
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| sync::config_set(c, &cfg))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
async fn webdav_test(core: State<'_, AppCore>, input: WebDavConfigInput) -> Result<String, String> {
    let username = input.username.trim().to_string();
    // 留空表示测试已保存的密码；有输入则测未保存的新密码
    let password = match input
        .password
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(pw) => pw.to_string(),
        None => get_webdav_password(&core).await?,
    };
    // PROPFIND 带认证探测（sync::probe）——OPTIONS 常被服务器匿名放行，
    // 无法证明凭据有效（DUFS 实测 OPTIONS 免认证 200、GET 错误凭据 401）
    let cfg = WebDavConfig {
        url: normalize_base(&input.url),
        username,
        directory: input.directory.trim().to_string(),
        auto_push_enabled: false,
        auto_push_interval_min: 60,
        auto_pull_enabled: false,
    };
    sync::probe(&core.http, &cfg, &password).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavPreview {
    pub remote_providers: usize,
    pub remote_models: usize,
    pub local_providers: usize,
    pub local_models: usize,
    pub will_overwrite: bool,
    pub message: String,
}

/// 预览 WebDAV 拉取将带来的变更（只读，不落库）。
#[tauri::command]
async fn webdav_preview(core: State<'_, AppCore>) -> Result<WebDavPreview, String> {
    let cfg = get_webdav_config(&core).await?;
    let password = get_webdav_password(&core).await?;
    let remote_text = sync::pull(&core.http, &cfg, &password).await?;
    let local_text = export_config_json(core.clone()).await?;

    let (rp, rm) = sync::export_counts(&remote_text)?;
    let (lp, lm) = sync::export_counts(&local_text)?;
    let changed = rp != lp || rm != lm;
    Ok(WebDavPreview {
        remote_providers: rp,
        remote_models: rm,
        local_providers: lp,
        local_models: lm,
        will_overwrite: changed,
        message: if changed {
            format!(
                "远端 {rp} 个供应商/{rm} 个模型，本地 {lp} 个供应商/{lm} 个模型，拉取将覆盖本地"
            )
        } else {
            "远端与本地配置一致，无需变更".into()
        },
    })
}

/// 手动推送。`force` 为 true 时跳过推送前差异预警（用户已确认覆盖）。
///
/// 差异预警（T3）：远端有本机没有的供应商/模型时，首次调用返回可读错误提示，
/// 前端弹确认框后以 force=true 重试——防止多设备场景无意覆盖掉另一台设备刚加的内容。
#[tauri::command]
async fn webdav_push(core: State<'_, AppCore>, force: Option<bool>) -> Result<(), String> {
    // 与自动推送互斥：手动推送期间自动轮跳过
    let _guard = core.autopush.push_lock.lock().await;
    if !force.unwrap_or(false) {
        let cfg = get_webdav_config(&core).await?;
        let password = get_webdav_password(&core).await?;
        let db = core.db.clone();
        let local_text = tokio::task::spawn_blocking(move || {
            db.with_any(|c| store::export::build_export_json(c).map_err(|e| e.to_string()))
        })
        .await
        .map_err(join_err)??;
        let remote = sync::try_pull(&core.http, &cfg, &password).await?;
        let diff = sync::push_diff(&local_text, remote.as_deref()).map_err(|e| e.to_string())?;
        if diff.blocks() {
            return Err(format!(
                "远端有 {} 个供应商 / {} 个模型是本机没有的，推送将覆盖掉它们。如确认以本机为准，请再次点击「仍然推送」。",
                diff.remote_only_providers, diff.remote_only_models
            ));
        }
    }
    push_now(&core).await
}

/// 构建导出 JSON → 本地留存快照 → PUT 覆盖远端。手动命令与自动推送共用。
async fn push_now(core: &AppCore) -> Result<(), String> {
    let cfg = get_webdav_config(core).await?;
    let password = get_webdav_password(core).await?;
    let db = core.db.clone();
    let export_text = tokio::task::spawn_blocking(move || {
        db.with_any(|c| store::export::build_export_json(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)??;

    // 推送前本地快照留存一份，用于误操作回退
    let db = core.db.clone();
    let snap = export_text.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| sync::snapshot_put(c, &snap))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;

    sync::push(&core.http, &cfg, &password, export_text.clone()).await?;

    // 记录本次推送的 exportedAt 作为自动拉取的新旧基线（防止刚推完又把自己拉回来）
    if let Some(ts) = sync::exported_at(&export_text) {
        let db = core.db.clone();
        tokio::task::spawn_blocking(move || {
            db.with(|c| sync::last_sync_put(c, ts))
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(join_err)??;
    }
    Ok(())
}

/// 自动推送专用入口：护栏检查（本地空配置不覆盖远端备份）→ 常规推送。
///
/// 2026-09 数据丢失修复：远端已有完整配置而本机为空时，自动推送会让位，
/// 原因写入上次自动推送状态（同步页可见）；手动推送不受此护栏限制。
async fn auto_push_guarded(core: &AppCore) -> Result<(), String> {
    let cfg = get_webdav_config(core).await?;
    let password = get_webdav_password(core).await?;
    let db = core.db.clone();
    let local_text = tokio::task::spawn_blocking(move || {
        db.with_any(|c| store::export::build_export_json(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)??;
    let remote = sync::try_pull(&core.http, &cfg, &password).await?;
    if let Some(reason) = sync::should_protect_remote(&local_text, remote.as_deref()) {
        return Err(reason);
    }
    push_now(core).await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavAutoPushStatusDto {
    pub at_ms: u64,
    pub ok: bool,
    pub message: String,
}

/// 最近一次自动推送结果（无记录返回 null）。
#[tauri::command]
async fn webdav_autopush_status(
    core: State<'_, AppCore>,
) -> Result<Option<WebDavAutoPushStatusDto>, String> {
    let last = core.autopush.last.lock().await.clone();
    Ok(last.map(|s| WebDavAutoPushStatusDto {
        at_ms: s.at_ms,
        ok: s.ok,
        message: s.message,
    }))
}

/// 最近一次自动拉取结果（无记录返回 null）。
#[tauri::command]
async fn webdav_autopull_status(
    core: State<'_, AppCore>,
) -> Result<Option<WebDavAutoPushStatusDto>, String> {
    let last = core.autopush.last_pull.lock().await.clone();
    Ok(last.map(|s| WebDavAutoPushStatusDto {
        at_ms: s.at_ms,
        ok: s.ok,
        message: s.message,
    }))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavSnapshotInfoDto {
    pub exists: bool,
    /// 快照导出时间（导出 JSON 的 exportedAt；解析失败为 None）
    pub at_ms: Option<i64>,
    pub chars: usize,
}

/// 本地「推送前快照」信息（同步页恢复入口的数据源）。
#[tauri::command]
async fn webdav_snapshot_info(core: State<'_, AppCore>) -> Result<WebDavSnapshotInfoDto, String> {
    let db = core.db.clone();
    let snap = tokio::task::spawn_blocking(move || {
        db.with_any(|c| sync::snapshot_get(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)??;
    let Some(text) = snap else {
        return Ok(WebDavSnapshotInfoDto {
            exists: false,
            at_ms: None,
            chars: 0,
        });
    };
    let at_ms = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("exportedAt").and_then(serde_json::Value::as_i64));
    Ok(WebDavSnapshotInfoDto {
        exists: true,
        at_ms,
        chars: text.chars().count(),
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavBackupDto {
    pub name: String,
    pub href: String,
    pub size: Option<u64>,
    pub ts: Option<i64>,
    /// 是否为当前配置文件（jai-config.json）
    pub is_current: bool,
}

/// 远端备份列表（PROPFIND Depth:1，仅当前配置与时间戳备份）。
#[tauri::command]
async fn webdav_backups_list(core: State<'_, AppCore>) -> Result<Vec<WebDavBackupDto>, String> {
    let cfg = get_webdav_config(&core).await?;
    let password = get_webdav_password(&core).await?;
    let items = sync::list_backups(&core.http, &cfg, &password).await?;
    Ok(items
        .into_iter()
        .map(|b| {
            let is_current = b.name == sync::CONFIG_FILE_NAME;
            WebDavBackupDto {
                ts: if is_current {
                    None
                } else {
                    sync::backup_timestamp(&b.name)
                },
                name: b.name,
                href: b.href,
                size: b.size,
                is_current,
            }
        })
        .collect())
}

/// 恢复指定远端备份到本地（GET 备份 → apply_import；与手动拉取同回声抑制，
/// 并把自动拉取基线对齐到远端当前版本，防止自动拉取立刻把恢复结果覆盖回去）。
#[tauri::command]
async fn webdav_backup_restore(
    core: State<'_, AppCore>,
    name: String,
) -> Result<import::ImportReport, String> {
    let _guard = core.autopush.push_lock.lock().await;
    let cfg = get_webdav_config(&core).await?;
    let password = get_webdav_password(&core).await?;
    let text = sync::fetch_backup(&core.http, &cfg, &password, &name).await?;
    let db = core.db.clone();
    let value = text.clone();
    let out = tokio::task::spawn_blocking(move || {
        db.with_any(|c| import::apply_import(c, &value, false).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)??;
    // 防回声：恢复后 40s 内抑制变更触发的自动推送
    core.autopush
        .suppress
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let hub = core.autopush.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(40)).await;
        hub.suppress
            .store(false, std::sync::atomic::Ordering::Relaxed);
    });
    // 自动拉取基线对齐远端当前 exportedAt（防止把刚恢复的旧版又拉回去）
    if let Ok(Some(remote_text)) = sync::try_pull(&core.http, &cfg, &password).await {
        if let Some(ts) = sync::exported_at(&remote_text) {
            let db = core.db.clone();
            tokio::task::spawn_blocking(move || {
                db.with(|c| sync::last_sync_put(c, ts))
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(join_err)??;
        }
    }
    Ok(out)
}

/// 删除指定远端备份（仅时间戳备份名；当前配置与无关文件拒绝）。
#[tauri::command]
async fn webdav_backup_delete(core: State<'_, AppCore>, name: String) -> Result<(), String> {
    let _guard = core.autopush.push_lock.lock().await;
    let cfg = get_webdav_config(&core).await?;
    let password = get_webdav_password(&core).await?;
    sync::delete_backup(&core.http, &cfg, &password, &name).await
}

/// 用「推送前快照」恢复本地配置（last-write-wins 误操作回退入口）。
///
/// 快照是上次推送前的完整导出（含供应商/模型/网关 Key/WebDAV 配置），
/// 通过既有 apply_import 合并落库；恢复后如开启自动推送，防抖会将其同步回远端。
#[tauri::command]
async fn webdav_snapshot_restore(core: State<'_, AppCore>) -> Result<import::ImportReport, String> {
    let db = core.db.clone();
    let snap = tokio::task::spawn_blocking(move || {
        db.with_any(|c| sync::snapshot_get(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)??
    .ok_or_else(|| "本地没有推送前快照（尚未执行过 WebDAV 推送）".to_string())?;
    let db = core.db.clone();
    let out = tokio::task::spawn_blocking(move || {
        db.with_any(|c| import::apply_import(c, &snap, false).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)??;
    core.autopush.notify_change();
    Ok(out)
}

/// WebDAV 自动同步循环：定时 tick 与变更通知合并；未启用时 30s 轮询配置等待开启。
/// - 变更触发：防抖 30s 后自动推送（带「空配置不覆盖远端」护栏）
/// - 定时触发：开启自动拉取则先按 exportedAt 时间戳拉取远端更新（last-write-wins，
///   空远端/不比本地新不拉），再按需自动推送
fn spawn_autopush(core: AppCore, app: AppHandle) {
    // setup 闭包不在 tokio runtime 上下文内，必须经 tauri 托管 runtime spawn
    tauri::async_runtime::spawn(async move {
        let mut rx = core.autopush.tx.subscribe();
        loop {
            let push_interval = current_autopush_interval(&core).await;
            let pull_interval = current_autopull_interval(&core).await;
            let wait = push_interval
                .or(pull_interval)
                .unwrap_or_else(|| std::time::Duration::from_secs(30));
            let change = tokio::select! {
                r = rx.changed() => {
                    if r.is_err() {
                        return; // hub 已随应用退出
                    }
                    true
                }
                _ = tokio::time::sleep(wait) => false,
            };
            if change {
                // 防抖：等 30s 合并突发变更
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                if core
                    .autopush
                    .suppress
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    continue;
                }
                // 期间被关闭则不推
                if current_autopush_interval(&core).await.is_none() {
                    continue;
                }
            } else if push_interval.is_none() && pull_interval.is_none() {
                // 定时醒来但均未启用：只是配置轮询
                continue;
            }
            // 抢不到锁（手动推/拉进行中）则跳过本轮
            let Ok(_guard) = core.autopush.push_lock.try_lock() else {
                eprintln!("[autosync] 手动同步进行中，跳过本轮");
                continue;
            };
            let now_ms = || {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64
            };
            // 定时唤醒且开启自动拉取：先拉远端更新（exportedAt last-write-wins）
            if !change && pull_interval.is_some() {
                let at = now_ms();
                let res = auto_pull_once(&core).await;
                let st = match &res {
                    Ok(()) => AutoPushStatus {
                        at_ms: at,
                        ok: true,
                        message: "自动拉取完成".into(),
                    },
                    Err(e) => AutoPushStatus {
                        at_ms: at,
                        ok: false,
                        message: e.clone(),
                    },
                };
                eprintln!(
                    "[autopull] {} {}",
                    if st.ok { "ok" } else { "err" },
                    st.message
                );
                if !st.ok {
                    notify_autosync(&app, "WebDAV 自动拉取失败".to_string(), st.message.clone());
                }
                *core.autopush.last_pull.lock().await = Some(st);
            }
            // 变更唤醒或定时且开启自动推送：推送本机配置（带护栏）
            if change || push_interval.is_some() {
                let at = now_ms();
                let res = auto_push_guarded(&core).await;
                let st = match &res {
                    Ok(()) => AutoPushStatus {
                        at_ms: at,
                        ok: true,
                        message: "自动推送成功".into(),
                    },
                    Err(e) => AutoPushStatus {
                        at_ms: at,
                        ok: false,
                        message: e.clone(),
                    },
                };
                eprintln!(
                    "[autopush] {} {}",
                    if st.ok { "ok" } else { "err" },
                    st.message
                );
                if !st.ok {
                    notify_autosync(&app, "WebDAV 自动推送失败".to_string(), st.message.clone());
                }
                *core.autopush.last.lock().await = Some(st);
            }
        }
    });
}

/// 当前自动推送间隔；未启用 / 未配置返回 None。
async fn current_autopush_interval(core: &AppCore) -> Option<std::time::Duration> {
    let db = core.db.clone();
    let res = tokio::task::spawn_blocking(move || {
        db.with_any(|c| sync::config_get(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err);
    let cfg: Option<WebDavConfig> = match res {
        Ok(Ok(c)) => c,
        _ => return None,
    };
    let cfg = cfg?;
    if !cfg.auto_push_enabled {
        return None;
    }
    Some(std::time::Duration::from_secs(
        u64::from(cfg.normalized_interval()) * 60,
    ))
}

/// 当前自动拉取间隔；未启用 / 未配置返回 None（与自动推送共用间隔分钟数）。
async fn current_autopull_interval(core: &AppCore) -> Option<std::time::Duration> {
    let db = core.db.clone();
    let res = tokio::task::spawn_blocking(move || {
        db.with_any(|c| sync::config_get(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err);
    let cfg: Option<WebDavConfig> = match res {
        Ok(Ok(c)) => c,
        _ => return None,
    };
    let cfg = cfg?;
    if !cfg.auto_pull_enabled {
        return None;
    }
    Some(std::time::Duration::from_secs(
        u64::from(cfg.normalized_interval()) * 60,
    ))
}

/// 自动拉取一次：远端非空且比上次成功同步更新时，导入远端配置并更新基线。
///
/// 与手动拉取相同的回声抑制（导入后 40s 内不触发变更自动推送），
/// 空远端 / 无时间戳 / 不比本地新 一律不动本地（与推送护栏对称防数据丢失）。
async fn auto_pull_once(core: &AppCore) -> Result<(), String> {
    let cfg = get_webdav_config(core).await?;
    let password = get_webdav_password(core).await?;
    let db = core.db.clone();
    let local_text = tokio::task::spawn_blocking(move || {
        db.with_any(|c| store::export::build_export_json(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)??;
    let Some(remote_text) = sync::try_pull(&core.http, &cfg, &password).await? else {
        return Ok(()); // 远端无文件：无事可拉
    };
    let db = core.db.clone();
    let last_sync = tokio::task::spawn_blocking(move || {
        db.with_any(|c| sync::last_sync_get(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)??;
    if !sync::should_pull(&local_text, &remote_text, last_sync) {
        return Ok(()); // 空远端 / 不比本地新 / 内容一致：无需拉取
    }
    let db = core.db.clone();
    let value = remote_text.clone();
    let _report = tokio::task::spawn_blocking(move || {
        db.with_any(|c| import::apply_import(c, &value, false).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)??;
    if let Some(ts) = sync::exported_at(&remote_text) {
        let db = core.db.clone();
        tokio::task::spawn_blocking(move || {
            db.with(|c| sync::last_sync_put(c, ts))
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(join_err)??;
    }
    // 防回声：拉取导入后 40s 内抑制变更触发的自动推送
    core.autopush
        .suppress
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let hub = core.autopush.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(40)).await;
        hub.suppress
            .store(false, std::sync::atomic::Ordering::Relaxed);
    });
    Ok(())
}

// ---------------------------------------------------------------- 供应商健康检查

/// 健康检查轮询间隔：固定 10 分钟/轮（代码常量，本期不做 UI 配置）。
const HEALTH_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(600);
/// 单个供应商探测硬超时（模型发现内部另有 20s HTTP 超时，此处兜底防悬挂）。
const HEALTH_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// 通知正文错误摘要最大长度。
const HEALTH_NOTIFY_SUMMARY_CHARS: usize = 140;

/// 单供应商探测核心（provider_test 与健康检查共用）：
/// 读凭据 → 拉模型列表。HTTP 200 即视为连通（0 个模型也算通）。
async fn probe_provider(core: &AppCore, row: &ProviderRow) -> Result<usize, String> {
    let models = discover_models(
        &core.http,
        &row.family,
        &row.base_url,
        row.api_key.as_deref(),
    )
    .await?;
    Ok(models.len())
}

/// 供应商定时健康检查循环：每 10 分钟顺序探测全部 enabled 供应商，
/// 结果写 last_ok_at/last_err_at/last_err_msg（列表健康徽章数据源，
/// 与真实流量 proxy 侧 mark 共用同一套 store 函数）。探测不改动
/// enabled，也不参与路由决策；仅在状态跃迁时发系统通知，
/// 应用启动后的首轮只记录不通知（避免每次开机弹一堆）。
fn spawn_health_check(core: AppCore, app: AppHandle) {
    // setup 闭包不在 tokio runtime 上下文内，必须经 tauri 托管 runtime spawn
    tauri::async_runtime::spawn(async move {
        let mut first_round = true;
        loop {
            if let Err(e) = health_round(&core, &app, first_round).await {
                eprintln!("[health] 本轮异常: {e}");
            }
            first_round = false;
            tokio::time::sleep(HEALTH_CHECK_INTERVAL).await;
        }
    });
}

/// 库内健康态：last_err_at 非空即处于失败态
/// （provider_mark_ok 会清空该列，provider_mark_err 会写入；真实流量同样落在这两列）。
fn provider_row_is_failing(row: &ProviderRow) -> bool {
    row.last_err_at.is_some()
}

/// 一轮健康检查：拉全量供应商，逐个（顺序）探测 enabled 的并按跃迁通知。
async fn health_round(core: &AppCore, app: &AppHandle, first_round: bool) -> Result<(), String> {
    let db = core.db.clone();
    let rows = tokio::task::spawn_blocking(move || {
        db.with(store::provider_list).map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;

    for row in rows.iter().filter(|r| r.enabled) {
        let was_failing = provider_row_is_failing(row);

        // 单点隔离：独立 spawn 任务探测，panic 只体现为 JoinError 不拖垮循环；
        // 超时的孤儿任务受 discover_models 内部 HTTP 超时约束，自行结束
        let handle = tokio::spawn({
            let core = core.clone();
            let row = row.clone();
            async move { probe_provider(&core, &row).await }
        });
        let res = match tokio::time::timeout(HEALTH_PROBE_TIMEOUT, handle).await {
            Ok(Ok(Ok(n))) => Ok(n),
            Ok(Ok(Err(msg))) => Err(msg),
            Ok(Err(join)) => Err(format!("探测任务异常: {join}")),
            Err(_) => Err(format!("探测超时（>{}s）", HEALTH_PROBE_TIMEOUT.as_secs())),
        };

        match res {
            Ok(n) => {
                let id = row.id.clone();
                let db = core.db.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = db.with(|c| store::provider_mark_ok(c, &id));
                })
                .await;
                if !first_round && was_failing {
                    eprintln!("[health] 恢复: {}（发现 {n} 个模型）", row.name);
                    notify_health(
                        app,
                        format!("供应商『{}』已恢复", row.name),
                        "健康检查：连接已恢复正常".to_string(),
                    );
                }
            }
            Err(msg) => {
                let id = row.id.clone();
                let m2 = msg.clone();
                let db = core.db.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = db.with(|c| store::provider_mark_err(c, &id, &m2));
                })
                .await;
                if !first_round && !was_failing {
                    eprintln!("[health] 失败: {} - {msg}", row.name);
                    notify_health(
                        app,
                        format!("供应商『{}』连接失败", row.name),
                        msg.chars().take(HEALTH_NOTIFY_SUMMARY_CHARS).collect(),
                    );
                }
            }
        }
    }
    Ok(())
}

/// 发送系统通知（失败只打日志，不影响探测流程）。
fn notify_health(app: &AppHandle, title: String, body: String) {
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        eprintln!("[health] 通知发送失败: {e}");
    }
}

/// 自动同步（推送/拉取）失败通知：标题 + 错误摘要（140 字符截断）。
fn notify_autosync(app: &AppHandle, title: String, body: String) {
    let summary: String = body.chars().take(HEALTH_NOTIFY_SUMMARY_CHARS).collect();
    if let Err(e) = app
        .notification()
        .builder()
        .title(title)
        .body(summary)
        .show()
    {
        eprintln!("[autosync] 通知发送失败: {e}");
    }
}

#[tauri::command]
async fn webdav_pull(core: State<'_, AppCore>) -> Result<import::ImportReport, String> {
    // 与自动推送互斥；拉取内容与远端一致，抑制后续防抖推送（防回声）
    let _guard = core.autopush.push_lock.lock().await;
    let cfg = get_webdav_config(&core).await?;
    let password = get_webdav_password(&core).await?;
    let text = sync::pull(&core.http, &cfg, &password).await?;
    let db = core.db.clone();
    let value = text.clone();
    let out = tokio::task::spawn_blocking(move || {
        db.with_any(|c| import::apply_import(c, &value, false).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)??;
    // 记录远端 exportedAt 作为自动拉取的新旧基线
    if let Some(ts) = sync::exported_at(&text) {
        let db = core.db.clone();
        tokio::task::spawn_blocking(move || {
            db.with(|c| sync::last_sync_put(c, ts))
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(join_err)??;
    }
    // 防抖窗口 30s，抑制再保持 40s
    core.autopush
        .suppress
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let hub = core.autopush.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(40)).await;
        hub.suppress
            .store(false, std::sync::atomic::Ordering::Relaxed);
    });
    Ok(out)
}

async fn get_webdav_config(core: &AppCore) -> Result<WebDavConfig, String> {
    let db = core.db.clone();
    let cfg: Option<WebDavConfig> = tokio::task::spawn_blocking(move || {
        db.with_any(|c| sync::config_get(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)??;
    cfg.ok_or_else(|| "尚未配置 WebDAV".to_string())
}

async fn get_webdav_password(core: &AppCore) -> Result<String, String> {
    let db = core.db.clone();
    // 密码明文存 meta（0006 起），随导出同步到 WebDAV
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::meta_get(c, "webdav_password"))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??
    .ok_or_else(|| "WebDAV 密码尚未录入，请在设置中保存".to_string())
}

// ---------------------------------------------------------------- MCP 管理

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInput {
    pub name: String,
    pub kind: String,
    pub command: Option<String>,
    pub args: Option<String>,
    pub url: Option<String>,
    pub env: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerUpdateInput {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub command: Option<String>,
    pub args: Option<String>,
    pub url: Option<String>,
    pub env: Option<String>,
}

#[tauri::command]
async fn mcp_list(core: State<'_, AppCore>) -> Result<Vec<McpServerRow>, String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with_any(|c| store::mcp_list(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
async fn mcp_create(
    core: State<'_, AppCore>,
    input: McpServerInput,
) -> Result<McpServerRow, String> {
    if input.name.trim().is_empty() {
        return Err("名称不能为空".into());
    }
    if !matches!(input.kind.as_str(), "stdio" | "sse" | "http") {
        return Err("kind 仅支持 stdio/sse/http".into());
    }
    let now = store::now_ms();
    let row = McpServerRow {
        id: uuid::Uuid::now_v7().to_string(),
        name: input.name.trim().to_string(),
        kind: input.kind,
        command: input.command.filter(|s| !s.trim().is_empty()),
        args: input.args.filter(|s| !s.trim().is_empty()),
        url: input.url.filter(|s| !s.trim().is_empty()),
        env: input.env.filter(|s| !s.trim().is_empty()),
        enabled: true,
        proxy_allowed: false, // 新建默认不开放代理转发，需显式开启
        created_at: now,
        updated_at: now,
    };
    let db = core.db.clone();
    let row2 = row.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::mcp_insert(c, &row2))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;
    Ok(row)
}

#[tauri::command]
async fn mcp_update(core: State<'_, AppCore>, input: McpServerUpdateInput) -> Result<(), String> {
    if input.name.trim().is_empty() {
        return Err("名称不能为空".into());
    }
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| {
            store::mcp_update(
                c,
                &input.id,
                input.name.trim(),
                &input.kind,
                input.command.as_deref().filter(|s| !s.trim().is_empty()),
                input.args.as_deref().filter(|s| !s.trim().is_empty()),
                input.url.as_deref().filter(|s| !s.trim().is_empty()),
                input.env.as_deref().filter(|s| !s.trim().is_empty()),
            )
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
async fn mcp_set_enabled(
    core: State<'_, AppCore>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::mcp_set_enabled(c, &id, enabled))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
async fn mcp_set_proxy_allowed(
    core: State<'_, AppCore>,
    id: String,
    allowed: bool,
) -> Result<(), String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::mcp_set_proxy_allowed(c, &id, allowed))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
async fn mcp_delete(core: State<'_, AppCore>, id: String) -> Result<(), String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::mcp_delete(c, &id))
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
async fn mcp_tools_list(
    core: State<'_, AppCore>,
    id: String,
) -> Result<Vec<gateway_core::mcp::McpTool>, String> {
    let row = fetch_mcp_server(&core.db, &id).await?;
    gateway_core::mcp::list_tools(&row).await
}

#[tauri::command]
async fn mcp_tools_call(
    core: State<'_, AppCore>,
    id: String,
    name: String,
    arguments: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let row = fetch_mcp_server(&core.db, &id).await?;
    gateway_core::mcp::call_tool(&row, &name, arguments).await
}

#[tauri::command]
async fn mcp_export_config(core: State<'_, AppCore>) -> Result<serde_json::Value, String> {
    let db = core.db.clone();
    let rows = tokio::task::spawn_blocking(move || {
        db.with_any(|c| store::mcp_list(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)??;
    let mut servers = serde_json::Map::new();
    for s in rows.into_iter().filter(|s| s.enabled) {
        let args: Vec<String> = s
            .args
            .as_deref()
            .map(|v| serde_json::from_str(v).unwrap_or_default())
            .unwrap_or_default();
        let env: serde_json::Map<String, serde_json::Value> = s
            .env
            .as_deref()
            .map(|v| serde_json::from_str(v).unwrap_or_default())
            .unwrap_or_default();
        let mut entry = serde_json::json!({
            "command": s.command,
            "args": args,
            "env": env,
        });
        if s.kind != "stdio" {
            entry["type"] = serde_json::json!(s.kind);
            entry["url"] = serde_json::json!(s.url);
        }
        servers.insert(s.name, entry);
    }
    Ok(serde_json::json!({ "mcpServers": servers }))
}

/// 导入 MCP 配置，自动识别三种格式：`{"mcpServers":{...}}` JSON（含裸对象）、
/// `codex mcp add ...` 命令行、`[mcp_servers.*]` TOML 片段。
/// 按 name 去重：已存在则更新，否则新建。返回导入报告。
#[tauri::command]
async fn mcp_import(core: State<'_, AppCore>, text: String) -> Result<serde_json::Value, String> {
    let entries = store::parse_mcp_import(&text)?;

    let now = store::now_ms();
    let db = core.db.clone();
    let (imported, updated, skipped) = tokio::task::spawn_blocking(move || {
        db.with(
            |c| -> Result<(usize, usize, Vec<String>), store::StoreError> {
                let existing = store::mcp_list(c)?;
                let mut imported = 0;
                let mut updated = 0;
                let mut skipped = Vec::new();
                for e in entries {
                    if let Some(reason) = e.skip_reason {
                        skipped.push(format!("{}: {reason}", e.name));
                        continue;
                    }
                    match existing.iter().find(|r| r.name == e.name) {
                        Some(row) => {
                            store::mcp_update(
                                c,
                                &row.id,
                                &e.name,
                                &e.kind,
                                e.command.as_deref(),
                                e.args.as_deref(),
                                e.url.as_deref(),
                                e.env.as_deref(),
                            )?;
                            updated += 1;
                        }
                        None => {
                            let row = McpServerRow {
                                id: uuid::Uuid::now_v7().to_string(),
                                name: e.name,
                                kind: e.kind,
                                command: e.command,
                                args: e.args,
                                url: e.url,
                                env: e.env,
                                enabled: true,
                                proxy_allowed: false,
                                created_at: now,
                                updated_at: now,
                            };
                            store::mcp_insert(c, &row)?;
                            imported += 1;
                        }
                    }
                }
                Ok((imported, updated, skipped))
            },
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;
    Ok(serde_json::json!({
        "imported": imported,
        "updated": updated,
        "skipped": skipped,
    }))
}

async fn fetch_mcp_server(db: &Db, id: &str) -> Result<McpServerRow, String> {
    let db = db.clone();
    let id = id.to_string();
    let row: Option<McpServerRow> = tokio::task::spawn_blocking(move || {
        db.with(|c| -> Result<Option<McpServerRow>, store::StoreError> {
            Ok(store::mcp_list(c)?.into_iter().find(|s| s.id == id))
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;
    row.ok_or_else(|| "MCP Server 不存在".to_string())
}

// ---------------------------------------------------------------- Skill 管理

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInput {
    pub name: String,
    pub description: String,
    pub content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillUpdateInput {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content: String,
}

#[tauri::command]
async fn skill_list(core: State<'_, AppCore>) -> Result<Vec<SkillRow>, String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with_any(|c| store::skill_list(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
async fn skill_create(core: State<'_, AppCore>, input: SkillInput) -> Result<SkillRow, String> {
    if input.name.trim().is_empty() {
        return Err("名称不能为空".into());
    }
    let now = store::now_ms();
    let row = SkillRow {
        id: uuid::Uuid::now_v7().to_string(),
        name: input.name.trim().to_string(),
        description: input.description.trim().to_string(),
        content: input.content,
        enabled: true,
        created_at: now,
        updated_at: now,
    };
    let db = core.db.clone();
    let row2 = row.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::skill_insert(c, &row2))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;
    Ok(row)
}

#[tauri::command]
async fn skill_update(core: State<'_, AppCore>, input: SkillUpdateInput) -> Result<(), String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| {
            store::skill_update(
                c,
                &input.id,
                input.name.trim(),
                input.description.trim(),
                &input.content,
            )
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
async fn skill_set_enabled(
    core: State<'_, AppCore>,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::skill_set_enabled(c, &id, enabled))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
async fn skill_delete(core: State<'_, AppCore>, id: String) -> Result<(), String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::skill_delete(c, &id))
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)?
}

/// 从 ZIP 导入技能：支持 skills.json 清单或 *.md/*.txt 文件包。
#[tauri::command]
async fn skill_import_zip(core: State<'_, AppCore>, data: Vec<u8>) -> Result<usize, String> {
    let drafts: Vec<SkillDraft> = gateway_core::skills::parse_skills_zip(&data)?;
    if drafts.is_empty() {
        return Err("ZIP 中没有可导入的技能".into());
    }

    let mut imported = 0usize;
    for d in &drafts {
        let now = store::now_ms();
        let row = SkillRow {
            id: uuid::Uuid::now_v7().to_string(),
            name: d.name.trim().to_string(),
            description: d.description.trim().to_string(),
            content: d.content.clone(),
            enabled: true,
            created_at: now,
            updated_at: now,
        };
        let db = core.db.clone();
        let row2 = row.clone();
        tokio::task::spawn_blocking(move || {
            db.with(|c| store::skill_insert(c, &row2))
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(join_err)??;
        imported += 1;
    }
    Ok(imported)
}

#[tauri::command]
async fn skill_export_markdown(core: State<'_, AppCore>) -> Result<String, String> {
    let db = core.db.clone();
    let rows = tokio::task::spawn_blocking(move || {
        db.with_any(|c| store::skill_list(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)??;
    let mut parts = Vec::new();
    for s in rows.into_iter().filter(|s| s.enabled) {
        parts.push(format!("## 技能：{}", s.name));
        if !s.description.is_empty() {
            parts.push(format!("描述：{}", s.description));
        }
        parts.push(s.content);
        parts.push(String::new());
    }
    Ok(parts.join(
        "

",
    ))
}

#[tauri::command]
async fn cors_allow_get(core: State<'_, AppCore>) -> Result<Vec<String>, String> {
    let raw = core
        .db
        .with(|c| store::meta_get(c, "cors_allow"))
        .map_err(|e| e.to_string())?;
    Ok(raw
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default())
}

#[tauri::command]
async fn cors_allow_set(core: State<'_, AppCore>, list: Vec<String>) -> Result<(), String> {
    let payload = serde_json::to_string(&list).map_err(|e| e.to_string())?;
    core.db
        .with(|c| store::meta_set(c, "cors_allow", &payload))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn families() -> Vec<&'static str> {
    vec!["openai_compat", "openai_responses", "anthropic", "gemini"]
}

// ---------------------------------------------------------------- 设置（M2）

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyConfigDto {
    pub enabled: bool,
    pub url: String,
    pub bypass: Vec<String>,
}

impl From<ProxyConfig> for ProxyConfigDto {
    fn from(c: ProxyConfig) -> Self {
        Self {
            enabled: c.enabled,
            url: c.url,
            bypass: c.bypass,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxySetInput {
    pub enabled: bool,
    pub url: String,
    pub bypass: Vec<String>,
}

/// 读取出站代理配置（D8）。
#[tauri::command]
async fn proxy_get(core: State<'_, AppCore>) -> Result<ProxyConfigDto, String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with_any(|c| netcfg::ProxyConfig::from_meta(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)?
    .map(ProxyConfigDto::from)
}

/// 保存出站代理配置。启用时先校验 URL（非法即拒绝）；保存后重启网关生效。
#[tauri::command]
async fn proxy_set(
    core: State<'_, AppCore>,
    input: ProxySetInput,
) -> Result<ProxyConfigDto, String> {
    let url = input.url.trim().to_string();
    if input.enabled {
        netcfg::validate_proxy_url(&url)?;
    }
    let cfg = ProxyConfig {
        enabled: input.enabled,
        url,
        bypass: input
            .bypass
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    };
    let db = core.db.clone();
    let dto = ProxyConfigDto::from(cfg.clone());
    tokio::task::spawn_blocking(move || db.with(|c| cfg.save(c)).map_err(|e| e.to_string()))
        .await
        .map_err(join_err)??;
    Ok(dto)
}

/// 用候选代理配置测试连通性（不落库）：探测 https://www.gstatic.com/generate_204。
#[tauri::command]
async fn proxy_test(input: ProxySetInput) -> Result<String, String> {
    let url = input.url.trim().to_string();
    if input.enabled {
        netcfg::validate_proxy_url(&url)?;
    }
    let cfg = ProxyConfig {
        enabled: input.enabled,
        url,
        bypass: input
            .bypass
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    };
    let client = netcfg::build_client(Some(&cfg), std::time::Duration::from_secs(8));
    let probe = "https://www.gstatic.com/generate_204";
    match client.get(probe).send().await {
        Ok(resp) if resp.status().is_success() => Ok("连接成功".into()),
        Ok(resp) => Err(format!("连接异常（HTTP {}）", resp.status())),
        Err(e) => Err(format!("连接失败: {e}")),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    /// 偏好监听端口；重启网关后生效（端口占用自动顺延）
    pub preferred_port: u16,
    /// 日志记录总开关（关闭后新请求不再落库）
    pub logs_enabled: bool,
    /// 日志保留天数（meta 可覆盖；默认 30）
    pub retention_days: i64,
    /// 日志行数上限（meta 可覆盖；默认 5 万）
    pub log_row_cap: i64,
}

#[tauri::command]
async fn settings_get(core: State<'_, AppCore>) -> Result<SettingsDto, String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with_any(|c| -> Result<SettingsDto, String> {
            let port = store::meta_get(c, "gateway_port")
                .map_err(|e| e.to_string())?
                .and_then(|s| s.parse::<u16>().ok())
                .unwrap_or(server::DEFAULT_PORT);
            let logs_enabled = store::meta_get(c, "logs_enabled")
                .map_err(|e| e.to_string())?
                .map(|s| s != "false")
                .unwrap_or(true);
            let retention_days = store::meta_get(c, "retention_days")
                .map_err(|e| e.to_string())?
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(store::retention::DEFAULT_RETENTION_DAYS);
            let log_row_cap = store::meta_get(c, "log_row_cap")
                .map_err(|e| e.to_string())?
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(store::retention::DEFAULT_LOG_ROW_CAP);
            Ok(SettingsDto {
                preferred_port: port,
                logs_enabled,
                retention_days,
                log_row_cap,
            })
        })
    })
    .await
    .map_err(join_err)?
}

/// 设置偏好端口。仅持久化；网关重启后生效（避免运行中静默换端口引发
/// 已连接客户端困惑，UI 会提示「重启网关生效」）。
#[tauri::command]
async fn settings_set_port(core: State<'_, AppCore>, port: u16) -> Result<u16, String> {
    if port == 0 {
        return Err("端口必须 ≥ 1".into());
    }
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::meta_set(c, "gateway_port", &port.to_string()))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;
    Ok(port)
}

/// 日志记录开关：写 meta + 实时切换 LogHandle（不重启即生效）。
/// 关闭不影响已入队事件。
#[tauri::command]
async fn settings_set_logs_enabled(
    core: State<'_, AppCore>,
    enabled: bool,
) -> Result<bool, String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::meta_set(c, "logs_enabled", if enabled { "true" } else { "false" }))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;
    core.logs.set_enabled(enabled);
    Ok(enabled)
}

/// 设置日志保留策略（天/行数）。由 retention 循环下次运行时读取。
#[tauri::command]
async fn settings_set_retention(
    core: State<'_, AppCore>,
    days: i64,
    row_cap: i64,
) -> Result<(), String> {
    if days < 1 || row_cap < 1000 {
        return Err("保留天数至少 1 天，行数至少 1000".into());
    }
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| {
            store::meta_set(c, "retention_days", &days.to_string())?;
            store::meta_set(c, "log_row_cap", &row_cap.to_string())?;
            Ok::<_, store::StoreError>(())
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)?
}

/// 读取本机环境变量（供应商表单“从环境变量导入 API Key”用）。
#[tauri::command]
fn read_env_var(name: String) -> Result<String, String> {
    std::env::var(&name).map_err(|e| format!("读取环境变量 {name} 失败: {e}"))
}

/// 检查端口是否被占用（用于设置页保存前提示）。
#[tauri::command]
fn port_in_use(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_err()
}

// ---------------------------------------------------------------- helpers

fn join_err(e: tokio::task::JoinError) -> String {
    format!("内部任务失败: {e}")
}

fn validate_family(f: &str) -> Result<(), String> {
    Family::from_db_str(f)
        .map(|_| ())
        .ok_or_else(|| format!("不支持的协议族: {f}"))
}

/// base_url 归一：去首尾空白与尾部斜杠。
fn normalize_base(s: &str) -> String {
    s.trim().trim_end_matches('/').to_string()
}

async fn fetch_provider(db: &Db, id: &str) -> Result<ProviderRow, String> {
    let db2 = db.clone();
    let id2 = id.to_string();
    tokio::task::spawn_blocking(move || {
        db2.with(|c| store::provider_get(c, &id2))
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "供应商不存在".to_string())
    })
    .await
    .map_err(join_err)?
}

// ---------------------------------------------------------------- 监督循环

enum StopKind {
    Manual,
    Crash(String),
}

/// 启动带看门狗的网关任务。
/// 端口顺延由 bind_with_fallback 保证；异常退出自动重启（§5-6）由此循环保证。
fn spawn_supervisor(
    app: &AppHandle,
    st: &GatewayState,
    core: &AppCore,
) -> Result<(), tauri::Error> {
    if st.supervisor.lock().unwrap().is_some() {
        return Ok(());
    }
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);

    st.stop_flag.store(false, Ordering::SeqCst);
    st.restarts.store(0, Ordering::SeqCst);

    let running = st.running.clone();
    let stop_flag = st.stop_flag.clone();
    let port_cell = st.port.clone();
    let restarts = st.restarts.clone();
    let preferred_port = st.preferred_port;
    let ctx = GatewayCtx::new(core.db.clone(), core.logs.clone());

    let app_handle = app.clone();
    // detached 任务：生命周期由 running 标志与 stop 信号管理，无需持有句柄
    tauri::async_runtime::spawn(async move {
        running.store(true, Ordering::SeqCst);
        loop {
            // 每轮重新绑定（上一轮可能刚释放端口）
            let (listener, actual_port) =
                match server::bind_with_fallback("127.0.0.1", preferred_port) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("[gateway] 绑定失败，停止监督循环: {e}");
                        let _ = app_handle.emit("gateway://event", format!("bind-failed:{e}"));
                        break;
                    }
                };
            port_cell.store(actual_port, Ordering::SeqCst);
            println!("[gateway] listening on 127.0.0.1:{actual_port}");

            let serve_ctx = ctx.clone();
            let serve = tokio::spawn(server::run_until_shutdown(
                listener,
                server::build_router(serve_ctx),
                stop_rx.clone(),
            ));
            tokio::pin!(serve);

            let kind = tokio::select! {
                _ = wait_stop(&mut stop_rx) => {
                    // 手动停机：等优雅关闭完成后才离开本轮，避免下一轮绑定撞旧监听
                    let io_res = serve.as_mut().await;
                    if let Err(e) = io_res {
                        eprintln!("[gateway] graceful shutdown io error: {e}");
                    }
                    StopKind::Manual
                }
                done = serve.as_mut() => match done {
                    Ok(Ok(())) => StopKind::Crash("serve 未因停机信号而退出".into()),
                    Ok(Err(e)) => StopKind::Crash(format!("serve io error: {e}")),
                    Err(e) => StopKind::Crash(format!("serve task joined err: {e}")),
                }
            };

            // stop_flag 兜底：即使 select 先落在 Crash 分支，用户已点停机则不重启
            match kind {
                StopKind::Manual => break,
                StopKind::Crash(reason) => {
                    if stop_flag.load(Ordering::SeqCst) {
                        break;
                    }
                    let n = restarts.fetch_add(1, Ordering::SeqCst) + 1;
                    eprintln!("[watchdog] 网关异常退出({reason})，第 {n} 次自动重启");
                    let _ = app_handle.emit("gateway://event", format!("restart:{n}"));
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
        running.store(false, Ordering::SeqCst);
        println!("[gateway] supervisor exited");
    });

    *st.supervisor.lock().unwrap() = Some(SupervisorInner { stop_tx });
    Ok(())
}

async fn wait_stop(rx: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if *rx.borrow_and_update() {
            return;
        }
        if rx.changed().await.is_err() {
            return; // sender dropped ⇒ 视为停机指令
        }
    }
}

fn request_stop(st: &GatewayState) {
    st.stop_flag.store(true, Ordering::SeqCst);
    if let Some(inner) = st.supervisor.lock().unwrap().take() {
        let _ = inner.stop_tx.send(true);
        // 监督循环的 Manual 分支会在 serve 优雅关闭完成后才退出并置 running=false，
        // 这里轮询等待即可（上限 2s；超时则后台自行收尾，UI 先行置为已停止）。
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(2000);
        while st.running.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    st.running.store(false, Ordering::SeqCst);
}

// ---------------------------------------------------------------- 状态外显

fn reflect_status(app: &AppHandle, st: &GatewayState) {
    let s = st.status();
    if let Some(tray) = st.tray.lock().unwrap().as_ref() {
        let text = if s.running {
            format!("状态：运行中 · 127.0.0.1:{} · 重启 {}", s.port, s.restarts)
        } else {
            "状态：已停止".to_string()
        };
        let _ = tray.status_item.set_text(text);
        let _ = tray.start_item.set_enabled(!s.running);
        let _ = tray.stop_item.set_enabled(s.running);
    }
    let _ = app.emit("gateway://status", &s);
}

// ---------------------------------------------------------------- 入口

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        // 托盘常驻（稳定性基线）：关闭窗口 = 隐藏到托盘，网关保持运行；
        // 真正退出走托盘菜单「退出 JAI」。
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            // 1) 数据目录 + 迁移（失败即中止启动 —— storage §4 早拦截）
            //    某些受限环境（CI/沙箱）对 ~/Library 无写权限，回退临时目录保证可演示。
            let data_dir = match app.path().app_data_dir() {
                Ok(dir) => {
                    if std::fs::create_dir_all(&dir).is_ok() {
                        dir
                    } else {
                        eprintln!(
                            "[store] 默认数据目录不可写({}), 回退到临时目录",
                            dir.display()
                        );
                        let fallback = std::env::temp_dir().join("jai-data");
                        std::fs::create_dir_all(&fallback)?;
                        fallback
                    }
                }
                Err(_) => {
                    let fallback = std::env::temp_dir().join("jai-data");
                    std::fs::create_dir_all(&fallback)?;
                    fallback
                }
            };
            let db_path = data_dir.join("jai.db");
            let db_str = db_path.to_string_lossy().to_string();

            let db = Db::open(&db_str)?;
            // 存量钥匙串凭据一次性迁移入库（0006）；此后运行时零钥匙串访问。
            // 后台执行：授权弹框可能等待用户输入，绝不能阻塞启动路径；
            // 弹框期间迁移不持 DB 锁（见 vault::migrate_keyring_secrets 分段持锁设计）。
            // 失败不中止启动：新路径可照常运行，下次启动重试（标记位未置）。
            tauri::async_runtime::spawn_blocking({
                let db = db.clone();
                move || {
                    if let Err(e) = vault::migrate_keyring_secrets(&db).map_err(|e| e.to_string()) {
                        eprintln!("[vault] 钥匙串存量迁移失败(下次启动重试): {e}");
                    }
                }
            });
            // 日志管道第二条连接 + 有界队列（稳定性基线 §5-3）
            let (log_handle, _log_task) = logs::spawn_logger(&db_str)?;
            println!("[store] db ready at {}", db_path.display());

            // 读取持久化设置（meta KV）：端口 / 日志开关（roadmap M2 设置页）
            let preferred_port: u16 = db
                .with(|c| {
                    store::meta_get(c, "gateway_port")
                        .map(|v| v.and_then(|s| s.parse::<u16>().ok()))
                })
                .ok()
                .flatten()
                .unwrap_or(server::DEFAULT_PORT);
            let logs_enabled: bool = db
                .with(|c| {
                    Ok(store::meta_get(c, "logs_enabled")?
                        .map(|s| s != "false")
                        .unwrap_or(true))
                })
                .unwrap_or(true);
            log_handle.set_enabled(logs_enabled);

            // 保活 timer（roadmap M2「保活 timer」）：每日清理日志保留窗口与
            // tool_id_map TTL；异常不影响主路径（任务内部自愈）。
            let _retention_task = store::retention::spawn_retention_loop(
                db.clone(),
                std::time::Duration::from_secs(24 * 3600),
                store::retention::DEFAULT_RETENTION_DAYS,
                store::retention::DEFAULT_LOG_ROW_CAP,
            );

            let core = AppCore {
                db: db.clone(),
                logs: log_handle,
                // 出站代理（D8）：启动时读 meta 构建；保存后重启网关生效
                http: netcfg::build_client(
                    db.with_any(netcfg::ProxyConfig::from_meta).ok().as_ref(),
                    std::time::Duration::from_secs(10),
                ),
                db_path: db_str,
                autopush: AutopushHub::new(),
            };
            ensure_gateway_key(&core)?;

            // WebDAV 自动同步循环（变更防抖 + 定时；未启用时低速轮询配置）
            spawn_autopush(core.clone(), app.handle().clone());

            // 供应商定时健康检查（10 分钟/轮；状态跃迁发系统通知，首轮不通知）
            spawn_health_check(core.clone(), app.handle().clone());

            // 2) 托盘
            let status_item =
                MenuItem::with_id(app, "status", "状态：已停止", false, None::<&str>)?;
            let start_item = MenuItem::with_id(app, "gw-start", "启动网关", true, None::<&str>)?;
            let stop_item = MenuItem::with_id(app, "gw-stop", "停止网关", false, None::<&str>)?;
            let sep1 = PredefinedMenuItem::separator(app)?;
            let sep2 = PredefinedMenuItem::separator(app)?;
            let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
            let sep3 = PredefinedMenuItem::separator(app)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出 JAI", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &status_item,
                    &sep1,
                    &start_item,
                    &stop_item,
                    &sep2,
                    &show_item,
                    &sep3,
                    &quit_item,
                ],
            )?;

            TrayIconBuilder::with_id("jai-tray")
                .icon(app.default_window_icon().expect("window icon").clone())
                .tooltip("JAI Gateway")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| {
                    let st = app.state::<GatewayState>();
                    let core = app.state::<AppCore>();
                    match event.id().as_ref() {
                        "gw-start" => {
                            if let Err(e) = spawn_supervisor(app, &st, &core) {
                                eprintln!("[tray] start failed: {e}");
                            }
                            reflect_status(app, &st);
                        }
                        "gw-stop" => {
                            request_stop(&st);
                            reflect_status(app, &st);
                        }
                        "show" => {
                            if let Some(w) = app.get_webview_window("main") {
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                        "quit" => app.exit(0),
                        _ => {}
                    }
                })
                .build(app)?;

            // 3) 受管状态 + 启动即拉起网关（常驻预期）
            let gw = GatewayState {
                preferred_port,
                running: Arc::new(AtomicBool::new(false)),
                stop_flag: Arc::new(AtomicBool::new(false)),
                port: Arc::new(AtomicU16::new(preferred_port)),
                restarts: Arc::new(AtomicU64::new(0)),
                supervisor: Mutex::new(None),
                tray: Mutex::new(Some(TrayHandles {
                    status_item,
                    start_item,
                    stop_item,
                })),
            };
            app.manage(gw);
            app.manage(core);
            {
                let st = app.state::<GatewayState>();
                let core_state = app.state::<AppCore>();
                spawn_supervisor(app.handle(), &st, &core_state)?;
                reflect_status(app.handle(), &st);
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            gateway_status,
            gateway_start,
            gateway_stop,
            provider_list,
            provider_create,
            provider_update,
            provider_delete,
            provider_set_enabled,
            provider_test,
            provider_test_draft,
            provider_discover_models,
            model_list,
            model_set_limits,
            model_set_alias,
            model_toggle,
            gateway_key_info,
            gateway_key_reveal,
            gateway_key_regenerate,
            logs_recent,
            stats_usage,
            export_config_json,
            export_config_to_file,
            reveal_in_folder,
            config_import,
            webdav_config_get,
            webdav_config_set,
            webdav_test,
            webdav_preview,
            webdav_push,
            webdav_pull,
            webdav_autopush_status,
            webdav_autopull_status,
            webdav_snapshot_info,
            webdav_snapshot_restore,
            webdav_backups_list,
            webdav_backup_restore,
            webdav_backup_delete,
            mcp_list,
            mcp_create,
            mcp_update,
            mcp_set_enabled,
            mcp_set_proxy_allowed,
            mcp_delete,
            mcp_tools_list,
            mcp_tools_call,
            mcp_export_config,
            mcp_import,
            skill_list,
            skill_export_markdown,
            skill_create,
            skill_update,
            skill_set_enabled,
            skill_delete,
            skill_import_zip,
            cors_allow_get,
            cors_allow_set,
            settings_get,
            settings_set_port,
            settings_set_logs_enabled,
            settings_set_retention,
            proxy_get,
            proxy_set,
            proxy_test,
            read_env_var,
            port_in_use,
            families,
        ])
        .run(tauri::generate_context!())
        .expect("error while running jai");
}

// ---------------------------------------------------------------- 网关启停命令

#[tauri::command]
fn gateway_status(state: State<'_, GatewayState>) -> GwStatus {
    state.status()
}

#[tauri::command]
async fn gateway_start(
    app: AppHandle,
    state: State<'_, GatewayState>,
    core: State<'_, AppCore>,
) -> Result<GwStatus, String> {
    if state.running.load(Ordering::SeqCst) {
        return Ok(state.status());
    }
    spawn_supervisor(&app, &state, &core).map_err(|e| e.to_string())?;
    reflect_status(&app, &state);
    Ok(state.status())
}

#[tauri::command]
async fn gateway_stop(app: AppHandle, state: State<'_, GatewayState>) -> Result<GwStatus, String> {
    request_stop(&state);
    reflect_status(&app, &state);
    Ok(state.status())
}
