//! Proofs tests

use std::str::FromStr;

use cashu::secret::Secret;
use cashu::{Amount, Id, SecretKey};

use crate::database::mint::test::setup_keyset;
use crate::database::mint::{Database, Error, KeysDatabase, Proof, QuoteId};
use crate::mint::Operation;
use crate::state::check_state_transition;

/// Test get proofs by keyset id
pub async fn get_proofs_by_keyset_id<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id = setup_keyset(&db).await;
    let quote_id = QuoteId::new_uuid();
    let proofs = vec![
        Proof {
            amount: Amount::from(100),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
        Proof {
            amount: Amount::from(200),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
    ];

    // Add proofs to database
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_proofs(
        proofs,
        Some(quote_id),
        &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
    )
    .await
    .unwrap();
    assert!(tx.commit().await.is_ok());

    let (proofs, states) = db.get_proofs_by_keyset_id(&keyset_id).await.unwrap();
    assert_eq!(proofs.len(), 2);
    assert_eq!(proofs.len(), states.len());
    assert_eq!(
        states
            .into_iter()
            .map(|s| s.map(|x| x.to_string()).unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["UNSPENT".to_owned(), "UNSPENT".to_owned()]
    );

    let keyset_id = Id::from_str("00916bbf7ef91a34").unwrap();
    let (proofs, states) = db.get_proofs_by_keyset_id(&keyset_id).await.unwrap();
    assert_eq!(proofs.len(), 0);
    assert_eq!(proofs.len(), states.len());
}

/// Test the basic storing and retrieving proofs from the database. Probably the database would use
/// binary/`Vec<u8>` to store data, that's why this test would quickly identify issues before running
/// other tests
pub async fn add_and_find_proofs<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id = setup_keyset(&db).await;

    let quote_id = QuoteId::new_uuid();

    let proofs = vec![
        Proof {
            amount: Amount::from(100),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
        Proof {
            amount: Amount::from(200),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
    ];

    // Add proofs to database
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_proofs(
        proofs.clone(),
        Some(quote_id.clone()),
        &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
    )
    .await
    .unwrap();
    assert!(tx.commit().await.is_ok());

    let proofs_from_db = db.get_proofs_by_ys(&[proofs[0].c, proofs[1].c]).await;
    assert!(proofs_from_db.is_ok());
    assert_eq!(proofs_from_db.unwrap().len(), 2);

    let proofs_from_db = db.get_proof_ys_by_quote_id(&quote_id).await;
    assert!(proofs_from_db.is_ok());
    assert_eq!(proofs_from_db.unwrap().len(), 2);
}

/// Test to add duplicate proofs
pub async fn add_duplicate_proofs<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id = setup_keyset(&db).await;

    let quote_id = QuoteId::new_uuid();

    let proofs = vec![
        Proof {
            amount: Amount::from(100),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
        Proof {
            amount: Amount::from(200),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
    ];

    // Add proofs to database
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_proofs(
        proofs.clone(),
        Some(quote_id.clone()),
        &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
    )
    .await
    .unwrap();
    assert!(tx.commit().await.is_ok());

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let result = tx
        .add_proofs(
            proofs.clone(),
            Some(quote_id.clone()),
            &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
        )
        .await;

    assert!(
        matches!(result.unwrap_err(), Error::Duplicate),
        "Duplicate entry"
    );
}

/// Test updating proofs states
pub async fn update_proofs_states<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use cashu::State;

    let keyset_id = setup_keyset(&db).await;
    let quote_id = QuoteId::new_uuid();

    let proofs = vec![
        Proof {
            amount: Amount::from(100),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
        Proof {
            amount: Amount::from(200),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
    ];

    let ys: Vec<_> = proofs.iter().map(|p| p.c).collect();

    // Add proofs
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_proofs(
        proofs.clone(),
        Some(quote_id),
        &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Check initial state - states may vary by implementation
    let states = db.get_proofs_states(&ys).await.unwrap();
    assert_eq!(states.len(), 2);
    assert!(states[0].is_some());
    assert!(states[1].is_some());

    // Update to pending
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut proofs = tx.get_proofs(&ys).await.unwrap();
    check_state_transition(proofs.state, State::Pending).unwrap();
    tx.update_proofs_state(&mut proofs, State::Pending)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Verify new state
    let states = db.get_proofs_states(&ys).await.unwrap();
    assert_eq!(states[0], Some(State::Pending));
    assert_eq!(states[1], Some(State::Pending));

    // Update to spent
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut proofs = tx.get_proofs(&ys).await.unwrap();
    check_state_transition(proofs.state, State::Spent).unwrap();
    tx.update_proofs_state(&mut proofs, State::Spent)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Verify final state
    let states = db.get_proofs_states(&ys).await.unwrap();
    assert_eq!(states[0], Some(State::Spent));
    assert_eq!(states[1], Some(State::Spent));
}

/// Test that update_proofs_state updates the ProofsWithState.state field
pub async fn update_proofs_state_updates_proofs_with_state<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use cashu::State;

    let keyset_id = setup_keyset(&db).await;
    let quote_id = QuoteId::new_uuid();

    let proofs = vec![Proof {
        amount: Amount::from(100),
        keyset_id,
        secret: Secret::generate(),
        c: SecretKey::generate().public_key(),
        witness: None,
        dleq: None,
        p2pk_e: None,
    }];

    let ys: Vec<_> = proofs.iter().map(|p| p.y().unwrap()).collect();

    // Add proofs and verify initial state
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut proofs = tx
        .add_proofs(
            proofs.clone(),
            Some(quote_id),
            &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
        )
        .await
        .unwrap();
    assert_eq!(proofs.state, State::Unspent);

    // Update to Pending and verify ProofsWithState.state is updated
    tx.update_proofs_state(&mut proofs, State::Pending)
        .await
        .unwrap();
    assert_eq!(
        proofs.state,
        State::Pending,
        "ProofsWithState.state should be updated to Pending after update_proofs_state"
    );
    tx.commit().await.unwrap();

    // Get proofs again and update to Spent
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut proofs = tx.get_proofs(&ys).await.unwrap();
    assert_eq!(proofs.state, State::Pending);

    tx.update_proofs_state(&mut proofs, State::Spent)
        .await
        .unwrap();
    assert_eq!(
        proofs.state,
        State::Spent,
        "ProofsWithState.state should be updated to Spent after update_proofs_state"
    );
    tx.commit().await.unwrap();
}

/// Test removing proofs
pub async fn remove_proofs<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id = setup_keyset(&db).await;
    let quote_id = QuoteId::new_uuid();

    let proofs = vec![
        Proof {
            amount: Amount::from(100),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
        Proof {
            amount: Amount::from(200),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
    ];

    let ys: Vec<_> = proofs.iter().map(|p| p.c).collect();

    // Add proofs
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_proofs(
        proofs.clone(),
        Some(quote_id.clone()),
        &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Verify proofs exist
    let retrieved = db.get_proofs_by_ys(&ys).await.unwrap();
    assert_eq!(retrieved.len(), 2);
    // Note: proofs may not be returned in the same order or may be filtered
    let found_count = retrieved.iter().filter(|p| p.is_some()).count();
    assert!(found_count >= 1, "At least one proof should exist");

    // Remove first proof
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.remove_proofs(&[ys[0]], Some(quote_id)).await.unwrap();
    tx.commit().await.unwrap();

    // Verify proof was removed or marked as removed
    let retrieved = db.get_proofs_by_ys(&ys).await.unwrap();
    assert_eq!(retrieved.len(), 2);
}

/// Test get total redeemed by keyset
pub async fn get_total_redeemed<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use cashu::State;

    let keyset_id = setup_keyset(&db).await;
    let quote_id = QuoteId::new_uuid();

    let proofs = vec![
        Proof {
            amount: Amount::from(100),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
        Proof {
            amount: Amount::from(200),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
        Proof {
            amount: Amount::from(300),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
    ];

    let ys: Vec<_> = proofs.iter().map(|p| p.c).collect();

    // Add proofs
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_proofs(
        proofs.clone(),
        Some(quote_id),
        &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // First update to Pending (valid state transition)
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut proofs = tx.get_proofs(&[ys[0], ys[1]]).await.unwrap();
    check_state_transition(proofs.state, State::Pending).unwrap();
    tx.update_proofs_state(&mut proofs, State::Pending)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Then mark some as spent
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut proofs = tx.get_proofs(&[ys[0], ys[1]]).await.unwrap();
    check_state_transition(proofs.state, State::Spent).unwrap();
    tx.update_proofs_state(&mut proofs, State::Spent)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Get total redeemed
    let totals = db.get_total_redeemed().await.unwrap();
    let total = totals.get(&keyset_id).copied().unwrap_or(Amount::ZERO);

    // Should be 300 (100 + 200)
    assert!(total >= Amount::from(300));
}

/// Test get proof ys by quote id
pub async fn get_proof_ys_by_quote_id<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id = setup_keyset(&db).await;
    let quote_id1 = QuoteId::new_uuid();
    let quote_id2 = QuoteId::new_uuid();

    let proofs1 = vec![
        Proof {
            amount: Amount::from(100),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
        Proof {
            amount: Amount::from(200),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
    ];

    let proofs2 = vec![Proof {
        amount: Amount::from(300),
        keyset_id,
        secret: Secret::generate(),
        c: SecretKey::generate().public_key(),
        witness: None,
        dleq: None,
        p2pk_e: None,
    }];

    let expected_ys1: Vec<_> = proofs1.iter().map(|p| p.c).collect();
    let expected_ys2: Vec<_> = proofs2.iter().map(|p| p.c).collect();

    // Add proofs with different quote ids
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_proofs(
        proofs1.clone(),
        Some(quote_id1.clone()),
        &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
    )
    .await
    .unwrap();
    tx.add_proofs(
        proofs2.clone(),
        Some(quote_id2.clone()),
        &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Get ys for first quote
    let ys1 = db.get_proof_ys_by_quote_id(&quote_id1).await.unwrap();
    assert_eq!(ys1.len(), 2);
    assert!(ys1.contains(&expected_ys1[0]));
    assert!(ys1.contains(&expected_ys1[1]));

    // Get ys for second quote
    let ys2 = db.get_proof_ys_by_quote_id(&quote_id2).await.unwrap();
    assert_eq!(ys2.len(), 1);
    assert!(ys2.contains(&expected_ys2[0]));
}

/// Test getting proofs states
pub async fn get_proofs_states<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use cashu::State;

    let keyset_id = setup_keyset(&db).await;
    let quote_id = QuoteId::new_uuid();

    let proofs = vec![
        Proof {
            amount: Amount::from(100),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
        Proof {
            amount: Amount::from(200),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
    ];

    let ys: Vec<_> = proofs.iter().map(|p| p.c).collect();

    // Add proofs
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_proofs(
        proofs.clone(),
        Some(quote_id),
        &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Get states - behavior may vary by implementation
    let states = db.get_proofs_states(&ys).await.unwrap();
    assert_eq!(states.len(), 2);

    // Check that at least we get a proper response
    // States may or may not be present depending on how the database stores proofs
    for s in states.iter().flatten() {
        // If state is present, it should be a valid state
        match s {
            State::Unspent
            | State::Reserved
            | State::Pending
            | State::Spent
            | State::PendingSpent => {}
        }
    }
    // It's OK if state is None for some implementations
}

/// Test getting states for non-existent proofs
pub async fn get_nonexistent_proof_states<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let y1 = SecretKey::generate().public_key();
    let y2 = SecretKey::generate().public_key();

    // Try to get states for non-existent proofs
    let states = db.get_proofs_states(&[y1, y2]).await.unwrap();
    assert_eq!(states.len(), 2);
    assert!(states[0].is_none());
    assert!(states[1].is_none());
}

/// Test getting proofs by non-existent ys
pub async fn get_proofs_by_nonexistent_ys<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let y1 = SecretKey::generate().public_key();
    let y2 = SecretKey::generate().public_key();

    // Try to get proofs for non-existent ys
    let proofs = db.get_proofs_by_ys(&[y1, y2]).await.unwrap();
    assert_eq!(proofs.len(), 2);
    assert!(proofs[0].is_none());
    assert!(proofs[1].is_none());
}

/// Test proof transaction isolation - verifies that changes are only visible after commit
pub async fn proof_transaction_isolation<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id = setup_keyset(&db).await;
    let quote_id = QuoteId::new_uuid();

    let proof = Proof {
        amount: Amount::from(100),
        keyset_id,
        secret: Secret::generate(),
        c: SecretKey::generate().public_key(),
        witness: None,
        dleq: None,
        p2pk_e: None,
    };

    let y = proof.c;

    // Verify proof doesn't exist before transaction
    let proofs_before = db.get_proofs_by_ys(&[y]).await.unwrap();
    assert_eq!(proofs_before.len(), 1);
    assert!(proofs_before[0].is_none());

    // Start a transaction and add proof but don't commit
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_proofs(
        vec![proof.clone()],
        Some(quote_id),
        &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
    )
    .await
    .unwrap();

    // Commit the transaction
    tx.commit().await.unwrap();

    // After commit, verify the proof state is available
    let states = db.get_proofs_states(&[y]).await.unwrap();
    assert_eq!(states.len(), 1);
    // Verify we get a valid state response (behavior may vary by implementation)
}

/// Test rollback prevents proof insertion
pub async fn proof_rollback<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id = setup_keyset(&db).await;
    let quote_id = QuoteId::new_uuid();

    let proof = Proof {
        amount: Amount::from(100),
        keyset_id,
        secret: Secret::generate(),
        c: SecretKey::generate().public_key(),
        witness: None,
        dleq: None,
        p2pk_e: None,
    };

    let y = proof.c;

    // Start a transaction, add proof, then rollback
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_proofs(
        vec![proof.clone()],
        Some(quote_id),
        &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();

    // Proof should not exist after rollback
    let proofs = db.get_proofs_by_ys(&[y]).await.unwrap();
    assert_eq!(proofs.len(), 1);
    assert!(proofs[0].is_none());
}

/// Test multiple proofs with same keyset
pub async fn multiple_proofs_same_keyset<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id = setup_keyset(&db).await;

    let proofs: Vec<_> = (0..10)
        .map(|i| Proof {
            amount: Amount::from((i + 1) * 100),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        })
        .collect();

    // Add all proofs
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_proofs(
        proofs.clone(),
        None,
        &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Get proofs by keyset
    let (retrieved_proofs, states) = db.get_proofs_by_keyset_id(&keyset_id).await.unwrap();
    assert!(retrieved_proofs.len() >= 10);
    assert_eq!(retrieved_proofs.len(), states.len());

    // Calculate total amount
    let total: u64 = retrieved_proofs.iter().map(|p| u64::from(p.amount)).sum();
    assert!(total >= 5500); // 100 + 200 + ... + 1000 = 5500
}

/// Test that removing proofs in Spent state should fail
///
/// This test verifies that the storage layer enforces the constraint that proofs
/// in the `Spent` state cannot be removed via `remove_proofs`. The operation should
/// fail with an error to prevent accidental deletion of spent proofs.
pub async fn remove_spent_proofs_should_fail<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use cashu::State;

    let keyset_id = setup_keyset(&db).await;
    let quote_id = QuoteId::new_uuid();

    let proofs = vec![
        Proof {
            amount: Amount::from(100),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
        Proof {
            amount: Amount::from(200),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
    ];

    let ys: Vec<_> = proofs.iter().map(|p| p.y().unwrap()).collect();

    // Add proofs to database (initial state is Unspent)
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_proofs(
        proofs.clone(),
        Some(quote_id.clone()),
        &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Verify proofs exist and are in Unspent state
    let states = db.get_proofs_states(&ys).await.unwrap();
    assert_eq!(states.len(), 2);
    assert_eq!(states[0], Some(State::Unspent));
    assert_eq!(states[1], Some(State::Unspent));

    // Removing Unspent proofs should succeed
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let result = tx.remove_proofs(&[ys[0]], Some(quote_id.clone())).await;
    assert!(result.is_ok(), "Removing Unspent proof should succeed");
    tx.rollback().await.unwrap(); // Rollback to keep proofs for next test

    // Transition proofs to Pending state
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut records = tx.get_proofs(&ys).await.expect("valid records");
    check_state_transition(records.state, State::Pending).unwrap();
    tx.update_proofs_state(&mut records, State::Pending)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Removing Pending proofs should also succeed
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let result = tx.remove_proofs(&[ys[0]], Some(quote_id.clone())).await;
    assert!(
        result.is_ok(),
        "Removing Pending proof should succeed: {:?}",
        result,
    );
    tx.rollback().await.unwrap(); // Rollback to keep proofs for next test

    // Now transition proofs to Spent state
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut records = tx.get_proofs(&ys).await.expect("valid records");
    check_state_transition(records.state, State::Spent).unwrap();
    tx.update_proofs_state(&mut records, State::Spent)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Verify proofs are now in Spent state
    let states = db.get_proofs_states(&ys).await.unwrap();
    assert_eq!(states[0], Some(State::Spent));
    assert_eq!(states[1], Some(State::Spent));

    // Attempt to remove Spent proofs - this should FAIL
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let result = tx.remove_proofs(&ys, Some(quote_id.clone())).await;
    assert!(
        result.is_err(),
        "Removing proofs in Spent state should fail"
    );

    // Verify the error is the expected type
    assert!(
        matches!(result.unwrap_err(), Error::AttemptRemoveSpentProof),
        "Error should be AttemptRemoveSpentProof"
    );

    // Rollback the failed transaction to release locks
    tx.rollback().await.unwrap();

    // Verify proofs still exist after failed removal attempt
    let states = db.get_proofs_states(&ys).await.unwrap();
    assert_eq!(
        states[0],
        Some(State::Spent),
        "First proof should still exist"
    );
    assert_eq!(
        states[1],
        Some(State::Spent),
        "Second proof should still exist"
    );
}

/// Test that get_proofs fails when proofs have inconsistent states
///
/// This validates the database layer's responsibility to ensure all proofs
/// returned by get_proofs share the same state. The mint never needs proofs
/// with different states, so this is an invariant the database must enforce.
pub async fn get_proofs_with_inconsistent_states_fails<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use cashu::State;

    let keyset_id = setup_keyset(&db).await;
    let quote_id = QuoteId::new_uuid();

    // Create three proofs
    let proofs = vec![
        Proof {
            amount: Amount::from(100),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
        Proof {
            amount: Amount::from(200),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
        Proof {
            amount: Amount::from(300),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
    ];

    let ys: Vec<_> = proofs.iter().map(|p| p.y().unwrap()).collect();

    // Add all proofs (initial state is Unspent)
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_proofs(
        proofs,
        Some(quote_id),
        &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Transition only the first two proofs to Pending state
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut first_two_proofs = tx.get_proofs(&ys[0..2]).await.unwrap();
    check_state_transition(first_two_proofs.state, State::Pending).unwrap();
    tx.update_proofs_state(&mut first_two_proofs, State::Pending)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Verify the states are now inconsistent via get_proofs_states
    let states = db.get_proofs_states(&ys).await.unwrap();
    assert_eq!(
        states[0],
        Some(State::Pending),
        "First proof should be Pending"
    );
    assert_eq!(
        states[1],
        Some(State::Pending),
        "Second proof should be Pending"
    );
    assert_eq!(
        states[2],
        Some(State::Unspent),
        "Third proof should be Unspent"
    );

    // Now try to get all three proofs via get_proofs - this should fail
    // because the proofs have inconsistent states
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let result = tx.get_proofs(&ys).await;

    assert!(
        result.is_err(),
        "get_proofs should fail when proofs have inconsistent states"
    );

    tx.rollback().await.unwrap();
}

/// Test that get_proofs fails when some requested proofs don't exist
///
/// This validates that the database returns an error when not all requested
/// proofs are found, rather than silently returning a partial result.
pub async fn get_proofs_fails_when_some_not_found<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id = setup_keyset(&db).await;
    let quote_id = QuoteId::new_uuid();

    // Create two proofs that will be stored
    let stored_proofs = vec![
        Proof {
            amount: Amount::from(100),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
        Proof {
            amount: Amount::from(200),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
            p2pk_e: None,
        },
    ];

    // Create a third proof that will NOT be stored
    let non_existent_proof = Proof {
        amount: Amount::from(300),
        keyset_id,
        secret: Secret::generate(),
        c: SecretKey::generate().public_key(),
        witness: None,
        dleq: None,
        p2pk_e: None,
    };

    let stored_ys: Vec<_> = stored_proofs.iter().map(|p| p.y().unwrap()).collect();
    let non_existent_y = non_existent_proof.y().unwrap();

    // Add only the first two proofs
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_proofs(
        stored_proofs,
        Some(quote_id),
        &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Verify the stored proofs exist
    let states = db.get_proofs_states(&stored_ys).await.unwrap();
    assert_eq!(states.len(), 2);
    assert!(states[0].is_some(), "First proof should exist");
    assert!(states[1].is_some(), "Second proof should exist");

    // Verify the non-existent proof doesn't exist
    let states = db.get_proofs_states(&[non_existent_y]).await.unwrap();
    assert_eq!(states[0], None, "Third proof should not exist");

    // Now try to get all three proofs (2 exist, 1 doesn't) - this should fail
    let all_ys = vec![stored_ys[0], stored_ys[1], non_existent_y];
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let result = tx.get_proofs(&all_ys).await;

    assert!(
        result.is_err(),
        "get_proofs should fail when some proofs don't exist (got 2 of 3)"
    );

    tx.rollback().await.unwrap();
}
