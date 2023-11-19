use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use cashu_sdk::mint::MintKeySetInfo as MintKeySetInfoSdk;
use cashu_sdk::nuts::CurrencyUnit;

use crate::Id;

pub struct MintKeySetInfo {
    inner: MintKeySetInfoSdk,
}

impl Deref for MintKeySetInfo {
    type Target = MintKeySetInfoSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MintKeySetInfoSdk> for MintKeySetInfo {
    fn from(inner: MintKeySetInfoSdk) -> MintKeySetInfo {
        MintKeySetInfo { inner }
    }
}

impl MintKeySetInfo {
    pub fn new(
        id: Arc<Id>,
        active: bool,
        unit: String,
        valid_from: u64,
        valid_to: Option<u64>,
        derivation_path: String,
        max_order: u8,
    ) -> Self {
        Self {
            inner: MintKeySetInfoSdk {
                id: *id.as_ref().deref(),
                active,
                unit: CurrencyUnit::from_str(&unit).unwrap(),
                valid_from,
                valid_to,
                derivation_path,
                max_order,
            },
        }
    }
}
