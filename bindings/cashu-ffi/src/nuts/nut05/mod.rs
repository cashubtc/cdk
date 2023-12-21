use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use cashu::nuts::{
    CurrencyUnit, MeltBolt11Request as MeltBolt11RequestSdk,
    MeltBolt11Response as MeltBolt11ResponseSdk,
    MeltQuoteBolt11Request as MeltQuoteBolt11RequestSdk,
    MeltQuoteBolt11Response as MeltQuoteBolt11ResponseSdk,
};
use cashu::Bolt11Invoice;

use crate::error::Result;
use crate::{BlindedMessage, BlindedSignature, Proof};

pub struct MeltQuoteBolt11Response {
    inner: MeltQuoteBolt11ResponseSdk,
}

impl MeltQuoteBolt11Response {
    pub fn new(
        quote: String,
        amount: u64,
        fee_reserve: u64,
        paid: bool,
        expiry: u64,
    ) -> Result<Self> {
        Ok(Self {
            inner: MeltQuoteBolt11ResponseSdk {
                quote,
                amount,
                fee_reserve,
                paid,
                expiry,
            },
        })
    }

    pub fn quote(&self) -> String {
        self.inner.quote.clone()
    }

    pub fn amount(&self) -> u64 {
        self.inner.amount
    }

    pub fn fee_reserve(&self) -> u64 {
        self.inner.fee_reserve
    }

    pub fn paid(&self) -> bool {
        self.inner.paid
    }

    pub fn expiry(&self) -> u64 {
        self.inner.expiry
    }
}

pub struct MeltQuoteBolt11Request {
    inner: MeltQuoteBolt11RequestSdk,
}

impl MeltQuoteBolt11Request {
    pub fn new(request: String, unit: String) -> Result<Self> {
        Ok(Self {
            inner: MeltQuoteBolt11RequestSdk {
                request: Bolt11Invoice::from_str(&request)?,
                unit: CurrencyUnit::from_str(&unit)?,
            },
        })
    }

    pub fn request(&self) -> String {
        self.inner.request.to_string()
    }

    pub fn unit(&self) -> String {
        self.inner.unit.to_string()
    }
}

pub struct MeltBolt11Request {
    inner: MeltBolt11RequestSdk,
}

impl MeltBolt11Request {
    pub fn new(
        quote: String,
        inputs: Vec<Arc<Proof>>,
        outputs: Option<Vec<Arc<BlindedMessage>>>,
    ) -> Result<Self> {
        Ok(Self {
            inner: MeltBolt11RequestSdk {
                quote,
                inputs: inputs.into_iter().map(|p| p.as_ref().into()).collect(),
                outputs: outputs
                    .map(|o| o.into_iter().map(|p| p.as_ref().deref().clone()).collect()),
            },
        })
    }

    pub fn inputs(&self) -> Vec<Arc<Proof>> {
        self.inner
            .inputs
            .clone()
            .into_iter()
            .map(|p| Arc::new(p.into()))
            .collect()
    }

    pub fn quote(&self) -> String {
        self.inner.quote.clone()
    }
}

pub struct MeltBolt11Response {
    inner: MeltBolt11ResponseSdk,
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
                change: change.map(|c| c.into_iter().map(|b| b.as_ref().deref().clone()).collect()),
            },
        }
    }

    pub fn paid(&self) -> bool {
        self.inner.paid
    }

    pub fn payment_preimage(&self) -> Option<String> {
        self.inner.payment_preimage.clone()
    }
}
