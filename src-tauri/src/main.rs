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
use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{
    self, import, logs, Db, GatewayKeyRow, McpServerRow, ModelRow, ProviderRow, SkillRow,
};
use gateway_core::skills::SkillDraft;
use gateway_core::sync::{self, WebDavConfig};
use gateway_core::vault;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State};

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

/// IPC 命令共享的业务核心（数据库 / 日志句柄 / HTTP 客户端）
pub struct AppCore {
    pub db: Db,
    pub logs: logs::LogHandle,
    pub http: reqwest::Client,
    pub db_path: String,
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
    pub extra_headers: Option<String>,
    pub last_ok_at: Option<i64>,
    pub last_err_at: Option<i64>,
    pub last_err_msg: Option<String>,
    pub has_key: bool,
}

fn to_dto(p: ProviderRow, has_key: bool) -> ProviderDto {
    ProviderDto {
        id: p.id,
        name: p.name,
        base_url: p.base_url,
        family: p.family,
        enabled: p.enabled,
        priority: p.priority,
        extra_headers: p.extra_headers,
        last_ok_at: p.last_ok_at,
        last_err_at: p.last_err_at,
        last_err_msg: p.last_err_msg,
        has_key,
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

    let mut out = Vec::with_capacity(rows.len());
    for p in rows {
        let has_key = tokio::task::spawn_blocking({
            let ref_ = p.keyring_ref.clone();
            move || vault::get_secret(&ref_)
        })
        .await
        .map_err(join_err)?
        .map_err(vault_msg)?
        .is_some();
        out.push(to_dto(p, has_key));
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewProvider {
    pub name: String,
    pub base_url: String,
    pub family: String,
    #[serde(default = "default_priority")]
    pub priority: i64,
    #[serde(default)]
    pub extra_headers: Option<String>,
    pub api_key: String,
}

fn default_priority() -> i64 {
    100
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
    let keyring_ref = vault::ref_for(&id);

    // storage §4：先写密钥环，成功后落库；落库失败回滚删除凭据
    let secret = input.api_key.clone();
    let r2 = keyring_ref.clone();
    tokio::task::spawn_blocking(move || vault::set_secret(&r2, &secret))
        .await
        .map_err(join_err)?
        .map_err(vault_msg)?;

    let row = ProviderRow {
        id: id.clone(),
        name: input.name.trim().to_string(),
        base_url: normalize_base(&input.base_url),
        family: input.family,
        enabled: true,
        priority: input.priority,
        extra_headers: input.extra_headers.filter(|s| !s.trim().is_empty()),
        keyring_ref,
        last_ok_at: None,
        last_err_at: None,
        last_err_msg: None,
        created_at: store::now_ms(),
        updated_at: store::now_ms(),
    };
    let db = core.db.clone();
    if let Err(e) =
        tokio::task::spawn_blocking(move || db.with(|c| store::provider_insert(c, &row)))
            .await
            .map_err(join_err)?
    {
        let rollback_ref = vault::ref_for(&id);
        let _ = tokio::task::spawn_blocking(move || vault::delete_secret(&rollback_ref)).await;
        return Err(format!("数据库写入失败(已回滚凭据): {e}"));
    }

    let db2 = core.db.clone();
    let created = tokio::task::spawn_blocking(move || {
        db2.with(|c| store::provider_get(c, &id))
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "创建后读取失败".to_string())
    })
    .await
    .map_err(join_err)??;

    Ok(to_dto(created, true))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProviderInput {
    pub id: String,
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub priority: Option<i64>,
    /// 外层 Some 表示要动这个字段；内层 None 表示清空
    pub extra_headers: Option<Option<String>>,
    /// Some(非空) 覆盖密钥；Some("") 忽略
    pub api_key: Option<String>,
}

#[tauri::command]
async fn provider_update(
    core: State<'_, AppCore>,
    input: UpdateProviderInput,
) -> Result<(), String> {
    if let Some(key) = &input.api_key {
        if !key.is_empty() {
            let row = fetch_provider(&core.db, &input.id).await?;
            let kr = row.keyring_ref;
            let k = key.clone();
            tokio::task::spawn_blocking(move || vault::set_secret(&kr, &k))
                .await
                .map_err(join_err)?
                .map_err(vault_msg)?;
        }
    }

    let normalized = input.base_url.as_deref().map(normalize_base);
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| {
            store::provider_update_fields(
                c,
                &input.id,
                input.name.as_deref(),
                normalized.as_deref(),
                input.priority,
                input.extra_headers.as_ref().map(|o| o.as_deref()),
            )
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
async fn provider_delete(core: State<'_, AppCore>, id: String) -> Result<(), String> {
    let row = fetch_provider(&core.db, &id).await?;
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::provider_delete(c, &id))
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;

    // 库先行成功；凭据尽力删除（幂等，失败不阻塞 UI）
    let _ = tokio::task::spawn_blocking(move || vault::delete_secret(&row.keyring_ref)).await;
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
    .map_err(join_err)?
}

/// 测试连接：跑一次模型发现。HTTP 200 即视为连通
/// （部分中转站隐藏 /models，0 个模型也算通；错误信息给出排查提示）。
#[tauri::command]
async fn provider_test(core: State<'_, AppCore>, id: String) -> Result<String, String> {
    let row = fetch_provider(&core.db, &id).await?;
    let secret = load_secret_or_none(&row.keyring_ref).await?;
    match discover_models(&core.http, &row.family, &row.base_url, secret.as_deref()).await {
        Ok(models) => {
            let n = models.len();
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
    let secret = load_secret_or_none(&row.keyring_ref).await?;

    let models = discover_models(&core.http, &row.family, &row.base_url, secret.as_deref()).await?;

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
    .map_err(join_err)?
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
    .map_err(join_err)?
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

// ---------------------------------------------------------------- M7：导入 + WebDAV

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavConfigDto {
    pub url: String,
    pub username: String,
    pub directory: String,
}

impl From<WebDavConfig> for WebDavConfigDto {
    fn from(c: WebDavConfig) -> Self {
        Self {
            url: c.url,
            username: c.username,
            directory: c.directory,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavConfigInput {
    pub url: String,
    pub username: String,
    pub directory: String,
    /// Some(非空) 覆盖密码；None/空串保持原密码
    pub password: Option<String>,
}

#[tauri::command]
async fn config_import(
    core: State<'_, AppCore>,
    text: String,
    strict: Option<bool>,
) -> Result<import::ImportReport, String> {
    let db = core.db.clone();
    let strict = strict.unwrap_or(false);
    tokio::task::spawn_blocking(move || {
        db.with_any(|c| import::apply_import(c, &text, strict).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
async fn webdav_config_get(core: State<'_, AppCore>) -> Result<Option<WebDavConfigDto>, String> {
    let db = core.db.clone();
    let cfg: Option<WebDavConfig> = tokio::task::spawn_blocking(move || {
        db.with_any(|c| sync::config_get(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)??;
    Ok(cfg.map(WebDavConfigDto::from))
}

#[tauri::command]
async fn webdav_config_set(
    core: State<'_, AppCore>,
    input: WebDavConfigInput,
) -> Result<(), String> {
    if let Some(pw) = input.password.as_deref().filter(|s| !s.trim().is_empty()) {
        let ref_ = sync::WEBDAV_KEYRING_REF.to_string();
        let pw = pw.to_string();
        tokio::task::spawn_blocking(move || vault::set_secret(&ref_, &pw))
            .await
            .map_err(join_err)?
            .map_err(vault_msg)?;
    }
    let cfg = WebDavConfig {
        url: normalize_base(&input.url),
        username: input.username.trim().to_string(),
        directory: input.directory.trim().to_string(),
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
async fn webdav_test(
    core: State<'_, AppCore>,
    input: WebDavConfigInput,
) -> Result<String, String> {
    let url = normalize_base(&input.url);
    let username = input.username.trim().to_string();
    let password = input.password.clone().unwrap_or_default();
    let resp = core
        .http
        .request(reqwest::Method::OPTIONS, &url)
        .basic_auth(&username, Some(&password))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("连接失败: {e}"))?;
    let status = resp.status().as_u16();
    if status == 401 || status == 403 {
        Err(format!("连接成功但认证失败（HTTP {status}）"))
    } else if (200..400).contains(&status) {
        Ok("连接成功".into())
    } else {
        Err(format!("连接异常（HTTP {status}）"))
    }
}

#[tauri::command]
async fn webdav_push(core: State<'_, AppCore>) -> Result<(), String> {
    let cfg = get_webdav_config(&core).await?;
    let password = get_webdav_password().await?;
    let export_text = export_config_json(core.clone()).await?;

    // 推送前本地快照留存一份，用于误操作回退
    let db = core.db.clone();
    let snap = export_text.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| sync::snapshot_put(c, &snap))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;

    sync::push(&core.http, &cfg, &password, export_text).await
}

#[tauri::command]
async fn webdav_pull(core: State<'_, AppCore>) -> Result<import::ImportReport, String> {
    let cfg = get_webdav_config(&core).await?;
    let password = get_webdav_password().await?;
    let text = sync::pull(&core.http, &cfg, &password).await?;
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with_any(|c| import::apply_import(c, &text, false).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)?
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

async fn get_webdav_password() -> Result<String, String> {
    let ref_ = sync::WEBDAV_KEYRING_REF.to_string();
    tokio::task::spawn_blocking(move || vault::get_secret(&ref_))
        .await
        .map_err(join_err)?
        .map_err(vault_msg)?
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
        enabled: true,
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
            db.with(|c| store::skill_insert(c, &row2)).map_err(|e| e.to_string())
        })
        .await
        .map_err(join_err)??;
        imported += 1;
    }
    Ok(imported)
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
    vec![
        "openai_compat",
        "openai_responses",
        "anthropic",
        "gemini",
    ]
}

/// 当前凭据存储方式：`keyring` 或 `file`（降级）。
#[tauri::command]
fn vault_storage_kind() -> &'static str {
    vault::storage_kind()
}

// ---------------------------------------------------------------- 设置（M2）

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

fn vault_msg(e: vault::VaultError) -> String {
    format!("系统密钥环操作失败（检查本机凭据设置）: {e}")
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

async fn load_secret_or_none(keyring_ref: &str) -> Result<Option<String>, String> {
    let r = keyring_ref.to_string();
    tokio::task::spawn_blocking(move || vault::get_secret(&r))
        .await
        .map_err(join_err)?
        .map_err(vault_msg)
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

            // 密钥环不可用（沙箱/CI）时降级为文件存储，保证添加供应商可用
            vault::init(&data_dir)?;

            let db = Db::open(&db_str)?;
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
                http: reqwest::Client::builder()
                    .connect_timeout(std::time::Duration::from_secs(10))
                    .build()
                    .expect("reqwest client"),
                db_path: db_str,
            };
            ensure_gateway_key(&core)?;

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
            vault_storage_kind,
            model_list,
            model_set_limits,
            model_toggle,
            gateway_key_info,
            gateway_key_reveal,
            gateway_key_regenerate,
            logs_recent,
            export_config_json,
            config_import,
            webdav_config_get,
            webdav_config_set,
            webdav_test,
            webdav_push,
            webdav_pull,
            mcp_list,
            mcp_create,
            mcp_update,
            mcp_set_enabled,
            mcp_delete,
            mcp_tools_list,
            mcp_tools_call,
            skill_list,
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
