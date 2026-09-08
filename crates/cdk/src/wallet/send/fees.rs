//! Fee-inclusive output splitting without recombining the prepared denominations.

use std::collections::HashSet;

use cdk_common::amount::FeeAndAmounts;

use crate::{Amount, Error};

pub(super) fn split_exact_with_fee(
    amount: Amount,
    fees: &FeeAndAmounts,
    budget: Amount,
    max_proofs: Option<usize>,
) -> Result<(Vec<Amount>, Amount), Error> {
    if amount > budget {
        return Err(Error::InsufficientFunds);
    }
    if amount == Amount::ZERO || fees.fee() == 0 {
        let split = amount.split(fees)?;
        return match max_proofs.is_none_or(|max| split.len() <= max) {
            true => Ok((split, Amount::ZERO)),
            false => Err(Error::InsufficientFunds),
        };
    }
    let mut denominations = fees.amounts().to_vec();
    denominations.sort_unstable();
    denominations.dedup();
    denominations.retain(|value| *value <= budget.to_u64());
    let largest = *denominations.last().ok_or(Error::InsufficientFunds)?;
    if denominations.contains(&0) {
        return Err(Error::InsufficientFunds);
    }
    let max_net_ppk = largest
        .checked_mul(1000)
        .ok_or(Error::AmountOverflow)?
        .checked_sub(fees.fee())
        .filter(|value| *value > 0)
        .ok_or(Error::InsufficientFunds)?;
    let first = amount
        .to_u64()
        .checked_mul(1000)
        .ok_or(Error::AmountOverflow)?
        .div_ceil(max_net_ppk);

    // Try a bounded number of proof counts. Unlike an exhaustive denomination
    // search, this also terminates promptly for unsupported sparse keysets.
    let last = first
        .saturating_add(u64::BITS as u64 - 1)
        .min(max_proofs.unwrap_or(usize::MAX) as u64);
    for count in first..=last {
        let fee = Amount::from(
            count
                .checked_mul(fees.fee())
                .ok_or(Error::AmountOverflow)?
                .div_ceil(1000),
        );
        let gross = amount.checked_add(fee).ok_or(Error::AmountOverflow)?;
        if gross > budget {
            return Err(Error::InsufficientFunds);
        }

        // Count the ordinary split before allocating any proofs.
        let mut remaining = gross.to_u64();
        let mut copies = vec![0; denominations.len()];
        for index in (0..denominations.len()).rev() {
            copies[index] = remaining / denominations[index];
            remaining %= denominations[index];
        }
        let size: u64 = copies.iter().sum();
        if remaining != 0 || size > count {
            continue;
        }

        // The minimum split has at least `first` proofs, so this search adds
        // at most 63 proofs regardless of the funding balance.
        if split_to_count(
            &denominations,
            &mut copies,
            denominations.len() - 1,
            count - size,
            &mut HashSet::new(),
        ) {
            let split = denominations
                .iter()
                .zip(copies)
                .flat_map(|(value, copies)| {
                    std::iter::repeat_n(Amount::from(*value), copies as usize)
                })
                .collect();
            return Ok((split, fee));
        }
    }
    Err(Error::Custom(
        "Cannot construct an exact fee-inclusive split with these denominations and proof limit"
            .to_owned(),
    ))
}

/// Try each split count from large denominations down, retaining smaller
/// proofs when splitting them would prevent an exact output count.
fn split_to_count(
    denominations: &[u64],
    copies: &mut [u64],
    index: usize,
    extra: u64,
    failed: &mut HashSet<(usize, u64, u64)>,
) -> bool {
    if extra == 0 {
        return true;
    }
    if index == 0 {
        return false;
    }
    // Lower denominations still contain their original counts at this point;
    // only the current count depends on splits made at higher denominations.
    let state = (index, extra, copies[index]);
    if failed.contains(&state) {
        return false;
    }
    let current = copies[index];
    let lower = copies[index - 1];
    let ratio = denominations[index] / denominations[index - 1];
    let max_splits = if denominations[index] % denominations[index - 1] == 0 {
        current.min(extra / (ratio - 1))
    } else {
        0
    };
    for splits in 0..=max_splits {
        copies[index] = current - splits;
        copies[index - 1] = lower + splits * ratio;
        if split_to_count(
            denominations,
            copies,
            index - 1,
            extra - splits * (ratio - 1),
            failed,
        ) {
            return true;
        }
    }
    copies[index] = current;
    copies[index - 1] = lower;
    failed.insert(state);
    false
}

#[cfg(test)]
mod tests {
    use super::split_exact_with_fee;
    use crate::{Amount, Error};

    #[test]
    fn overflowing_fee_calculations_return_an_error() {
        for (amount, denominations, fee_ppk) in [
            (21, vec![1, 1u64 << 63], 1000),
            (u64::MAX / 1000 + 1, vec![1, 1 << 32], 1000),
            (21, vec![1, 1 << 40], (1u64 << 40) * 1000 - 1),
        ] {
            let fees = (fee_ppk, denominations).into();
            assert!(matches!(
                split_exact_with_fee(amount.into(), &fees, u64::MAX.into(), None),
                Err(Error::AmountOverflow)
            ));
        }
    }

    #[test]
    fn exact_net_amount_across_fee_rounding_boundaries() {
        for fee_ppk in [0, 1, 100, 500, 999, 1000, 1500, 2000, 4000] {
            let fees = (fee_ppk, (0..16).map(|i| 1 << i).collect()).into();
            for amount in 1..=128 {
                let (split, fee) =
                    split_exact_with_fee(Amount::from(amount), &fees, Amount::from(1024), Some(32))
                        .unwrap();
                assert_eq!(fee.to_u64(), (split.len() as u64 * fee_ppk).div_ceil(1000));
                assert_eq!(Amount::try_sum(split).unwrap() - fee, Amount::from(amount));
            }
        }
    }

    #[test]
    fn exact_net_with_few_and_many_proofs() {
        // With outputs capped at 1024, these amounts require the stated count.
        // Counts above 64 also exercise the distinction between the candidate
        // search bound and the number of proofs in a valid result.
        for fee_ppk in [0, 1, 100, 500, 999, 1000, 1500, 2000, 4000] {
            let fees = (fee_ppk, (0..=10).map(|i| 1 << i).collect()).into();
            for count in [1usize, 2, 31, 32, 63, 64, 65, 128] {
                let gross = count as u64 * 1024;
                let expected_fee = (count as u64 * fee_ppk).div_ceil(1000);
                let amount = gross - expected_fee;
                let (split, fee) =
                    split_exact_with_fee(amount.into(), &fees, gross.into(), Some(count))
                        .unwrap_or_else(|error| {
                            panic!("count={count}, fee_ppk={fee_ppk}: {error}")
                        });
                assert_eq!(split, vec![Amount::from(1024); count]);
                assert_eq!(fee, Amount::from(expected_fee));
                assert_eq!(Amount::try_sum(split).unwrap() - fee, Amount::from(amount));
                assert!(
                    split_exact_with_fee(amount.into(), &fees, gross.into(), Some(count - 1))
                        .is_err()
                );
            }
        }
    }

    #[test]
    fn large_amount_can_need_only_one_proof() {
        let gross = 1u64 << 40;
        for fee_ppk in [0u64, 1, 100, 500, 999, 1000, 1500, 2000, 4000] {
            let fees = (fee_ppk, (0..=40).map(|i| 1 << i).collect()).into();
            let expected_fee = fee_ppk.div_ceil(1000);
            let amount = gross - expected_fee;
            let (split, fee) =
                split_exact_with_fee(amount.into(), &fees, gross.into(), Some(1)).unwrap();
            assert_eq!(split, vec![Amount::from(gross)]);
            assert_eq!(fee, Amount::from(expected_fee));
            assert_eq!(Amount::try_sum(split).unwrap() - fee, Amount::from(amount));
        }
    }

    #[test]
    fn twenty_one_sats_preserves_the_fee_and_funding_limits() {
        let fees = (1000, (0..8).map(|i| 1 << i).collect()).into();
        let (split, fee) = split_exact_with_fee(21.into(), &fees, 24.into(), Some(3)).unwrap();
        assert_eq!(split, vec![4.into(), 4.into(), 16.into()]);
        assert_eq!(fee, Amount::from(3));
        assert!(split_exact_with_fee(21.into(), &fees, 23.into(), None).is_err());
        assert!(split_exact_with_fee(21.into(), &fees, 100.into(), Some(2)).is_err());
    }

    #[test]
    fn supported_sparse_split_is_exact() {
        let fees = (1000, vec![2, 8, 32]).into();
        let (split, fee) = split_exact_with_fee(21.into(), &fees, 100.into(), None).unwrap();
        assert_eq!(split, vec![8.into(); 3]);
        assert_eq!(Amount::try_sum(split).unwrap() - fee, Amount::from(21));
    }

    #[test]
    fn sparse_split_reconsiders_smaller_denominations() {
        let fees = (500, vec![1, 4, 8]).into();
        let (split, fee) = split_exact_with_fee(13.into(), &fees, 32.into(), Some(6)).unwrap();

        assert_eq!(split.len(), 6);
        assert!(split
            .iter()
            .all(|value| fees.amounts().contains(&value.to_u64())));
        assert_eq!(fee, Amount::from(3));
        assert_eq!(Amount::try_sum(split).unwrap() - fee, Amount::from(13));
    }

    #[test]
    fn sparse_split_preserves_smaller_proofs_when_splitting_a_larger_one() {
        let fees = (1000, vec![1, 2, 8]).into();
        for budget in [10u64, 16, 64, 1000] {
            let (split, fee) =
                split_exact_with_fee(5.into(), &fees, budget.into(), Some(5)).unwrap();
            assert_eq!(split, vec![Amount::from(2); 5]);
            assert_eq!(fee, Amount::from(5));
            assert_eq!(Amount::try_sum(split).unwrap() - fee, Amount::from(5));
        }
    }

    #[test]
    fn sparse_splits_match_feasible_small_tokens() {
        // Enumerate actual tokens independently of the splitting algorithm,
        // including every subset of the five smallest binary denominations.
        for mask in 1..32 {
            let denominations: Vec<u64> = (0..5)
                .filter(|bit| mask & (1 << bit) != 0)
                .map(|bit| 1 << bit)
                .collect();
            let mut totals = std::collections::BTreeSet::from([0u64]);
            for count in 1..=8usize {
                totals = totals
                    .iter()
                    .flat_map(|total| denominations.iter().map(move |value| total + value))
                    .collect();
                for fee_ppk in [100u64, 500, 1000, 1500] {
                    let fees = (fee_ppk, denominations.clone()).into();
                    let redemption_fee = (count as u64 * fee_ppk).div_ceil(1000);
                    for &gross in &totals {
                        if gross <= redemption_fee {
                            continue;
                        }
                        let net = gross - redemption_fee;
                        let (split, fee) = split_exact_with_fee(
                            net.into(),
                            &fees,
                            gross.into(),
                            Some(count),
                        )
                        .unwrap_or_else(|error| {
                            panic!("denominations={denominations:?}, net={net}, count={count}, fee_ppk={fee_ppk}: {error}")
                        });
                        assert!(split.len() <= count);
                        assert!(split
                            .iter()
                            .all(|value| denominations.contains(&value.to_u64())));
                        assert_eq!(fee.to_u64(), (split.len() as u64 * fee_ppk).div_ceil(1000));
                        let total = Amount::try_sum(split).unwrap();
                        assert!(total <= Amount::from(gross));
                        assert_eq!(total - fee, Amount::from(net));
                    }
                }
            }
        }
    }

    #[test]
    fn unpayable_sparse_keyset_does_not_search_the_funding_balance() {
        let budget = 1u64 << 32;
        let fees = (1001, vec![1, budget]).into();
        assert!(split_exact_with_fee(21.into(), &fees, budget.into(), None).is_err());
    }
}
