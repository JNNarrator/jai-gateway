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
use gateway_core::probe::{self, DraftProbeReport, ProbeConfig, ProbeTarget};
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
use std::time::{Duration, Instant};
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
    /// 「调度参数变了，请重新评估」信号（WebDAV 开关/间隔被改时置位）
    ///
    /// 与 `tx` 分开是刻意的：`tx` 的语义是「业务数据变了 → 防抖后推一次」，
    /// 而改调度参数（开关/间隔）**不该触发推送**，只需要让调度循环尽快重读配置。
    /// 两者共用一个通道会让「改一下间隔」顺带推一次远端。
    cfg_tx: tokio::sync::watch::Sender<u64>,
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
        let (cfg_tx, _cfg_rx) = tokio::sync::watch::channel(0);
        Self {
            tx,
            cfg_tx,
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

    /// WebDAV 调度参数（自动推送/拉取开关、两个间隔）被修改时调用。
    ///
    /// 只让调度循环**立刻重读配置**（含「关→开」这类状态跃迁的采样），
    /// 不触发任何推送、不走防抖。没有它就只能等睡眠到期才发现配置变了 ——
    /// 那时长间隔（最长 6 小时）会让「改了没反应」，短窗口内的
    /// 「关掉再打开」也可能整段被漏采样（重新启用本该重新等一个完整间隔）。
    pub fn notify_config_change(&self) {
        let next = *self.cfg_tx.borrow() + 1;
        let _ = self.cfg_tx.send(next);
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
    /// 最近一轮健康检查摘要（UX-T3 横幅数据源）
    pub health_summary: std::sync::Arc<std::sync::Mutex<HealthSummary>>,
    /// 草稿端点探测的「通过」凭据（D9-T1）。**进程内存储、重启即失效** ——
    /// 刻意不落库：半年前测过的渠道不该被信任。
    pub probe_receipts: std::sync::Arc<gateway_core::probe::ProbeReceiptStore>,
    /// 密钥白/黑名单的 5s TTL 缓存（D9-T6b）。与网关 `GatewayCtx.rules` 是**同一份**：
    /// 保存规则后 IPC 侧立刻失效，用户不必等 TTL 过期才看到生效。
    pub key_rules: std::sync::Arc<gateway_core::server::security::KeyRulesCache>,
}

/// 最近一轮健康检查摘要（UX-T3 横幅数据源）。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthSummary {
    pub checked_at_ms: Option<i64>,
    pub down: Vec<HealthDownProvider>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthDownProvider {
    pub name: String,
    pub message: String,
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
    /// 供应商级「推理档位值域」声明（0011）：None/空 = 未声明 ⇒ 原样透传
    pub reasoning_effort_levels: Option<Vec<String>>,
    /// 供应商级「工具声明数上限」声明（0012）：None = 未声明 ⇒ 不拦（由上游裁决）
    pub max_tools: Option<i64>,
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
        reasoning_effort_levels: p.reasoning_effort_levels,
        max_tools: p.max_tools,
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
    /// 最近一次端点探测的指纹（`provider_probe_draft` 的返回里带）。
    /// 只在 `require_probe_pass` 开启时被使用。
    #[serde(default)]
    pub probe_fingerprint: Option<String>,
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

    // base_url 结构性校验（D9-T1）：这是「用户填什么就存什么」的入口之一。
    // 见 `validate_base_url` 的说明（允许 loopback，但不允许私网 / 云元数据）。
    let base_url = normalize_base(&input.base_url);
    validate_base_url(&base_url)?;
    // 门禁（默认关闭，见 `check_probe_gate`）
    check_probe_gate(&core, input.probe_fingerprint.as_deref())?;

    let id = uuid::Uuid::now_v7().to_string();
    let row = ProviderRow {
        id: id.clone(),
        name: input.name.trim().to_string(),
        base_url,
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
        max_tools: None,
        reasoning_effort_levels: None,
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
    /// 最近一次端点探测的指纹（`provider_probe_draft` 的返回里带）。
    /// 只在 `require_probe_pass` 开启时被使用。
    #[serde(default)]
    pub probe_fingerprint: Option<String>,
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
    if let Some(b) = &normalized {
        validate_base_url(b)?;
    }
    // 门禁只在「探测目标真的变了」时生效（base_url / 密钥）——否则改个显示名
    // 也会被 30 分钟 TTL 挡住，那是纯粹的骚扰。
    if normalized.is_some() || new_key.is_some() {
        check_probe_gate(&core, input.probe_fingerprint.as_deref())?;
    }
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

/// 「端点探测」入参（新建/编辑供应商弹窗）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeDraftInput {
    pub base_url: String,
    pub family: String,
    #[serde(default)]
    pub api_key: String,
    /// 编辑已有渠道时传 id：表单里没重输 key 就用**库里存的** key 探测。
    /// 不这样做的话，编辑态探测会以「未鉴权」身份打上游 → 用户看到假失败。
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub extra_headers: Option<String>,
    /// 是否允许访问本机地址（Ollama / LM Studio 等本机部署）。默认 false。
    #[serde(default)]
    pub allow_loopback: bool,
}

/// 端点探测：对**草稿**渠道发一次真实的最小推理请求（`max_tokens = 1`），
/// 逐端点给出「通过 / 失败 + 失败分类 + 延迟」。不写库、不落凭据。
///
/// 复用 `core.http` → 自动继承出站代理与 10s connect timeout（与转发链路同一出口）。
/// 结论同时存进进程内 receipt，供 `require_probe_pass` 门禁使用（默认关闭）。
#[tauri::command]
async fn provider_probe_draft(
    core: State<'_, AppCore>,
    input: ProbeDraftInput,
) -> Result<DraftProbeReport, String> {
    let typed_key = Some(input.api_key.trim().to_string()).filter(|s| !s.is_empty());
    let api_key = match typed_key {
        Some(k) => Some(k),
        None => match input.provider_id.as_deref() {
            Some(id) => core
                .db
                .with(|c| store::provider_get(c, id))
                .ok()
                .flatten()
                .and_then(|p| p.api_key),
            None => None,
        },
    };
    let target = ProbeTarget {
        family: input.family,
        base_url: normalize_base(&input.base_url),
        api_key,
        model: input.model,
        extra_headers: input.extra_headers.filter(|s| !s.trim().is_empty()),
    };
    let cfg = ProbeConfig {
        allow_loopback: input.allow_loopback,
        ..ProbeConfig::default()
    };
    let report = probe::probe_draft(&core.http, &target, &cfg).await;
    let now = store::now_ms();
    core.probe_receipts.prune(now);
    core.probe_receipts.put(&report);
    Ok(report)
}

/// 保存渠道时的 base_url 结构性校验（D9-T1 / 决策 D2）。
///
/// 这里**允许 loopback**（`allow_loopback = true`）：Ollama / LM Studio / vLLM 这类
/// 本机部署是正当用法，而保存动作本身不发起任何请求。真正的「用户填什么就打什么」
/// 入口是草稿探测，那里对 loopback 要求显式勾选（决策 D3）。
/// 私网 / link-local（含云元数据 `169.254.169.254`）/ 组播 / 保留段在任何情况下都拒绝。
///
/// 刻意**不做**热路径（`try_candidate`）每请求校验：那会让「已存渠道突然被拦」
/// 变成可用性事故，收益却很小。
fn validate_base_url(raw: &str) -> Result<(), String> {
    gateway_core::netguard::validate_outbound_url(raw, true)
        .map(|_| ())
        .map_err(|e| format!("Base URL 不合法：{e}"))
}

/// `require_probe_pass` 门禁（D9-T1，meta KV，**默认关闭** = 只展示不拦截）。
///
/// 开启后，保存渠道必须带上一次「刚刚探测通过」的指纹：receipt 存在进程内、
/// TTL 30 分钟、且要求至少一个非信息性端点真的通过（全是 Skipped 不算）。
fn check_probe_gate(core: &AppCore, fingerprint: Option<&str>) -> Result<(), String> {
    let enabled = core
        .db
        .with(|c| store::meta_get(c, "require_probe_pass"))
        .ok()
        .flatten()
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            v == "true" || v == "1"
        })
        .unwrap_or(false);
    if !enabled {
        return Ok(());
    }
    let Some(fp) = fingerprint.filter(|s| !s.is_empty()) else {
        return Err("该渠道尚未完成端点探测：请先在弹窗里点「端点探测」".into());
    };
    if !core.probe_receipts.validate(fp, store::now_ms()) {
        return Err("端点探测未通过或已过期（30 分钟），请重新探测后再保存".into());
    }
    Ok(())
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
        let input_mm = m.input_modalities.clone();
        let output_mm = m.output_modalities.clone();
        tokio::task::spawn_blocking(move || {
            db.with(|c| {
                store::model_upsert(
                    c,
                    &pid,
                    &name,
                    Some(ctx_w),
                    out_w,
                    input_mm.as_deref(),
                    output_mm.as_deref(),
                )
            })
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

/// 模型级输入/输出模态标注（0010）：`Some([..])` 明确标注，`None` 回到「未知」。
/// 传空数组亦等价于「未知」（读取侧 parse 归一）——语义见 gateway_core::modality。
#[tauri::command]
async fn model_set_modalities(
    core: State<'_, AppCore>,
    model_id: String,
    input_modalities: Option<Vec<gateway_core::modality::Modality>>,
    output_modalities: Option<Vec<gateway_core::modality::Modality>>,
) -> Result<(), String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| {
            store::model_set_modalities(
                c,
                &model_id,
                input_modalities.as_deref(),
                output_modalities.as_deref(),
            )
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;

    core.autopush.notify_change();
    Ok(())
}

/// 模型级「推理档位值域」声明（0011）。`None`/空数组 = 回到继承供应商级。
#[tauri::command]
async fn model_set_reasoning_levels(
    core: State<'_, AppCore>,
    model_id: String,
    levels: Option<Vec<String>>,
) -> Result<(), String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::model_set_reasoning_levels(c, &model_id, levels.as_deref()))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;

    core.autopush.notify_change();
    Ok(())
}

/// 供应商级「推理档位值域」声明（0011）。`None`/空数组 = 未声明 ⇒ 原样透传。
#[tauri::command]
async fn provider_set_reasoning_levels(
    core: State<'_, AppCore>,
    id: String,
    levels: Option<Vec<String>>,
) -> Result<(), String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::provider_set_reasoning_levels(c, &id, levels.as_deref()))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;

    core.autopush.notify_change();
    Ok(())
}

/// 模型级「工具声明数上限」声明（0012）。`None`/0 = 回到继承供应商级（或不拦）。
#[tauri::command]
async fn model_set_max_tools(
    core: State<'_, AppCore>,
    model_id: String,
    max_tools: Option<i64>,
) -> Result<(), String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::model_set_max_tools(c, &model_id, max_tools))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;

    core.autopush.notify_change();
    Ok(())
}

/// 供应商级「工具声明数上限」声明（0012）。`None`/0 = 未声明 ⇒ 不拦（由上游裁决）。
#[tauri::command]
async fn provider_set_max_tools(
    core: State<'_, AppCore>,
    id: String,
    max_tools: Option<i64>,
) -> Result<(), String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::provider_set_max_tools(c, &id, max_tools))
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
    /// 密钥 id（多密钥：吊销 / 指定 reveal 都按它定位）
    pub id: String,
    pub prefix: String,
    pub label: Option<String>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    /// 非空即已吊销（列表里不再返回已吊销的，保留字段供 DTO 完整性）
    pub revoked_at: Option<i64>,
    /// 仅 reveal/create/regenerate 携带全文；list/info 恒为空串
    pub key: String,
}

fn key_info(k: GatewayKeyRow, with_full: bool) -> GatewayKeyInfo {
    GatewayKeyInfo {
        id: k.id,
        prefix: k.prefix,
        label: k.label,
        created_at: k.created_at,
        last_used_at: k.last_used_at,
        revoked_at: k.revoked_at,
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

/// 列出全部未吊销密钥（**不含全文**，常态只给前缀）。
#[tauri::command]
async fn gateway_key_list(core: State<'_, AppCore>) -> Result<Vec<GatewayKeyInfo>, String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with_any(|c| match store::gw_keys_active(c) {
            Ok(rows) => Ok(rows.into_iter().map(|k| key_info(k, false)).collect()),
            Err(e) => Err(e.to_string()),
        })
    })
    .await
    .map_err(join_err)?
}

/// 新建一把密钥（**不吊销旧的**）。返回值带全文 —— 这是唯一一次能拿到全文的时机
/// （列表与 info 都不带），UI 必须提示用户当场复制。
#[tauri::command]
async fn gateway_key_create(
    core: State<'_, AppCore>,
    label: Option<String>,
) -> Result<GatewayKeyInfo, String> {
    let new_key = gen_gateway_key();
    let nk = new_key.clone();
    // 标签归一在 store 层（`gw_key_create`）统一做，这里不重复
    let db = core.db.clone();
    let row = tokio::task::spawn_blocking(move || {
        db.with(|c| store::gw_key_create(c, &nk, label.as_deref()))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;
    core.autopush.notify_change();
    Ok(key_info(row, true))
}

/// 按 id 吊销一把密钥（软删，保留审计痕迹）。其他密钥不受影响。
#[tauri::command]
async fn gateway_key_revoke(core: State<'_, AppCore>, id: String) -> Result<bool, String> {
    let db = core.db.clone();
    let changed = tokio::task::spawn_blocking(move || {
        db.with(|c| store::gw_key_revoke(c, &id))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;
    core.autopush.notify_change();
    Ok(changed)
}

/// 显示全文。`id` 省略时取最新一把（单密钥语义的兼容路径）。
#[tauri::command]
async fn gateway_key_reveal(
    core: State<'_, AppCore>,
    id: Option<String>,
) -> Result<GatewayKeyInfo, String> {
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with_any(|c| match id.as_deref() {
            None => match store::gw_key_active(c) {
                Ok(Some(k)) => Ok(key_info(k, true)),
                Ok(None) => Err("无活跃网关密钥".to_string()),
                Err(e) => Err(e.to_string()),
            },
            Some(want) => match store::gw_keys_active(c) {
                Ok(rows) => match rows.into_iter().find(|k| k.id == want) {
                    Some(k) => Ok(key_info(k, true)),
                    None => Err("该密钥不存在或已吊销".to_string()),
                },
                Err(e) => Err(e.to_string()),
            },
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
        id: row.id,
        prefix: row.prefix,
        label: row.label,
        created_at: row.created_at,
        last_used_at: None,
        revoked_at: None,
        key: new_key,
    })
}

// -------------------------------------------------- 密钥白/黑名单（D9-T6b）

/// 一把密钥的规则 DTO。四个集合一一对应迁移 0013 的两张表。
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyRulesDto {
    pub provider_allow: Vec<String>,
    pub provider_deny: Vec<String>,
    pub model_allow: Vec<String>,
    pub model_deny: Vec<String>,
}

impl KeyRulesDto {
    fn from_rules(r: &gateway_core::store::keyrules::KeyRules) -> Self {
        let v = |s: &std::collections::BTreeSet<String>| s.iter().cloned().collect::<Vec<_>>();
        Self {
            provider_allow: v(&r.provider_allow),
            provider_deny: v(&r.provider_deny),
            model_allow: v(&r.model_allow),
            model_deny: v(&r.model_deny),
        }
    }

    fn to_rules(&self) -> gateway_core::store::keyrules::KeyRules {
        let s = |v: &Vec<String>| v.iter().cloned().collect::<std::collections::BTreeSet<_>>();
        gateway_core::store::keyrules::KeyRules {
            provider_allow: s(&self.provider_allow),
            provider_deny: s(&self.provider_deny),
            model_allow: s(&self.model_allow),
            model_deny: s(&self.model_deny),
        }
    }
}

/// 规则选择器的一行候选（渠道 × 模型）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleOptionDto {
    pub provider_id: String,
    pub provider_name: String,
    pub model_name: String,
}

/// 读某把密钥的规则（没配过 → 四个空数组 = 不限制）。
#[tauri::command]
async fn gateway_key_rules_get(
    core: State<'_, AppCore>,
    key_id: String,
) -> Result<KeyRulesDto, String> {
    let db = core.db.clone();
    let rules = tokio::task::spawn_blocking(move || {
        db.with(|c| store::keyrules::key_rules_get(c, &key_id))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;
    Ok(KeyRulesDto::from_rules(&rules))
}

/// 覆盖式保存某把密钥的规则，并**立刻失效缓存**（否则要等 5s TTL 才生效）。
#[tauri::command]
async fn gateway_key_rules_set(
    core: State<'_, AppCore>,
    key_id: String,
    rules: KeyRulesDto,
) -> Result<(), String> {
    let db = core.db.clone();
    let r = rules.to_rules();
    let id = key_id.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| store::keyrules::key_rules_set(c, &id, &r))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;
    core.key_rules.invalidate(Some(&key_id));
    Ok(())
}

/// 规则选择器的候选清单（启用中的渠道 × 模型）。
#[tauri::command]
async fn gateway_key_rules_options(core: State<'_, AppCore>) -> Result<Vec<RuleOptionDto>, String> {
    let db = core.db.clone();
    let rows = tokio::task::spawn_blocking(move || {
        db.with(store::keyrules::rule_options)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;
    Ok(rows
        .into_iter()
        .map(|r| RuleOptionDto {
            provider_id: r.provider_id,
            provider_name: r.provider_name,
            model_name: r.model_name,
        })
        .collect())
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
    /// 定时自动拉取总开关（按 exportedAt 时间戳 last-write-wins）
    pub auto_pull_enabled: bool,
    /// 定时拉取间隔分钟数（与推送间隔**相互独立**）
    pub auto_pull_interval_min: u32,
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
            auto_pull_interval_min: c.auto_pull_interval_min,
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
    /// 自动拉取间隔分钟；None 保持原值
    pub auto_pull_interval_min: Option<u32>,
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

/// 自动同步调度器的纯决策单测。
///
/// 这些用例覆盖 bug 清单 20：**定时唤醒点与两个动作的到期判定**。
/// 全部用「基准时刻 + 偏移」构造「现在」，不依赖真实时间流逝，因此不会 flaky；
/// 调度逻辑本身是纯函数（`Instant` 显式传入），这正是当初把它抽出来的原因 ——
/// 旧实现把「睡多久」和「该不该跑」揉在循环体里，只能靠真实等待去验证。
#[cfg(test)]
mod autosync_schedule_tests {
    use super::*;

    fn mins(n: u64) -> Duration {
        Duration::from_secs(n * 60)
    }

    /// 基准时刻 + n 分钟（可控时钟：所有「现在」都由它推导）。
    fn at(base: Instant, n: u64) -> Instant {
        base + mins(n)
    }

    /// 造一个动作：`Some(m)` = 已启用、间隔 m 分钟、启用起点为 `base`；`None` = 未启用。
    fn action(base: Instant, interval_min: Option<u64>) -> SyncAction {
        match interval_min {
            Some(m) => {
                let mut clock = ActionClock::disabled();
                clock.sync_enabled(true, base);
                SyncAction {
                    interval: Some(mins(m)),
                    clock,
                }
            }
            None => SyncAction {
                interval: None,
                clock: ActionClock::disabled(),
            },
        }
    }

    /// 基线：刚启用的动作**不立刻跑**，而是等满一个间隔（与旧行为一致）。
    #[test]
    fn never_run_action_fires_after_one_full_interval() {
        let base = Instant::now();
        let push = action(base, Some(60));
        assert!(!push.is_due(base), "刚启用不该立刻同步");
        assert_eq!(push.time_until_due(base), Some(mins(60)));
        assert!(!push.is_due(at(base, 59)));
        assert!(push.is_due(at(base, 60)));
        assert!(push.is_due(at(base, 61)), "过期也算到期");
    }

    /// **bug 20 主用例**：推送 6 小时 / 拉取 30 分钟 —— 拉取不得被推送的长间隔绑死。
    ///
    /// 旧实现 `let wait = push_interval.or(pull_interval)` 取的是推送的 6 小时，
    /// 于是拉取实际也 6 小时一次（用户报「自动拉取没生效」）。
    /// 反证：把 `next_wake` 改回「先看推送」，本用例立刻变红。
    #[test]
    fn pull_is_not_throttled_by_push_interval() {
        let base = Instant::now();
        let push = action(base, Some(360));
        let pull = action(base, Some(30));
        // 定时唤醒点 = 最近到期点 = 拉取的 30 分钟（旧实现会给 360）
        assert_eq!(
            next_wake(base, push, pull),
            Some(mins(30)),
            "唤醒点应取最近到期点，而不是 push.or(pull) 里那个推送的长间隔"
        );
        assert_eq!(push.time_until_due(base), Some(mins(360)));
        // 30 分钟：拉取到期、推送未到期
        let plan = plan_sync(at(base, 30), false, push, pull);
        assert!(plan.pull, "拉取应按自己的 30 分钟间隔到期");
        assert!(!plan.push, "推送不该被拉取的短间隔拽跑");
        // 拉取跑完后，下一个到期点是 60 分钟（每 30 分钟一次）
        let mut pull_after = pull;
        pull_after.clock.mark_run(at(base, 30));
        assert_eq!(next_wake(at(base, 30), push, pull_after), Some(mins(30)));
    }

    /// 反向护栏：拉取的短间隔不得把推送的到期点一起拽跑（旧实现「每 tick 都推」）。
    #[test]
    fn push_is_not_over_triggered_by_pull_ticks() {
        let base = Instant::now();
        let push = action(base, Some(360));
        let pull = action(base, Some(30));
        for m in [30u64, 60, 90, 120, 150, 180, 210, 240, 270, 300, 330] {
            let plan = plan_sync(at(base, m), false, push, pull);
            assert!(plan.pull, "第 {m} 分钟拉取应到期");
            assert!(
                !plan.push,
                "推送间隔 360 分钟，第 {m} 分钟不该推送（否则远端被反复覆盖）"
            );
        }
        let plan = plan_sync(at(base, 360), false, push, pull);
        assert!(plan.push, "第 360 分钟推送才该到期");
        assert!(plan.pull, "第 360 分钟拉取同时到期");
    }

    /// **饿死回归**：变更唤醒不得把定时到期点往后推。
    ///
    /// 旧实现每轮循环都新建 `sleep(wait)`，`rx.changed()` 一到就把定时器丢弃重建，
    /// 而 `notify_change()` 在供应商/模型/MCP/技能任一编辑时都会调用 ——
    /// 于是「编辑得比间隔勤」的用户永远等不到定时 tick，自动拉取被饿死。
    /// 这是 #20 描述的**同症状、不同根因**，且当时是可复现的真实缺陷。
    ///
    /// 反证：若到期点从「此刻」重新起算（旧行为），第 m 分钟的剩余时长会恒为
    /// 30 分钟而不是 `30 - m`，本用例立刻变红。
    #[test]
    fn change_wakes_do_not_push_back_the_deadline() {
        let base = Instant::now();
        let push = action(base, Some(360));
        let pull = action(base, Some(30));
        // 模拟用户每分钟编辑一次（每次都会 notify_change → 变更唤醒），持续 29 分钟
        for m in 1..30u64 {
            let plan = plan_sync(at(base, m), true, push, pull);
            assert!(!plan.pull, "变更唤醒不拉取（避免覆盖用户刚做的改动）");
            assert_eq!(
                next_wake(at(base, m), push, pull),
                Some(mins(30 - m)),
                "第 {m} 分钟的变更唤醒不得把拉取到期点往后推（否则自动拉取永远不跑）"
            );
        }
        // 第 30 分钟定时唤醒：拉取照常到期
        assert!(
            plan_sync(at(base, 30), false, push, pull).pull,
            "第 30 分钟定时唤醒必须能拉到"
        );
    }

    /// 未启用的动作永不执行，也不参与唤醒点计算。
    #[test]
    fn disabled_action_never_runs() {
        let base = Instant::now();
        let off = action(base, None);
        assert!(!off.is_enabled());
        assert_eq!(off.time_until_due(base), None);
        let plan = plan_sync(at(base, 1000), false, action(base, Some(60)), off);
        assert!(!plan.pull);
        assert!(plan.push);
        assert_eq!(
            next_wake(base, off, off),
            None,
            "均未启用 → 回落 30s 配置轮询"
        );
        assert_eq!(next_wake(base, action(base, Some(60)), off), Some(mins(60)));
    }

    /// 变更唤醒：开了自动推送就推一次（与间隔无关），未开则什么也不做。
    #[test]
    fn change_wake_pushes_regardless_of_interval() {
        let base = Instant::now();
        let plan = plan_sync(
            at(base, 1),
            true,
            action(base, Some(360)),
            action(base, Some(30)),
        );
        assert!(plan.push, "变更唤醒即推（改完就同步），与 360 分钟间隔无关");
        assert!(!plan.pull, "变更唤醒绝不拉取");
        // notify_change 与开关无关（导入/编辑都会调用），未启用时不能误推
        let plan = plan_sync(
            at(base, 1),
            true,
            action(base, None),
            action(base, Some(30)),
        );
        assert!(!plan.push && !plan.pull);
    }

    /// 关掉再打开应重新等一个完整间隔，而不是「上次执行是三天前」就立刻同步。
    #[test]
    fn re_enable_restarts_the_interval() {
        let base = Instant::now();
        let mut clock = ActionClock::disabled();
        clock.sync_enabled(true, base);
        // 刚跑完（第 30 分钟）→ 距下次到期还有 30 分钟，不是「立刻又到期」
        clock.mark_run(at(base, 30));
        assert_eq!(clock.time_until_due(at(base, 30), mins(30)), Some(mins(30)));
        assert_eq!(clock.time_until_due(at(base, 59), mins(30)), Some(mins(1)));
        assert_eq!(
            clock.time_until_due(at(base, 60), mins(30)),
            Some(Duration::ZERO)
        );
        clock.sync_enabled(false, at(base, 30));
        assert_eq!(
            clock.time_until_due(at(base, 30), mins(30)),
            None,
            "停用即清空"
        );
        clock.sync_enabled(true, at(base, 30));
        let act = SyncAction {
            interval: Some(mins(30)),
            clock,
        };
        assert_eq!(act.time_until_due(at(base, 30)), Some(mins(30)));
        assert!(!act.is_due(at(base, 30)), "重新启用不该立刻同步");
        assert!(act.is_due(at(base, 60)));
    }

    /// 改间隔按「上次执行 + 新间隔」重算：缩短可立即到期，延长则顺延。
    #[test]
    fn interval_change_reanchors_from_last_run() {
        let base = Instant::now();
        let mut clock = ActionClock::disabled();
        clock.sync_enabled(true, base);
        clock.mark_run(at(base, 100));
        // 上次执行在第 100 分钟；间隔 30 → 第 130 分钟到期
        assert_eq!(
            clock.time_until_due(at(base, 100), mins(30)),
            Some(mins(30)),
            "刚跑完不该立刻又到期"
        );
        assert_eq!(
            clock.time_until_due(at(base, 130), mins(30)),
            Some(Duration::ZERO)
        );
        // 同一时刻（第 130 分钟）把间隔改成 360 → 还剩 330 分钟
        // （按上次执行时刻算，而不是从改配置那一刻重新等 360 分钟）
        assert_eq!(
            clock.time_until_due(at(base, 130), mins(360)),
            Some(mins(330))
        );
        // 把间隔从 360 缩短到 30 时，若距上次执行已超过 30 分钟则**立即到期**
        // （缩短间隔即时生效，而不是再等一个完整新间隔）
        assert_eq!(
            clock.time_until_due(at(base, 200), mins(30)),
            Some(Duration::ZERO)
        );
    }

    /// 两个动作同时到期时，唤醒点归零（本轮两个都跑）。
    #[test]
    fn both_due_yields_zero_wait() {
        let base = Instant::now();
        let push = action(base, Some(30));
        let pull = action(base, Some(30));
        assert_eq!(next_wake(at(base, 30), push, pull), Some(Duration::ZERO));
        let plan = plan_sync(at(base, 30), false, push, pull);
        assert!(plan.pull && plan.push);
    }

    /// 睡眠封顶：长间隔也要定期醒来重读配置（否则改了间隔要等最长 6 小时才生效，
    /// 因为 `webdav_config_set` 不触发变更通知）。
    ///
    /// 反证：去掉 `.min(AUTOSYNC_MAX_SLEEP)`，推送 360 分钟时下面第一条断言立刻变红。
    #[test]
    fn sleep_duration_caps_long_waits() {
        assert_eq!(sleep_duration(Some(mins(360))), AUTOSYNC_MAX_SLEEP);
        assert_eq!(sleep_duration(Some(mins(30))), AUTOSYNC_MAX_SLEEP);
        // 比封顶值更近的到期点按原值（不无谓地缩短）
        assert_eq!(
            sleep_duration(Some(Duration::from_secs(5))),
            Duration::from_secs(5)
        );
        // 已到期 → 立刻醒（不做退避；退避只用于「拿不到锁」那条路径）
        assert_eq!(sleep_duration(Some(Duration::ZERO)), Duration::ZERO);
        // 均未启用 → 配置轮询
        assert_eq!(sleep_duration(None), AUTOSYNC_CONFIG_POLL);
        // 轮询间隔本身也应小于等于封顶值，否则「均未启用」会比封顶还慢
        assert!(AUTOSYNC_CONFIG_POLL <= AUTOSYNC_MAX_SLEEP);
    }

    /// 端到端串一遍调度决策：360 分钟推送 + 30 分钟拉取，第 30 分钟那轮
    /// 睡眠为 0（拉取已到期），执行后下一个唤醒点是封顶值（30 分钟后才再到期）。
    #[test]
    fn schedule_round_trip_for_360_push_and_30_pull() {
        let base = Instant::now();
        let push = action(base, Some(360));
        let mut pull = action(base, Some(30));
        // 起点：最近到期点是拉取的 30 分钟，但睡眠被封顶
        assert_eq!(
            sleep_duration(next_wake(base, push, pull)),
            AUTOSYNC_MAX_SLEEP
        );
        // 第 30 分钟醒来：只拉不推
        let t = at(base, 30);
        let plan = plan_sync(t, false, push, pull);
        assert!(plan.pull && !plan.push);
        // 拉取执行完毕 → 重新锚定，下一个到期点是第 60 分钟
        pull.clock.mark_run(t);
        assert_eq!(next_wake(t, push, pull), Some(mins(30)));
        assert_eq!(sleep_duration(next_wake(t, push, pull)), AUTOSYNC_MAX_SLEEP);
        // 第 360 分钟：两个同时到期
        let t = at(base, 360);
        let plan = plan_sync(t, false, push, pull);
        assert!(plan.pull && plan.push);
    }

    /// 「调度参数变了」与「业务数据变了」必须是**两个独立信号**。
    ///
    /// 反证：若让 `notify_config_change` 复用 `tx`（或反过来），改一次间隔就会
    /// 顺带触发一次防抖推送 —— 把配置推上远端，而用户只是调了个间隔。
    /// 本用例在任一方向合并通道时立刻变红。
    #[test]
    fn config_change_signal_is_independent_from_data_change() {
        let hub = AutopushHub::new();
        let mut data_rx = hub.tx.subscribe();
        let mut cfg_rx = hub.cfg_tx.subscribe();

        // 只发「调度参数变更」：数据通道不得被唤醒（否则会触发推送）
        hub.notify_config_change();
        assert!(cfg_rx.has_changed().unwrap(), "配置通道应被唤醒");
        assert!(
            !data_rx.has_changed().unwrap(),
            "改间隔/开关不得触发推送（会顺带覆盖远端）"
        );
        // 标记已读，避免影响后续断言
        cfg_rx.borrow_and_update();
        data_rx.borrow_and_update();

        // 只发「业务数据变更」：配置通道不得被唤醒
        hub.notify_change();
        assert!(data_rx.has_changed().unwrap(), "数据通道应被唤醒");
        assert!(
            !cfg_rx.has_changed().unwrap(),
            "业务数据变更不得被当成调度参数变更"
        );
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
        auto_pull_interval_min: input
            .auto_pull_interval_min
            .unwrap_or(old.auto_pull_interval_min),
    };
    let db = core.db.clone();
    tokio::task::spawn_blocking(move || {
        db.with(|c| sync::config_set(c, &cfg))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(join_err)??;
    // 调度参数（开关/间隔）变了 → 让调度循环立刻重读配置。
    // 只做「重新评估」，不触发推送：改间隔不该顺带把配置推上去。
    core.autopush.notify_config_change();
    Ok(())
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
        auto_pull_interval_min: 60,
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

/// 推送前差异明细（可视 diff，UX-T5）：远端独有/本地独有逐条列出。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushDiffDetailDto {
    pub remote_exists: bool,
    /// [名称, base_url] 对
    pub remote_only_providers: Vec<[String; 2]>,
    /// [providerId, modelName] 对
    pub remote_only_models: Vec<[String; 2]>,
    pub local_only_providers: Vec<[String; 2]>,
    pub local_only_models: Vec<[String; 2]>,
    pub blocks: bool,
}

#[tauri::command]
async fn webdav_push_diff(core: State<'_, AppCore>) -> Result<PushDiffDetailDto, String> {
    let cfg = get_webdav_config(&core).await?;
    let password = get_webdav_password(&core).await?;
    let db = core.db.clone();
    let local_text = tokio::task::spawn_blocking(move || {
        db.with_any(|c| store::export::build_export_json(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)??;
    let remote = sync::try_pull(&core.http, &cfg, &password).await?;
    let d = sync::push_diff_detail(&local_text, remote.as_deref()).map_err(|e| e.to_string())?;
    let pair = |v: &[(String, String)]| {
        v.iter()
            .map(|(a, b)| [a.clone(), b.clone()])
            .collect::<Vec<[String; 2]>>()
    };
    Ok(PushDiffDetailDto {
        remote_exists: d.remote_exists,
        remote_only_providers: pair(&d.remote_only_providers),
        remote_only_models: pair(&d.remote_only_models),
        local_only_providers: pair(&d.local_only_providers),
        local_only_models: pair(&d.local_only_models),
        blocks: d.blocks(),
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

/// WebDAV 自动同步循环：定时 tick 与变更通知合并；均未启用时 30s 轮询配置等待开启。
///
/// 调度语义（bug 清单 20）：
/// - **每个动作各自判定到期**：推送看 `auto_push_interval_min`、拉取看
///   `auto_pull_interval_min`，各自从「上次执行时刻」起算。两个间隔完全独立，
///   「推送 6 小时 / 拉取 30 分钟」这类组合按用户意图生效。
/// - **定时唤醒点 = 最近的一个到期点**（既不是某个间隔本身，也不是各间隔的最小值）：
///   不会像旧实现那样用 `push_interval.or(pull_interval)` 只取其中一个间隔
///   （拉取被推送的长间隔绑死），也不会让到期早的动作把到期晚的动作一起拽跑
///   （旧实现定时分支里「开了自动推送就每 tick 都推」）。
/// - **到期点是绝对时刻，不因变更唤醒而重置**：旧实现每轮都新建 `sleep(wait)`，
///   任何一次 `notify_change()`（供应商/模型/MCP/技能任一编辑都会触发）都会把它重置，
///   于是「编辑得比间隔勤」的用户永远等不到定时 tick —— 自动拉取被饿死，
///   症状正是用户报的「自动拉取没生效」。
/// - 变更唤醒仍走防抖 30s 后推送一次（与间隔无关，这是「改完就同步」的体验保证）。
/// - 时钟用**单调时钟** `Instant`：系统时间被 NTP 校正或用户手改，不该让同步
///   饿死或瞬间风暴；挂钟毫秒（`AutoPushStatus::at_ms`）只用于界面展示。
/// - **三类唤醒**：`Wake::Timer` 定时到期（按各自到期点判定）、
///   `Wake::Change` 业务数据变更（防抖后推送一次）、`Wake::Config` 调度参数变更
///   （只重读配置，**不推送**）。后两者都不移动到期点。
/// - **读配置失败时不改时钟**：瞬时 IO 错误若被当成「用户关掉了」会清空锚点、
///   重新计时，把下一次执行推迟一整个间隔 —— 见 `current_autosync_intervals`。
fn spawn_autopush(core: AppCore, app: AppHandle) {
    // setup 闭包不在 tokio runtime 上下文内，必须经 tauri 托管 runtime spawn
    tauri::async_runtime::spawn(async move {
        let mut rx = core.autopush.tx.subscribe();
        let mut cfg_rx = core.autopush.cfg_tx.subscribe();
        let mut push_clock = ActionClock::disabled();
        let mut pull_clock = ActionClock::disabled();
        loop {
            // 读配置失败时**不动时钟**（见 current_autosync_intervals 的说明），稍后重试
            let Some((push_interval, pull_interval)) = current_autosync_intervals(&core).await
            else {
                tokio::time::sleep(AUTOSYNC_CONFIG_POLL).await;
                continue;
            };
            let now = Instant::now();
            push_clock.sync_enabled(push_interval.is_some(), now);
            pull_clock.sync_enabled(pull_interval.is_some(), now);
            let push = SyncAction {
                interval: push_interval,
                clock: push_clock,
            };
            let pull = SyncAction {
                interval: pull_interval,
                clock: pull_clock,
            };

            // 均未启用 → 30s 轮询配置；否则睡到最近的一个到期点
            let wait = sleep_duration(next_wake(now, push, pull));
            let wake = tokio::select! {
                r = rx.changed() => {
                    if r.is_err() {
                        return; // hub 已随应用退出
                    }
                    Wake::Change
                }
                r = cfg_rx.changed() => {
                    if r.is_err() {
                        return; // hub 已随应用退出
                    }
                    // 调度参数（开关/间隔）被改：立刻回到循环顶部重读配置，
                    // 让「关→开」这类跃迁被及时采样，也让新间隔立即生效。
                    Wake::Config
                }
                _ = tokio::time::sleep(wait) => Wake::Timer,
            };
            if wake == Wake::Config {
                continue;
            }
            let change = wake == Wake::Change;
            if change {
                // 变更唤醒只为自动推送服务：未开启则无需防抖等待
                if !push.is_enabled() {
                    continue;
                }
                // 防抖：等 30s 合并突发变更
                tokio::time::sleep(AUTOSYNC_DEBOUNCE).await;
                if core.autopush.suppress.load(Ordering::Relaxed) {
                    continue;
                }
            }

            // 唤醒期间配置可能被改（开关/间隔），决策一律以最新配置为准；
            // 这一次读取代了「防抖前再查一次开关」的预检 —— 若期间推送被关，
            // 下面的 `plan_sync` 会因 `push.is_enabled() == false` 得出「不推」，
            // 预检只会多一次读库、不可能多推出什么。
            let Some((push_interval, pull_interval)) = current_autosync_intervals(&core).await
            else {
                continue; // 读失败：保持时钟不动，下一轮重试
            };
            let now = Instant::now();
            push_clock.sync_enabled(push_interval.is_some(), now);
            pull_clock.sync_enabled(pull_interval.is_some(), now);
            let plan = plan_sync(
                now,
                change,
                SyncAction {
                    interval: push_interval,
                    clock: push_clock,
                },
                SyncAction {
                    interval: pull_interval,
                    clock: pull_clock,
                },
            );
            if !plan.pull && !plan.push {
                continue;
            }
            // 抢不到锁（手动推/拉进行中）则跳过本轮
            let Ok(_guard) = core.autopush.push_lock.try_lock() else {
                eprintln!("[autosync] 手动同步进行中，跳过本轮");
                // 退避：动作已到期且锁被占时必须显式等一会儿，否则 `sleep(0)` 会空转
                tokio::time::sleep(AUTOSYNC_LOCK_BACKOFF).await;
                continue;
            };
            // 拉取先于推送：先把远端更新拿下来，再按需把本机状态推上去
            //
            // 到期点按**动作结束后**的时刻重算（固定延迟），而不是动作开始前：
            // 若某次同步耗时超过间隔（远端慢/卡），用开始时刻会让下一轮立刻又判定「已到期」，
            // 变成背靠背连跑。固定延迟保证每次执行之间至少隔一个完整间隔。
            if plan.pull {
                run_auto_pull(&core, &app).await;
                pull_clock.mark_run(Instant::now());
            }
            if plan.push {
                run_auto_push(&core, &app).await;
                push_clock.mark_run(Instant::now());
            }
        }
    });
}

/// 均未启用自动同步时的配置轮询间隔。
const AUTOSYNC_CONFIG_POLL: Duration = Duration::from_secs(30);
/// 变更唤醒后的防抖窗口（合并突发变更）。
const AUTOSYNC_DEBOUNCE: Duration = Duration::from_secs(30);
/// 单轮睡眠上限：到期点再远，也至少隔这么久重新读一次配置。
///
/// 为什么必须封顶：`webdav_config_set` **不**触发变更通知（它改的是调度参数本身，
/// 不是要同步的业务数据）。若睡满一个长间隔（推送设 6 小时时），
/// 用户刚把「拉取间隔」改成 30 分钟，也要等最长 6 小时才生效 —— 表现为「改了没反应」。
/// 封顶后只是「醒来重新评估配置」，**不会提前执行动作**：是否该跑由 `plan_sync`
/// 按绝对到期点判定，与睡眠时长无关。
const AUTOSYNC_MAX_SLEEP: Duration = Duration::from_secs(60);
/// 拿不到推送锁（手动推/拉进行中）时的退避时长。
///
/// 必需：改成「睡到到期点」后，若动作**已到期**且锁被占，`next_wake` 会返回
/// `Some(ZERO)` → `sleep(0)` 立即返回 → 又拿不到锁 → `continue`，
/// 形成**空转**（旧实现每轮固定睡满一个间隔，没有这个问题）。
/// 退避保证最坏情况下也只是每 `AUTOSYNC_LOCK_BACKOFF` 重试一次。
const AUTOSYNC_LOCK_BACKOFF: Duration = Duration::from_secs(5);

/// 自动同步循环本轮是被什么唤醒的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wake {
    /// 业务数据变更（供应商/模型/MCP/技能等）→ 防抖后推一次
    Change,
    /// 调度参数变更（开关/间隔）→ 仅重新评估，不推送
    Config,
    /// 定时到期
    Timer,
}

/// 一个自动同步动作的调度时钟（单调）。
///
/// 记录「上次实际执行时刻」与「本次启用起点」，取二者中**较晚**者作为计时基准 ——
/// 于是**只有真正执行过**才会把到期点往后推，变更唤醒、配置重读都不会。
/// 这正是旧实现缺的那一环：旧代码的定时器每轮重建，会被变更唤醒重置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActionClock {
    /// 上次实际执行时刻（None = 尚未执行过）
    last_run: Option<Instant>,
    /// 本次「启用」的起点（None = 未启用）
    enabled_at: Option<Instant>,
}

impl ActionClock {
    fn disabled() -> Self {
        Self {
            last_run: None,
            enabled_at: None,
        }
    }

    /// 同步「是否启用」：启用且尚无起点则记下起点；停用则清空（重新启用重新计时）。
    ///
    /// 停用清空是刻意的：用户关掉再打开应重新等一个完整间隔，
    /// 而不是因为「上次执行是三天前」而在打开瞬间立刻同步一次。
    fn sync_enabled(&mut self, enabled: bool, now: Instant) {
        match (enabled, self.enabled_at) {
            (true, None) => self.enabled_at = Some(now),
            (false, _) => {
                self.enabled_at = None;
                self.last_run = None;
            }
            (true, Some(_)) => {}
        }
    }

    /// 距下次到期的剩余时长；未启用返回 None，已到期/过期返回 `Some(ZERO)`。
    fn time_until_due(&self, now: Instant, interval: Duration) -> Option<Duration> {
        let base = self.last_run.or(self.enabled_at)?;
        Some(interval.saturating_sub(now.saturating_duration_since(base)))
    }

    /// 标记刚执行过（把到期点推到 `now + interval`）。
    fn mark_run(&mut self, now: Instant) {
        self.last_run = Some(now);
    }
}

/// 一个自动同步动作的完整调度输入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SyncAction {
    /// 间隔；None = 未启用
    interval: Option<Duration>,
    clock: ActionClock,
}

impl SyncAction {
    fn is_enabled(&self) -> bool {
        self.interval.is_some()
    }

    fn time_until_due(&self, now: Instant) -> Option<Duration> {
        self.interval
            .and_then(|i| self.clock.time_until_due(now, i))
    }

    fn is_due(&self, now: Instant) -> bool {
        self.time_until_due(now) == Some(Duration::ZERO)
    }
}

/// 本次唤醒的执行计划。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SyncPlan {
    pull: bool,
    push: bool,
}

/// 距下一次定时唤醒的时长：两个动作中**最近**的一个到期点。
/// 均未启用返回 None（调用方回落到配置轮询）。
fn next_wake(now: Instant, push: SyncAction, pull: SyncAction) -> Option<Duration> {
    match (push.time_until_due(now), pull.time_until_due(now)) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// 本轮该睡多久：取「最近到期点」，但封顶在 `AUTOSYNC_MAX_SLEEP`；
/// 均未启用时回落到 `AUTOSYNC_CONFIG_POLL`。
///
/// 只影响「隔多久重新评估一次配置」，不影响动作是否执行（见 `plan_sync`）。
fn sleep_duration(next_due: Option<Duration>) -> Duration {
    match next_due {
        Some(d) => d.min(AUTOSYNC_MAX_SLEEP),
        None => AUTOSYNC_CONFIG_POLL,
    }
}

/// 本次唤醒该执行哪些动作。
///
/// - 变更唤醒：推送一次（与间隔无关，这是「改完就同步」的体验保证），
///   **不拉取** —— 刚改完本地配置就去拉远端，可能把用户刚做的改动覆盖掉。
/// - 定时唤醒：各自按自己的间隔判定是否到期（互不影响）。
fn plan_sync(now: Instant, change_wake: bool, push: SyncAction, pull: SyncAction) -> SyncPlan {
    if change_wake {
        return SyncPlan {
            pull: false,
            push: push.is_enabled(),
        };
    }
    SyncPlan {
        pull: pull.is_due(now),
        push: push.is_due(now),
    }
}

/// 挂钟毫秒（仅用于界面展示；调度判定一律走单调时钟）。
fn wall_clock_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 自动拉取一次并记录结果（失败发系统通知）。
async fn run_auto_pull(core: &AppCore, app: &AppHandle) {
    let at = wall_clock_ms();
    let res = auto_pull_once(core).await;
    record_auto_sync(
        &core.autopush.last_pull,
        app,
        "autopull",
        "WebDAV 自动拉取失败",
        "自动拉取完成",
        at,
        &res,
    )
    .await;
}

/// 自动推送一次并记录结果（失败发系统通知）。
async fn run_auto_push(core: &AppCore, app: &AppHandle) {
    let at = wall_clock_ms();
    let res = auto_push_guarded(core).await;
    record_auto_sync(
        &core.autopush.last,
        app,
        "autopush",
        "WebDAV 自动推送失败",
        "自动推送成功",
        at,
        &res,
    )
    .await;
}

/// 记录并外显一次自动同步结果：成功静默，失败发系统通知。
async fn record_auto_sync(
    slot: &Arc<tokio::sync::Mutex<Option<AutoPushStatus>>>,
    app: &AppHandle,
    tag: &str,
    fail_title: &str,
    ok_message: &str,
    at_ms: u64,
    res: &Result<(), String>,
) {
    let st = match res {
        Ok(()) => AutoPushStatus {
            at_ms,
            ok: true,
            message: ok_message.to_string(),
        },
        Err(e) => AutoPushStatus {
            at_ms,
            ok: false,
            message: e.clone(),
        },
    };
    eprintln!(
        "[{tag}] {} {}",
        if st.ok { "ok" } else { "err" },
        st.message
    );
    if !st.ok {
        notify_autosync(app, fail_title.to_string(), st.message.clone());
    }
    *slot.lock().await = Some(st);
}

/// 读取一次配置，同时给出推送/拉取两个间隔（`Some((None, None))` = 读到了但均未启用）。
///
/// 一次读库同时取两个值：既省一次查询，也避免两次读之间配置被改而拿到
/// 「半个旧配置 + 半个新配置」。
///
/// **返回 `None` 表示「读配置失败」，与「读到了但未启用」严格区分**：
/// 调用方在读失败时**不得**更新调度时钟。否则一次瞬时错误（SQLite busy、
/// 磁盘抖动、`spawn_blocking` join 失败）会被 `sync_enabled(false, _)` 当成
/// 「用户关掉了」→ 清空锚点 → 重新计时，把下一次执行整体推迟一个完整间隔。
/// 那正是本条目要消灭的症状（自动同步被无限推迟），只是触发源换成了偶发 IO 错误。
async fn current_autosync_intervals(
    core: &AppCore,
) -> Option<(Option<Duration>, Option<Duration>)> {
    let db = core.db.clone();
    let res = tokio::task::spawn_blocking(move || {
        db.with_any(|c| sync::config_get(c).map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err);
    let cfg: WebDavConfig = match res {
        // 读到了配置
        Ok(Ok(Some(c))) => c,
        // 读到了「没有配置」→ 均未启用（这是确定的信息，可以更新时钟）
        Ok(Ok(None)) => return Some((None, None)),
        // 读取失败 → 不返回结论，由调用方保持时钟不动
        _ => return None,
    };
    Some((
        cfg.auto_push_enabled
            .then(|| Duration::from_secs(u64::from(cfg.normalized_push_interval()) * 60)),
        cfg.auto_pull_enabled
            .then(|| Duration::from_secs(u64::from(cfg.normalized_pull_interval()) * 60)),
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

    // 本轮摘要：失败供应商 (name, message) 累积，轮末写入内存槽供横幅读取
    let mut down: Vec<(String, String)> = Vec::new();

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
                    notify_user(
                        app,
                        format!("供应商『{}』已恢复", row.name),
                        "健康检查：连接已恢复正常".to_string(),
                    );
                }
            }
            Err(msg) => {
                down.push((row.name.clone(), msg.clone()));
                let id = row.id.clone();
                let m2 = msg.clone();
                let db = core.db.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = db.with(|c| store::provider_mark_err(c, &id, &m2));
                })
                .await;
                if !first_round && !was_failing {
                    eprintln!("[health] 失败: {} - {msg}", row.name);
                    notify_user(
                        app,
                        format!("供应商『{}』连接失败", row.name),
                        msg.chars().take(HEALTH_NOTIFY_SUMMARY_CHARS).collect(),
                    );
                }
            }
        }
    }
    // 轮末写入摘要（即便 0 失败也刷新 checked_at，横幅据此显示"已检查"）
    *core.health_summary.lock().unwrap() = HealthSummary {
        checked_at_ms: Some(store::now_ms() as i64),
        down: down
            .into_iter()
            .map(|(name, message)| HealthDownProvider { name, message })
            .collect(),
    };
    Ok(())
}

/// 读取最近一轮健康检查摘要（UX-T3 横幅数据源）。
#[tauri::command]
async fn health_summary(core: State<'_, AppCore>) -> Result<HealthSummary, String> {
    Ok(core.health_summary.lock().unwrap().clone())
}

/// 发送系统通知（失败只打日志，不影响调用方流程）。
///
/// 供应商健康跃迁与「启动失败」共用 —— 不要在别处再写一份 `notification().builder()`。
fn notify_user(app: &AppHandle, title: String, body: String) {
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
    // 规则缓存用 AppCore 里那一份：`gateway_key_rules_set` 保存完就失效它，
    // 否则用户要等 5s TTL 才看到新规则生效（会以为没保存上）。
    let ctx =
        GatewayCtx::new(core.db.clone(), core.logs.clone()).with_rules(core.key_rules.clone());

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

/// 显示并把主窗口置前。
///
/// 托盘菜单「显示主窗口」与单实例回调**共用**这一个实现 —— 不要复制两份。
fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn main() {
    tauri::Builder::default()
        // 单实例：第二个实例不启动，改为把已有窗口置前 —— 否则双击两次图标会起两个
        // 网关进程，端口自动顺延后两个网关各监听一个端口，用户不知道客户端该连哪个。
        //
        // 必须放在插件链**首位**：它的单实例判定发生在 Tauri runtime 初始化阶段，
        // 早于 `setup()`，所以第二个实例不会执行 `Db::open`（不会碰数据目录）。
        // 这条时序要在真机上确认（见方案 T4.2 第 3 点的回退方案）。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main_window(app);
        }))
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

            // 迁移失败 = 中止启动（storage §4 早拦截）。此前这里是裸 `?` —— 失败直接
            // panic 退出（`.expect("error while running jai")`），用户只看到一个闪退的
            // 图标，且不知道有没有回滚点。现在把「迁移前备份在哪」一并告诉用户。
            let db = match Db::open(&db_str) {
                Ok(db) => db,
                Err(e) => {
                    let hint = store::latest_migration_backup(&db_path)
                        .map(|p| format!("；迁移前备份在 {}", p.display()))
                        .unwrap_or_default();
                    let msg = format!("数据库打开/迁移失败：{e}{hint}");
                    eprintln!("[jai] {msg}");
                    notify_user(app.handle(), "JAI 启动失败".into(), msg.clone());
                    return Err(msg.into());
                }
            };
            // 启动按需回收磁盘（bug 清单 14 的永久解法）：SQLite 删除大行后文件不会自动缩小，
            // 历史事故实测出现过「895 MB 库里 894 MB 全是空洞」。必须在**日志连接建立之前**
            // 执行（VACUUM 需要独占），失败仅告警、不阻塞启动。
            {
                let cfg = store::retention::ReclaimConfig::from_env();
                match db.with(|c| store::retention::reclaim_if_bloated(c, cfg)) {
                    Ok(Some((before, after))) => eprintln!(
                        "[store] 启动回收磁盘：{} MB → {} MB（空闲页占比超过阈值）",
                        before / 1048576,
                        after / 1048576
                    ),
                    Ok(None) => {}
                    Err(e) => eprintln!("[store] 启动回收跳过（不影响启动）: {e}"),
                }
            }
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

            // 存量自愈（bug 清单 14）：历史自引用快照可能已把 meta 单行撑到几百 MB，
            // 启动时用当前配置重建一份干净快照。失败不阻塞启动（快照只是回退手段）。
            if let Ok(Some(old)) = db.with(sync::heal_oversized_snapshot) {
                eprintln!("[sync] 已重建异常快照：旧 {old} 字节（自引用递归遗留，见 bug 14）");
            }

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
                health_summary: std::sync::Arc::new(
                    std::sync::Mutex::new(HealthSummary::default()),
                ),
                probe_receipts: std::sync::Arc::new(gateway_core::probe::ProbeReceiptStore::new()),
                key_rules: std::sync::Arc::new(
                    gateway_core::server::security::KeyRulesCache::new(),
                ),
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
                        "show" => show_main_window(app),
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
            provider_probe_draft,
            provider_discover_models,
            model_list,
            model_set_limits,
            model_set_alias,
            model_toggle,
            model_set_modalities,
            model_set_reasoning_levels,
            provider_set_reasoning_levels,
            model_set_max_tools,
            provider_set_max_tools,
            gateway_key_info,
            gateway_key_list,
            gateway_key_create,
            gateway_key_revoke,
            gateway_key_reveal,
            gateway_key_regenerate,
            gateway_key_rules_get,
            gateway_key_rules_set,
            gateway_key_rules_options,
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
            webdav_push_diff,
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
            health_summary,
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
