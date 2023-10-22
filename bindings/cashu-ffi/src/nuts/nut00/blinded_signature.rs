use std::ops::Deref;
use std::sync::Arc;

use cashu::nuts::nut00::BlindedSignature as BlindedSignatureSdk;

use crate::{Amount, Id, PublicKey};

pub struct BlindedSignature {
    inner: BlindedSignatureSdk,
}

impl BlindedSignature {
    pub fn new(id: Arc<Id>, amount: Arc<Amount>, c: Arc<PublicKey>) -> Self {
        Self {
            inner: BlindedSignatureSdk {
                id: *id.as_ref().deref(),
                amount: *amount.as_ref().deref(),
                c: c.as_ref().into(),
            },
        }
    }

    pub fn id(&self) -> Arc<Id> {
        Arc::new(self.inner.id.into())
    }

    pub fn amount(&self) -> Arc<Amount> {
        Arc::new(self.inner.amount.into())
    }

    pub fn c(&self) -> Arc<PublicKey> {
        Arc::new(self.inner.c.clone().into())
    }
}

impl From<&BlindedSignature> for BlindedSignatureSdk {
    fn from(blinded_signature: &BlindedSignature) -> BlindedSignatureSdk {
        blinded_signature.inner.clone()
    }
}

impl From<BlindedSignatureSdk> for BlindedSignature {
    fn from(blinded_signature: BlindedSignatureSdk) -> BlindedSignature {
        BlindedSignature {
            inner: blinded_signature,
        }
    }
}
