//! 配置导出 JSON 构建（storage §8 / WebDAV 同步增强版）。
//!
//! 语义：meta + providers + models + gateway_key，密钥随配置同步——
//! 0006 起供应商凭据明文入库、WebDAV 密码存 meta，导出即完整快照，
//! 换机器拉取即用。src-tauri 的 `export_config_json` 命令直接复用本构建器。

use rusqlite::Connection;
use serde_json::{json, Value};

use super::{gw_key_active, model_list_by_provider, provider_list, StoreError};

/// 构建导出 JSON 字符串。`providers.api_key` 非空才携带；顶层 `gateway_key`
/// 为当前 active 网关密钥（无则缺省 null）。
pub fn build_export_json(c: &Connection) -> Result<String, StoreError> {
    let providers = provider_list(c)?;
    let mut models_out = Vec::new();
    for p in &providers {
        let rows = model_list_by_provider(c, &p.id)?;
        for m in rows {
            models_out.push(m);
        }
    }

    // meta KV 全量导出（webdav_url/username/directory/auto_push_*/webdav_password 随行；
    // 端口/CORS 等本机设置由导入侧白名单过滤）
    let meta_rows: Vec<(String, String)> = {
        let mut stmt = c.prepare("SELECT key,value FROM meta ORDER BY key")?;
        let it = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        it.collect::<Result<Vec<_>, _>>()?
    };

    let providers_out: Vec<Value> = providers
        .iter()
        .map(|p| {
            // 显式白名单构造：明文凭据按同步契约携带，其余敏感列天然不在行内
            let mut v = json!({
                "id": p.id,
                "name": p.name,
                "base_url": p.base_url,
                "family": p.family,
                "enabled": p.enabled,
                "priority": p.priority,
                "extra_headers": p.extra_headers,
                "website": p.website,
            });
            if let Some(k) = p.api_key.as_deref().filter(|s| !s.is_empty()) {
                v["api_key"] = Value::String(k.to_string());
            }
            v
        })
        .collect();

    let gateway_key = gw_key_active(c)?.map(|k| k.key);

    let payload = json!({
        "format": "jai-export/v1",
        "exportedAt": super::now_ms(),
        "note": "供应商 API Key / 网关 Key / WebDAV 密码随配置同步（与本地 SQLite 同级安全模型）",
        "gateway_key": gateway_key,
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
            "INSERT INTO providers(id,name,base_url,family,enabled,priority,weight,api_key,website,created_at,updated_at)
             VALUES ('p1','密钥供应商','https://api.x.com/v1','openai_compat',1,100,1,'sk-upstream-secret','https://x.com',?1,?1)",
            rusqlite::params![now],
        )
        .unwrap();
        c.execute(
            "INSERT INTO providers(id,name,base_url,family,enabled,priority,weight,created_at,updated_at)
             VALUES ('p2','无钥供应商','https://api.y.com/v1','openai_compat',1,100,1,?1,?1)",
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
            "INSERT INTO meta(key,value) VALUES ('cors_allow','[\"https://a.b\"]'),('webdav_password','dav-pass')",
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
        assert_eq!(v["providers"][0]["api_key"], "sk-upstream-secret");
        assert_eq!(v["providers"][0]["website"], "https://x.com");
        assert_eq!(v["providers"][1]["api_key"], Value::Null, "无凭据不带 api_key 字段");
        assert_eq!(v["models"][0]["modelName"], "gpt-4o");
        assert_eq!(v["gateway_key"], "sk-jai-secretvalue");
        // webdav_password 随 meta 全量导出
        let meta = v["meta"].as_array().unwrap();
        assert!(meta
            .iter()
            .any(|kv| kv[0] == "webdav_password" && kv[1] == "dav-pass"));
    }

    #[test]
    fn export_no_gateway_key_when_absent() {
        let conn = open_and_migrate(":memory:").unwrap();
        let s = build_export_json(&conn).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["gateway_key"], Value::Null);
    }

    #[test]
    fn export_carries_secrets_per_sync_contract() {
        let conn = open_and_migrate(":memory:").unwrap();
        seed(&conn);
        let s = build_export_json(&conn).unwrap();

        // 同步契约：上游密钥 / 网关密钥 / WebDAV 密码随导出携带
        assert!(s.contains("sk-upstream-secret"), "上游密钥应随导出同步");
        assert!(s.contains("sk-jai-secretvalue"), "网关密钥应随导出同步");
        assert!(s.contains("dav-pass"), "WebDAV 密码应随导出同步");
        assert!(s.contains("api_key"), "providers 应带 api_key 字段");
        // keyring 引用永不出现（字段已退场）
        assert!(
            !s.contains("keyring"),
            "keyring 引用不得出现（字段名也不要）"
        );
        assert!(!s.contains("jai/provider"), "密钥环引用地址不得出现");
    }
}
