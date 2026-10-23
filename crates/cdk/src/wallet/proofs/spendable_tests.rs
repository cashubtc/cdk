use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::util::unix_time;
use cdk_common::wallet::KeysetLoadPolicy;
use cdk_common::CurrencyUnit;

use crate::nuts::{Id, KeySetInfo, ProofsMethods};
use crate::wallet::send::SendOptions;
use crate::wallet::test_utils::{
    create_test_db, create_test_wallet_with_mock, make_inactive_keyset, test_keyset,
    test_keyset_id, test_melt_quote, test_mint_url, test_proof_info, MockMintConnector,
};
use crate::{Amount, Error};

/// The mint rejects an expired keyset in inputs as well as outputs, so its
/// proofs must not reach selection either.
#[tokio::test]
async fn expired_keyset_proofs_are_held_back() {
    let mut expired = test_keyset();
    expired.final_expiry = Some(unix_time() - 1);
    let expired_id = expired.id;

    let mut survivor = make_inactive_keyset();
    survivor.active = Some(true);
    let survivor_id = survivor.id;

    let db = create_test_db().await;
    let mint_url = test_mint_url();
    db.add_mint(mint_url.clone(), None).await.unwrap();
    db.update_proofs(
        vec![
            test_proof_info(expired_id, 32, mint_url.clone()),
            test_proof_info(survivor_id, 8, mint_url.clone()),
        ],
        vec![],
    )
    .await
    .unwrap();

    let mock = Arc::new(MockMintConnector::new());
    mock.set_mint_keys_response(Ok(vec![expired, survivor]));
    let wallet = create_test_wallet_with_mock(db, mock).await;
    wallet.keysets(KeysetLoadPolicy::Refresh).await.unwrap();

    let spendable = wallet
        .retain_spendable_proofs(wallet.get_unspent_proofs().await.unwrap())
        .await
        .unwrap();

    assert_eq!(spendable.len(), 1);
    assert_eq!(spendable[0].keyset_id, survivor_id);
    assert_eq!(
        wallet.total_balance().await.unwrap(),
        Amount::from(40),
        "the balance keeps the held-back proofs: they are still a record of what the wallet holds"
    );
}

/// An expired keyset the mint still reports as active would be chosen for
/// outputs on fee alone, and every operation built on it answered with 12003.
#[tokio::test]
async fn expired_keyset_is_not_chosen_for_outputs() {
    let mut expired = test_keyset();
    expired.final_expiry = Some(unix_time() - 1);
    expired.input_fee_ppk = 0;

    let mut survivor = make_inactive_keyset();
    survivor.active = Some(true);
    survivor.input_fee_ppk = 100;
    survivor.id = Id::v2_from_data(
        &survivor.keys,
        &survivor.unit,
        survivor.input_fee_ppk,
        survivor.final_expiry,
    );
    let survivor_id = survivor.id;

    let db = create_test_db().await;
    let mint_url = test_mint_url();
    db.add_mint(mint_url.clone(), None).await.unwrap();

    let mock = Arc::new(MockMintConnector::new());
    mock.set_mint_keys_response(Ok(vec![expired, survivor]));
    let wallet = create_test_wallet_with_mock(db, mock).await;
    wallet.keysets(KeysetLoadPolicy::Refresh).await.unwrap();

    assert_eq!(wallet.active_keyset().await.unwrap().id, survivor_id);
}

/// An inactive keyset is one the mint rotated out, not one it refuses. Its
/// proofs are still valid inputs and are deliberately spent down first.
#[tokio::test]
async fn inactive_keyset_proofs_stay_spendable() {
    let active = test_keyset();
    let mut inactive = make_inactive_keyset();
    inactive.active = Some(false);
    let inactive_id = inactive.id;

    let db = create_test_db().await;
    let mint_url = test_mint_url();
    db.add_mint(mint_url.clone(), None).await.unwrap();
    db.update_proofs(
        vec![test_proof_info(inactive_id, 16, mint_url.clone())],
        vec![],
    )
    .await
    .unwrap();

    let mock = Arc::new(MockMintConnector::new());
    mock.set_mint_keys_response(Ok(vec![active, inactive]));
    let wallet = create_test_wallet_with_mock(db, mock).await;
    wallet.keysets(KeysetLoadPolicy::Refresh).await.unwrap();

    let spendable = wallet
        .retain_spendable_proofs(wallet.get_unspent_proofs().await.unwrap())
        .await
        .unwrap();

    assert_eq!(spendable.total_amount().unwrap(), Amount::from(16));
}

#[tokio::test]
async fn nothing_is_held_back_without_an_expiry() {
    let active = test_keyset();
    let active_id = active.id;

    let db = create_test_db().await;
    let mint_url = test_mint_url();
    db.add_mint(mint_url.clone(), None).await.unwrap();
    db.update_proofs(
        vec![test_proof_info(active_id, 4, mint_url.clone())],
        vec![],
    )
    .await
    .unwrap();

    let mock = Arc::new(MockMintConnector::new());
    mock.set_mint_keys_response(Ok(vec![active]));
    let wallet = create_test_wallet_with_mock(db, mock).await;
    wallet.keysets(KeysetLoadPolicy::Refresh).await.unwrap();

    let proofs = wallet.get_unspent_proofs().await.unwrap();
    let spendable = wallet
        .retain_spendable_proofs(proofs.clone())
        .await
        .unwrap();

    assert_eq!(spendable, proofs);
}

/// The filter is only useful if the operations actually route their proof pool
/// through it, so drive a real send rather than calling the filter directly.
/// Both halves share one setup and differ only in whether the keyset holding
/// the proofs has expired.
async fn prepare_send_over_keyset(expired: bool) -> Result<(), Error> {
    let mut spendable = test_keyset();
    spendable.input_fee_ppk = 0;

    let mut holder = make_inactive_keyset();
    holder.active = Some(true);
    holder.final_expiry = expired.then(|| unix_time() - 1);
    holder.id = Id::v2_from_data(
        &holder.keys,
        &holder.unit,
        holder.input_fee_ppk,
        holder.final_expiry,
    );
    let holder_id = holder.id;

    let db = create_test_db().await;
    let mint_url = test_mint_url();
    db.add_mint(mint_url.clone(), None).await.unwrap();
    db.update_proofs(
        vec![test_proof_info(holder_id, 100, mint_url.clone())],
        vec![],
    )
    .await
    .unwrap();

    let mock = Arc::new(MockMintConnector::new());
    mock.set_mint_keys_response(Ok(vec![spendable, holder]));
    let wallet = create_test_wallet_with_mock(db, mock).await;
    wallet.keysets(KeysetLoadPolicy::Refresh).await.unwrap();

    wallet
        .prepare_send(Amount::from(10), SendOptions::default())
        .await
        .map(|_| ())
}

#[tokio::test]
async fn send_fails_locally_when_only_expired_proofs_remain() {
    let error = prepare_send_over_keyset(true)
        .await
        .expect_err("a send built on an expired keyset must not reach the mint");

    assert!(
        matches!(error, Error::InsufficientFunds),
        "expected InsufficientFunds, got {error:?}"
    );
}

/// The control for the case above: the same wallet, the same amount, and the
/// same single proof, differing only in the keyset's expiry.
#[tokio::test]
async fn send_succeeds_when_the_keyset_has_not_expired() {
    prepare_send_over_keyset(false)
        .await
        .expect("an unexpired keyset's proofs are still spendable");
}

/// A cold in-memory cache has to fall back to the stored keysets, or a wallet
/// that has not talked to the mint yet would happily spend expired proofs.
#[tokio::test]
async fn expired_keyset_is_read_from_the_database_when_the_cache_is_cold() {
    let expired_id = test_keyset_id();

    let db = create_test_db().await;
    let mint_url = test_mint_url();
    db.add_mint(mint_url.clone(), None).await.unwrap();
    db.add_mint_keysets(
        mint_url.clone(),
        vec![KeySetInfo {
            id: expired_id,
            unit: CurrencyUnit::Sat,
            active: true,
            input_fee_ppk: 0,
            final_expiry: Some(unix_time() - 1),
        }],
    )
    .await
    .unwrap();
    db.update_proofs(
        vec![test_proof_info(expired_id, 32, mint_url.clone())],
        vec![],
    )
    .await
    .unwrap();

    let wallet = create_test_wallet_with_mock(db, Arc::new(MockMintConnector::new())).await;
    assert!(
        wallet.metadata_cache.get_cached().is_none(),
        "the cache must be cold or this test proves nothing about the database path"
    );

    let spendable = wallet
        .retain_spendable_proofs(wallet.get_unspent_proofs().await.unwrap())
        .await
        .unwrap();

    assert!(spendable.is_empty());
}

/// An expired keyset the wallet holds no proofs on must not disturb the proofs
/// it does hold. This goes through the partition rather than the early return.
#[tokio::test]
async fn an_expired_keyset_with_no_proofs_drops_nothing() {
    let mut expired = make_inactive_keyset();
    expired.active = Some(true);
    expired.final_expiry = Some(unix_time() - 1);
    expired.id = Id::v2_from_data(
        &expired.keys,
        &expired.unit,
        expired.input_fee_ppk,
        expired.final_expiry,
    );

    let holder = test_keyset();
    let holder_id = holder.id;

    let db = create_test_db().await;
    let mint_url = test_mint_url();
    db.add_mint(mint_url.clone(), None).await.unwrap();
    db.update_proofs(
        vec![test_proof_info(holder_id, 4, mint_url.clone())],
        vec![],
    )
    .await
    .unwrap();

    let mock = Arc::new(MockMintConnector::new());
    mock.set_mint_keys_response(Ok(vec![holder, expired]));
    let wallet = create_test_wallet_with_mock(db, mock).await;
    wallet.keysets(KeysetLoadPolicy::Refresh).await.unwrap();

    let proofs = wallet.get_unspent_proofs().await.unwrap();
    let spendable = wallet
        .retain_spendable_proofs(proofs.clone())
        .await
        .unwrap();

    assert_eq!(spendable, proofs);
}

/// Melt is the other path that gathers its own proof pool, so it needs the
/// same proof that the filter is actually wired in rather than merely present.
async fn prepare_melt_over_keyset(expired: bool) -> Result<(), Error> {
    let mut spendable = test_keyset();
    spendable.input_fee_ppk = 0;

    let mut holder = make_inactive_keyset();
    holder.active = Some(true);
    holder.input_fee_ppk = 0;
    holder.final_expiry = expired.then(|| unix_time() - 1);
    holder.id = Id::v2_from_data(
        &holder.keys,
        &holder.unit,
        holder.input_fee_ppk,
        holder.final_expiry,
    );
    let holder_id = holder.id;

    let db = create_test_db().await;
    let mint_url = test_mint_url();
    db.add_mint(mint_url.clone(), None).await.unwrap();

    let mut quote = test_melt_quote();
    quote.amount = Amount::from(8);
    quote.fee_reserve = Amount::ZERO;
    let quote_id = quote.id.clone();
    db.add_melt_quote(quote).await.unwrap();

    db.update_proofs(
        vec![test_proof_info(holder_id, 100, mint_url.clone())],
        vec![],
    )
    .await
    .unwrap();

    let mock = Arc::new(MockMintConnector::new());
    mock.set_mint_keys_response(Ok(vec![spendable, holder]));
    let wallet = create_test_wallet_with_mock(db, mock).await;
    wallet.keysets(KeysetLoadPolicy::Refresh).await.unwrap();

    wallet
        .prepare_melt(&quote_id, HashMap::new())
        .await
        .map(|_| ())
}

#[tokio::test]
async fn melt_fails_locally_when_only_expired_proofs_remain() {
    let error = prepare_melt_over_keyset(true)
        .await
        .expect_err("a melt built on an expired keyset must not reach the mint");

    assert!(
        matches!(error, Error::InsufficientFunds),
        "expected InsufficientFunds, got {error:?}"
    );
}

/// The control for the case above.
#[tokio::test]
async fn melt_succeeds_when_the_keyset_has_not_expired() {
    prepare_melt_over_keyset(false)
        .await
        .expect("an unexpired keyset's proofs are still spendable");
}
