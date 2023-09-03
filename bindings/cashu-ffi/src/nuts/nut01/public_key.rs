use std::ops::Deref;

use cashu::nuts::nut01::PublicKey as PublicKeySdk;

use crate::error::Result;

pub struct PublicKey {
    inner: PublicKeySdk,
}

impl From<PublicKeySdk> for PublicKey {
    fn from(inner: PublicKeySdk) -> Self {
        Self { inner }
    }
}

impl From<&PublicKey> for PublicKeySdk {
    fn from(pk: &PublicKey) -> PublicKeySdk {
        pk.inner.clone()
    }
}

impl Deref for PublicKey {
    type Target = PublicKeySdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl PublicKey {
    pub fn from_hex(hex: String) -> Result<Self> {
        Ok(Self {
            inner: PublicKeySdk::from_hex(hex)?,
        })
    }

    pub fn to_hex(&self) -> Result<String> {
        Ok(self.inner.to_hex())
    }
}
