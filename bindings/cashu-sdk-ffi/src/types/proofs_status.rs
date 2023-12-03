use std::ops::Deref;
use std::sync::Arc;

use cashu_sdk::types::ProofsStatus as ProofsStatusSdk;

use crate::Proof;

pub struct ProofsStatus {
    inner: ProofsStatusSdk,
}

impl Deref for ProofsStatus {
    type Target = ProofsStatusSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<ProofsStatusSdk> for ProofsStatus {
    fn from(inner: ProofsStatusSdk) -> ProofsStatus {
        ProofsStatus { inner }
    }
}

impl ProofsStatus {
    pub fn new(spendable: Vec<Arc<Proof>>, spent: Vec<Arc<Proof>>) -> Self {
        Self {
            inner: ProofsStatusSdk {
                spendable: spendable
                    .iter()
                    .map(|p| p.as_ref().deref().clone())
                    .collect(),
                spent: spent.iter().map(|p| p.as_ref().deref().clone()).collect(),
            },
        }
    }

    pub fn spendable(&self) -> Vec<Arc<Proof>> {
        self.inner
            .spendable
            .clone()
            .into_iter()
            .map(|p| Arc::new(p.into()))
            .collect()
    }

    pub fn spent(&self) -> Vec<Arc<Proof>> {
        self.inner
            .spent
            .clone()
            .into_iter()
            .map(|p| Arc::new(p.into()))
            .collect()
    }
}
