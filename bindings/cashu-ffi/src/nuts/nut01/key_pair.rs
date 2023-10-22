use std::ops::Deref;
use std::sync::Arc;

use cashu::nuts::nut01::mint::KeyPair as KeyPairSdk;

use crate::{PublicKey, SecretKey};

pub struct KeyPair {
    inner: KeyPairSdk,
}

impl KeyPair {
    pub fn from_secret_key(secret_key: Arc<SecretKey>) -> Self {
        Self {
            inner: KeyPairSdk::from_secret_key(secret_key.as_ref().deref().clone()),
        }
    }

    pub fn secret_key(&self) -> Arc<SecretKey> {
        Arc::new(self.inner.secret_key.clone().into())
    }

    pub fn public_key(&self) -> Arc<PublicKey> {
        Arc::new(self.inner.public_key.clone().into())
    }
}
