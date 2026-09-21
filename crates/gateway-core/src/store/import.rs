//! 配置导入（roadmap M7 + WebDAV 同步增强）：解析 `jai-export/v1` JSON，按名称+Base URL 去重 upsert。
//!
//! 密钥随配置同步：远端 `api_key` 非空才覆盖本地（空不清空）；新建供应商直接带 key/website，
//! 导入后立即可路由。顶层 `gateway_key` 非空且与本地 active 不同时轮换（吊销旧 key）。
//! meta 白名单：`webdav_url/username/directory/webdav_password`。
//! **不含** `webdav_auto_*`（本机调度偏好）——见 `sync::MACHINE_LOCAL_META_KEYS`。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::modality::Modality;

use super::{
    gw_key_active, gw_key_rotate, meta_set, model_upsert, provider_get_by_name_base,
    provider_insert, provider_set_api_key, provider_set_website, provider_update_fields,
    ProviderRow,
};

/// 导入 meta 的白名单键（同步契约：仅**共享的**连接配置与凭据；端口/CORS 等本机设置不迁移）。
///
/// 四个 `webdav_auto_*`（自动推送开关/间隔、自动拉取开关/间隔）**故意不在列**：
/// 它们描述「这台机器多久同步一次」，属本机偏好。此前在列内会造成确定性故障 ——
/// A 机 `auto_pull_enabled=0`（默认关）随导出物到达新机器 B，把 B 用户刚打开的
/// 「自动拉取」在第一次拉取时自己关掉，此后 B 再也不自动拉，表现为
/// 「换台电脑就没成功过」（bug 清单 19）。排除清单以 `sync::MACHINE_LOCAL_META_KEYS`
/// 为单一事实源，导出侧同样剔除。
const META_IMPORT_WHITELIST: [&str; 4] = [
    "webdav_url",
    "webdav_username",
    "webdav_directory",
    "webdav_password",
];

/// 导入报告：可用于 UI/CLI 的差异与缺失凭据清单。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub providers_imported: usize,
    pub providers_skipped_duplicate: usize,
    pub models_imported: usize,
    pub missing_keys: Vec<String>,
    pub invalid_providers: Vec<String>,
}

/// 解析并应用导出 JSON。`strict` 为 true 时任一供应商无效即整体失败（默认 false：部分成功）。
pub fn apply_import(c: &Connection, text: &str, strict: bool) -> Result<ImportReport, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("导入 JSON 解析失败: {e}"))?;

    let format = v
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !format.starts_with("jai-export/") {
        return Err("不是 JAI 导出文件（缺少 format=jai-export/*）".into());
    }

    let providers = v
        .get("providers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let models = v
        .get("models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut report = ImportReport::default();
    // 导出文件中的原 provider id → 本地 provider id
    let mut id_map: Map<String, Value> = Map::new();
    let mut seen_local: Vec<(String, String)> = Vec::new(); // (name, base_url) 去重

    for p in providers {
        let Some(name) = p
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            report.invalid_providers.push("缺少 name".into());
            if strict {
                return Err("存在缺少 name 的供应商".into());
            }
            continue;
        };
        let Some(base_url) = p
            .get("base_url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            report
                .invalid_providers
                .push(format!("{name}: 缺少 base_url"));
            if strict {
                return Err(format!("供应商 {name} 缺少 base_url"));
            }
            continue;
        };
        let family = p
            .get("family")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // 白名单与 providers.family 的 CHECK 约束同款拼写（0003 起 openai_responses 合法，
        // 此前漏了它 —— 该族供应商无法随导出配置在机器间同步）
        if !matches!(
            family.as_str(),
            "openai_compat" | "openai_responses" | "anthropic" | "gemini"
        ) {
            report
                .invalid_providers
                .push(format!("{name}: 未知协议族 {family}"));
            if strict {
                return Err(format!("供应商 {name} 未知协议族 {family}"));
            }
            continue;
        }

        let enabled = p.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        let priority = p.get("priority").and_then(Value::as_i64).unwrap_or(100);
        let extra_headers = p
            .get("extra_headers")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty());
        // 远端密钥非空才覆盖（空不清空本地）；新建时直接随行
        let remote_api_key = p
            .get("api_key")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let website = p
            .get("website")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        // 推理档位值域（0011）：JSON 数组；缺省 None = 未声明（不覆盖本地已有声明）
        let remote_effort_levels = p
            .get("reasoning_effort_levels")
            .and_then(crate::effort::from_json_array);
        // 工具声明数上限（0012）：缺省 None = 未声明（不覆盖本地已有声明）
        let remote_max_tools = p
            .get("max_tools")
            .and_then(Value::as_i64)
            .filter(|n| *n > 0);

        // 本地按 (name, base_url) 去重；同一导出文件内重复只导入一次
        let dup_in_file = seen_local.iter().any(|(n, b)| n == name && b == base_url);
        let existing = provider_get_by_name_base(c, name, base_url).map_err(|e| e.to_string())?;
        let provider_id = if dup_in_file {
            if let Some(prev) = id_map.get(name) {
                prev.as_str().unwrap_or_default().to_string()
            } else {
                // 不会发生：dup_in_file 表示已写入 map
                continue;
            }
        } else if let Some(row) = existing {
            seen_local.push((name.to_string(), base_url.to_string()));
            // 去重命中：更新启停/优先级/扩展头（名称/Base URL 不变）
            if row.name != name || row.base_url != base_url {
                let _ = provider_update_fields(
                    c,
                    &row.id,
                    Some(name),
                    Some(base_url),
                    Some(priority),
                    None,
                    Some(extra_headers.as_deref()),
                );
            } else {
                let _ = provider_update_fields(
                    c,
                    &row.id,
                    None,
                    None,
                    Some(priority),
                    None,
                    Some(extra_headers.as_deref()),
                );
            }
            if row.enabled != enabled {
                super::provider_set_enabled(c, &row.id, enabled).map_err(|e| e.to_string())?;
            }
            if let Some(k) = &remote_api_key {
                provider_set_api_key(c, &row.id, Some(k)).map_err(|e| e.to_string())?;
            }
            if let Some(w) = &website {
                provider_set_website(c, &row.id, Some(w)).map_err(|e| e.to_string())?;
            }
            if let Some(lv) = &remote_effort_levels {
                super::provider_set_reasoning_levels(c, &row.id, Some(lv))
                    .map_err(|e| e.to_string())?;
            }
            if let Some(n) = remote_max_tools {
                super::provider_set_max_tools(c, &row.id, Some(n)).map_err(|e| e.to_string())?;
            }
            report.providers_skipped_duplicate += 1;
            row.id
        } else {
            seen_local.push((name.to_string(), base_url.to_string()));
            let id = uuid::Uuid::now_v7().to_string();
            let row = ProviderRow {
                id: id.clone(),
                name: name.to_string(),
                base_url: base_url.to_string(),
                family: family.clone(),
                enabled,
                priority,
                weight: 1,
                extra_headers,
                // 新建即带凭据：导入后立即可路由，消除「待录入密钥」状态
                api_key: remote_api_key.clone(),
                website: website.clone(),
                reasoning_effort_levels: remote_effort_levels.clone(),
                max_tools: remote_max_tools,
                last_ok_at: None,
                last_err_at: None,
                last_err_msg: None,
                created_at: super::now_ms(),
                updated_at: super::now_ms(),
            };
            provider_insert(c, &row).map_err(|e| e.to_string())?;
            report.providers_imported += 1;
            if remote_api_key.is_none() {
                report.missing_keys.push(name.to_string());
            }
            id
        };
        id_map.insert(
            p.get("id")
                .and_then(Value::as_str)
                .unwrap_or(name)
                .to_string(),
            Value::String(provider_id.clone()),
        );
    }

    // 模型：按导出 providerId 映射到本地 provider；找不到则跳过
    for m in models {
        let Some(pid_ref) = m.get("providerId").and_then(Value::as_str) else {
            continue;
        };
        let Some(local_id) = id_map.get(pid_ref).and_then(Value::as_str) else {
            continue;
        };
        let model_name = m
            .get("modelName")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if model_name.is_empty() {
            continue;
        }
        let context_window = m.get("contextWindow").and_then(Value::as_i64);
        let max_output_tokens = m
            .get("maxOutputTokens")
            .and_then(Value::as_i64)
            .unwrap_or(4096);
        let enabled = m.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        // 模态集合随导出快照携带（0010）；缺省 None 不覆盖本地已有标注。
        // 老快照（0009 期）只有 supportsMultimodal 布尔 → 回填为等价模态集合：
        // true ⇒ 文本+图像，false ⇒ 仅文本。
        let legacy_bool = m.get("supportsMultimodal").and_then(Value::as_bool);
        let input_modalities = m
            .get("inputModalities")
            .and_then(crate::modality::from_json_array)
            .or_else(|| match legacy_bool {
                Some(true) => Some(vec![Modality::Text, Modality::Image]),
                Some(false) => Some(vec![Modality::Text]),
                None => None,
            });
        let output_modalities = m
            .get("outputModalities")
            .and_then(crate::modality::from_json_array);
        model_upsert(
            c,
            local_id,
            model_name,
            context_window,
            max_output_tokens,
            input_modalities.as_deref(),
            output_modalities.as_deref(),
        )
        .map_err(|e| e.to_string())?;
        // 推理档位值域（0011）：与模态同口径 —— 缺省不覆盖本地已有声明
        if let Some(levels) = m
            .get("reasoningEffortLevels")
            .and_then(crate::effort::from_json_array)
        {
            if let Ok(Some(model_row)) = super::model_get_by_provider_name(c, local_id, model_name)
            {
                super::model_set_reasoning_levels(c, &model_row.id, Some(&levels))
                    .map_err(|e| e.to_string())?;
            }
        }
        // 工具声明数上限（0012，模型级）：同口径 —— 缺省不覆盖本地
        if let Some(n) = m.get("maxTools").and_then(Value::as_i64).filter(|n| *n > 0) {
            if let Ok(Some(model_row)) = super::model_get_by_provider_name(c, local_id, model_name)
            {
                super::model_set_max_tools(c, &model_row.id, Some(n)).map_err(|e| e.to_string())?;
            }
        }
        if !enabled {
            if let Ok(Some(model_row)) = super::model_get_by_provider_name(c, local_id, model_name)
            {
                let _ = super::model_toggle(c, &model_row.id, false);
            }
        }
        report.models_imported += 1;
    }

    // 网关密钥：远端非空且与本地 active 不同 → 轮换（吊销旧 key）；相同则跳过
    if let Some(remote_key) = v
        .get("gateway_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let local_active = gw_key_active(c).map_err(|e| e.to_string())?;
        let differs = local_active
            .as_ref()
            .map(|k| k.key != remote_key)
            .unwrap_or(true);
        if differs {
            gw_key_rotate(c, remote_key, Some("同步导入")).map_err(|e| e.to_string())?;
        }
    }

    // meta 白名单应用：仅 WebDAV 连接配置与密码。
    // 第二个条件（本机调度偏好）是纵深防御：即便日后有人把 `webdav_auto_*` 又加回白名单，
    // 这里也会拦住，不让对方机器的同步开关覆盖本机（bug 清单 19 的守门线）。
    if let Some(meta) = v.get("meta").and_then(Value::as_array) {
        for kv in meta {
            let (Some(k), Some(val)) = (
                kv.get(0).and_then(Value::as_str),
                kv.get(1).and_then(Value::as_str),
            ) else {
                continue;
            };
            if META_IMPORT_WHITELIST.contains(&k)
                && !crate::sync::is_machine_local_meta_key(k)
                && !val.trim().is_empty()
            {
                meta_set(c, k, val).map_err(|e| e.to_string())?;
            }
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::open_and_migrate;

    fn export_json() -> String {
        r#"{
            "format":"jai-export/v1",
            "exportedAt":1,
            "meta":[],
            "providers":[
                {"id":"p_exp","name":"Imported","base_url":"https://api.x.test/v1","family":"openai_compat","enabled":true,"priority":50,"extra_headers":null}
            ],
            "models":[
                {"id":"m_exp","providerId":"p_exp","modelName":"gpt-4o","upstreamModelId":null,"contextWindow":128000,"maxOutputTokens":4096,"enabled":true}
            ]
        }"#
        .to_string()
    }

    #[test]
    fn import_creates_provider_models_and_missing_key() {
        let c = open_and_migrate(":memory:").unwrap();
        let report = apply_import(&c, &export_json(), false).unwrap();
        assert_eq!(report.providers_imported, 1);
        assert_eq!(report.models_imported, 1);
        assert_eq!(report.missing_keys, vec!["Imported"]);
    }

    #[test]
    fn import_deduplicates_by_name_and_base() {
        let c = open_and_migrate(":memory:").unwrap();
        let _ = apply_import(&c, &export_json(), false).unwrap();
        let report = apply_import(&c, &export_json(), false).unwrap();
        assert_eq!(report.providers_skipped_duplicate, 1);
        assert_eq!(report.providers_imported, 0);
        assert!(report.missing_keys.is_empty(), "重复导入不重复计入缺失密钥");
    }

    #[test]
    fn import_rejects_wrong_format() {
        let c = open_and_migrate(":memory:").unwrap();
        assert!(apply_import(&c, "{}", false).is_err());
    }

    /// 往返：build_export_json → apply_import，凭据/官网/网关密钥/WebDAV 全部就位。
    #[test]
    fn import_roundtrip_carries_secrets() {
        let c = open_and_migrate(":memory:").unwrap();
        let now = crate::store::now_ms();
        c.execute(
            "INSERT INTO providers(id,name,base_url,family,enabled,priority,weight,api_key,website,created_at,updated_at)
             VALUES ('p1','Prov','https://api.r.test/v1','openai_compat',1,100,1,'sk-upstream-rt','https://r.test',?1,?1)",
            rusqlite::params![now],
        )
        .unwrap();
        c.execute(
            "INSERT INTO gateway_keys(id,key,prefix,created_at) VALUES ('k1','sk-jai-remote-active-key','sk-jai-re',?1)",
            rusqlite::params![now],
        )
        .unwrap();
        crate::store::meta_set(&c, "webdav_url", "https://dav.test/").unwrap();
        crate::store::meta_set(&c, "webdav_password", "dav-pass").unwrap();
        crate::store::meta_set(&c, "gateway_port", "1314").unwrap();
        crate::store::meta_set(&c, "cors_allow", "[\"https://a.b\"]").unwrap();

        let exported = crate::store::export::build_export_json(&c).unwrap();
        // 导入到另一个全新库（模拟换机器）
        let c2 = open_and_migrate(":memory:").unwrap();
        let report = apply_import(&c2, &exported, false).unwrap();
        assert_eq!(report.providers_imported, 1);
        assert!(
            report.missing_keys.is_empty(),
            "带密钥的新供应商不应进缺失清单"
        );

        let row = crate::store::provider_list(&c2).unwrap().remove(0);
        assert_eq!(row.api_key.as_deref(), Some("sk-upstream-rt"));
        assert_eq!(row.website.as_deref(), Some("https://r.test"));
        assert_eq!(
            crate::store::gw_key_active(&c2).unwrap().unwrap().key,
            "sk-jai-remote-active-key"
        );
        assert_eq!(
            crate::store::meta_get(&c2, "webdav_password")
                .unwrap()
                .as_deref(),
            Some("dav-pass")
        );
        assert_eq!(
            crate::store::meta_get(&c2, "webdav_url")
                .unwrap()
                .as_deref(),
            Some("https://dav.test/")
        );
        // 白名单外 meta（端口/CORS）不导入
        assert_eq!(crate::store::meta_get(&c2, "gateway_port").unwrap(), None);
        assert_eq!(crate::store::meta_get(&c2, "cors_allow").unwrap(), None);
    }

    /// 0011/0012 往返：供应商级 + 模型级「推理档位值域」与「工具数上限」随导出/导入落地，
    /// 且 `openai_responses` 供应商不再被误判为「未知协议族」。
    #[test]
    fn roundtrip_carries_reasoning_effort_levels_and_responses_family() {
        let c = open_and_migrate(":memory:").unwrap();
        let now = crate::store::now_ms();
        c.execute(
            "INSERT INTO providers(id,name,base_url,family,enabled,priority,weight,reasoning_effort_levels,created_at,updated_at)
             VALUES ('p1','基元律动','https://tokenrhythm.studio/v1','openai_compat',1,100,1,'low,medium,high,xhigh,max',?1,?1)",
            rusqlite::params![now],
        )
        .unwrap();
        c.execute(
            "INSERT INTO providers(id,name,base_url,family,enabled,priority,weight,created_at,updated_at)
             VALUES ('p2','Responses 上游','https://api.resp.test/v1','openai_responses',1,100,1,?1,?1)",
            rusqlite::params![now],
        )
        .unwrap();
        crate::store::model_upsert(&c, "p1", "deepseek-flash", Some(512_000), 8192, None, None)
            .unwrap();
        let m = crate::store::model_get_by_provider_name(&c, "p1", "deepseek-flash")
            .unwrap()
            .unwrap();
        crate::store::model_set_reasoning_levels(&c, &m.id, Some(&["high".into()])).unwrap();
        crate::store::model_set_max_tools(&c, &m.id, Some(256)).unwrap();
        crate::store::provider_set_max_tools(&c, "p2", Some(512)).unwrap();

        let exported = crate::store::export::build_export_json(&c).unwrap();
        let c2 = open_and_migrate(":memory:").unwrap();
        let report = apply_import(&c2, &exported, false).unwrap();
        assert!(
            report.invalid_providers.is_empty(),
            "openai_responses 是合法族，不应进无效清单：{:?}",
            report.invalid_providers
        );
        assert_eq!(report.providers_imported, 2);
        assert_eq!(
            report.models_imported, 1,
            "两张供应商表都要落地，模型随 p1 导入"
        );

        let providers = crate::store::provider_list(&c2).unwrap();
        let jy = providers.iter().find(|p| p.name == "基元律动").unwrap();
        assert_eq!(
            jy.reasoning_effort_levels.as_deref(),
            Some(
                &[
                    "low".to_string(),
                    "medium".into(),
                    "high".into(),
                    "xhigh".into(),
                    "max".into()
                ][..]
            )
        );
        let resp = providers
            .iter()
            .find(|p| p.name == "Responses 上游")
            .unwrap();
        assert_eq!(resp.reasoning_effort_levels, None);

        // 0012 工具数上限：供应商级 + 模型级都要随导出/导入携带
        assert_eq!(jy.max_tools, None, "基元律动未声明上限");
        assert_eq!(resp.max_tools, Some(512), "供应商级上限应携带");

        let m2 = crate::store::model_get_by_provider_name(&c2, &jy.id, "deepseek-flash")
            .unwrap()
            .unwrap();
        assert_eq!(
            m2.reasoning_effort_levels.as_deref(),
            Some(&["high".to_string()][..]),
            "模型级声明应随导出/导入携带"
        );
        assert_eq!(m2.max_tools, Some(256), "模型级上限应携带");
    }

    /// 远端 api_key 非空才覆盖；空不清空本地。
    #[test]
    fn import_overwrites_existing_key_only_when_remote_nonempty() {
        let c = open_and_migrate(":memory:").unwrap();
        let now = crate::store::now_ms();
        c.execute(
            "INSERT INTO providers(id,name,base_url,family,enabled,priority,weight,api_key,created_at,updated_at)
             VALUES ('p1','Local','https://api.l.test/v1','openai_compat',1,100,1,'sk-local-keep',?1,?1)",
            rusqlite::params![now],
        )
        .unwrap();

        // 远端空 → 本地保留
        let remote_empty = r#"{
            "format":"jai-export/v1",
            "meta":[],
            "providers":[{"name":"Local","base_url":"https://api.l.test/v1","family":"openai_compat"}]
        }"#;
        apply_import(&c, remote_empty, false).unwrap();
        let row = crate::store::provider_get(&c, "p1").unwrap().unwrap();
        assert_eq!(row.api_key.as_deref(), Some("sk-local-keep"));

        // 远端非空 → 覆盖
        let remote_full = r#"{
            "format":"jai-export/v1",
            "meta":[],
            "providers":[{"name":"Local","base_url":"https://api.l.test/v1","family":"openai_compat","api_key":"sk-remote-new"}]
        }"#;
        apply_import(&c, remote_full, false).unwrap();
        let row = crate::store::provider_get(&c, "p1").unwrap().unwrap();
        assert_eq!(row.api_key.as_deref(), Some("sk-remote-new"));
    }

    /// gateway_key 拉取覆盖语义：不同则轮换（吊销旧 key），相同则幂等跳过。
    #[test]
    fn import_rotates_gateway_key_only_on_diff() {
        let c = open_and_migrate(":memory:").unwrap();
        let now = crate::store::now_ms();
        c.execute(
            "INSERT INTO gateway_keys(id,key,prefix,created_at) VALUES ('k1','sk-jai-local-old-key000','sk-jai-lo',?1)",
            rusqlite::params![now],
        )
        .unwrap();

        let pull = |key: &str| {
            format!(
                r#"{{"format":"jai-export/v1","meta":[],"providers":[],"gateway_key":"{key}"}}"#
            )
        };

        // 不同 → 轮换为远端 key
        apply_import(&c, &pull("sk-jai-remote-new-key0000"), false).unwrap();
        let active = crate::store::gw_key_active(&c).unwrap().unwrap();
        assert_eq!(active.key, "sk-jai-remote-new-key0000");
        let revoked = c
            .query_row(
                "SELECT COUNT(*) FROM gateway_keys WHERE revoked_at IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(revoked, 1, "旧 key 被吊销保留审计痕迹");

        // 相同 → 跳过（不产生新 key 行）
        apply_import(&c, &pull("sk-jai-remote-new-key0000"), false).unwrap();
        let total = c
            .query_row("SELECT COUNT(*) FROM gateway_keys", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap();
        assert_eq!(total, 2, "幂等：相同 key 不轮换");
        let active = crate::store::gw_key_active(&c).unwrap().unwrap();
        assert_eq!(active.key, "sk-jai-remote-new-key0000");
    }
}
