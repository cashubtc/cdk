use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use cashu::nuts::CurrencyUnit;
use cashu::types::MintQuote as MintQuoteSdk;

use crate::{Amount, Bolt11Invoice};

pub struct MintQuote {
    inner: MintQuoteSdk,
}

impl Deref for MintQuote {
    type Target = MintQuoteSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MintQuoteSdk> for MintQuote {
    fn from(inner: MintQuoteSdk) -> MintQuote {
        MintQuote { inner }
    }
}

impl MintQuote {
    pub fn new(
        id: String,
        amount: Arc<Amount>,
        unit: String,
        request: Arc<Bolt11Invoice>,
        paid: bool,
        expiry: u64,
    ) -> Self {
        Self {
            inner: MintQuoteSdk {
                id,
                amount: amount.as_ref().deref().clone(),
                unit: CurrencyUnit::from_str(&unit).unwrap(),
                request: request.as_ref().deref().clone(),
                paid,
                expiry,
            },
        }
    }
}
