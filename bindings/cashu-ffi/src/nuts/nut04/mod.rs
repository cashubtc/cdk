use std::{ops::Deref, sync::Arc};

use cashu::nuts::nut04::{MintRequest as MintRequestSdk, PostMintResponse as PostMintResponseSdk};

use crate::{Amount, BlindedMessage, BlindedSignature};

pub struct MintRequest {
    inner: MintRequestSdk,
}

impl MintRequest {
    pub fn new(outputs: Vec<Arc<BlindedMessage>>) -> Self {
        Self {
            inner: MintRequestSdk {
                outputs: outputs
                    .into_iter()
                    .map(|o| o.as_ref().deref().clone())
                    .collect(),
            },
        }
    }

    pub fn outputs(&self) -> Vec<Arc<BlindedMessage>> {
        self.inner
            .outputs
            .clone()
            .into_iter()
            .map(|o| Arc::new(o.into()))
            .collect()
    }

    pub fn total_amount(&self) -> Arc<Amount> {
        Arc::new(self.inner.total_amount().into())
    }
}

pub struct PostMintResponse {
    inner: PostMintResponseSdk,
}

impl PostMintResponse {
    pub fn new(promises: Vec<Arc<BlindedSignature>>) -> Self {
        Self {
            inner: PostMintResponseSdk {
                promises: promises.into_iter().map(|p| p.as_ref().into()).collect(),
            },
        }
    }

    pub fn promises(&self) -> Vec<Arc<BlindedSignature>> {
        self.inner
            .promises
            .clone()
            .into_iter()
            .map(|p| Arc::new(p.into()))
            .collect()
    }
}

impl From<cashu::nuts::nut04::PostMintResponse> for PostMintResponse {
    fn from(inner: cashu::nuts::nut04::PostMintResponse) -> PostMintResponse {
        PostMintResponse { inner }
    }
}
