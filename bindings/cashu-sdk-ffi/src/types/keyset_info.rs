use std::ops::Deref;
use std::sync::Arc;

use cashu_sdk::types::KeysetInfo as KeySetInfoSdk;

use crate::Id;

pub struct KeySetInfo {
    inner: KeySetInfoSdk,
}

impl Deref for KeySetInfo {
    type Target = KeySetInfoSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<KeySetInfoSdk> for KeySetInfo {
    fn from(inner: KeySetInfoSdk) -> KeySetInfo {
        KeySetInfo { inner }
    }
}

impl KeySetInfo {
    pub fn new(
        id: Arc<Id>,
        unit: String,
        valid_from: u64,
        valid_to: Option<u64>,
        derivation_path: String,
        max_order: u8,
    ) -> Self {
        Self {
            inner: KeySetInfoSdk {
                id: *id.as_ref().deref(),
                unit,
                valid_from,
                valid_to,
                derivation_path,
                max_order,
            },
        }
    }
}
