//! 钥匙串退场 —— 0006 起凭据明文入库，本模块只剩一次性存量迁移。
//!
//! 运行时对系统钥匙串的访问仅此一处（首次启动迁移，可能弹最后一次授权框）；
//! 此后启动、转发、导入导出均零钥匙串访问。下个版本可随 keyring crate 一并删除。

use crate::store::{meta_get, meta_set, StoreError};
use thiserror::Error;

pub const SERVICE: &str = "JAI";

/// 存量迁移标记位（meta.keyring_migrated = "1"）。
const MIGRATED_FLAG: &str = "keyring_migrated";

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("keyring: {0}")]
    Keyring(#[from] keyring::Error),
    #[error("db: {0}")]
    Db(String),
}

/// 测试工具：内存 keyring mock（keyring crate 的全局默认 builder 注入）。
///
/// 仅供 `migrate_keyring_secrets` 的迁移单测使用，不触碰真实系统凭据。
#[doc(hidden)]
pub mod testing {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// 共享内存凭据：让多次 `Entry::new` 都解析到同一把秘密。
    #[derive(Debug)]
    pub struct SharedMockCredential {
        inner: Arc<Mutex<Option<Vec<u8>>>>,
    }

    impl keyring::credential::CredentialApi for SharedMockCredential {
        fn set_secret(&self, secret: &[u8]) -> Result<(), keyring::Error> {
            *self.inner.lock().unwrap() = Some(secret.to_vec());
            Ok(())
        }
        fn get_secret(&self) -> Result<Vec<u8>, keyring::Error> {
            let guard = self.inner.lock().unwrap();
            guard
                .as_ref()
                .map(|s| s.clone())
                .ok_or(keyring::Error::NoEntry)
        }
        fn delete_credential(&self) -> Result<(), keyring::Error> {
            let mut guard = self.inner.lock().unwrap();
            if guard.take().is_none() {
                Err(keyring::Error::NoEntry)
            } else {
                Ok(())
            }
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn debug_fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            Ok(())
        }
    }

    #[derive(Default)]
    pub struct SharedMockBuilder {
        #[allow(clippy::type_complexity)]
        pub store: Mutex<HashMap<(String, String), Arc<Mutex<Option<Vec<u8>>>>>>,
    }

    impl keyring::credential::CredentialBuilderApi for SharedMockBuilder {
        fn build(
            &self,
            _target: Option<&str>,
            service: &str,
            user: &str,
        ) -> Result<Box<keyring::Credential>, keyring::Error> {
            let mut store = self.store.lock().unwrap();
            let cell = store
                .entry((service.to_string(), user.to_string()))
                .or_insert_with(|| Arc::new(Mutex::new(None)))
                .clone();
            Ok(Box::new(SharedMockCredential { inner: cell }))
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    /// 注入全局 mock builder；任何后续 keyring 操作都在内存中进行。
    pub fn set_mock_default() {
        let builder: Box<keyring::CredentialBuilder> = Box::<SharedMockBuilder>::default();
        keyring::set_default_credential_builder(builder);
    }

    /// 直接写一条凭据（绕过 service/account 拼接，测试便捷用）。
    pub fn set(account: &str, secret: &str) -> Result<(), keyring::Error> {
        keyring::Entry::new(super::SERVICE, account)?.set_password(secret)
    }
}

fn provider_account(provider_id: &str) -> String {
    format!("jai/provider/{provider_id}")
}

/// keyring 读：None = 条目不存在。
fn get_secret(account: &str) -> Result<Option<String>, VaultError> {
    match keyring::Entry::new(SERVICE, account)?.get_password() {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// keyring 删：条目不存在视为成功（幂等）。
fn delete_secret(account: &str) -> Result<(), VaultError> {
    match keyring::Entry::new(SERVICE, account)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// 用户实例的 WebDAV 根地址切换为 https（用户确认的一次性改写）。
const WEBDAV_HTTPS_HOST: &str = "http://jn_file.88933.vip";
const WEBDAV_HTTPS_HOST_TO: &str = "https://jn_file.88933.vip";

/// 存量密钥迁移（Rust 侧一次性，0006 之后启动时调用）：
/// 1. meta 标记位已置 → 直接返回（幂等，也顺带跳过 https 改写之外的一切）
/// 2. 逐个读钥匙串 `jai/provider/{id}` → 写 providers.api_key；`jai/webdav` → meta.webdav_password
/// 3. 删除对应钥匙串项
/// 4. 置标记位
///
/// 钥匙串读取**不持 DB 锁**（授权弹框可能等待用户输入数秒甚至更久，
/// 持锁会拖死 UI）；每条凭据读取后再短暂拿锁落库。
/// 任一钥匙串读取失败（NoEntry）不阻塞迁移——该项无凭据或已清理；
/// 删除凭据失败也尽力继续（不影响新路径运行）。
/// 迁移完成后**永不**再访问钥匙串。
pub fn migrate_keyring_secrets(db: &crate::store::Db) -> Result<(), VaultError> {
    // ---- 锁内阶段 1：https 改写 + 标记位检查 + 收集 provider ids ----
    let (migrated, provider_ids) = db
        .with(|c| -> Result<(bool, Vec<String>), StoreError> {
            // 用户实例的 WebDAV http→https 改写与标记位无关，单独判定（未配置则无操作）
            if let Some(url) = meta_get(c, "webdav_url")? {
                if url.trim_end_matches('/') == WEBDAV_HTTPS_HOST && url.starts_with("http://") {
                    let tail = if url.ends_with('/') { "/" } else { "" };
                    meta_set(c, "webdav_url", &format!("{WEBDAV_HTTPS_HOST_TO}{tail}"))?;
                }
            }
            let migrated = meta_get(c, MIGRATED_FLAG)?
                .map(|v| v == "1")
                .unwrap_or(false);
            let ids: Vec<String> = {
                let mut stmt = c.prepare("SELECT id FROM providers ORDER BY created_at")?;
                let it = stmt.query_map([], |r| r.get::<_, String>(0))?;
                it.collect::<Result<Vec<_>, _>>()?
            };
            Ok((migrated, ids))
        })
        .map_err(|e| VaultError::Db(e.to_string()))?;
    if migrated {
        return Ok(());
    }

    // ---- 无锁阶段 2：逐个读钥匙串（此处可能弹授权框），逐条短暂持锁落库 ----
    // 供应商凭据：id 即 keyring account 尾段（旧 ref_for 拼接规则）
    for id in provider_ids {
        let account = provider_account(&id);
        match get_secret(&account) {
            Ok(Some(secret)) => {
                if let Err(e) =
                    db.with(|c| crate::store::provider_set_api_key(c, &id, Some(&secret)))
                {
                    eprintln!("[vault] 迁移写入 {account} 失败(跳过): {e}");
                }
            }
            Ok(None) => {}
            Err(e) => eprintln!("[vault] 迁移读取 {account} 失败(跳过): {e}"),
        }
        // 凭据已入库（或本就不存在），钥匙串项一律清除
        if let Err(e) = delete_secret(&account) {
            eprintln!("[vault] 迁移删除 {account} 失败(忽略): {e}");
        }
    }

    // WebDAV 密码
    match get_secret("jai/webdav") {
        Ok(Some(pw)) => {
            if let Err(e) = db.with(|c| meta_set(c, "webdav_password", &pw)) {
                eprintln!("[vault] 迁移写入 webdav_password 失败(跳过): {e}");
            }
        }
        Ok(None) => {}
        Err(e) => eprintln!("[vault] 迁移读取 jai/webdav 失败(跳过): {e}"),
    }
    if let Err(e) = delete_secret("jai/webdav") {
        eprintln!("[vault] 迁移删除 jai/webdav 失败(忽略): {e}");
    }

    // ---- 锁内阶段 3：置标记位 ----
    db.with(|c| meta_set(c, MIGRATED_FLAG, "1"))
        .map_err(|e| VaultError::Db(e.to_string()))?;
    println!("[vault] 钥匙串存量迁移完成，此后不再访问系统钥匙串");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// mock keyring 是进程级全局（builder 替换 + 共享存储），本组测试必须串行。
    static KEYRING_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn migrates_provider_and_webdav_secrets_then_deletes_keyring() {
        let _guard = KEYRING_TEST_LOCK.lock().unwrap();
        testing::set_mock_default();
        let db = crate::store::Db::in_memory().unwrap();
        let now = crate::store::now_ms();
        db.with(|c| {
            c.execute(
                "INSERT INTO providers(id,name,base_url,family,enabled,priority,weight,created_at,updated_at)
                 VALUES ('p1','A','https://api.a.com/v1','openai_compat',1,100,1,?1,?1),
                        ('p2','B','https://api.b.com/v1','openai_compat',1,100,1,?1,?1)",
                rusqlite::params![now],
            )
            .map_err(crate::store::StoreError::Sqlite)
        })
        .unwrap();
        testing::set(&provider_account("p1"), "sk-upstream-1").unwrap();
        testing::set("jai/webdav", "dav-pass").unwrap();

        migrate_keyring_secrets(&db).unwrap();

        let row = db.with(|c| crate::store::provider_get(c, "p1"))
            .unwrap()
            .unwrap();
        assert_eq!(row.api_key.as_deref(), Some("sk-upstream-1"));
        let row2 = db.with(|c| crate::store::provider_get(c, "p2"))
            .unwrap()
            .unwrap();
        assert_eq!(row2.api_key, None, "无钥匙串凭据的供应商保持 None");
        assert_eq!(
            db.with(|c| meta_get(c, "webdav_password"))
                .unwrap()
                .as_deref(),
            Some("dav-pass")
        );
        // 钥匙串项已删除
        assert!(get_secret(&provider_account("p1")).unwrap().is_none());
        assert!(get_secret("jai/webdav").unwrap().is_none());
        // 标记位已置
        assert_eq!(
            db.with(|c| meta_get(c, MIGRATED_FLAG)).unwrap().as_deref(),
            Some("1")
        );
    }

    #[test]
    fn migration_is_idempotent_via_flag() {
        let _guard = KEYRING_TEST_LOCK.lock().unwrap();
        testing::set_mock_default();
        let db = crate::store::Db::in_memory().unwrap();
        let now = crate::store::now_ms();
        db.with(|c| {
            c.execute(
                "INSERT INTO providers(id,name,base_url,family,enabled,priority,weight,created_at,updated_at)
                 VALUES ('p-idem','A','https://api.a.com/v1','openai_compat',1,100,1,?1,?1)",
                rusqlite::params![now],
            )
            .map_err(crate::store::StoreError::Sqlite)
        })
        .unwrap();
        testing::set(&provider_account("p-idem"), "sk-first").unwrap();
        migrate_keyring_secrets(&db).unwrap();

        // 标记位已置：即使钥匙串出现新值也不再读取
        testing::set(&provider_account("p-idem"), "sk-second").unwrap();
        migrate_keyring_secrets(&db).unwrap();
        let row = db.with(|c| crate::store::provider_get(c, "p-idem"))
            .unwrap()
            .unwrap();
        assert_eq!(row.api_key.as_deref(), Some("sk-first"));
    }

    #[test]
    fn rewrites_webdav_url_to_https_even_when_migrated() {
        let _guard = KEYRING_TEST_LOCK.lock().unwrap();
        testing::set_mock_default();
        let db = crate::store::Db::in_memory().unwrap();
        db.with(|c| meta_set(c, "webdav_url", "http://jn_file.88933.vip/")).unwrap();
        db.with(|c| meta_set(c, MIGRATED_FLAG, "1")).unwrap();
        migrate_keyring_secrets(&db).unwrap();
        assert_eq!(
            db.with(|c| meta_get(c, "webdav_url")).unwrap().as_deref(),
            Some("https://jn_file.88933.vip/")
        );
    }
}
