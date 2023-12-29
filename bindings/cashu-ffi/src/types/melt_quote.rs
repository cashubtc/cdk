use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use cashu::nuts::CurrencyUnit;
use cashu::types::MeltQuote as MeltQuoteSdk;

use crate::{Amount, Bolt11Invoice};

pub struct MeltQuote {
    inner: MeltQuoteSdk,
}

impl Deref for MeltQuote {
    type Target = MeltQuoteSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<MeltQuoteSdk> for MeltQuote {
    fn from(inner: MeltQuoteSdk) -> MeltQuote {
        MeltQuote { inner }
    }
}

impl MeltQuote {
    pub fn new(
        id: String,
        amount: Arc<Amount>,
        unit: String,
        request: Arc<Bolt11Invoice>,
        fee_reserve: Arc<Amount>,
        paid: bool,
        expiry: u64,
    ) -> Self {
        Self {
            inner: MeltQuoteSdk {
                id,
                amount: *amount.as_ref().deref(),
                unit: CurrencyUnit::from_str(&unit).unwrap(),
                request: request.as_ref().deref().clone(),
                fee_reserve: *fee_reserve.as_ref().deref(),
                paid,
                expiry,
            },
        }
    }
}
