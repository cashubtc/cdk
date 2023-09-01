use std::{ops::Deref, sync::Arc};

use cashu_sdk::types::SendProofs as SendProofSdk;

use cashu_ffi::Proof;

pub struct SendProofs {
    inner: SendProofSdk,
}

impl SendProofs {
    pub fn new(change_proofs: Vec<Arc<Proof>>, send_proofs: Vec<Arc<Proof>>) -> Self {
        Self {
            inner: SendProofSdk {
                change_proofs: change_proofs
                    .iter()
                    .map(|p| p.as_ref().deref().clone())
                    .collect(),
                send_proofs: send_proofs
                    .iter()
                    .map(|p| p.as_ref().deref().clone())
                    .collect(),
            },
        }
    }

    pub fn send_proofs(&self) -> Vec<Arc<Proof>> {
        self.inner
            .send_proofs
            .clone()
            .into_iter()
            .map(|p| Arc::new(p.into()))
            .collect()
    }

    pub fn change_proofs(&self) -> Vec<Arc<Proof>> {
        self.inner
            .change_proofs
            .clone()
            .into_iter()
            .map(|p| Arc::new(p.into()))
            .collect()
    }
}

impl From<cashu_sdk::types::SendProofs> for SendProofs {
    fn from(inner: cashu_sdk::types::SendProofs) -> SendProofs {
        SendProofs { inner }
    }
}
