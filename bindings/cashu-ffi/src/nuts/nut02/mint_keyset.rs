use std::ops::Deref;

use cashu::nuts::nut02::mint::KeySet as KeySetSdk;

pub struct MintKeySet {
    inner: KeySetSdk,
}

impl Deref for MintKeySet {
    type Target = KeySetSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl MintKeySet {
    pub fn generate(secret: String, derivation_path: String, max_order: u8) -> Self {
        Self {
            inner: KeySetSdk::generate(secret, derivation_path, max_order),
        }
    }
}
