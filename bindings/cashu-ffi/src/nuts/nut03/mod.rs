use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use cashu::nuts::nut03::RequestMintResponse as RequestMintResponseSdk;
use cashu::nuts::{SplitRequest as SplitRequestSdk, SplitResponse as SplitResponseSdk};
use cashu::Bolt11Invoice;

use crate::error::Result;
use crate::{Amount, BlindedMessage, BlindedSignature, Proof};

pub struct RequestMintResponse {
    inner: RequestMintResponseSdk,
}

impl RequestMintResponse {
    pub fn new(invoice: String, hash: String) -> Result<Self> {
        let pr = Bolt11Invoice::from_str(&invoice)?;

        Ok(Self {
            inner: RequestMintResponseSdk { pr, hash },
        })
    }

    pub fn invoice(&self) -> String {
        self.inner.pr.to_string()
    }

    pub fn hash(&self) -> String {
        self.inner.hash.to_string()
    }
}

impl From<cashu::nuts::nut03::RequestMintResponse> for RequestMintResponse {
    fn from(mint_response: cashu::nuts::nut03::RequestMintResponse) -> RequestMintResponse {
        RequestMintResponse {
            inner: mint_response,
        }
    }
}

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

impl From<cashu::nuts::SplitResponse> for SplitResponse {
    fn from(inner: cashu::nuts::SplitResponse) -> SplitResponse {
        SplitResponse { inner }
    }
}
