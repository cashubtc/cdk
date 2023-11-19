use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use cashu::nuts::{
    CurrencyUnit, Id as IdSdk, KeySet as KeySetSdk, KeysetResponse as KeysetResponseSdk,
};

use crate::error::Result;
use crate::nuts::nut01::keys::Keys;
use crate::KeySetInfo;

pub struct Id {
    inner: IdSdk,
}

impl Deref for Id {
    type Target = IdSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl Id {
    pub fn new(id: String) -> Result<Self> {
        Ok(Self {
            inner: IdSdk::from_str(&id)?,
        })
    }
}

impl From<IdSdk> for Id {
    fn from(inner: IdSdk) -> Id {
        Id { inner }
    }
}

impl From<Id> for IdSdk {
    fn from(id: Id) -> IdSdk {
        id.inner
    }
}

pub struct KeySet {
    inner: KeySetSdk,
}

impl Deref for KeySet {
    type Target = KeySetSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl KeySet {
    pub fn new(id: Arc<Id>, unit: String, keys: Arc<Keys>) -> Self {
        Self {
            inner: KeySetSdk {
                id: *id.as_ref().deref(),
                unit: CurrencyUnit::from_str(&unit).unwrap(),
                keys: keys.as_ref().deref().clone(),
            },
        }
    }

    pub fn id(&self) -> Arc<Id> {
        Arc::new(self.inner.id.into())
    }

    pub fn unit(&self) -> String {
        self.inner.unit.clone().to_string()
    }

    pub fn keys(&self) -> Arc<Keys> {
        Arc::new(self.inner.keys.clone().into())
    }
}

impl From<cashu::nuts::nut02::KeySet> for KeySet {
    fn from(inner: cashu::nuts::nut02::KeySet) -> KeySet {
        KeySet { inner }
    }
}

pub struct KeySetResponse {
    inner: KeysetResponseSdk,
}

impl KeySetResponse {
    pub fn new(keyset_ids: Vec<Arc<KeySetInfo>>) -> Self {
        let keysets = keyset_ids
            .into_iter()
            .map(|ki| ki.as_ref().deref().clone())
            .collect();
        Self {
            inner: KeysetResponseSdk { keysets },
        }
    }

    pub fn keysets(&self) -> Vec<Arc<KeySetInfo>> {
        self.inner
            .clone()
            .keysets
            .into_iter()
            .map(|keyset_info| Arc::new(keyset_info.into()))
            .collect()
    }
}

impl From<KeysetResponseSdk> for KeySetResponse {
    fn from(inner: KeysetResponseSdk) -> KeySetResponse {
        KeySetResponse { inner }
    }
}
