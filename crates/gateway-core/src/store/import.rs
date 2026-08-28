//! 配置导入（roadmap M7）：解析 `jai-export/v1` JSON，按名称+Base URL 去重 upsert。
//!
//! 导出文件不携带任何密钥；导入后所有供应商进入「待录入密钥」状态，报告会列出缺失凭据清单。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{
    model_upsert, provider_get_by_name_base, provider_insert, provider_update_fields, ProviderRow,
};

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
        if !matches!(family.as_str(), "openai_compat" | "anthropic" | "gemini") {
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
                keyring_ref: format!("jai/provider/{id}"),
                last_ok_at: None,
                last_err_at: None,
                last_err_msg: None,
                created_at: super::now_ms(),
                updated_at: super::now_ms(),
            };
            provider_insert(c, &row).map_err(|e| e.to_string())?;
            report.providers_imported += 1;
            report.missing_keys.push(name.to_string());
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
        model_upsert(c, local_id, model_name, context_window, max_output_tokens)
            .map_err(|e| e.to_string())?;
        if !enabled {
            if let Ok(Some(model_row)) = super::model_get_by_provider_name(c, local_id, model_name)
            {
                let _ = super::model_toggle(c, &model_row.id, false);
            }
        }
        report.models_imported += 1;
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
}
