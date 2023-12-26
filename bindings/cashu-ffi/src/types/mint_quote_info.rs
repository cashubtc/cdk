use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use cashu::nuts::CurrencyUnit;
use cashu::types::MintQuoteInfo as MintQuoteInfoSdk;

use crate::{Amount, Bolt11Invoice};

pub struct MintQuoteInfo {
    inner: MintQuoteInfoSdk,
}

impl Deref for MintQuoteInfo {
    type Target = MintQuoteInfoSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MintQuoteInfoSdk> for MintQuoteInfo {
    fn from(inner: MintQuoteInfoSdk) -> MintQuoteInfo {
        MintQuoteInfo { inner }
    }
}

impl MintQuoteInfo {
    pub fn new(
        id: String,
        amount: Arc<Amount>,
        unit: String,
        request: Option<Arc<Bolt11Invoice>>,
        paid: bool,
        expiry: u64,
    ) -> Self {
        Self {
            inner: MintQuoteInfoSdk {
                id,
                amount: amount.as_ref().deref().clone(),
                unit: CurrencyUnit::from_str(&unit).unwrap(),
                request: request.map(|r| r.as_ref().deref().clone()),
                paid,
                expiry,
            },
        }
    }
}
