//! Backend contract tests for wallet preparation transactions.

use super::*;
use crate::database::wallet::{CounterReservation, OperationPreparation, QuoteReservation};
use crate::wallet::{IssueSagaState, MeltOperationData, MeltSagaState, MintOperationData};

fn melt_preparation(quote: &MeltQuote, ys: Vec<PublicKey>) -> OperationPreparation {
    OperationPreparation {
        saga: WalletSaga::new(
            uuid::Uuid::new_v4(),
            WalletSagaState::Melt(MeltSagaState::ProofsReserved),
            quote.amount,
            test_mint_url(),
            quote.unit.clone(),
            OperationData::Melt(MeltOperationData {
                quote_id: quote.id.clone(),
                amount: quote.amount,
                fee_reserve: quote.fee_reserve,
                counter_start: None,
                counter_end: None,
                change_amount: None,
                metadata: HashMap::new(),
                final_proof_ys: None,
                change_blinded_messages: None,
            }),
        ),
        proof_ys: ys,
        proofs: vec![],
        quotes: vec![QuoteReservation {
            quote_id: quote.id.clone(),
            version: quote.version,
        }],
        counter: None,
    }
}

async fn assert_available<DB>(db: &DB, proof: &ProofInfo, quote: &MeltQuote)
where
    DB: Database<Error>,
{
    assert_eq!(
        db.get_proofs_by_ys(vec![proof.y]).await.unwrap(),
        vec![proof.clone()]
    );
    assert_eq!(
        db.get_balance(
            Some(proof.mint_url.clone()),
            Some(proof.unit.clone()),
            Some(vec![State::Unspent])
        )
        .await
        .unwrap(),
        proof.proof.amount.to_u64()
    );
    assert_eq!(
        db.get_melt_quote(&quote.id).await.unwrap(),
        Some(quote.clone())
    );
    assert!(db
        .list_transactions(None, None, None)
        .await
        .unwrap()
        .is_empty());
}

/// A successful preparation owns both the quote and inputs and cannot be replayed.
pub async fn preparation_commits_owned_resources<DB>(db: DB)
where
    DB: Database<Error>,
{
    for method in [
        KnownMethod::Bolt11,
        KnownMethod::Bolt12,
        KnownMethod::Onchain,
    ] {
        let mut quote = test_melt_quote();
        quote.payment_method = method.into();
        db.add_melt_quote(quote.clone()).await.unwrap();
        let mut proof = test_proof_info(test_keyset_id(), 2048, test_mint_url());
        proof.derivation_index = Some(17);
        db.update_proofs(vec![proof.clone()], vec![]).await.unwrap();
        let mut request = melt_preparation(&quote, vec![]);
        request.proofs = vec![proof.clone()];
        db.prepare_operation(request.clone()).await.unwrap();
        assert_eq!(
            db.get_saga(&request.saga.id).await.unwrap(),
            Some(request.saga.clone())
        );
        let reserved = db.get_reserved_proofs(&request.saga.id).await.unwrap();
        assert_eq!(reserved.len(), 1);
        assert_eq!(reserved[0].state, State::Reserved);
        assert_eq!(reserved[0].derivation_index, Some(17));
        let acquired = db.get_melt_quote(&quote.id).await.unwrap().unwrap();
        assert_eq!(
            acquired.used_by_operation,
            Some(request.saga.id.to_string())
        );
        assert_eq!(acquired.version, quote.version + 1);
        assert!(db.prepare_operation(request.clone()).await.is_err());
        db.release_proofs(&request.saga.id).await.unwrap();
        db.release_melt_quote(&request.saga.id).await.unwrap();
        db.delete_saga(&request.saga.id).await.unwrap();
        assert_eq!(
            db.get_proofs_by_ys(vec![proof.y]).await.unwrap()[0].state,
            State::Unspent
        );
        // Keep this helper's balance assertions independent between methods.
        db.update_proofs(vec![], vec![proof.y]).await.unwrap();
    }
}

/// An insertion failure after resource updates rolls everything back.
pub async fn preparation_rolls_back_on_saga_conflict<DB>(db: DB)
where
    DB: Database<Error>,
{
    let quote = test_melt_quote();
    let proof = test_proof_info(test_keyset_id(), 2048, test_mint_url());
    db.add_melt_quote(quote.clone()).await.unwrap();
    db.update_proofs(vec![proof.clone()], vec![]).await.unwrap();
    let request = melt_preparation(&quote, vec![proof.y]);
    db.add_saga(request.saga.clone()).await.unwrap();
    assert!(db.prepare_operation(request.clone()).await.is_err());
    assert_available(&db, &proof, &quote).await;
    assert_eq!(
        db.get_saga(&request.saga.id).await.unwrap(),
        Some(request.saga)
    );
}

/// Missing/owned inputs and a stale quote cannot leave partial reservations.
pub async fn preparation_rolls_back_on_input_conflict<DB>(db: DB)
where
    DB: Database<Error>,
{
    let quote = test_melt_quote();
    let proof = test_proof_info(test_keyset_id(), 2048, test_mint_url());
    db.add_melt_quote(quote.clone()).await.unwrap();
    db.update_proofs(vec![proof.clone()], vec![]).await.unwrap();
    let missing = SecretKey::generate().public_key();
    let request = melt_preparation(&quote, vec![proof.y, missing]);
    assert!(db.prepare_operation(request.clone()).await.is_err());
    assert_available(&db, &proof, &quote).await;
    assert!(db.get_saga(&request.saga.id).await.unwrap().is_none());
    let owner = uuid::Uuid::new_v4();
    db.reserve_melt_quote(&quote.id, &owner).await.unwrap();
    let request = melt_preparation(&quote, vec![proof.y]);
    assert!(db.prepare_operation(request.clone()).await.is_err());
    assert_eq!(
        db.get_proofs_by_ys(vec![proof.y]).await.unwrap(),
        vec![proof.clone()]
    );
    assert_eq!(
        db.get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap()
            .used_by_operation,
        Some(owner.to_string())
    );
    db.release_melt_quote(&owner).await.unwrap();
    db.reserve_proofs(vec![proof.y], &owner).await.unwrap();
    let mut explicit = melt_preparation(&quote, vec![]);
    explicit.proofs = vec![proof.clone()];
    assert!(db.prepare_operation(explicit).await.is_err());
    assert_eq!(db.get_reserved_proofs(&owner).await.unwrap().len(), 1);
    assert_eq!(
        db.get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap()
            .used_by_operation,
        None
    );
}

/// Only one competing preparation may own a quote or a proof.
pub async fn preparation_competes_for_quote_and_inputs<DB>(db: DB)
where
    DB: Database<Error> + Sync,
{
    for shared_quote in [false, true] {
        let quote = test_melt_quote();
        let other_quote = test_melt_quote();
        db.add_melt_quote(quote.clone()).await.unwrap();
        db.add_melt_quote(other_quote.clone()).await.unwrap();
        let proof = test_proof_info(test_keyset_id(), 2048, test_mint_url());
        db.update_proofs(vec![proof.clone()], vec![]).await.unwrap();
        let a = melt_preparation(&quote, vec![proof.y]);
        let b = melt_preparation(
            if shared_quote { &quote } else { &other_quote },
            vec![proof.y],
        );
        let (ra, rb) = tokio::join!(
            db.prepare_operation(a.clone()),
            db.prepare_operation(b.clone())
        );
        assert_ne!(ra.is_ok(), rb.is_ok());
        let (winner, loser) = if ra.is_ok() { (a, b) } else { (b, a) };
        assert_eq!(
            db.get_reserved_proofs(&winner.saga.id).await.unwrap().len(),
            1
        );
        assert!(db
            .get_reserved_proofs(&loser.saga.id)
            .await
            .unwrap()
            .is_empty());
        assert!(db.get_saga(&loser.saga.id).await.unwrap().is_none());
        db.release_proofs(&winner.saga.id).await.unwrap();
    }
}

/// Batch quote ownership and output counters are all-or-nothing.
pub async fn preparation_mint_counters_and_batch_quotes<DB>(db: DB)
where
    DB: Database<Error>,
{
    use crate::BlindedMessage;
    let quotes = vec![
        test_mint_quote(test_mint_url()),
        test_mint_quote(test_mint_url()),
    ];
    for quote in &quotes {
        db.add_mint_quote(quote.clone()).await.unwrap();
    }
    let outputs = vec![BlindedMessage::new(
        Amount::from(1),
        test_keyset_id(),
        SecretKey::generate().public_key(),
    )];
    let saga = WalletSaga::new(
        uuid::Uuid::new_v4(),
        WalletSagaState::Issue(IssueSagaState::SecretsPrepared),
        Amount::from(1),
        test_mint_url(),
        CurrencyUnit::Sat,
        OperationData::Mint(MintOperationData::new_batch(
            quotes.iter().map(|q| q.id.clone()).collect(),
            Amount::from(1),
            Some(0),
            Some(1),
            Some(outputs),
        )),
    );
    let mut request = OperationPreparation {
        saga,
        proof_ys: vec![],
        proofs: vec![],
        quotes: quotes
            .iter()
            .map(|quote| QuoteReservation {
                quote_id: quote.id.clone(),
                version: quote.version,
            })
            .collect(),
        counter: Some(CounterReservation {
            keyset_id: test_keyset_id(),
            start: 0,
            end: 1,
        }),
    };
    db.increment_keyset_counter(&test_keyset_id(), 2)
        .await
        .unwrap();
    assert!(db.prepare_operation(request.clone()).await.is_err());
    for quote in &quotes {
        assert_eq!(
            db.get_mint_quote(&quote.id).await.unwrap(),
            Some(quote.clone())
        );
    }
    assert!(db.get_saga(&request.saga.id).await.unwrap().is_none());
    assert_eq!(
        db.increment_keyset_counter(&test_keyset_id(), 0)
            .await
            .unwrap(),
        2
    );
    // Acquiring the first quote must roll back if the second is owned.
    let owner = uuid::Uuid::new_v4();
    db.reserve_mint_quote(&quotes[1].id, &owner).await.unwrap();
    request.counter.as_mut().unwrap().start = 2;
    request.counter.as_mut().unwrap().end = 3;
    if let OperationData::Mint(data) = &mut request.saga.data {
        data.counter_start = Some(2);
        data.counter_end = Some(3);
    }
    assert!(db.prepare_operation(request.clone()).await.is_err());
    assert_eq!(
        db.get_mint_quote(&quotes[0].id).await.unwrap(),
        Some(quotes[0].clone())
    );
    db.release_mint_quote(&owner).await.unwrap();
    db.prepare_operation(request.clone()).await.unwrap();
    assert_eq!(
        db.increment_keyset_counter(&test_keyset_id(), 0)
            .await
            .unwrap(),
        3
    );
    for quote in &quotes {
        assert_eq!(
            db.get_mint_quote(&quote.id)
                .await
                .unwrap()
                .unwrap()
                .used_by_operation,
            Some(request.saga.id.to_string())
        );
    }
}

/// Advancing a melt cannot commit inputs or counters with a stale saga.
pub async fn preparation_melt_transition_is_atomic<DB>(db: DB)
where
    DB: Database<Error>,
{
    let quote = test_melt_quote();
    let proof = test_proof_info(test_keyset_id(), 2048, test_mint_url());
    db.add_melt_quote(quote.clone()).await.unwrap();
    db.update_proofs(vec![proof.clone()], vec![]).await.unwrap();
    let initial = melt_preparation(&quote, vec![proof.y]);
    db.prepare_operation(initial.clone()).await.unwrap();
    let mut next = initial.clone();
    next.saga
        .update_state(WalletSagaState::Melt(MeltSagaState::MeltRequested));
    next.quotes[0].version += 1;
    if let OperationData::Melt(data) = &mut next.saga.data {
        data.final_proof_ys = Some(vec![proof.y]);
    }
    let mut bad = next.clone();
    bad.quotes[0].version += 1;
    assert!(db.prepare_operation(bad).await.is_err());
    assert_eq!(
        db.get_saga(&initial.saga.id).await.unwrap(),
        Some(initial.saga.clone())
    );
    assert_eq!(
        db.get_reserved_proofs(&next.saga.id).await.unwrap()[0].state,
        State::Reserved
    );
    let output = crate::BlindedMessage::new(
        Amount::ZERO,
        test_keyset_id(),
        SecretKey::generate().public_key(),
    );
    next.counter = Some(CounterReservation {
        keyset_id: test_keyset_id(),
        start: 0,
        end: 1,
    });
    if let OperationData::Melt(data) = &mut next.saga.data {
        data.counter_start = Some(0);
        data.counter_end = Some(1);
        data.change_blinded_messages = Some(vec![output]);
    }
    db.increment_keyset_counter(&test_keyset_id(), 2)
        .await
        .unwrap();
    assert!(db.prepare_operation(next.clone()).await.is_err());
    assert_eq!(
        db.get_saga(&initial.saga.id).await.unwrap(),
        Some(initial.saga.clone())
    );
    assert_eq!(
        db.get_reserved_proofs(&next.saga.id).await.unwrap()[0].state,
        State::Reserved
    );
    next.counter.as_mut().unwrap().start = 2;
    next.counter.as_mut().unwrap().end = 3;
    if let OperationData::Melt(data) = &mut next.saga.data {
        data.counter_start = Some(2);
        data.counter_end = Some(3);
    }
    db.prepare_operation(next.clone()).await.unwrap();
    assert_eq!(
        db.get_reserved_proofs(&next.saga.id).await.unwrap()[0].state,
        State::Pending
    );
    assert_eq!(
        db.get_saga(&next.saga.id).await.unwrap(),
        Some(next.saga.clone())
    );
    assert!(db.prepare_operation(next).await.is_err());
}

/// A nested swap acquires its inputs and counter together with both saga versions.
pub async fn preparation_nested_swap_owns_inputs<DB>(db: DB)
where
    DB: Database<Error>,
{
    let quote = test_melt_quote();
    let proof = test_proof_info(test_keyset_id(), 2048, test_mint_url());
    db.add_melt_quote(quote.clone()).await.unwrap();
    db.update_proofs(vec![proof.clone()], vec![]).await.unwrap();
    let parent = melt_preparation(&quote, vec![proof.y]);
    db.prepare_operation(parent.clone()).await.unwrap();
    let child = WalletSaga::new(
        uuid::Uuid::new_v4(),
        WalletSagaState::Swap(SwapSagaState::ProofsReserved),
        proof.proof.amount,
        test_mint_url(),
        CurrencyUnit::Sat,
        OperationData::Swap(SwapOperationData {
            parent: Some(crate::wallet::SwapParent {
                id: parent.saga.id,
                version: 0,
            }),
            input_amount: proof.proof.amount,
            output_amount: proof.proof.amount,
            counter_start: Some(0),
            counter_end: Some(1),
            blinded_messages: None,
        }),
    );
    let request = OperationPreparation {
        saga: child.clone(),
        proof_ys: vec![proof.y],
        proofs: vec![],
        quotes: vec![],
        counter: Some(CounterReservation {
            keyset_id: test_keyset_id(),
            start: 0,
            end: 1,
        }),
    };
    db.add_saga(child.clone()).await.unwrap();
    assert!(db.prepare_operation(request.clone()).await.is_err());
    assert_eq!(
        db.get_saga(&parent.saga.id).await.unwrap(),
        Some(parent.saga.clone())
    );
    assert_eq!(
        db.get_reserved_proofs(&parent.saga.id).await.unwrap().len(),
        1
    );
    assert_eq!(
        db.increment_keyset_counter(&test_keyset_id(), 0)
            .await
            .unwrap(),
        0
    );
    db.delete_saga(&child.id).await.unwrap();
    db.prepare_operation(request.clone()).await.unwrap();
    assert_eq!(
        db.get_saga(&parent.saga.id).await.unwrap().unwrap().version,
        1
    );
    assert_eq!(db.get_reserved_proofs(&child.id).await.unwrap().len(), 1);
    assert!(db
        .get_reserved_proofs(&parent.saga.id)
        .await
        .unwrap()
        .is_empty());
    db.release_proofs(&parent.saga.id).await.unwrap();
    assert_eq!(
        db.get_reserved_proofs(&child.id).await.unwrap()[0].state,
        State::Reserved
    );
    let mut stale = parent.saga;
    stale.update_state(stale.state);
    assert!(!db.update_saga(stale).await.unwrap());
    assert!(db.prepare_operation(request).await.is_err());
}
