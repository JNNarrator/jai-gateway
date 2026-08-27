//! 配置导出 JSON 构建（storage §8 / roadmap M2 验收 4）。
//!
//! 语义：meta + providers + models，**零敏感字段**——密钥环引用、网关密钥、
//! 上游密钥一律不出现。src-tauri 的 `export_config_json` 命令直接复用本构建器，
//! 让「全文扫描无敏感串」能在单测/集成层验证，而不是只能靠真机 grep。

use rusqlite::Connection;
use serde_json::{Value, json};

use super::{StoreError, provider_list, model_list_by_provider};

/// 构建导出 JSON 字符串。`providers.keyring_ref` 字段被整体剔除（字段名都不出现）。
pub fn build_export_json(c: &Connection) -> Result<String, StoreError> {
    let providers = provider_list(c)?;
    let mut models_out = Vec::new();
    for p in &providers {
        let rows = model_list_by_provider(c, &p.id)?;
        for m in rows {
            models_out.push(m);
        }
    }

    // meta KV（不含敏感项：网关密钥与 keyring 引用本就不存于 meta）
    let meta_rows: Vec<(String, String)> = {
        let mut stmt = c.prepare("SELECT key,value FROM meta ORDER BY key")?;
        let it = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        it.collect::<Result<Vec<_>, _>>()?
    };

    let providers_out: Vec<Value> = providers
        .iter()
        .map(|p| {
            // ProviderRow 的 keyring_ref 已 skip_serializing，这里再显式构造
            // 白名单字段，双保险且未来新增敏感列不泄漏。
            json!({
                "id": p.id,
                "name": p.name,
                "base_url": p.base_url,
                "family": p.family,
                "enabled": p.enabled,
                "priority": p.priority,
                "extra_headers": p.extra_headers,
            })
        })
        .collect();

    let payload = json!({
        "format": "jai-export/v1",
        "exportedAt": super::now_ms(),
        "note": "API Key 保存在各设备系统钥匙串中，不随导出迁移",
        "meta": meta_rows,
        "providers": providers_out,
        "models": models_out,
    });

    serde_json::to_string_pretty(&payload).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::open_and_migrate;

    fn seed(c: &Connection) {
        let now = crate::store::now_ms();
        c.execute(
            "INSERT INTO providers(id,name,base_url,family,enabled,priority,keyring_ref,created_at,updated_at)
             VALUES ('p1','密钥供应商','https://api.x.com/v1','openai_compat',1,100,'jai/provider/p1',?1,?1)",
            rusqlite::params![now],
        )
        .unwrap();
        c.execute(
            "INSERT INTO models(id,provider_id,model_name,context_window,max_output_tokens,enabled)
             VALUES ('m1','p1','gpt-4o',128000,4096,1)",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO gateway_keys(id,key,prefix,created_at) VALUES ('k1','sk-jai-secretvalue','sk-jai-se',?1)",
            rusqlite::params![now],
        )
        .unwrap();
        c.execute(
            "INSERT INTO meta(key,value) VALUES ('cors_allow','[\"https://a.b\"]')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn export_contains_only_expected_content() {
        let conn = open_and_migrate(":memory:").unwrap();
        seed(&conn);
        let s = build_export_json(&conn).unwrap();

        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["format"], "jai-export/v1");
        assert_eq!(v["providers"][0]["name"], "密钥供应商");
        assert_eq!(v["models"][0]["modelName"], "gpt-4o");
        assert_eq!(v["meta"][0][0], "cors_allow");
    }

    #[test]
    fn export_leaks_no_secrets() {
        let conn = open_and_migrate(":memory:").unwrap();
        seed(&conn);
        let s = build_export_json(&conn).unwrap();

        // M2 验收 4：全文扫描
        assert!(!s.contains("sk-jai"), "网关密钥全文不得出现");
        assert!(!s.contains("sk-"), "任何 sk-* 形式的密钥不得出现");
        assert!(!s.contains("keyring"), "keyring 引用不得出现（字段名也不要）");
        assert!(!s.contains("jai/provider"), "密钥环引用地址不得出现");
        assert!(!s.contains("sk-upstream"), "上游密钥不得出现");
        assert!(!s.contains("secret"), "秘密字样不得出现");
        // meta 白名单外内容不出现
        assert!(!s.contains("request_logs"));
    }
}