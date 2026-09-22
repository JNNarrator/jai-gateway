//! WebDAV **真机验收**（`#[ignore]`：需要真实服务器凭据，不随常规回归跑）。
//!
//! 发布检查单（`docs/design/release.md` §4）要求每次发版有 WebDAV 真机验收，
//! 这里把它做成可重复执行的一条命令 —— mock 服务器测不出真实服务器的 404 形状、
//! `PROPFIND` 实现差异、以及「目录不存在时 PUT 会不会自动建目录」这类行为。
//!
//! 用法（**目录必须是一个隔离的新目录**，本用例只在该目录内增删，结束时会清空它）：
//!
//! ```bash
//! JAI_DAV_LIVE_URL=https://dav.example.com \
//! JAI_DAV_LIVE_USER=user JAI_DAV_LIVE_PASS=pass \
//! JAI_DAV_LIVE_DIR=jai-e2e-$(date +%s) \
//!   cargo test -p gateway-core --test webdav_live_e2e -- --ignored --nocapture
//! ```
//!
//! 覆盖链路（全部走生产代码路径，不 mock）：连接测试 → 推送 → 拉取 → 覆盖前留存时间戳
//! 备份 → 备份列表/读取/删除 → 第二台机器 `apply_import` 落库（供应商 key / 模型 / 网关 Key）
//! → 「远端还没有配置文件」的 404 语义 → 清理。

use gateway_core::store::{self, Db};
use gateway_core::sync::{self, WebDavConfig};
use serde_json::json;

/// 造一台「老机器」的完整导出物：带 key 的供应商 + 模型 + 网关 Key。
fn seed_machine(db: &Db) {
    db.with_any(|c| {
        store::import::apply_import(
            c,
            &json!({
                "format": "jai-export/v1",
                "gateway_key": "sk-jai-live-e2e",
                "meta": [],
                "providers": [{
                    "id": "plive", "name": "真机验收供应商", "base_url": "https://api.live.test/v1",
                    "family": "openai_compat", "enabled": true, "priority": 50,
                    "extra_headers": null, "api_key": "sk-upstream-live"
                }],
                "models": [{
                    "id": "mlive", "providerId": "plive", "modelName": "gpt-4o",
                    "upstreamModelId": null, "contextWindow": 128000,
                    "maxOutputTokens": 4096, "enabled": true
                }]
            })
            .to_string(),
            false,
        )
        .map_err(|e| e.to_string())
    })
    .unwrap();
}

fn export_of(db: &Db) -> String {
    db.with_any(|c| store::export::build_export_json(c).map_err(|e| e.to_string()))
        .unwrap()
}

fn live_cfg(directory: &str) -> WebDavConfig {
    WebDavConfig {
        url: std::env::var("JAI_DAV_LIVE_URL").expect("需要 JAI_DAV_LIVE_URL"),
        username: std::env::var("JAI_DAV_LIVE_USER").expect("需要 JAI_DAV_LIVE_USER"),
        directory: directory.to_string(),
        auto_push_enabled: false,
        auto_push_interval_min: 60,
        auto_pull_enabled: false,
        auto_pull_interval_min: 60,
    }
}

fn live_pass() -> String {
    std::env::var("JAI_DAV_LIVE_PASS").expect("需要 JAI_DAV_LIVE_PASS")
}

/// 清空隔离目录：删掉目录内所有 `jai-config*` 文件，再尝试删目录本身。
/// 只认本用例自己造的文件名，绝不碰别人放进去的东西。
async fn cleanup_dir(http: &reqwest::Client, cfg: &WebDavConfig, password: &str) {
    if let Ok(items) = sync::list_backups(http, cfg, password).await {
        for it in items {
            let name = it.name.clone();
            let url = if it.name == sync::CONFIG_FILE_NAME {
                cfg.config_url()
            } else {
                match sync::backup_href(cfg, &name) {
                    Ok(u) => u,
                    Err(_) => continue,
                }
            };
            let _ = http
                .delete(&url)
                .basic_auth(&cfg.username, Some(password))
                .send()
                .await;
        }
    }
    // 目录本身：能删就删（部分服务器/服务账号不允许，失败只提示不判失败）
    let dir = format!("{}/", cfg.config_dir());
    match http
        .delete(&dir)
        .basic_auth(&cfg.username, Some(password))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => println!("[cleanup] 已删除隔离目录 {dir}"),
        Ok(r) => println!(
            "[cleanup] 目录 {dir} 未删除（HTTP {}），可手工清理",
            r.status()
        ),
        Err(e) => println!("[cleanup] 目录 {dir} 删除请求失败：{e}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "真机验收：需要 JAI_DAV_LIVE_* 环境变量，按需手动执行（见文件头用法）"]
async fn webdav_live_end_to_end() {
    let dir = std::env::var("JAI_DAV_LIVE_DIR").expect("需要 JAI_DAV_LIVE_DIR（隔离目录名）");
    assert!(
        !dir.trim().is_empty() && dir != "/",
        "必须指定一个隔离目录，不能直接跑在远端根目录上"
    );
    let password = live_pass();
    let cfg = live_cfg(&dir);
    let http = reqwest::Client::new();

    // 1) 连接测试（PROPFIND 带认证；OPTIONS 免认证的服务器测不出凭据错误）
    let probe = sync::probe(&http, &cfg, &password)
        .await
        .unwrap_or_else(|e| panic!("连接测试失败：{e}"));
    println!("[1/8] 连接测试：{probe}");
    assert_eq!(probe, "连接成功");

    // 2) 推送（目录不存在时由服务器决定是否自动创建 —— 本次实测 DUFS 会建）
    let machine_a = Db::in_memory().unwrap();
    seed_machine(&machine_a);
    let first = export_of(&machine_a);
    sync::push(&http, &cfg, &password, first.clone())
        .await
        .unwrap_or_else(|e| panic!("推送失败：{e}"));
    println!("[2/8] 推送成功（{} 字节）", first.len());

    // 3) 拉取：内容必须逐字节一致
    let got = sync::try_pull(&http, &cfg, &password)
        .await
        .expect("拉取请求失败")
        .expect("远端应有配置");
    assert_eq!(got, first, "远端内容与推送内容不一致");
    println!("[3/8] 拉取一致（{} 字节）", got.len());

    // 4) 再推一次（内容变化）→ 旧版必须被留存为时间戳备份
    // 注意：`apply_import` 会为**新**供应商生成本地 id（文件里的 id 只用于映射模型），
    // 所以这里必须按真实本地 id 去改，不能拿导出物里的 `plive` 当 id 用。
    let a_pid = machine_a
        .with_any(|c| {
            let p = store::provider_list(c)
                .map_err(|e| e.to_string())?
                .remove(0);
            Ok::<_, String>(p.id)
        })
        .unwrap();
    machine_a
        .with_any(|c| {
            store::provider_set_website(c, &a_pid, Some("https://live.test"))
                .map_err(|e| e.to_string())
        })
        .unwrap();
    let second = export_of(&machine_a);
    assert!(second.contains("https://live.test"), "第二次导出应带上改动");
    assert_ne!(second, first, "第二次导出应与第一次不同");
    sync::push(&http, &cfg, &password, second.clone())
        .await
        .unwrap_or_else(|e| panic!("第二次推送失败：{e}"));
    let items = sync::list_backups(&http, &cfg, &password)
        .await
        .unwrap_or_else(|e| panic!("备份列表失败：{e}"));
    let backups: Vec<String> = items
        .iter()
        .filter(|b| b.name != sync::CONFIG_FILE_NAME)
        .map(|b| b.name.clone())
        .collect();
    assert_eq!(
        backups.len(),
        1,
        "覆盖前应恰好留存一份备份，实际 {backups:?}"
    );
    println!("[4/8] 覆盖前留存备份：{}", backups[0]);

    // 5) 读取备份 → 必须是第一次推送的那份旧内容（数据不丢）
    let old = sync::fetch_backup(&http, &cfg, &password, &backups[0])
        .await
        .unwrap_or_else(|e| panic!("备份读取失败：{e}"));
    assert_eq!(old, first, "备份内容不是被覆盖前的旧版");
    println!("[5/8] 备份内容 = 覆盖前的旧版（{} 字节）", old.len());

    // 6) 删除备份（幂等：再删一次仍成功）
    sync::delete_backup(&http, &cfg, &password, &backups[0])
        .await
        .unwrap_or_else(|e| panic!("备份删除失败：{e}"));
    sync::delete_backup(&http, &cfg, &password, &backups[0])
        .await
        .expect("已删除的备份再删应幂等成功");
    let left = sync::list_backups(&http, &cfg, &password).await.unwrap();
    assert!(
        left.iter().all(|b| b.name != backups[0]),
        "备份删除后不应再出现在列表里"
    );
    println!("[6/8] 备份删除 + 幂等删除均成功");

    // 7) 「换台电脑」：拉取 → apply_import 落库，数据（含上游密钥、网关 Key）必须到位
    let machine_b = Db::in_memory().unwrap();
    let pulled = sync::pull(&http, &cfg, &password)
        .await
        .unwrap_or_else(|e| panic!("拉取失败：{e}"));
    machine_b
        .with_any(|c| store::import::apply_import(c, &pulled, false).map_err(|e| e.to_string()))
        .unwrap();
    machine_b
        .with_any(|c| {
            // 同前：新机器上的供应商 id 是本地新生成的，按名字/列表核对（不认导出物里的 id）
            let p = store::provider_list(c)
                .map_err(|e| e.to_string())?
                .remove(0);
            assert_eq!(p.name, "真机验收供应商");
            assert_eq!(
                p.api_key.as_deref(),
                Some("sk-upstream-live"),
                "上游密钥未落库（新机器上所有请求会 401）"
            );
            assert_eq!(
                p.website.as_deref(),
                Some("https://live.test"),
                "B 机应拿到第二次推送的内容"
            );
            let models = store::model_list_by_provider(c, &p.id).map_err(|e| e.to_string())?;
            assert_eq!(models.len(), 1, "模型未落库");
            assert_eq!(models[0].model_name, "gpt-4o");
            let key = store::gw_key_active(c)
                .map_err(|e| e.to_string())?
                .expect("网关 Key 应随配置落库");
            assert_eq!(key.key, "sk-jai-live-e2e", "网关 Key 未落库");
            Ok::<(), String>(())
        })
        .unwrap();
    println!("[7/8] 第二台机器导入：供应商 / 上游密钥 / 模型 / 网关 Key 全部到位");

    // 8) 404 语义：往一个「远端还没有配置文件」的目录拉取
    //    - 服务器用空正文/纯文本/XML 回 404（实测 DUFS：`text/plain` + `Not Found`）
    //      ⇒ 应提示「远端尚无配置文件」
    //    - 若服务器用网页回 404（nginx dav_module、`error_page 404` 改写）
    //      ⇒ 应提示「不是可用的 WebDAV 端点」
    //    两者都是设计内行为，这里只要求「不是端点问题被误报成配置问题」这一类反向错误。
    let empty_dir = format!("{dir}-empty-{}", std::process::id());
    let empty_cfg = live_cfg(&empty_dir);
    let err = sync::pull(&http, &empty_cfg, &password)
        .await
        .expect_err("空目录拉取应当报错");
    println!("[8/8] 空目录拉取提示：{err}");
    assert!(
        err.contains("远端尚无配置文件") || err.contains("不是可用的 WebDAV 端点"),
        "404 提示既不是「远端尚无配置文件」也不是「端点不可用」：{err}"
    );

    cleanup_dir(&http, &cfg, &password).await;
    println!("真机验收通过：{dir}");
}
