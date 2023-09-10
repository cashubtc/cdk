use std::ops::Deref;

use cashu::secret::Secret as SecretSdk;

pub struct Secret {
    inner: SecretSdk,
}

impl Deref for Secret {
    type Target = SecretSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Default for Secret {
    fn default() -> Self {
        Self::new()
    }
}

impl Secret {
    pub fn new() -> Self {
        Self {
            inner: SecretSdk::new(),
        }
    }

    pub fn as_bytes(&self) -> Vec<u8> {
        self.inner.as_bytes().to_vec()
    }
}

impl From<SecretSdk> for Secret {
    fn from(inner: SecretSdk) -> Secret {
        Secret { inner }
    }
}

impl From<Secret> for SecretSdk {
    fn from(secret: Secret) -> SecretSdk {
        secret.inner
    }
}
