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
    pub fn new(sats: u64) -> Self {
        Self {
            inner: AmountSdk::from_sat(sats),
        }
    }

    pub fn to_sat(&self) -> u64 {
        self.inner.to_sat()
    }

    pub fn to_msat(&self) -> u64 {
        self.inner.to_msat()
    }

    pub fn from_sat(sats: u64) -> Self {
        Self {
            inner: AmountSdk::from_sat(sats),
        }
    }

    pub fn from_msat(msats: u64) -> Self {
        Self {
            inner: AmountSdk::from_msat(msats),
        }
    }

    pub const ZERO: Amount = Amount {
        inner: AmountSdk::ZERO,
    };

    /// Split into parts that are powers of two
    pub fn split(&self) -> Vec<Arc<Self>> {
        let sats = self.inner.to_sat();
        (0_u64..64)
            .rev()
            .filter_map(|bit| {
                let part = 1 << bit;
                ((sats & part) == part).then_some(Arc::new(Self::from_sat(part)))
            })
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
