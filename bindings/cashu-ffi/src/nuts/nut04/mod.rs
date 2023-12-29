use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use cashu::nuts::{
    CurrencyUnit, MintBolt11Request as MintBolt11RequestSdk,
    MintBolt11Response as MintBolt11ResponseSdk,
    MintQuoteBolt11Request as MintQuoteBolt11RequestSdk,
    MintQuoteBolt11Response as MintQuoteBolt11ResponseSdk,
};

use crate::{Amount, BlindedMessage, BlindedSignature};

pub struct MintQuoteBolt11Request {
    inner: MintQuoteBolt11RequestSdk,
}

impl Deref for MintQuoteBolt11Request {
    type Target = MintQuoteBolt11RequestSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl MintQuoteBolt11Request {
    pub fn new(amount: Arc<Amount>, unit: String) -> Self {
        Self {
            inner: MintQuoteBolt11RequestSdk {
                amount: *amount.as_ref().deref(),
                unit: CurrencyUnit::from_str(&unit).unwrap(),
            },
        }
    }

    pub fn amount(&self) -> Arc<Amount> {
        Arc::new(self.inner.amount.into())
    }

    pub fn unit(&self) -> Arc<CurrencyUnit> {
        Arc::new(self.inner.clone().unit)
    }
}

impl From<MintQuoteBolt11RequestSdk> for MintQuoteBolt11Request {
    fn from(inner: MintQuoteBolt11RequestSdk) -> MintQuoteBolt11Request {
        MintQuoteBolt11Request { inner }
    }
}

pub struct MintQuoteBolt11Response {
    inner: MintQuoteBolt11ResponseSdk,
}

impl Deref for MintQuoteBolt11Response {
    type Target = MintQuoteBolt11ResponseSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl MintQuoteBolt11Response {
    pub fn new(quote: String, request: String, paid: bool, expiry: u64) -> Self {
        Self {
            inner: MintQuoteBolt11ResponseSdk {
                quote,
                request,
                paid,
                expiry,
            },
        }
    }

    pub fn quote(&self) -> String {
        self.quote.clone()
    }

    pub fn request(&self) -> String {
        self.request.clone()
    }

    pub fn paid(&self) -> bool {
        self.paid
    }

    pub fn expiry(&self) -> u64 {
        self.expiry
    }
}

impl From<MintQuoteBolt11ResponseSdk> for MintQuoteBolt11Response {
    fn from(inner: MintQuoteBolt11ResponseSdk) -> MintQuoteBolt11Response {
        MintQuoteBolt11Response { inner }
    }
}

pub struct MintBolt11Request {
    inner: MintBolt11RequestSdk,
}

impl Deref for MintBolt11Request {
    type Target = MintBolt11RequestSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl MintBolt11Request {
    pub fn new(quote: String, outputs: Vec<Arc<BlindedMessage>>) -> Self {
        Self {
            inner: MintBolt11RequestSdk {
                quote,
                outputs: outputs.iter().map(|o| o.as_ref().deref().clone()).collect(),
            },
        }
    }

    pub fn quote(&self) -> String {
        self.quote.clone()
    }

    pub fn outputs(&self) -> Vec<Arc<BlindedMessage>> {
        self.inner
            .outputs
            .clone()
            .into_iter()
            .map(|o| Arc::new(o.into()))
            .collect()
    }
}

pub struct MintBolt11Response {
    inner: MintBolt11ResponseSdk,
}

impl Deref for MintBolt11Response {
    type Target = MintBolt11ResponseSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl MintBolt11Response {
    pub fn new(signatures: Vec<Arc<BlindedSignature>>) -> Self {
        Self {
            inner: MintBolt11ResponseSdk {
                signatures: signatures
                    .into_iter()
                    .map(|s| s.as_ref().deref().clone())
                    .collect(),
            },
        }
    }

    pub fn signatures(&self) -> Vec<Arc<BlindedSignature>> {
        self.inner
            .signatures
            .clone()
            .into_iter()
            .map(|o| Arc::new(o.into()))
            .collect()
    }
}

impl From<MintBolt11ResponseSdk> for MintBolt11Response {
    fn from(inner: MintBolt11ResponseSdk) -> MintBolt11Response {
        MintBolt11Response { inner }
    }
}
