use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use cashu::nuts::nut05::{
    CheckFeesRequest as CheckFeesRequestSdk, CheckFeesResponse as CheckFeesResponseSdk,
    MeltRequest as MeltRequestSdk, MeltResponse as MeltResponseSdk,
};
use cashu::Bolt11Invoice;

use crate::error::Result;
use crate::{Amount, Proof};

pub struct CheckFeesRequest {
    inner: CheckFeesRequestSdk,
}

impl CheckFeesRequest {
    pub fn new(invoice: String) -> Result<Self> {
        Ok(Self {
            inner: CheckFeesRequestSdk {
                pr: Bolt11Invoice::from_str(&invoice)?,
            },
        })
    }

    pub fn invoice(&self) -> String {
        self.inner.pr.to_string()
    }
}

pub struct CheckFeesResponse {
    inner: CheckFeesResponseSdk,
}

impl CheckFeesResponse {
    pub fn new(amount: Arc<Amount>) -> Self {
        Self {
            inner: CheckFeesResponseSdk {
                fee: *amount.as_ref().deref(),
            },
        }
    }

    pub fn amount(&self) -> Arc<Amount> {
        Arc::new(self.inner.fee.into())
    }
}

impl From<cashu::nuts::nut05::CheckFeesResponse> for CheckFeesResponse {
    fn from(inner: cashu::nuts::nut05::CheckFeesResponse) -> CheckFeesResponse {
        Self { inner }
    }
}

impl From<CheckFeesResponse> for cashu::nuts::nut05::CheckFeesResponse {
    fn from(res: CheckFeesResponse) -> cashu::nuts::nut05::CheckFeesResponse {
        res.inner
    }
}

pub struct MeltRequest {
    inner: MeltRequestSdk,
}

impl MeltRequest {
    pub fn new(proofs: Vec<Arc<Proof>>, invoice: String) -> Result<Self> {
        let pr = Bolt11Invoice::from_str(&invoice)?;
        Ok(Self {
            inner: MeltRequestSdk {
                pr,
                proofs: proofs.into_iter().map(|p| p.as_ref().into()).collect(),
            },
        })
    }

    pub fn proofs(&self) -> Vec<Arc<Proof>> {
        self.inner
            .proofs
            .clone()
            .into_iter()
            .map(|p| Arc::new(p.into()))
            .collect()
    }

    pub fn invoice(&self) -> String {
        self.inner.pr.to_string()
    }
}

pub struct MeltResponse {
    inner: MeltResponseSdk,
}

impl MeltResponse {
    pub fn new(paid: bool, preimage: Option<String>) -> Self {
        Self {
            inner: MeltResponseSdk { paid, preimage },
        }
    }

    pub fn paid(&self) -> bool {
        self.inner.paid
    }

    pub fn preimage(&self) -> Option<String> {
        self.inner.preimage.clone()
    }
}
