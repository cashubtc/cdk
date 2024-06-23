use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::nuts::CurrencyUnit;

/// Number of satoshis
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Amount {
    pub value: u64,
    pub unit: CurrencyUnit,
}

impl Amount {
    pub fn new(value: u64, unit: CurrencyUnit) -> Self {
        Self { value, unit }
    }

    /// Amount from sats with Sat unit
    pub fn from_sats(value: u64) -> Self {
        Self {
            value,
            unit: CurrencyUnit::Sat,
        }
    }

    /// Split into parts that are powers of two
    pub fn split(&self) -> Vec<Self> {
        let sats = self.value;
        (0_u64..64)
            .rev()
            .filter_map(|bit| {
                let part = 1 << bit;
                ((sats & part) == part).then_some(Self::from(part))
            })
            .collect()
    }

    /// Split into parts that are powers of two by target
    pub fn split_targeted(&self, target: &SplitTarget) -> Vec<Self> {
        let mut parts = match target {
            SplitTarget::None => self.split(),
            SplitTarget::Value(amount) => {
                if self.le(&amount) {
                    return self.split();
                }

                let value = self.value;
                let mut parts_total = 0;
                let mut parts = Vec::new();

                // The powers of two that are need to create target value
                let parts_of_value = amount.split();

                while parts_total.lt(&value) {
                    for part in parts_of_value.iter() {
                        let part = part.value;
                        if (part + parts_total).le(&value) {
                            parts.push(part);
                        } else {
                            let amount_left = value - parts_total;
                            parts.extend(
                                Amount::new(amount_left, self.unit)
                                    .split()
                                    .iter()
                                    .map(|a| a.value),
                            );
                        }

                        parts_total = parts.iter().sum();

                        if parts_total.eq(&value) {
                            break;
                        }
                    }
                }

                parts
                    .into_iter()
                    .map(|p| Amount::new(p, self.unit.clone()))
                    .collect()
            }
        };

        parts.sort();
        parts
    }
}

/// Kinds of targeting that are supported
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub enum SplitTarget {
    /// Default target; least amount of proofs
    #[default]
    None,
    /// Target amount for wallet to have most proofs that add up to value
    Value(Amount),
}

impl Default for Amount {
    fn default() -> Self {
        Self {
            value: 0,
            unit: CurrencyUnit::default(),
        }
    }
}

impl From<u64> for Amount {
    fn from(value: u64) -> Self {
        Self {
            value,
            unit: CurrencyUnit::default(),
        }
    }
}

impl From<&u64> for Amount {
    fn from(value: &u64) -> Self {
        Self {
            value: *value,
            unit: CurrencyUnit::default(),
        }
    }
}

impl From<Amount> for u64 {
    fn from(value: Amount) -> Self {
        value.value
    }
}

impl std::ops::Add for Amount {
    type Output = Amount;

    fn add(self, rhs: Amount) -> Self::Output {
        assert_eq!(self.unit, rhs.unit);
        Amount::new(self.value + rhs.value, self.unit)
    }
}

impl std::ops::Sub for Amount {
    type Output = Amount;

    fn sub(self, rhs: Amount) -> Self::Output {
        assert_eq!(self.unit, rhs.unit);

        Amount::new(self.value - rhs.value, self.unit)
    }
}

impl Ord for Amount {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.value.cmp(&other.value)
    }
}

impl PartialOrd for Amount {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.value, self.unit)
    }
}

impl core::iter::Sum for Amount {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        let (value, unit) = iter.fold((0, HashSet::new()), |(mut value, mut units), amount| {
            value += amount.value;
            units.insert(amount.unit);
            (value, units)
        });

        assert_eq!(unit.len(), 1);

        Amount::new(value, *unit.iter().next().expect("No unit in amount"))
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

    #[test]
    fn test_split_target_amount() {
        let amount = Amount::new(65, CurrencyUnit::Sat);

        let split = amount.split_targeted(&SplitTarget::Value(Amount::from_sats(32)));
        assert_eq!(
            vec![
                Amount::from_sats(1),
                Amount::from_sats(32),
                Amount::from_sats(32)
            ],
            split
        );

        let amount = Amount::from_sats(150);

        let split = amount.split_targeted(&SplitTarget::Value(Amount::from_sats(50)));
        assert_eq!(
            vec![
                Amount::from_sats(2),
                Amount::from_sats(2),
                Amount::from_sats(2),
                Amount::from_sats(16),
                Amount::from_sats(16),
                Amount::from_sats(16),
                Amount::from_sats(32),
                Amount::from_sats(32),
                Amount::from_sats(32)
            ],
            split
        );

        let amount = Amount::from_sats(63);

        let split = amount.split_targeted(&SplitTarget::Value(Amount::from_sats(32)));
        assert_eq!(
            vec![
                Amount::from_sats(1),
                Amount::from_sats(2),
                Amount::from_sats(4),
                Amount::from_sats(8),
                Amount::from_sats(16),
                Amount::from_sats(32)
            ],
            split
        );
    }
}
