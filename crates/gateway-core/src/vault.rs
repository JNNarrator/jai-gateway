//! OS 密钥环封装 —— storage §4 生命周期。
//!
//! service 固定 "JAI"，account = `provider/{uuid}`。DB 只存引用地址。
//!
//! 降级策略（bug 清单 #1）：
//! 受限环境（沙箱/CI/无钥匙串权限）默认密钥环不可用时，自动切换到
//! 数据目录下的 `vault_fallback.json`（Unix 0600）。UI 可显示存储类型。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use thiserror::Error;

pub const SERVICE: &str = "JAI";

/// 测试工具：内存 keyring mock（keyring crate 的全局默认 builder 注入）。
///
/// 集成测试与单元测试共用；`set_mock_default()` 后所有 `Entry::new`
/// 都落到内存存储，不触碰真实系统凭据。
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

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("keyring: {0}")]
    Keyring(#[from] keyring::Error),
    #[error("file vault: {0}")]
    File(String),
}

/// 文件降级存储的底层读写（可独立测试；`MODE` 全局只能初始化一次）。
#[derive(Debug, Clone)]
pub struct FileVault(pub PathBuf);

impl FileVault {
    pub fn read_map(&self) -> Result<HashMap<String, String>, VaultError> {
        let text = std::fs::read_to_string(&self.0).map_err(|e| VaultError::File(e.to_string()))?;
        serde_json::from_str(&text).map_err(|e| VaultError::File(format!("解析失败: {e}")))
    }

    pub fn write_map(&self, map: &HashMap<String, String>) -> Result<(), VaultError> {
        let text = serde_json::to_string_pretty(map)
            .map_err(|e| VaultError::File(format!("序列化失败: {e}")))?;
        std::fs::write(&self.0, text).map_err(|e| VaultError::File(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn set(&self, ref_: &str, secret: &str) -> Result<(), VaultError> {
        let mut map = self.read_map()?;
        map.insert(ref_.to_string(), secret.to_string());
        self.write_map(&map)
    }

    pub fn get(&self, ref_: &str) -> Result<Option<String>, VaultError> {
        let map = self.read_map()?;
        Ok(map.get(ref_).cloned())
    }

    pub fn delete(&self, ref_: &str) -> Result<(), VaultError> {
        let mut map = self.read_map()?;
        map.remove(ref_);
        self.write_map(&map)
    }
}

#[derive(Clone, Debug)]
enum VaultMode {
    Keyring,
    File(FileVault),
}

static MODE: OnceLock<VaultMode> = OnceLock::new();

fn mode() -> &'static VaultMode {
    MODE.get().unwrap_or(&VaultMode::Keyring)
}

pub fn ref_for(provider_id: &str) -> String {
    format!("jai/provider/{provider_id}")
}

fn entry(account: &str) -> Result<keyring::Entry, VaultError> {
    Ok(keyring::Entry::new(SERVICE, account)?)
}

fn is_keyring_unavailable(e: &keyring::Error) -> bool {
    matches!(
        e,
        keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_)
    )
}

/// 数据目录初始化：探测系统密钥环；不可用时切换到文件降级。
/// 必须在任何 set/get/delete 之前调用一次（Docker/CI/沙箱友好）。
pub fn init(data_dir: &Path) -> Result<(), VaultError> {
    if MODE.get().is_some() {
        return Ok(());
    }

    match probe_keyring() {
        Ok(()) => {
            let _ = MODE.set(VaultMode::Keyring);
            Ok(())
        }
        Err(VaultError::Keyring(ke)) if is_keyring_unavailable(&ke) => {
            let dir = data_dir.to_path_buf();
            std::fs::create_dir_all(&dir).map_err(|e| VaultError::File(e.to_string()))?;
            let path = dir.join("vault_fallback.json");
            if !path.exists() {
                FileVault(path.clone()).write_map(&HashMap::new())?;
            }
            let _ = MODE.set(VaultMode::File(FileVault(path)));
            println!(
                "[vault] 系统密钥环不可用({ke})，已降级为文件存储: {}",
                data_dir.join("vault_fallback.json").display()
            );
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// 当前凭据存储类型：`keyring` / `file`。
pub fn storage_kind() -> &'static str {
    match mode() {
        VaultMode::Keyring => "keyring",
        VaultMode::File(_) => "file",
    }
}

fn probe_keyring() -> Result<(), VaultError> {
    let acct = "jai/self-probe";
    let e = entry(acct)?;
    e.set_password("probe-value")?;
    let got = e.get_password()?;
    debug_assert_eq!(got, "probe-value");
    let _ = e.delete_credential();
    Ok(())
}

/// 启动探测：set/get/delete 三连。storage §4 ——
/// 密钥环不可用的环境在添加供应商之前拦截。
pub fn probe() -> Result<(), VaultError> {
    match mode() {
        VaultMode::Keyring => probe_keyring(),
        VaultMode::File(_) => Ok(()),
    }
}

pub fn set_secret(ref_: &str, secret: &str) -> Result<(), VaultError> {
    match mode() {
        VaultMode::Keyring => {
            entry(ref_)?.set_password(secret)?;
            Ok(())
        }
        VaultMode::File(v) => v.set(ref_, secret),
    }
}

/// None = 条目不存在。
pub fn get_secret(ref_: &str) -> Result<Option<String>, VaultError> {
    match mode() {
        VaultMode::Keyring => match entry(ref_)?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        },
        VaultMode::File(v) => v.get(ref_),
    }
}

/// 尽力删除：条目不存在视为成功（幂等）。
pub fn delete_secret(ref_: &str) -> Result<(), VaultError> {
    match mode() {
        VaultMode::Keyring => match entry(ref_)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        },
        VaultMode::File(v) => v.delete(ref_),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // CI/测试环境统一走共享 mock 平台（不写真实钥匙串）
    #[test]
    fn roundtrip_on_mock_backend() {
        testing::set_mock_default();
        let r = ref_for("test-provider");
        assert!(get_secret(&r).unwrap().is_none());
        set_secret(&r, "sk-upstream-xyz").unwrap();
        assert_eq!(get_secret(&r).unwrap().as_deref(), Some("sk-upstream-xyz"));
        delete_secret(&r).unwrap();
        assert!(get_secret(&r).unwrap().is_none());
        delete_secret(&r).unwrap(); // 幂等
    }

    #[test]
    fn file_vault_roundtrip_and_permissions() {
        let dir = std::env::temp_dir().join(format!("jai-vault-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vault_fallback.json");
        let v = FileVault(path.clone());
        v.write_map(&HashMap::new()).unwrap();

        v.set("jai/provider/x", "shh").unwrap();
        assert_eq!(v.get("jai/provider/x").unwrap().as_deref(), Some("shh"));
        v.delete("jai/provider/x").unwrap();
        assert_eq!(v.get("jai/provider/x").unwrap(), None);
        v.delete("jai/provider/x").unwrap(); // 幂等

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "文件降级存储必须 0600 权限");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
