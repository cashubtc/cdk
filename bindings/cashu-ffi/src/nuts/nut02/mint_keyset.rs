use std::ops::Deref;
use std::str::FromStr;

use cashu::nuts::nut02::mint::KeySet as KeySetSdk;
use cashu::nuts::CurrencyUnit;

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
    pub fn generate(secret: String, unit: String, derivation_path: String, max_order: u8) -> Self {
        Self {
            inner: KeySetSdk::generate(
                secret.as_bytes(),
                CurrencyUnit::from_str(&unit).unwrap(),
                &derivation_path,
                max_order,
            ),
        }
    }
}

impl From<cashu::nuts::nut02::mint::KeySet> for MintKeySet {
    fn from(inner: cashu::nuts::nut02::mint::KeySet) -> MintKeySet {
        MintKeySet { inner }
    }
}
