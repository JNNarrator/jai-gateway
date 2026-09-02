//! 临时独立网关：用于本机 dsh 真机联调。
//!
//! 从环境变量读取上游密钥（ONEMODEL_API_KEY / JIYUANLVDONG_API_KEY），
//! 写入临时 JAI 数据目录并启动网关，打印实际端口与网关 Key 后常驻运行。

use gateway_core::server::{self, GatewayCtx};
use gateway_core::store::{self, Db};
use std::path::PathBuf;

fn env_or(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.trim().is_empty())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = env_or("JAI_STANDALONE_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("jai-standalone-e2e"));
    std::fs::create_dir_all(&data_dir)?;

    let db_path = data_dir.join("jai.db");
    let db = Db::open(&db_path.to_string_lossy())?;
    let (logs, _task) = store::logs::spawn_logger(&db_path.to_string_lossy())?;

    let now = store::now_ms();
    let key = "sk-jai-standalone-test-0000000000000000";
    db.with(|c| {
        store::gw_key_rotate(c, key, Some("standalone"))?;

        if let Some(_secret) = env_or("ONEMODEL_API_KEY") {
            let pid = "p-onemodel";
            let base_url = "https://one-model.com/v1";
            store::provider_insert(
                c,
                &store::ProviderRow {
                    id: pid.into(),
                    name: "onemodel".into(),
                    base_url: base_url.into(),
                    family: "openai_responses".into(),
                    enabled: true,
                    priority: 1,
                    weight: 1,
                    extra_headers: None,
                    api_key: env_or(if pid == "p-onemodel" {
                        "ONEMODEL_API_KEY"
                    } else {
                        "JIYUANLVDONG_API_KEY"
                    }),
                    website: None,
                    last_ok_at: None,
                    last_err_at: None,
                    last_err_msg: None,
                    created_at: now,
                    updated_at: now,
                },
            )?;
            store::model_upsert(c, pid, "deepseek-v4-pro", Some(1_000_000), 8192)?;
        }

        if let Some(_secret) = env_or("JIYUANLVDONG_API_KEY") {
            let pid = "p-jiyuanlvdong";
            let base_url = "https://tokenrhythm.studio/v1";
            store::provider_insert(
                c,
                &store::ProviderRow {
                    id: pid.into(),
                    name: "jiyuanlvdong".into(),
                    base_url: base_url.into(),
                    family: "openai_compat".into(),
                    enabled: true,
                    priority: 2,
                    weight: 1,
                    extra_headers: None,
                    api_key: env_or(if pid == "p-onemodel" {
                        "ONEMODEL_API_KEY"
                    } else {
                        "JIYUANLVDONG_API_KEY"
                    }),
                    website: None,
                    last_ok_at: None,
                    last_err_at: None,
                    last_err_msg: None,
                    created_at: now,
                    updated_at: now,
                },
            )?;
            for model in ["glm-5", "kimi-k2.6"] {
                store::model_upsert(c, pid, model, Some(200_000), 8192)?;
            }
        }

        Ok::<_, store::StoreError>(())
    })?;

    let preferred_port: u16 = env_or("JAI_STANDALONE_PORT")
        .and_then(|s| s.parse().ok())
        .unwrap_or(13140);
    let ctx = GatewayCtx::new(db.clone(), logs);
    let app = server::build_router(ctx);
    let (listener, port) = server::bind_with_fallback("127.0.0.1", preferred_port)?;
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);

    println!("JAI_STANDALONE_PORT={port}");
    println!("JAI_STANDALONE_KEY={key}");
    println!("JAI_STANDALONE_READY");

    let _ = server::run_until_shutdown(listener, app, stop_rx).await;
    let _ = stop_tx.send(true);
    Ok(())
}
