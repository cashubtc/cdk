use std::ops::Deref;
use std::sync::Arc;

use cashu::nuts::nut06::{SplitRequest as SplitRequestSdk, SplitResponse as SplitResponseSdk};

use crate::{Amount, BlindedMessage, BlindedSignature, Proof};

pub struct SplitRequest {
    inner: SplitRequestSdk,
}

impl Deref for SplitRequest {
    type Target = SplitRequestSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl SplitRequest {
    pub fn new(proofs: Vec<Arc<Proof>>, outputs: Vec<Arc<BlindedMessage>>) -> Self {
        let proofs = proofs.into_iter().map(|p| p.as_ref().into()).collect();
        let outputs = outputs.into_iter().map(|o| o.as_ref().into()).collect();

        Self {
            inner: SplitRequestSdk::new(proofs, outputs),
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

    pub fn outputs(&self) -> Vec<Arc<BlindedMessage>> {
        self.inner
            .outputs
            .clone()
            .into_iter()
            .map(|o| Arc::new(o.into()))
            .collect()
    }

    pub fn proofs_amount(&self) -> Arc<Amount> {
        Arc::new(self.inner.proofs_amount().into())
    }

    pub fn output_amount(&self) -> Arc<Amount> {
        Arc::new(self.inner.output_amount().into())
    }
}

pub struct SplitResponse {
    inner: SplitResponseSdk,
}

impl SplitResponse {
    pub fn new(promises: Vec<Arc<BlindedSignature>>) -> Self {
        let promises = promises.into_iter().map(|p| p.as_ref().into()).collect();
        Self {
            inner: SplitResponseSdk::new(promises),
        }
    }

    pub fn promises(&self) -> Vec<Arc<BlindedSignature>> {
        self.inner
            .promises
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|p| Arc::new(p.into()))
            .collect()
    }

    pub fn promises_amount(&self) -> Option<Arc<Amount>> {
        self.inner.promises_amount().map(|a| Arc::new(a.into()))
    }
}

impl From<cashu::nuts::nut06::SplitResponse> for SplitResponse {
    fn from(inner: cashu::nuts::nut06::SplitResponse) -> SplitResponse {
        SplitResponse { inner }
    }
}
