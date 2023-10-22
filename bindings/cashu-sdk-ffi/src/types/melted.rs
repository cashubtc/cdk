use std::ops::Deref;
use std::sync::Arc;

use cashu_ffi::Proof;
use cashu_sdk::types::Melted as MeltedSdk;

pub struct Melted {
    inner: MeltedSdk,
}

impl Deref for Melted {
    type Target = MeltedSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<cashu_sdk::types::Melted> for Melted {
    fn from(inner: cashu_sdk::types::Melted) -> Melted {
        Melted { inner }
    }
}

impl Melted {
    pub fn new(paid: bool, preimage: Option<String>, change: Option<Vec<Arc<Proof>>>) -> Self {
        Self {
            inner: MeltedSdk {
                paid,
                preimage,
                change: change.map(|c| c.iter().map(|p| p.as_ref().deref().clone()).collect()),
            },
        }
    }

    pub fn preimage(&self) -> Option<String> {
        self.inner.preimage.clone()
    }

    pub fn paid(&self) -> bool {
        self.inner.paid
    }

    pub fn change(&self) -> Option<Vec<Arc<Proof>>> {
        self.inner
            .change
            .clone()
            .map(|c| c.into_iter().map(|p| Arc::new(p.into())).collect())
    }
}
