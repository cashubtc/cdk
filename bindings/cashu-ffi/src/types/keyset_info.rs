use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use cashu::nuts::{CurrencyUnit, KeySetInfo as KeySetInfoSdk};

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
    pub fn new(id: Arc<Id>, unit: String, active: bool) -> Self {
        Self {
            inner: KeySetInfoSdk {
                id: *id.as_ref().deref(),
                unit: CurrencyUnit::from_str(&unit).unwrap(),
                active,
            },
        }
    }
}
