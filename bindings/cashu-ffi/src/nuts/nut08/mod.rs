use std::ops::Deref;
use std::sync::Arc;

use cashu::nuts::nut08::{
    MeltBolt11Request as MeltBolt11RequestSdk, MeltBolt11Response as MeltBolt11ResponseSdk,
};

use crate::error::Result;
use crate::{BlindedMessage, BlindedSignature, Proof};

pub struct MeltBolt11Request {
    inner: MeltBolt11RequestSdk,
}

impl Deref for MeltBolt11Request {
    type Target = MeltBolt11RequestSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl MeltBolt11Request {
    pub fn new(
        quote: String,
        proofs: Vec<Arc<Proof>>,
        outputs: Option<Vec<Arc<BlindedMessage>>>,
    ) -> Result<Self> {
        Ok(Self {
            inner: MeltBolt11RequestSdk {
                quote,
                inputs: proofs.iter().map(|p| p.as_ref().into()).collect(),
                outputs: outputs
                    .map(|outputs| outputs.into_iter().map(|o| o.as_ref().into()).collect()),
            },
        })
    }

    pub fn inputs(&self) -> Vec<Arc<Proof>> {
        self.inner
            .inputs
            .clone()
            .into_iter()
            .map(|o| Arc::new(o.into()))
            .collect()
    }

    pub fn quote(&self) -> String {
        self.inner.quote.clone()
    }

    pub fn outputs(&self) -> Option<Vec<Arc<BlindedMessage>>> {
        self.inner
            .outputs
            .clone()
            .map(|outputs| outputs.into_iter().map(|o| Arc::new(o.into())).collect())
    }
}

pub struct MeltBolt11Response {
    inner: MeltBolt11ResponseSdk,
}

impl Deref for MeltBolt11Response {
    type Target = MeltBolt11ResponseSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<cashu::nuts::nut08::MeltBolt11Response> for MeltBolt11Response {
    fn from(inner: cashu::nuts::nut08::MeltBolt11Response) -> MeltBolt11Response {
        MeltBolt11Response { inner }
    }
}

impl From<MeltBolt11Response> for cashu::nuts::nut08::MeltBolt11Response {
    fn from(res: MeltBolt11Response) -> cashu::nuts::nut08::MeltBolt11Response {
        res.inner
    }
}

impl MeltBolt11Response {
    pub fn new(
        paid: bool,
        payment_preimage: Option<String>,
        change: Option<Vec<Arc<BlindedSignature>>>,
    ) -> Self {
        Self {
            inner: MeltBolt11ResponseSdk {
                paid,
                payment_preimage,
                change: change
                    .map(|change| change.into_iter().map(|bs| bs.as_ref().into()).collect()),
            },
        }
    }

    pub fn paid(&self) -> bool {
        self.inner.paid
    }

    pub fn payment_preimage(&self) -> Option<String> {
        self.inner.payment_preimage.clone()
    }

    pub fn change(&self) -> Option<Vec<Arc<BlindedSignature>>> {
        self.inner
            .change
            .clone()
            .map(|change| change.into_iter().map(|bs| Arc::new(bs.into())).collect())
    }
}
