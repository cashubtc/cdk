use std::str::FromStr;
use std::sync::Arc;

use cashu::nuts::nut08::{MeltRequest as MeltRequestSdk, MeltResponse as MeltResponseSdk};
use cashu::Bolt11Invoice;

use crate::error::Result;
use crate::{BlindedMessage, BlindedSignature, Proof};

pub struct MeltRequest {
    inner: MeltRequestSdk,
}

impl MeltRequest {
    pub fn new(
        proofs: Vec<Arc<Proof>>,
        invoice: String,
        outputs: Option<Vec<Arc<BlindedMessage>>>,
    ) -> Result<Self> {
        let pr = Bolt11Invoice::from_str(&invoice)?;

        Ok(Self {
            inner: MeltRequestSdk {
                proofs: proofs.iter().map(|p| p.as_ref().into()).collect(),
                pr,
                outputs: outputs
                    .map(|outputs| outputs.into_iter().map(|o| o.as_ref().into()).collect()),
            },
        })
    }

    pub fn proofs(&self) -> Vec<Arc<Proof>> {
        self.inner
            .proofs
            .clone()
            .into_iter()
            .map(|o| Arc::new(o.into()))
            .collect()
    }

    pub fn invoice(&self) -> String {
        self.inner.pr.to_string()
    }

    pub fn outputs(&self) -> Option<Vec<Arc<BlindedMessage>>> {
        self.inner
            .outputs
            .clone()
            .map(|outputs| outputs.into_iter().map(|o| Arc::new(o.into())).collect())
    }
}

pub struct MeltResponse {
    inner: MeltResponseSdk,
}

impl MeltResponse {
    pub fn new(
        paid: bool,
        preimage: Option<String>,
        change: Option<Vec<Arc<BlindedSignature>>>,
    ) -> Self {
        Self {
            inner: MeltResponseSdk {
                paid,
                preimage,
                change: change
                    .map(|change| change.into_iter().map(|bs| bs.as_ref().into()).collect()),
            },
        }
    }

    pub fn paid(&self) -> bool {
        self.inner.paid
    }

    pub fn preimage(&self) -> Option<String> {
        self.inner.preimage.clone()
    }

    pub fn change(&self) -> Option<Vec<Arc<BlindedSignature>>> {
        self.inner
            .change
            .clone()
            .map(|change| change.into_iter().map(|bs| Arc::new(bs.into())).collect())
    }
}

impl From<cashu::nuts::nut08::MeltResponse> for MeltResponse {
    fn from(inner: cashu::nuts::nut08::MeltResponse) -> MeltResponse {
        MeltResponse { inner }
    }
}

impl From<MeltResponse> for cashu::nuts::nut08::MeltResponse {
    fn from(res: MeltResponse) -> cashu::nuts::nut08::MeltResponse {
        res.inner
    }
}
