use std::ops::Deref;
use std::sync::Arc;

use cashu_ffi::Proof;
use cashu_sdk::types::SendProofs as SendProofsSdk;

pub struct SendProofs {
    inner: SendProofsSdk,
}

impl Deref for SendProofs {
    type Target = SendProofsSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<SendProofsSdk> for SendProofs {
    fn from(inner: SendProofsSdk) -> SendProofs {
        SendProofs { inner }
    }
}

impl SendProofs {
    pub fn new(change_proofs: Vec<Arc<Proof>>, send_proofs: Vec<Arc<Proof>>) -> Self {
        Self {
            inner: SendProofsSdk {
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
