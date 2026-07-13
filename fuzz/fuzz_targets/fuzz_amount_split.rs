#![no_main]

//! Fuzz amount splitting and fee accounting invariants.

use std::collections::BTreeSet;

use cashu::amount::{FeeAndAmounts, SplitTarget};
use cashu::Amount;
use libfuzzer_sys::arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;

#[derive(Debug)]
struct Input {
    amount: u16,
    fee: u16,
    denominations: Vec<u16>,
    target: u16,
    explicit_values: Vec<u16>,
    mode: u8,
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> libfuzzer_sys::arbitrary::Result<Self> {
        let denomination_count = u.int_in_range(0..=8)?;
        let denominations = (0..denomination_count)
            .map(|_| u.arbitrary::<u16>())
            .collect::<Result<_, _>>()?;
        let value_count = u.int_in_range(0..=8)?;
        let explicit_values = (0..value_count)
            .map(|_| u.arbitrary::<u16>())
            .collect::<Result<_, _>>()?;
        Ok(Self {
            amount: u.arbitrary()?,
            fee: u.arbitrary()?,
            denominations,
            target: u.arbitrary()?,
            explicit_values,
            mode: u.arbitrary()?,
        })
    }
}

fn assert_split_invariants(parts: &[Amount], amount: Amount, denominations: &[u64]) {
    assert_eq!(
        Amount::try_sum(parts.iter().copied()).expect("bounded split cannot overflow"),
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

fuzz_target!(|input: Input| {
    let amount = Amount::from(u64::from(input.amount % 513));
    let mut denominations = input
        .denominations
        .into_iter()
        .map(|value| u64::from(value % 257))
        .collect::<BTreeSet<_>>();

    // Exercise the malformed public input explicitly. It must return an error
    // instead of reaching the division in the greedy splitting loop.
    if input.mode % 8 == 0 {
        denominations.insert(0);
        let fee_and_amounts = FeeAndAmounts::from((u64::from(input.fee),
            denominations.iter().copied().collect()));
        assert!(amount.split(&fee_and_amounts).is_err());
        return;
    }

    // Including denomination one makes every bounded amount representable.
    // Filter out zero to avoid the InvalidAmount error.
    denominations.insert(1);
    denominations.retain(|&d| d > 0);
    let denominations = denominations.into_iter().collect::<Vec<_>>();
    // Bound fee to keep split_with_fee's recursion converging.  With fee
    // ppk >= 1000/len(denominations) the recursion diverges (stack overflow).
    let fee_and_amounts = FeeAndAmounts::from((u64::from(input.fee % 11),
        denominations.clone()));

    let split = amount
        .split(&fee_and_amounts)
        .expect("denomination one makes every bounded amount representable");
    assert_split_invariants(&split, amount, &denominations);
    assert_eq!(
        split,
        amount
            .split(&fee_and_amounts)
            .expect("repeated split must succeed"),
        "splitting must be deterministic"
    );

    let target = match input.mode % 3 {
        0 => SplitTarget::None,
        1 => SplitTarget::Value(Amount::from(u64::from(input.target % 257) + 1)),
        _ => SplitTarget::Values(
            input
                .explicit_values
                .into_iter()
                .map(|value| {
                    Amount::from(denominations[usize::from(value) % denominations.len()])
                })
                .collect(),
        ),
    };
    if let Ok(targeted) = amount.split_targeted(&target, &fee_and_amounts) {
        assert_split_invariants(&targeted, amount, &denominations);
    }

    if let Ok(with_fee) = amount.split_with_fee(&fee_and_amounts) {
        let total = Amount::try_sum(with_fee.iter().copied())
            .expect("bounded fee-adjusted split cannot overflow");
        let fee = Amount::from(
            (with_fee.len() as u64 * fee_and_amounts.fee()).div_ceil(1000),
        );
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
});
