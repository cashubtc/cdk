// https://github.com/clarkmoody/cashu-rs
use serde::{Deserialize, Serialize};

/// Number of satoshis
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Amount(#[serde(with = "bitcoin::amount::serde::as_sat")] bitcoin::Amount);

impl Amount {
    pub const ZERO: Amount = Amount(bitcoin::Amount::ZERO);

    /// Split into parts that are powers of two
    pub fn split(&self) -> Vec<Self> {
        let sats = self.0.to_sat();
        (0_u64..64)
            .rev()
            .filter_map(|bit| {
                let part = 1 << bit;
                ((sats & part) == part).then_some(Self::from(part))
            })
            .collect()
    }

    pub fn to_sat(&self) -> u64 {
        self.0.to_sat()
    }

    pub fn to_msat(&self) -> u64 {
        self.0.to_sat() * 1000
    }

    pub fn from_sat(sat: u64) -> Self {
        Self(bitcoin::Amount::from_sat(sat))
    }

    pub fn from_msat(msat: u64) -> Self {
        Self(bitcoin::Amount::from_sat(msat / 1000))
    }
}

impl Default for Amount {
    fn default() -> Self {
        Amount::ZERO
    }
}

impl From<u64> for Amount {
    fn from(value: u64) -> Self {
        Self(bitcoin::Amount::from_sat(value))
    }
}

impl From<Amount> for u64 {
    fn from(value: Amount) -> Self {
        value.0.to_sat()
    }
}

impl std::ops::Add for Amount {
    type Output = Amount;

    fn add(self, rhs: Amount) -> Self::Output {
        Amount(self.0 + rhs.0)
    }
}

impl std::ops::AddAssign for Amount {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl std::ops::Sub for Amount {
    type Output = Amount;

    fn sub(self, rhs: Amount) -> Self::Output {
        Amount(self.0 - rhs.0)
    }
}

impl core::iter::Sum for Amount {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        let sats: u64 = iter.map(|amt| amt.0.to_sat()).sum();
        Amount::from(sats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_amount() {
        assert_eq!(Amount::from_sat(1).split(), vec![Amount::from_sat(1)]);
        assert_eq!(Amount::from_sat(2).split(), vec![Amount::from_sat(2)]);
        assert_eq!(
            Amount::from_sat(3).split(),
            vec![Amount::from_sat(2), Amount::from_sat(1)]
        );
        let amounts: Vec<Amount> = [8, 2, 1].iter().map(|a| Amount::from_sat(*a)).collect();
        assert_eq!(Amount::from_sat(11).split(), amounts);
        let amounts: Vec<Amount> = [128, 64, 32, 16, 8, 4, 2, 1]
            .iter()
            .map(|a| Amount::from_sat(*a))
            .collect();
        assert_eq!(Amount::from_sat(255).split(), amounts);
    }
}
