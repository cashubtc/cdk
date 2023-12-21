use std::ops::Deref;
use std::sync::Arc;

use cashu::nuts::{SwapRequest as SwapRequestSdk, SwapResponse as SwapResponseSdk};

use crate::{Amount, BlindedMessage, BlindedSignature, Proof};

pub struct SwapRequest {
    inner: SwapRequestSdk,
}

impl Deref for SwapRequest {
    type Target = SwapRequestSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl SwapRequest {
    pub fn new(proofs: Vec<Arc<Proof>>, outputs: Vec<Arc<BlindedMessage>>) -> Self {
        let proofs = proofs.into_iter().map(|p| p.as_ref().into()).collect();
        let outputs = outputs.into_iter().map(|o| o.as_ref().into()).collect();

        Self {
            inner: SwapRequestSdk::new(proofs, outputs),
        }
    }

    pub fn proofs(&self) -> Vec<Arc<Proof>> {
        self.inner
            .inputs
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
        Arc::new(self.inner.input_amount().into())
    }

    pub fn output_amount(&self) -> Arc<Amount> {
        Arc::new(self.inner.output_amount().into())
    }
}

pub struct SwapResponse {
    inner: SwapResponseSdk,
}

impl SwapResponse {
    pub fn new(signatures: Vec<Arc<BlindedSignature>>) -> Self {
        let signatures = signatures.into_iter().map(|p| p.as_ref().into()).collect();
        Self {
            inner: SwapResponseSdk::new(signatures),
        }
    }

    pub fn signatures(&self) -> Vec<Arc<BlindedSignature>> {
        self.inner
            .signatures
            .clone()
            .into_iter()
            .map(|p| Arc::new(p.into()))
            .collect()
    }
}

impl From<SwapResponseSdk> for SwapResponse {
    fn from(inner: SwapResponseSdk) -> SwapResponse {
        SwapResponse { inner }
    }
}
