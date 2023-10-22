use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

use cashu::nuts::nut01::{Keys as KeysSdk, Response as KeysResponseSdk};
use cashu::Amount as AmountSdk;

use crate::{Amount, PublicKey};

pub struct Keys {
    inner: KeysSdk,
}

impl Deref for Keys {
    type Target = KeysSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<Keys> for KeysSdk {
    fn from(keys: Keys) -> KeysSdk {
        keys.inner
    }
}

impl From<KeysSdk> for Keys {
    fn from(keys: KeysSdk) -> Keys {
        let keys = keys
            .keys()
            .into_iter()
            .map(|(amount, pk)| (amount.to_sat().to_string(), Arc::new(pk.into())))
            .collect();

        Keys::new(keys)
    }
}

impl Keys {
    pub fn new(keys: HashMap<String, Arc<PublicKey>>) -> Self {
        let keys = keys
            .into_iter()
            .map(|(amount, pk)| {
                (
                    AmountSdk::from_sat(amount.parse::<u64>().unwrap()),
                    pk.as_ref().into(),
                )
            })
            .collect();

        Self {
            inner: KeysSdk::new(keys),
        }
    }

    pub fn keys(&self) -> HashMap<String, Arc<PublicKey>> {
        self.inner
            .keys()
            .into_iter()
            .map(|(amount, pk)| (amount.to_sat().to_string(), Arc::new(pk.into())))
            .collect()
    }

    pub fn amount_key(&self, amount: Arc<Amount>) -> Option<Arc<PublicKey>> {
        self.inner
            .amount_key(*amount.as_ref().deref())
            .map(|pk| Arc::new(pk.into()))
    }

    pub fn as_hashmap(&self) -> HashMap<String, String> {
        self.inner
            .as_hashmap()
            .into_iter()
            .map(|(amount, pk)| (amount.to_sat().to_string(), pk))
            .collect()
    }
}

pub struct KeysResponse {
    inner: KeysResponseSdk,
}

impl Deref for KeysResponse {
    type Target = KeysResponseSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<KeysResponse> for KeysResponseSdk {
    fn from(keys: KeysResponse) -> KeysResponseSdk {
        keys.inner
    }
}

impl From<KeysResponseSdk> for KeysResponse {
    fn from(keys: KeysResponseSdk) -> KeysResponse {
        KeysResponse { inner: keys }
    }
}

impl KeysResponse {
    pub fn new(keys: Arc<Keys>) -> Self {
        Self {
            inner: KeysResponseSdk {
                keys: keys.as_ref().deref().clone(),
            },
        }
    }
}
