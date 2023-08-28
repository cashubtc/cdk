use std::ops::Deref;

use cashu::nuts::nut01::SecretKey as SecretKeySdk;

use crate::error::Result;

pub struct SecretKey {
    inner: SecretKeySdk,
}

impl Deref for SecretKey {
    type Target = SecretKeySdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<SecretKeySdk> for SecretKey {
    fn from(inner: SecretKeySdk) -> Self {
        Self { inner }
    }
}

impl From<SecretKey> for SecretKeySdk {
    fn from(sk: SecretKey) -> SecretKeySdk {
        sk.inner
    }
}

impl SecretKey {
    pub fn from_hex(hex: String) -> Result<Self> {
        Ok(Self {
            inner: SecretKeySdk::from_hex(hex)?,
        })
    }

    pub fn to_hex(&self) -> Result<String> {
        Ok(self.inner.to_hex()?)
    }
}
