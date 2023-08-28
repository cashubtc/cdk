use std::{collections::HashMap, ops::Deref, sync::Arc};

use crate::{Amount, PublicKey};
use cashu::nuts::nut01::Keys as KeysSdk;

pub struct Keys {
    inner: KeysSdk,
}

impl Deref for Keys {
    type Target = KeysSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Keys {
    pub fn new(keys: HashMap<String, Arc<PublicKey>>) -> Self {
        let keys = keys
            .into_iter()
            .map(|(amount, pk)| (amount.parse::<u64>().unwrap(), pk.as_ref().into()))
            .collect();

        Self {
            inner: KeysSdk::new(keys),
        }
    }

    pub fn keys(&self) -> HashMap<String, Arc<PublicKey>> {
        self.inner
            .keys()
            .into_iter()
            .map(|(amount, pk)| (amount.to_string(), Arc::new(pk.into())))
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
            .map(|(amount, pk)| (amount.to_string(), pk))
            .collect()
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
            .map(|(amount, pk)| (amount.to_string(), Arc::new(pk.into())))
            .collect();

        Keys::new(keys)
    }
}
