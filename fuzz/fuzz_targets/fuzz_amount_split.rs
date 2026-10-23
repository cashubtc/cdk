#![no_main]

//! Fuzz amount splitting and fee accounting invariants.

use std::collections::BTreeSet;

use cashu::amount::{Error, FeeAndAmounts, SplitTarget};
use cashu::Amount;
use libfuzzer_sys::arbitrary::{Arbitrary, Result as ArbitraryResult, Unstructured};
use libfuzzer_sys::fuzz_target;

/// Draws log-uniformly over the whole `u64` range, keeping small values
/// frequent while still reaching the ceiling where the checked arithmetic in
/// `split` and `split_with_fee` is the only thing standing between the greedy
/// loop and an overflow.
fn arbitrary_u64(u: &mut Unstructured<'_>) -> ArbitraryResult<u64> {
    let shift = u.int_in_range::<u32>(0..=63)?;
    Ok(u.arbitrary::<u64>()? >> shift)
}

/// The complete power-of-two ladder a real mint publishes. Only a full ladder
/// represents a near-`u64::MAX` amount within `MAX_SPLIT_PARTS`, so without it
/// every large amount stops at the part-count guard and the fee arithmetic
/// above it never runs.
fn power_of_two_ladder() -> impl Iterator<Item = u64> {
    (0..64).map(|exponent| 1u64 << exponent)
}

/// `split_targeted` re-sums its accumulator on every pass, so its amount stays
/// narrow: widening it buys a quadratic walk rather than new coverage.
fn targeted_amount(raw: u16) -> Amount {
    Amount::from(u64::from(raw % 513))
}

#[derive(Debug)]
struct Input {
    amount: u64,
    fee: u64,
    denominations: Vec<u64>,
    targeted_amount: u16,
    target: u16,
    explicit_values: Vec<u16>,
    mode: u8,
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> ArbitraryResult<Self> {
        let denomination_count = u.int_in_range(0..=8)?;
        let denominations = (0..denomination_count)
            .map(|_| arbitrary_u64(u))
            .collect::<Result<_, _>>()?;
        let value_count = u.int_in_range(0..=8)?;
        let explicit_values = (0..value_count)
            .map(|_| u.arbitrary::<u16>())
            .collect::<Result<_, _>>()?;
        Ok(Self {
            amount: arbitrary_u64(u)?,
            fee: arbitrary_u64(u)?,
            denominations,
            targeted_amount: u.arbitrary()?,
            target: u.arbitrary()?,
            explicit_values,
            mode: u.arbitrary()?,
        })
    }
}

fn assert_split_invariants(parts: &[Amount], amount: Amount, denominations: &[u64]) {
    assert_eq!(
        Amount::try_sum(parts.iter().copied()).expect("a successful split cannot overflow its sum"),
        amount,
        "split parts must sum to the requested amount"
    );
    let mut sorted = parts.to_vec();
    sorted.sort();
    assert_eq!(
        sorted.windows(2).all(|window| window[0] <= window[1]),
        true,
        "split output must be sortable independently of order"
    );
    assert!(
        parts
            .iter()
            .all(|part| denominations.contains(&part.to_u64())),
        "every split part must be an available denomination"
    );
}

/// Every way `split` and `split_with_fee` may refuse an amount. Any other
/// error means an input shape reached code it was supposed to be rejected
/// before, and a panic or an unbounded walk means the guards are gone.
fn assert_bounded_failure(err: &Error) {
    assert!(
        matches!(
            err,
            Error::AmountOverflow
                | Error::CannotSplitAmount(_, _)
                | Error::SplitTooManyParts(_, _)
                | Error::SplitWithFeeDidNotConverge(_)
        ),
        "splitting must fail in a bounded way, got {err:?}"
    );
}

/// `split_targeted` adds the two target-shape rejections to the failures
/// `split` itself may return.
fn assert_bounded_targeted_failure(err: &Error) {
    match err {
        Error::SplitValuesGreater | Error::InvalidAmount(_) => {}
        other => assert_bounded_failure(other),
    }
}

fuzz_target!(|input: Input| {
    let amount = Amount::from(input.amount);
    let mut denominations = input.denominations.iter().copied().collect::<BTreeSet<_>>();

    if input.mode % 8 == 0 {
        denominations.insert(0);
        let fee_and_amounts = FeeAndAmounts::from((input.fee, denominations.into_iter().collect()));
        assert!(amount.split(&fee_and_amounts).is_err());
        return;
    }

    denominations.retain(|&denomination| denomination > 0);
    if input.mode % 2 == 0 || denominations.is_empty() {
        denominations.extend(power_of_two_ladder());
    }
    let denominations = denominations.into_iter().collect::<Vec<_>>();
    let fee_and_amounts = FeeAndAmounts::from((input.fee, denominations.clone()));

    match amount.split(&fee_and_amounts) {
        Ok(split) => {
            assert_split_invariants(&split, amount, &denominations);
            assert_eq!(
                split,
                amount
                    .split(&fee_and_amounts)
                    .expect("repeated split must succeed"),
                "splitting must be deterministic"
            );
        }
        Err(err) => assert_bounded_failure(&err),
    }

    match amount.split_with_fee(&fee_and_amounts) {
        Ok(with_fee) => {
            let total = Amount::try_sum(with_fee.iter().copied())
                .expect("a successful split cannot overflow its sum");
            let fee_ppk = (with_fee.len() as u64)
                .checked_mul(fee_and_amounts.fee())
                .expect("a split that cleared its own fee cannot overflow it here");
            let fee = Amount::from(fee_ppk.div_ceil(1000));
            assert!(
                total.checked_sub(fee).is_some_and(|net| net >= amount),
                "fee-adjusted split must cover the requested net amount"
            );
            assert!(
                with_fee
                    .iter()
                    .all(|part| denominations.contains(&part.to_u64())),
                "fee-adjusted parts must use available denominations"
            );
        }
        Err(err) => assert_bounded_failure(&err),
    }

    let targeted = targeted_amount(input.targeted_amount);
    let target = match input.mode % 3 {
        0 => SplitTarget::None,
        1 => SplitTarget::Value(Amount::from(u64::from(input.target % 257) + 1)),
        _ => SplitTarget::Values(
            input
                .explicit_values
                .into_iter()
                .map(|value| Amount::from(denominations[usize::from(value) % denominations.len()]))
                .collect(),
        ),
    };
    match targeted.split_targeted(&target, &fee_and_amounts) {
        Ok(parts) => assert_split_invariants(&parts, targeted, &denominations),
        Err(err) => assert_bounded_targeted_failure(&err),
    }
});
