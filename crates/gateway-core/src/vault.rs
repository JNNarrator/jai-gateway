//! OS 密钥环封装 —— storage §4 生命周期。
//!
//! service 固定 "JAI"，account = `provider/{uuid}`。DB 只存引用地址。
//! 单元测试走 keyring mock（不触真实系统凭据库）。

use thiserror::Error;

pub const SERVICE: &str = "JAI";

/// 测试工具：内存 keyring mock（keyring crate 的全局默认 builder 注入）。
///
/// 集成测试（`tests/` 目录）与单元测试共用；`set_mock_default()` 后
/// 所有 `Entry::new` 都落到内存存储，不触碰真实系统凭据。
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
}

pub fn ref_for(provider_id: &str) -> String {
    format!("jai/provider/{provider_id}")
}

fn entry(account: &str) -> Result<keyring::Entry, VaultError> {
    Ok(keyring::Entry::new(SERVICE, account)?)
}

pub fn set_secret(ref_: &str, secret: &str) -> Result<(), VaultError> {
    entry(ref_)?.set_password(secret)?;
    Ok(())
}

/// None = 条目不存在。
pub fn get_secret(ref_: &str) -> Result<Option<String>, VaultError> {
    match entry(ref_)?.get_password() {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 尽力删除：条目不存在视为成功（幂等）。
pub fn delete_secret(ref_: &str) -> Result<(), VaultError> {
    match entry(ref_)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// 启动探测：set/get/delete 三连。storage §4 ——
/// 密钥环不可用的环境在添加供应商之前拦截。
pub fn probe() -> Result<(), VaultError> {
    let acct = "jai/self-probe";
    let e = entry(acct)?;
    e.set_password("probe-value")?;
    let got = e.get_password()?;
    debug_assert_eq!(got, "probe-value");
    let _ = e.delete_credential();
    Ok(())
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
        assert_eq!(
            get_secret(&r).unwrap().as_deref(),
            Some("sk-upstream-xyz")
        );
        delete_secret(&r).unwrap();
        assert!(get_secret(&r).unwrap().is_none());
        delete_secret(&r).unwrap(); // 幂等
    }
}
