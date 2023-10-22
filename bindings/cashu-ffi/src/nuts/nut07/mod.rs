use std::ops::Deref;
use std::sync::Arc;

use cashu::nuts::nut07::{
    CheckSpendableRequest as CheckSpendableRequestSdk,
    CheckSpendableResponse as CheckSpendableResponseSdk,
};

use crate::nuts::nut00::proof::mint::Proof;

pub struct CheckSpendableRequest {
    inner: CheckSpendableRequestSdk,
}

impl Deref for CheckSpendableRequest {
    type Target = CheckSpendableRequestSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl CheckSpendableRequest {
    pub fn new(proofs: Vec<Arc<Proof>>) -> Self {
        Self {
            inner: CheckSpendableRequestSdk {
                proofs: proofs.into_iter().map(|p| p.as_ref().into()).collect(),
            },
        }
    }

    pub fn proofs(&self) -> Vec<Arc<Proof>> {
        self.inner
            .proofs
            .clone()
            .into_iter()
            .map(|p| Arc::new(p.into()))
            .collect()
    }
}

pub struct CheckSpendableResponse {
    inner: CheckSpendableResponseSdk,
}

impl CheckSpendableResponse {
    pub fn new(spendable: Vec<bool>, pending: Vec<bool>) -> Self {
        Self {
            inner: CheckSpendableResponseSdk { spendable, pending },
        }
    }

    pub fn spendable(&self) -> Vec<bool> {
        self.inner.spendable.clone()
    }

    pub fn pending(&self) -> Vec<bool> {
        self.inner.pending.clone()
    }
}

impl From<cashu::nuts::nut07::CheckSpendableResponse> for CheckSpendableResponse {
    fn from(inner: cashu::nuts::nut07::CheckSpendableResponse) -> CheckSpendableResponse {
        CheckSpendableResponse { inner }
    }
}
