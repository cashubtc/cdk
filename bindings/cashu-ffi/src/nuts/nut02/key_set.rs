use std::ops::Deref;
use std::sync::Arc;

use cashu::nuts::nut02::{Id as IdSdk, KeySet as KeySetSdk, Response};

use crate::error::Result;
use crate::nuts::nut01::keys::Keys;

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
            inner: IdSdk::try_from_base64(&id)?,
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
    pub fn new(id: Arc<Id>, keys: Arc<Keys>) -> Self {
        Self {
            inner: KeySetSdk {
                id: *id.as_ref().deref(),
                keys: keys.as_ref().deref().clone(),
            },
        }
    }

    pub fn id(&self) -> Arc<Id> {
        Arc::new(self.inner.id.into())
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
    inner: Response,
}

impl KeySetResponse {
    pub fn new(keyset_ids: Vec<Arc<Id>>) -> Self {
        let keysets = keyset_ids.into_iter().map(|id| id.inner).collect();
        Self {
            inner: Response { keysets },
        }
    }

    pub fn keyset_ids(&self) -> Vec<Arc<Id>> {
        self.inner
            .clone()
            .keysets
            .into_iter()
            .map(|id| Arc::new(id.into()))
            .collect()
    }
}

impl From<cashu::nuts::nut02::Response> for KeySetResponse {
    fn from(inner: Response) -> KeySetResponse {
        KeySetResponse { inner }
    }
}
