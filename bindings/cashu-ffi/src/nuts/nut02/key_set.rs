use std::collections::HashSet;
use std::ops::Deref;
use std::sync::Arc;

use cashu::nuts::nut02::KeySet as KeySetSdk;
use cashu::nuts::nut02::Response;

use crate::nuts::nut01::keys::Keys;

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
    pub fn new(id: String, keys: Arc<Keys>) -> Self {
        Self {
            inner: KeySetSdk {
                id,
                keys: keys.as_ref().deref().clone(),
            },
        }
    }

    pub fn id(&self) -> String {
        self.inner.id.clone()
    }

    pub fn keys(&self) -> Arc<Keys> {
        Arc::new(self.inner.keys.clone().into())
    }
}

pub struct KeySetResponse {
    inner: Response,
}

impl KeySetResponse {
    pub fn new(keyset_ids: Vec<String>) -> Self {
        let keysets = HashSet::from_iter(keyset_ids);
        Self {
            inner: Response { keysets },
        }
    }

    pub fn keyset_ids(&self) -> Vec<String> {
        self.inner.clone().keysets.into_iter().collect()
    }
}
