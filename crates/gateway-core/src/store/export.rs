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

    // meta KV 导出（webdav_url/username/directory/webdav_password 随行；
    // 端口/CORS 等本机设置由导入侧白名单过滤）。
    //
    // 排除两类 key：
    // 1) `webdav_last_snapshot`：它是「上一版导出物」本身，若随导出携带，
    //    每次推送都会把上一版快照嵌进新快照（自引用递归），体积逐次翻倍 ——
    //    实测 19 轮后单行 595 MB、DB 938 MB、WAL 629 MB、远端 `jai-config.json` 42 MB
    //    且远端时间戳备份链 0.01→0.02→…→21 MB 同步翻倍（bug 清单 14）。
    //    导入侧白名单一直过滤该 key，导出侧此前漏了；此处用 `sync::snapshot_meta_key()`
    //    单一常量比对，避免字面量散落导致再次漏改。
    // 2) `webdav_auto_*`（本机调度偏好）：见 `sync::MACHINE_LOCAL_META_KEYS` 的说明。
    //    带出去会让对方机器的开关互相覆盖（bug 清单 19「换台电脑就没成功过」）；
    //    导出侧一并剔除，还能在「只升级了一台机器」的半升级状态下保护旧版本。
    let meta_rows: Vec<(String, String)> = {
        let mut stmt = c.prepare("SELECT key,value FROM meta WHERE key <> ?1 ORDER BY key")?;
        let it = stmt.query_map([crate::sync::snapshot_meta_key()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        it.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|(k, _)| !crate::sync::is_machine_local_meta_key(k))
            .collect()
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
        // 模态集合随快照导出（0010）
        super::super::model_set_modalities(
            &conn,
            "m1",
            Some(&[
                crate::modality::Modality::Text,
                crate::modality::Modality::Image,
            ]),
            Some(&[crate::modality::Modality::Text]),
        )
        .unwrap();
        let s = build_export_json(&conn).unwrap();

        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["format"], "jai-export/v1");
        assert_eq!(v["providers"][0]["name"], "密钥供应商");
        assert_eq!(v["providers"][0]["api_key"], "sk-upstream-secret");
        assert_eq!(v["providers"][0]["website"], "https://x.com");
        assert_eq!(
            v["providers"][1]["api_key"],
            Value::Null,
            "无凭据不带 api_key 字段"
        );
        assert_eq!(v["models"][0]["modelName"], "gpt-4o");
        assert_eq!(
            v["models"][0]["supportsMultimodal"], true,
            "多模态标注应随导出快照携带"
        );
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

    /// 回归（bug 清单 14）：导出物必须**自引用安全**。
    /// 修复前导出带上 `webdav_last_snapshot` 自己，推送即把上一版快照嵌进新快照。
    #[test]
    fn export_excludes_self_snapshot_key() {
        let conn = open_and_migrate(":memory:").unwrap();
        seed(&conn);
        crate::store::meta_set(
            &conn,
            crate::sync::snapshot_meta_key(),
            "SENTINEL-OLD-SNAPSHOT",
        )
        .unwrap();

        let s = build_export_json(&conn).unwrap();
        assert!(
            !s.contains(crate::sync::snapshot_meta_key()),
            "导出物不得包含快照 key 本身"
        );
        assert!(
            !s.contains("SENTINEL-OLD-SNAPSHOT"),
            "导出物不得含上一版快照内容"
        );
        // 白名单内的其它 meta 仍随导出（同步契约不变）
        assert!(s.contains("dav-pass"), "WebDAV 密码仍应随导出");
    }

    /// 回归守门人（bug 清单 14）：`push_now` 的顺序是「构建导出 → 存为快照 → PUT」，
    /// 因此**连续推送轮次后导出体积必须恒定**。修复前每轮翻倍
    /// （实测 19 轮后本地单行 595 MB、远端 42 MB）。
    #[test]
    fn export_size_stable_across_push_cycles() {
        let conn = open_and_migrate(":memory:").unwrap();
        seed(&conn);

        let mut sizes = Vec::new();
        for _ in 0..8 {
            let s = build_export_json(&conn).unwrap();
            sizes.push(s.len());
            crate::sync::snapshot_put(&conn, &s).unwrap();
        }
        assert_eq!(
            sizes[0],
            *sizes.last().unwrap(),
            "8 轮推送后导出体积必须不变（存在自引用则会翻倍）：{sizes:?}"
        );
        assert!(
            sizes[0] < 8 * 1024,
            "正常配置导出应为 KB 级，实际 {} 字节",
            sizes[0]
        );
    }
}
