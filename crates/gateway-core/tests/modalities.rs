//! 输入/输出模态集合（input/output modalities）全链路验收 —— 先红后绿。
//!
//! 口径：docs/design/multimodal-support.md。单一 vision 布尔升级为
//! 「输入/输出模态集合」，旧 `supports_multimodal` 列保留可读并降为**派生视图**：
//!   派生 = input 含 image → true；input 为 NULL 时回落旧列；两者皆无 → null（未知）。
//!
//! 覆盖层次：枚举编解码 → 迁移 0010 → store 读写/派生 → 上游发现解析 →
//! 出站 GET /v1/models → 配置导出/导入（含老快照回填）。
//! 用例前缀（enum_/store_/discover_/proxy_/sync_）供分组判据过滤使用。

use axum::extract::State;
use gateway_core::discover;
use gateway_core::modality::{self, Modality};
use gateway_core::server::proxy::{models_list, GatewayCtx};
use gateway_core::store::{self, Db, ProviderRow};
use serde_json::{json, Value};

fn seed_provider(c: &rusqlite::Connection, id: &str) {
    let now = store::now_ms();
    store::provider_insert(
        c,
        &ProviderRow {
            id: id.into(),
            name: format!("prov-{id}"),
            base_url: "https://up.example/v1".into(),
            family: "openai_compat".into(),
            enabled: true,
            priority: 1,
            weight: 1,
            extra_headers: None,
            api_key: None,
            website: None,
            last_ok_at: None,
            last_err_at: None,
            last_err_msg: None,
            max_tools: None,
            reasoning_effort_levels: None,
            created_at: now,
            updated_at: now,
        },
    )
    .expect("seed provider");
}

// ================================================================ 1. 枚举编解码

#[test]
fn enum_modality_encode_parse_roundtrip() {
    use Modality::*;

    // 规范序 text,image,audio,video + 去重
    assert_eq!(modality::encode(&[Image, Text]), "text,image");
    assert_eq!(modality::encode(&[Text, Text, Video]), "text,video");
    assert_eq!(
        modality::encode(&[Video, Audio, Image, Text]),
        "text,image,audio,video"
    );
    assert_eq!(modality::encode(&[]), "");

    // 解析：大小写/空白归一、去重、规范序
    assert_eq!(modality::parse("text,image"), Some(vec![Text, Image]));
    assert_eq!(modality::parse(" IMAGE , text "), Some(vec![Text, Image]));
    assert_eq!(
        modality::parse("TEXT,IMAGE,AUDIO,VIDEO"),
        Some(vec![Text, Image, Audio, Video])
    );
    assert_eq!(modality::parse("text,bogus,image"), Some(vec![Text, Image]));

    // 未知语义：空串 / 全非法 → None（不臆断）
    assert_eq!(modality::parse(""), None);
    assert_eq!(modality::parse("   "), None);
    assert_eq!(modality::parse("bogus"), None);
    assert_eq!(modality::parse_opt(None), None);
    assert_eq!(modality::parse_opt(Some("audio")), Some(vec![Audio]));

    // token 归一 + serde 小写
    assert_eq!(Modality::from_token("Image"), Some(Image));
    assert_eq!(Modality::from_token("nope"), None);
    assert_eq!(serde_json::to_value(Image).unwrap(), json!("image"));
}

#[test]
fn enum_derive_supports_multimodal_prefers_modalities() {
    use Modality::*;
    let derive = modality::derive_supports_multimodal;

    assert_eq!(derive(Some(&[Text, Image]), None), Some(true));
    assert_eq!(
        derive(Some(&[Text]), Some(true)),
        Some(false),
        "模态集合是真相源，压过旧列"
    );
    assert_eq!(derive(Some(&[Audio]), Some(true)), Some(false));
    assert_eq!(derive(None, Some(true)), Some(true), "无集合时回落旧列");
    assert_eq!(derive(None, Some(false)), Some(false));
    assert_eq!(derive(None, None), None, "两者皆无 → 未知");
}

// ================================================================ 2. 迁移与存储

#[test]
fn store_migration_0010_adds_modality_columns() {
    let db = Db::in_memory().unwrap();
    db.with(|c| {
        let mut stmt = c.prepare("PRAGMA table_info(models)")?;
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<Result<_, _>>()?;
        assert!(
            cols.iter().any(|x| x == "input_modalities"),
            "迁移 0010 应加 input_modalities，实际列: {cols:?}"
        );
        assert!(cols.iter().any(|x| x == "output_modalities"));
        assert!(
            cols.iter().any(|x| x == "supports_multimodal"),
            "旧列保留可读（不删列，老库平滑）"
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn store_roundtrips_modalities_and_derives_bool() {
    let db = Db::in_memory().unwrap();
    db.with(|c| {
        seed_provider(c, "p1");
        store::model_upsert(
            c,
            "p1",
            "omni",
            Some(128_000),
            8192,
            Some(&[Modality::Text, Modality::Image]),
            Some(&[Modality::Text]),
        )?;
        let row = store::model_get_by_provider_name(c, "p1", "omni")?.unwrap();
        assert_eq!(
            row.input_modalities.as_deref(),
            Some(&[Modality::Text, Modality::Image][..])
        );
        assert_eq!(
            row.output_modalities.as_deref(),
            Some(&[Modality::Text][..])
        );
        assert_eq!(row.supports_multimodal, Some(true), "派生：含 image");

        // 未知入站不覆盖已有标注（COALESCE）
        store::model_upsert(c, "p1", "omni", Some(128_000), 8192, None, None)?;
        let row = store::model_get_by_provider_name(c, "p1", "omni")?.unwrap();
        assert_eq!(
            row.input_modalities.as_deref(),
            Some(&[Modality::Text, Modality::Image][..]),
            "None 不应清掉已有标注"
        );

        // 纯音频输入：不派生 vision
        store::model_upsert(
            c,
            "p1",
            "voice",
            Some(32_000),
            4096,
            Some(&[Modality::Audio]),
            None,
        )?;
        let row = store::model_get_by_provider_name(c, "p1", "voice")?.unwrap();
        assert_eq!(row.supports_multimodal, Some(false));
        assert_eq!(row.output_modalities, None, "未知保持 NULL");
        Ok(())
    })
    .unwrap();
}

#[test]
fn store_legacy_bool_fallback_and_manual_clear() {
    let db = Db::in_memory().unwrap();
    db.with(|c| {
        seed_provider(c, "p1");

        // 老数据：只有旧布尔列有值
        store::model_upsert(c, "p1", "legacy", Some(128_000), 4096, None, None)?;
        c.execute(
            "UPDATE models SET supports_multimodal=1 WHERE model_name='legacy'",
            [],
        )?;
        let row = store::model_get_by_provider_name(c, "p1", "legacy")?.unwrap();
        assert_eq!(row.supports_multimodal, Some(true), "旧列应回落生效");
        assert_eq!(row.input_modalities, None);

        // 新集合优先：input=[text] 压过旧列 true
        store::model_upsert(
            c,
            "p1",
            "mixed",
            Some(128_000),
            4096,
            Some(&[Modality::Text]),
            None,
        )?;
        c.execute(
            "UPDATE models SET supports_multimodal=1 WHERE model_name='mixed'",
            [],
        )?;
        let row = store::model_get_by_provider_name(c, "p1", "mixed")?.unwrap();
        assert_eq!(row.supports_multimodal, Some(false));

        // 手动清除 → 未知；旧列必须一并清 NULL，否则幽灵 true 复现
        store::model_set_modalities(c, &row.id, None, None)?;
        let row = store::model_get_by_provider_name(c, "p1", "mixed")?.unwrap();
        assert_eq!(row.supports_multimodal, None, "清除后应回到未知");
        assert_eq!(row.input_modalities, None);
        assert_eq!(row.output_modalities, None);
        Ok(())
    })
    .unwrap();
}

// ================================================================ 3. 发现解析

#[test]
fn discover_parses_modalities_across_families() {
    // Gemini：最规范的 inputModalities/outputModalities（大写）
    let gemini = json!({
        "name": "models/gemini-2.0-flash",
        "inputModalities": ["TEXT", "IMAGE"],
        "outputModalities": ["TEXT"]
    });
    let (i, o) = discover::parse_gemini_modalities(&gemini);
    assert_eq!(i.as_deref(), Some(&[Modality::Text, Modality::Image][..]));
    assert_eq!(o.as_deref(), Some(&[Modality::Text][..]));

    // OpenRouter 风格：architecture.input_modalities / output_modalities
    let orouter = json!({
        "id": "openai/gpt-4o",
        "architecture": {
            "input_modalities": ["text", "image", "audio"],
            "output_modalities": ["text"]
        }
    });
    let (i, o) = discover::parse_openai_modalities(&orouter);
    assert_eq!(
        i.as_deref(),
        Some(&[Modality::Text, Modality::Image, Modality::Audio][..])
    );
    assert_eq!(o.as_deref(), Some(&[Modality::Text][..]));

    // openai 系中转的旧 vision 键（布尔）→ 文本+图像
    let (i, o) = discover::parse_openai_modalities(&json!({"id": "v", "supports_vision": true}));
    assert_eq!(i.as_deref(), Some(&[Modality::Text, Modality::Image][..]));
    assert_eq!(o, None, "未声明输出 → NULL，不臆断");

    // 明确不支持图像 → 文本
    let (i, _) = discover::parse_openai_modalities(&json!({"id": "t", "multimodal": false}));
    assert_eq!(i.as_deref(), Some(&[Modality::Text][..]));

    // 完全取不到 → 两个 NULL
    let (i, o) = discover::parse_openai_modalities(&json!({"id": "unknown"}));
    assert_eq!((i, o), (None, None));
    let (i, o) = discover::parse_gemini_modalities(&json!({"name": "models/x"}));
    assert_eq!((i, o), (None, None));
}

// ================================================================ 4. 出站 /v1/models

#[tokio::test]
async fn proxy_models_list_exposes_input_output_modalities() {
    let db = Db::in_memory().unwrap();
    db.with(|c| {
        seed_provider(c, "p1");
        store::model_upsert(
            c,
            "p1",
            "vision-model",
            Some(128_000),
            8192,
            Some(&[Modality::Text, Modality::Image]),
            Some(&[Modality::Text]),
        )?;
        store::model_upsert(c, "p1", "unknown", Some(128_000), 8192, None, None)?;
        Ok(())
    })
    .unwrap();

    let dir = std::env::temp_dir().join(format!("jai-modalities-{}", rand::random::<u32>()));
    let log_path = dir.join("main.db");
    std::fs::create_dir_all(&dir).unwrap();
    let (logs, _t) = store::logs::spawn_logger(log_path.to_str().unwrap()).unwrap();
    let ctx = GatewayCtx::new(db.clone(), logs);

    let resp = models_list(State(ctx)).await;
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: Value = serde_json::from_slice(&body).unwrap();
    let data = v["data"].as_array().unwrap();
    let find = |name: &str| {
        data.iter()
            .find(|m| m["id"] == format!("prov-p1/{name}"))
            .unwrap_or_else(|| panic!("{name} 模型应存在"))
    };

    assert_eq!(
        find("vision-model")["inputModalities"],
        json!(["text", "image"])
    );
    assert_eq!(find("vision-model")["outputModalities"], json!(["text"]));
    assert_eq!(
        find("vision-model")["supportsMultimodal"],
        true,
        "旧字段保留：派生自 input 含 image"
    );
    assert_eq!(find("vision-model")["contextWindow"], 128_000, "旧字段不删");
    assert_eq!(
        find("unknown")["inputModalities"],
        Value::Null,
        "未知输出 null，不臆断"
    );
    assert_eq!(find("unknown")["outputModalities"], Value::Null);
    assert_eq!(find("unknown")["supportsMultimodal"], Value::Null);
}

// ================================================================ 5. 配置同步

#[test]
fn sync_export_import_carry_modalities() {
    let src = Db::in_memory().unwrap();
    src.with(|c| {
        seed_provider(c, "p1");
        store::model_upsert(
            c,
            "p1",
            "omni",
            Some(200_000),
            8192,
            Some(&[Modality::Text, Modality::Audio]),
            Some(&[Modality::Text]),
        )?;
        Ok(())
    })
    .unwrap();

    let exported = src.with(store::export::build_export_json).unwrap();
    let v: Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(
        v["models"][0]["inputModalities"],
        json!(["text", "audio"]),
        "导出应携带模态集合"
    );
    assert_eq!(v["models"][0]["outputModalities"], json!(["text"]));

    let dst = Db::in_memory().unwrap();
    let report = dst
        .with_any(|c| store::import::apply_import(c, &exported, false))
        .unwrap();
    assert_eq!(report.models_imported, 1);
    dst.with(|c| {
        let pid = store::provider_list(c)?.first().unwrap().id.clone();
        let row = store::model_get_by_provider_name(c, &pid, "omni")?.unwrap();
        assert_eq!(
            row.input_modalities.as_deref(),
            Some(&[Modality::Text, Modality::Audio][..]),
            "往返不失真"
        );
        assert_eq!(
            row.output_modalities.as_deref(),
            Some(&[Modality::Text][..])
        );
        assert_eq!(
            row.supports_multimodal,
            Some(false),
            "无 image → 派生 false"
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn sync_import_legacy_snapshot_backfills_modalities() {
    // 老快照：只有 supportsMultimodal（模拟 0009 期的导出文件）
    let legacy = json!({
        "format": "jai-export/v1",
        "providers": [{
            "id": "p-old",
            "name": "老供应商",
            "base_url": "https://up.example/v1",
            "family": "openai_compat",
            "enabled": true,
            "priority": 1,
            "extra_headers": null,
            "website": null
        }],
        "models": [{
            "id": "m-old",
            "providerId": "p-old",
            "modelName": "old-vision",
            "upstreamModelId": null,
            "contextWindow": 128000,
            "maxOutputTokens": 4096,
            "enabled": true,
            "supportsMultimodal": true
        }],
        "gateway_key": null
    })
    .to_string();

    let dst = Db::in_memory().unwrap();
    let report = dst
        .with_any(|c| store::import::apply_import(c, &legacy, false))
        .unwrap();
    assert_eq!(report.models_imported, 1);
    dst.with(|c| {
        let pid = store::provider_list(c)?.first().unwrap().id.clone();
        let row = store::model_get_by_provider_name(c, &pid, "old-vision")?.unwrap();
        assert_eq!(
            row.input_modalities.as_deref(),
            Some(&[Modality::Text, Modality::Image][..]),
            "老快照的布尔应回填为 文本+图像"
        );
        assert_eq!(row.supports_multimodal, Some(true));
        Ok(())
    })
    .unwrap();
}
