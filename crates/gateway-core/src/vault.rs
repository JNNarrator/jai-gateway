//! OS 密钥环封装 —— storage §4 生命周期。
//!
//! service 固定 "JAI"，account = `provider/{uuid}`。DB 只存引用地址。
//! 单元测试走 keyring mock（不触真实系统凭据库）。

use thiserror::Error;

pub const SERVICE: &str = "JAI";

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
    use keyring::credential::{CredentialApi, CredentialBuilderApi};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// 共享内存凭据：让多次 `Entry::new` 都解析到同一把秘密。
    #[derive(Debug)]
    struct SharedMockCredential {
        inner: Arc<Mutex<Option<Vec<u8>>>>,
    }

    impl CredentialApi for SharedMockCredential {
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
        fn debug_fmt(
            &self,
            _f: &mut std::fmt::Formatter<'_>,
        ) -> std::fmt::Result {
            Ok(())
        }
    }

    #[derive(Default)]
    struct SharedMockBuilder {
        #[allow(clippy::type_complexity)]
        store: Mutex<HashMap<(String, String), Arc<Mutex<Option<Vec<u8>>>>>>,
    }

    impl CredentialBuilderApi for SharedMockBuilder {
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

    // CI/测试环境统一走共享 mock 平台（不写真实钥匙串）
    #[test]
    fn roundtrip_on_mock_backend() {
        let builder: Box<keyring::CredentialBuilder> = Box::<SharedMockBuilder>::default();
        keyring::set_default_credential_builder(builder);
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
