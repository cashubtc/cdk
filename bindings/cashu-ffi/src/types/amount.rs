use std::ops::Deref;
use std::sync::Arc;

use cashu::Amount as AmountSdk;

pub struct Amount {
    inner: AmountSdk,
}

impl Deref for Amount {
    type Target = AmountSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Amount {
    pub fn new(amount: u64) -> Self {
        Self {
            inner: AmountSdk::from(amount),
        }
    }

    pub const ZERO: Amount = Amount {
        inner: AmountSdk::ZERO,
    };

    /// Split into parts that are powers of two
    pub fn split(&self) -> Vec<Arc<Self>> {
        self.inner
            .split()
            .into_iter()
            .map(|a| Arc::new(a.into()))
            .collect()
    }
}

impl From<AmountSdk> for Amount {
    fn from(amount: AmountSdk) -> Amount {
        Amount { inner: amount }
    }
}

impl From<&Amount> for AmountSdk {
    fn from(amount: &Amount) -> AmountSdk {
        amount.inner
    }
}

impl From<u64> for Amount {
    fn from(amount: u64) -> Amount {
        AmountSdk::from(amount).into()
    }
}
