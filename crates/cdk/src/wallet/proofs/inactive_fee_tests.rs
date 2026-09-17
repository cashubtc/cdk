use std::collections::HashMap;

use cdk_common::amount::KeysetFeeAndAmounts;

use crate::fees::calculate_fee;
use crate::nuts::{Id, Proofs, ProofsMethods};
use crate::wallet::test_utils::{make_inactive_keyset, test_keyset_id, test_proof};
use crate::{Amount, Error, Wallet};

fn fees(inactive: Id) -> KeysetFeeAndAmounts {
    [inactive, test_keyset_id()]
        .into_iter()
        .map(|id| (id, (500, vec![1, 2, 4, 8, 16]).into()))
        .collect()
}

fn net(proofs: &Proofs, fees: &KeysetFeeAndAmounts) -> Amount {
    let rates = fees.iter().map(|(id, fee)| (*id, fee.fee())).collect();
    proofs.total_amount().unwrap()
        - calculate_fee(&proofs.count_by_keyset(), &rates)
            .unwrap()
            .total
}

#[test]
fn inactive_selection_covers_fees() {
    let inactive = make_inactive_keyset().id;
    let fees = fees(inactive);
    let proofs = vec![test_proof(inactive, 8), test_proof(inactive, 4)];
    let selected =
        Wallet::select_proofs(8.into(), proofs, &vec![test_keyset_id()], &fees, true).unwrap();
    assert_eq!(selected.len(), 2);
    assert!(net(&selected, &fees) >= Amount::from(8));
}

#[test]
fn inactive_selection_rejects_insufficient_net_amount() {
    let inactive = make_inactive_keyset().id;
    let result = Wallet::select_proofs(
        8.into(),
        vec![test_proof(inactive, 8)],
        &vec![test_keyset_id()],
        &fees(inactive),
        true,
    );
    assert!(matches!(result, Err(Error::InsufficientFunds)));
}

#[test]
fn inactive_selection_can_cover_shortfall_with_active_proof() {
    let inactive = make_inactive_keyset().id;
    let fees = fees(inactive);
    let selected = Wallet::select_proofs(
        8.into(),
        vec![test_proof(inactive, 8), test_proof(test_keyset_id(), 1)],
        &vec![test_keyset_id()],
        &fees,
        true,
    )
    .unwrap();
    assert_eq!(net(&selected, &fees), Amount::from(8));
}

#[test]
fn inactive_selection_still_prefers_older_equivalent_proofs() {
    let inactive = make_inactive_keyset().id;
    let older = test_proof(inactive, 8);
    let newer = test_proof(inactive, 8);
    let extra = test_proof(inactive, 1);
    let indices = HashMap::from([(older.clone(), 1), (newer.clone(), 2), (extra.clone(), 3)]);
    let selected = Wallet::select_proofs_with_derivation_indices(
        8.into(),
        vec![newer.clone(), older.clone(), extra.clone()],
        &vec![test_keyset_id()],
        &fees(inactive),
        true,
        &indices,
    )
    .unwrap();
    assert!(selected.contains(&older));
    assert!(selected.contains(&extra));
    assert!(!selected.contains(&newer));
}

#[test]
fn inactive_selection_without_fees_keeps_nominal_selection() {
    let inactive = make_inactive_keyset().id;
    let selected = Wallet::select_proofs(
        8.into(),
        vec![test_proof(inactive, 8), test_proof(inactive, 4)],
        &vec![test_keyset_id()],
        &fees(inactive),
        false,
    )
    .unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected.total_amount().unwrap(), Amount::from(8));
}
