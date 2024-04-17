use std::fmt;

use serde::{Deserialize, Serialize};

/// Number of satoshis
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Amount(u64);

impl Amount {
    pub const ZERO: Amount = Amount(0);

    /// Split into parts that are powers of two
    pub fn split(&self) -> Vec<Self> {
        let sats = self.0;
        (0_u64..64)
            .rev()
            .filter_map(|bit| {
                let part = 1 << bit;
                ((sats & part) == part).then_some(Self::from(part))
            })
            .collect()
    }
}

impl Default for Amount {
    fn default() -> Self {
        Amount::ZERO
    }
}

impl Default for &Amount {
    fn default() -> Self {
        &Amount::ZERO
    }
}

impl From<u64> for Amount {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<Amount> for u64 {
    fn from(value: Amount) -> Self {
        value.0
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

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl core::iter::Sum for Amount {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        let sats: u64 = iter.map(|amt| amt.0).sum();
        Amount::from(sats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_amount() {
        assert_eq!(Amount::from(1).split(), vec![Amount::from(1)]);
        assert_eq!(Amount::from(2).split(), vec![Amount::from(2)]);
        assert_eq!(
            Amount::from(3).split(),
            vec![Amount::from(2), Amount::from(1)]
        );
        let amounts: Vec<Amount> = [8, 2, 1].iter().map(|a| Amount::from(*a)).collect();
        assert_eq!(Amount::from(11).split(), amounts);
        let amounts: Vec<Amount> = [128, 64, 32, 16, 8, 4, 2, 1]
            .iter()
            .map(|a| Amount::from(*a))
            .collect();
        assert_eq!(Amount::from(255).split(), amounts);
    }
}
