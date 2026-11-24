//! Saga database tests

use crate::database::mint::{Acquired, Database, Error};
use crate::mint::{
    MeltFinalizationData, MeltSagaState, OperationKind, Saga, SagaStateEnum, SwapSagaState,
};
use crate::payment::PaymentIdentifier;
use crate::{Amount, CurrencyUnit};

/// Test adding and retrieving a saga
pub async fn add_and_get_saga<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();
    let saga = Saga::new_swap(operation_id, SwapSagaState::SetupComplete)
        .with_timestamps(1234567890, 1234567890);

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_saga(&operation_id).await.unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.operation_id, saga.operation_id);
    assert_eq!(retrieved.operation_kind(), saga.operation_kind());
    assert_eq!(retrieved.state, saga.state);
    assert_eq!(retrieved.quote_id(), saga.quote_id());
    assert_eq!(retrieved.finalization_data(), saga.finalization_data());
    tx.commit().await.unwrap();
}

/// Test adding duplicate saga fails
pub async fn add_duplicate_saga<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();
    let saga = Saga::new_swap(operation_id, SwapSagaState::SetupComplete)
        .with_timestamps(1234567890, 1234567890);

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let result = tx.add_saga(&saga).await;
    assert!(result.is_err());
    tx.rollback().await.unwrap();
}

/// A melt saga in `PaymentAttempted`, already stored in the database.
async fn stored_attempted_melt_saga<DB>(db: &DB) -> uuid::Uuid
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();
    let saga = Saga::new_melt(
        operation_id,
        MeltSagaState::PaymentAttempted,
        "test_quote_id".to_string(),
    )
    .with_timestamps(1234567890, 1234567890);

    let mut tx = Database::begin_transaction(db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    operation_id
}

/// Moves a stored saga to `new_state` in a transaction of its own.
async fn move_saga<DB>(db: &DB, operation_id: &uuid::Uuid, new_state: SagaStateEnum)
where
    DB: Database<Error>,
{
    let mut tx = Database::begin_transaction(db).await.unwrap();
    let mut saga = tx.get_saga_for_update(operation_id).await.unwrap().unwrap();
    tx.update_acquired_saga(&mut saga, new_state).await.unwrap();
    tx.commit().await.unwrap();
}

/// Loads a saga and ends the transaction, so the copy outlives its row lock.
async fn stale_saga<DB>(db: &DB, operation_id: &uuid::Uuid) -> Acquired<Saga>
where
    DB: Database<Error>,
{
    let mut tx = Database::begin_transaction(db).await.unwrap();
    let saga = tx.get_saga_for_update(operation_id).await.unwrap().unwrap();
    tx.commit().await.unwrap();

    saga
}

fn sample_finalization_data() -> MeltFinalizationData {
    MeltFinalizationData {
        total_spent: Amount::from(9_123).with_unit(CurrencyUnit::Sat),
        payment_lookup_id: PaymentIdentifier::CustomId("lookup_123".to_string()),
        payment_proof: Some("proof_123".to_string()),
    }
}

/// Test updating saga state
pub async fn update_saga_state<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();
    let saga = Saga::new_swap(operation_id, SwapSagaState::SetupComplete)
        .with_timestamps(1234567890, 1234567890);

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    let new_state = SagaStateEnum::Swap(SwapSagaState::Signed);
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut saga = tx
        .get_saga_for_update(&operation_id)
        .await
        .unwrap()
        .expect("saga should exist");
    tx.update_acquired_saga(&mut saga, new_state.clone())
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_saga(&operation_id).await.unwrap().unwrap();
    assert_eq!(retrieved.state, new_state);
    assert!(retrieved.updated_at >= saga.updated_at);
    tx.commit().await.unwrap();
}

/// A saga kept past its transaction can't undo a `Finalizing` handoff.
pub async fn update_acquired_saga_refuses_stale_state<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = stored_attempted_melt_saga(&db).await;
    let mut stale = stale_saga(&db, &operation_id).await;
    let finalization_data = sample_finalization_data();
    move_saga(
        &db,
        &operation_id,
        SagaStateEnum::Melt(MeltSagaState::finalizing(finalization_data.clone())),
    )
    .await;

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let err = tx
        .update_acquired_saga(
            &mut stale,
            SagaStateEnum::Melt(MeltSagaState::PaymentFailed),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::ConcurrentUpdate), "{err}");
    tx.rollback().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let stored = tx.get_saga(&operation_id).await.unwrap().unwrap();
    assert_eq!(
        stored.state,
        SagaStateEnum::Melt(MeltSagaState::finalizing(finalization_data))
    );
    tx.commit().await.unwrap();
}

/// Test updating saga state with persisted melt finalization data.
pub async fn update_acquired_saga_with_finalization_data<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();
    let saga = Saga::new_melt(
        operation_id,
        MeltSagaState::SetupComplete,
        "test_quote_id".to_string(),
    )
    .with_timestamps(1234567890, 1234567890);

    let finalization_data = sample_finalization_data();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut saga = tx
        .get_saga_for_update(&operation_id)
        .await
        .unwrap()
        .expect("saga should exist");
    tx.update_acquired_saga(
        &mut saga,
        SagaStateEnum::Melt(MeltSagaState::finalizing(finalization_data.clone())),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_saga(&operation_id).await.unwrap().unwrap();
    assert_eq!(
        retrieved.state,
        SagaStateEnum::Melt(MeltSagaState::finalizing(finalization_data.clone()))
    );
    assert_eq!(retrieved.finalization_data(), Some(&finalization_data));
    tx.commit().await.unwrap();
}

/// A saga kept past its transaction can't move a failed payment into `Finalizing`.
pub async fn update_acquired_saga_with_finalization_data_refuses_stale_state<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = stored_attempted_melt_saga(&db).await;
    let mut stale = stale_saga(&db, &operation_id).await;
    move_saga(
        &db,
        &operation_id,
        SagaStateEnum::Melt(MeltSagaState::PaymentFailed),
    )
    .await;

    let finalization_data = sample_finalization_data();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let err = tx
        .update_acquired_saga(
            &mut stale,
            SagaStateEnum::Melt(MeltSagaState::finalizing(finalization_data)),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::ConcurrentUpdate), "{err}");
    tx.rollback().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let stored = tx.get_saga(&operation_id).await.unwrap().unwrap();
    assert_eq!(
        stored.state,
        SagaStateEnum::Melt(MeltSagaState::PaymentFailed)
    );
    assert_eq!(stored.finalization_data(), None);
    tx.commit().await.unwrap();
}

/// Illegal saga state transitions are rejected before modifying the row.
pub async fn update_acquired_saga_refuses_invalid_transition<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();
    let saga = Saga::new_melt(
        operation_id,
        MeltSagaState::SetupComplete,
        "test_quote_id".to_string(),
    )
    .with_timestamps(1234567890, 1234567890);

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    // SetupComplete -> PaymentFailed is invalid (must go through PaymentAttempted)
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut acquired = tx
        .get_saga_for_update(&operation_id)
        .await
        .unwrap()
        .expect("saga should exist");
    let err = tx
        .update_acquired_saga(
            &mut acquired,
            SagaStateEnum::Melt(MeltSagaState::PaymentFailed),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidStateTransition(_)), "{err}");
    tx.rollback().await.unwrap();

    // Advance legally to Finalizing
    let finalization_data = sample_finalization_data();
    move_saga(
        &db,
        &operation_id,
        SagaStateEnum::Melt(MeltSagaState::finalizing(finalization_data.clone())),
    )
    .await;

    // Finalizing -> PaymentAttempted regression is rejected
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut acquired = tx
        .get_saga_for_update(&operation_id)
        .await
        .unwrap()
        .expect("saga should exist");
    let err = tx
        .update_acquired_saga(
            &mut acquired,
            SagaStateEnum::Melt(MeltSagaState::PaymentAttempted),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidStateTransition(_)), "{err}");
    tx.rollback().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_saga(&operation_id).await.unwrap().unwrap();
    assert_eq!(
        retrieved.state,
        SagaStateEnum::Melt(MeltSagaState::finalizing(finalization_data.clone()))
    );
    assert_eq!(retrieved.finalization_data(), Some(&finalization_data));
    tx.commit().await.unwrap();

    // PaymentFailed -> Finalizing is also rejected
    let failed_op = stored_attempted_melt_saga(&db).await;
    move_saga(
        &db,
        &failed_op,
        SagaStateEnum::Melt(MeltSagaState::PaymentFailed),
    )
    .await;
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut acquired = tx
        .get_saga_for_update(&failed_op)
        .await
        .unwrap()
        .expect("saga should exist");
    let err = tx
        .update_acquired_saga(
            &mut acquired,
            SagaStateEnum::Melt(MeltSagaState::finalizing(finalization_data)),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidStateTransition(_)), "{err}");
    tx.rollback().await.unwrap();
}

/// Test deleting saga
pub async fn delete_saga<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();
    let saga = Saga::new_swap(operation_id, SwapSagaState::SetupComplete)
        .with_timestamps(1234567890, 1234567890);

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_saga(&operation_id).await.unwrap();
    assert!(retrieved.is_some());
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.delete_saga(&operation_id).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_saga(&operation_id).await.unwrap();
    assert!(retrieved.is_none());
    tx.commit().await.unwrap();
}

/// Test getting incomplete sagas for swap operation
pub async fn get_incomplete_swap_sagas<DB>(db: DB)
where
    DB: Database<Error>,
{
    let saga1 = Saga::new_swap(uuid::Uuid::new_v4(), SwapSagaState::SetupComplete)
        .with_timestamps(1234567890, 1234567890);

    let saga2 = Saga::new_swap(uuid::Uuid::new_v4(), SwapSagaState::Signed)
        .with_timestamps(1234567891, 1234567891);

    let saga3 = Saga::new_melt(
        uuid::Uuid::new_v4(),
        MeltSagaState::SetupComplete,
        "test_quote_id".to_string(),
    )
    .with_timestamps(1234567892, 1234567892);

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga1).await.unwrap();
    tx.add_saga(&saga2).await.unwrap();
    tx.add_saga(&saga3).await.unwrap();
    tx.commit().await.unwrap();

    let incomplete_swaps = db.get_incomplete_sagas(OperationKind::Swap).await.unwrap();
    assert!(incomplete_swaps.len() >= 2);
    assert!(incomplete_swaps
        .iter()
        .any(|s| s.operation_id == saga1.operation_id));
    assert!(incomplete_swaps
        .iter()
        .any(|s| s.operation_id == saga2.operation_id));
    assert!(!incomplete_swaps
        .iter()
        .any(|s| s.operation_id == saga3.operation_id));
}

/// Test saga with quote_id for melt operations
pub async fn get_melt_saga_by_quote_id<DB>(db: DB)
where
    DB: Database<Error>,
{
    let quote_id = crate::QuoteId::new();
    let saga = Saga::new_melt(
        uuid::Uuid::new_v4(),
        MeltSagaState::SetupComplete,
        quote_id.to_string(),
    )
    .with_timestamps(1234567890, 1234567890);

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    let retrieved = db.get_melt_saga_by_quote_id(&quote_id).await.unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.operation_id, saga.operation_id);
    assert_eq!(retrieved.quote_id(), Some(quote_id.to_string().as_str()));
}

/// Test melt-specific quote lookup ignores non-melt sagas even when `quote_id` collides
pub async fn get_melt_saga_by_quote_id_filters_non_melt<DB>(db: DB)
where
    DB: Database<Error>,
{
    let quote_id = crate::QuoteId::new();
    let swap_saga = Saga::from_db(
        uuid::Uuid::new_v4(),
        SagaStateEnum::Swap(SwapSagaState::SetupComplete),
        Some(quote_id.to_string()),
        1234567890,
        1234567890,
    );
    let melt_saga = Saga::new_melt(
        uuid::Uuid::new_v4(),
        MeltSagaState::SetupComplete,
        quote_id.to_string(),
    )
    .with_timestamps(1234567891, 1234567891);

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&swap_saga).await.unwrap();
    tx.add_saga(&melt_saga).await.unwrap();
    let stored_swap = tx.get_saga(&swap_saga.operation_id).await.unwrap().unwrap();
    assert_eq!(stored_swap.quote_id(), Some(quote_id.to_string().as_str()));
    tx.commit().await.unwrap();

    let retrieved = db.get_melt_saga_by_quote_id(&quote_id).await.unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.operation_id, melt_saga.operation_id);
    assert_eq!(retrieved.quote_id(), Some(quote_id.to_string().as_str()));
}

/// Test getting incomplete sagas for melt operation
pub async fn get_incomplete_melt_sagas<DB>(db: DB)
where
    DB: Database<Error>,
{
    let saga1 = Saga::new_melt(
        uuid::Uuid::new_v4(),
        MeltSagaState::SetupComplete,
        "melt_quote_1".to_string(),
    )
    .with_timestamps(1234567890, 1234567890);

    let saga2 = Saga::new_melt(
        uuid::Uuid::new_v4(),
        MeltSagaState::PaymentAttempted,
        "melt_quote_2".to_string(),
    )
    .with_timestamps(1234567891, 1234567891);

    let saga3 = Saga::new_swap(uuid::Uuid::new_v4(), SwapSagaState::SetupComplete)
        .with_timestamps(1234567892, 1234567892);

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga1).await.unwrap();
    tx.add_saga(&saga2).await.unwrap();
    tx.add_saga(&saga3).await.unwrap();
    tx.commit().await.unwrap();

    let incomplete_melts = db.get_incomplete_sagas(OperationKind::Melt).await.unwrap();
    assert!(incomplete_melts.len() >= 2);
    assert!(incomplete_melts
        .iter()
        .any(|s| s.operation_id == saga1.operation_id));
    assert!(incomplete_melts
        .iter()
        .any(|s| s.operation_id == saga2.operation_id));
    assert!(!incomplete_melts
        .iter()
        .any(|s| s.operation_id == saga3.operation_id));
}

/// Test getting saga for non-existent operation
pub async fn get_nonexistent_saga<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_saga(&operation_id).await.unwrap();
    assert!(retrieved.is_none());
    tx.commit().await.unwrap();
}

/// Test acquiring a non-existent saga returns `None`.
pub async fn get_saga_for_update_nonexistent<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let saga = tx.get_saga_for_update(&operation_id).await.unwrap();
    assert!(saga.is_none(), "Non-existent saga should return None");
    tx.commit().await.unwrap();
}

/// Test deleting non-existent saga is idempotent
pub async fn delete_nonexistent_saga<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let saga = tx.get_saga(&operation_id).await.unwrap();
    assert!(saga.is_none(), "Non-existent saga should return None");
    tx.commit().await.unwrap();
}

/// Test saga with quote_id for melt operations
pub async fn saga_with_quote_id<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();
    let quote_id = "test_melt_quote_123";
    let saga = Saga::new_melt(
        operation_id,
        MeltSagaState::SetupComplete,
        quote_id.to_string(),
    )
    .with_timestamps(1234567890, 1234567890);

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_saga(&operation_id).await.unwrap().unwrap();
    assert_eq!(retrieved.quote_id(), Some(quote_id));
    tx.commit().await.unwrap();
}

/// Test saga transaction rollback
pub async fn saga_transaction_rollback<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();
    let saga = Saga::new_swap(operation_id, SwapSagaState::SetupComplete)
        .with_timestamps(1234567890, 1234567890);

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.rollback().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_saga(&operation_id).await.unwrap();
    assert!(retrieved.is_none());
    tx.commit().await.unwrap();
}

/// Test multiple sagas with different states
pub async fn multiple_sagas_different_states<DB>(db: DB)
where
    DB: Database<Error>,
{
    let sagas = vec![
        Saga::new_swap(uuid::Uuid::new_v4(), SwapSagaState::SetupComplete)
            .with_timestamps(1234567890, 1234567890),
        Saga::new_swap(uuid::Uuid::new_v4(), SwapSagaState::Signed)
            .with_timestamps(1234567891, 1234567891),
        Saga::new_melt(
            uuid::Uuid::new_v4(),
            MeltSagaState::SetupComplete,
            "quote1".to_string(),
        )
        .with_timestamps(1234567892, 1234567892),
        Saga::new_melt(
            uuid::Uuid::new_v4(),
            MeltSagaState::PaymentAttempted,
            "quote2".to_string(),
        )
        .with_timestamps(1234567893, 1234567893),
    ];

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    for saga in &sagas {
        tx.add_saga(saga).await.unwrap();
    }
    tx.commit().await.unwrap();

    for saga in &sagas {
        let mut tx = Database::begin_transaction(&db).await.unwrap();
        let retrieved = tx.get_saga(&saga.operation_id).await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.operation_id, saga.operation_id);
        assert_eq!(retrieved.operation_kind(), saga.operation_kind());
        assert_eq!(retrieved.state, saga.state);
        assert_eq!(retrieved.quote_id(), saga.quote_id());
        assert_eq!(retrieved.finalization_data(), saga.finalization_data());
        tx.commit().await.unwrap();
    }
}

/// Test that fresh saga writes cannot create a payloadless `Finalizing(None)` row.
pub async fn add_saga_rejects_payloadless_finalizing<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();
    let invalid_saga = Saga::new_melt(
        operation_id,
        MeltSagaState::Finalizing(None),
        "quote_payloadless".to_string(),
    )
    .with_timestamps(1234567890, 1234567890);

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let err = tx.add_saga(&invalid_saga).await.unwrap_err();
    assert!(
        matches!(err, Error::InvalidStateTransition(_)),
        "expected InvalidStateTransition on payloadless Finalizing insert, got {err:?}"
    );
    tx.rollback().await.unwrap();

    // Also verify transitioning an existing saga to Finalizing(None) is rejected.
    let valid_saga = Saga::new_melt(
        operation_id,
        MeltSagaState::SetupComplete,
        "quote_payloadless".to_string(),
    )
    .with_timestamps(1234567890, 1234567890);
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&valid_saga).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut acquired = tx
        .get_saga_for_update(&operation_id)
        .await
        .unwrap()
        .unwrap();
    let err = tx
        .update_acquired_saga(
            &mut acquired,
            SagaStateEnum::Melt(MeltSagaState::Finalizing(None)),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::InvalidStateTransition(_)),
        "expected InvalidStateTransition on transition to Finalizing(None), got {err:?}"
    );
    tx.rollback().await.unwrap();
}

/// Test that mutating an acquired swap saga's in-memory kind cannot update the stored row.
pub async fn update_acquired_saga_refuses_cross_kind_snapshot_mutation<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();
    let swap_saga = Saga::new_swap(operation_id, SwapSagaState::SetupComplete)
        .with_timestamps(1234567890, 1234567890);

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&swap_saga).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut acquired = tx
        .get_saga_for_update(&operation_id)
        .await
        .unwrap()
        .unwrap();
    // Mutate the in-memory snapshot from Swap(SetupComplete) to Melt(SetupComplete),
    // which shares the `setup_complete` state tag. The SQL update must still check
    // `operation_kind` and reject the cross-kind update.
    acquired.state = SagaStateEnum::Melt(MeltSagaState::SetupComplete);
    let err = tx
        .update_acquired_saga(
            &mut acquired,
            SagaStateEnum::Melt(MeltSagaState::PaymentAttempted),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::ConcurrentUpdate),
        "expected ConcurrentUpdate on cross-kind snapshot mutation, got {err:?}"
    );
    tx.rollback().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let stored = tx.get_saga(&operation_id).await.unwrap().unwrap();
    assert_eq!(stored.operation_kind(), OperationKind::Swap);
    assert_eq!(
        stored.state,
        SagaStateEnum::Swap(SwapSagaState::SetupComplete)
    );
    tx.commit().await.unwrap();
}

/// Verify that a pre-seeded legacy `Finalizing(None)` row can be enriched once
/// and rejects a second overwrite from a stale `Finalizing(None)` snapshot.
pub async fn legacy_finalizing_enrichment_rejects_second_overwrite<DB>(
    db: &DB,
    operation_id: uuid::Uuid,
) where
    DB: Database<Error>,
{
    let mut stale_tx = Database::begin_transaction(db).await.unwrap();
    let mut stale_snapshot = stale_tx
        .get_saga_for_update(&operation_id)
        .await
        .unwrap()
        .expect("legacy row exists");
    assert_eq!(
        stale_snapshot.state,
        SagaStateEnum::Melt(MeltSagaState::Finalizing(None))
    );
    stale_tx.rollback().await.unwrap();

    let first_data = MeltFinalizationData {
        total_spent: Amount::from(9_100).with_unit(CurrencyUnit::Sat),
        payment_lookup_id: PaymentIdentifier::CustomId("first-lookup".to_string()),
        payment_proof: Some("first-preimage".to_string()),
    };
    let second_data = MeltFinalizationData {
        total_spent: Amount::from(9_200).with_unit(CurrencyUnit::Sat),
        payment_lookup_id: PaymentIdentifier::CustomId("second-lookup".to_string()),
        payment_proof: Some("second-preimage".to_string()),
    };

    // First enrichment commits successfully.
    let mut tx1 = Database::begin_transaction(db).await.unwrap();
    let mut acquired1 = tx1
        .get_saga_for_update(&operation_id)
        .await
        .unwrap()
        .expect("legacy row exists");
    tx1.update_acquired_saga(
        &mut acquired1,
        SagaStateEnum::Melt(MeltSagaState::finalizing(first_data.clone())),
    )
    .await
    .expect("first enrichment should succeed");
    tx1.commit().await.unwrap();

    // Second enrichment using the stale Finalizing(None) snapshot must fail
    // with ConcurrentUpdate and preserve the first payload.
    let mut tx2 = Database::begin_transaction(db).await.unwrap();
    let err = tx2
        .update_acquired_saga(
            &mut stale_snapshot,
            SagaStateEnum::Melt(MeltSagaState::finalizing(second_data)),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::ConcurrentUpdate),
        "expected ConcurrentUpdate on stale enrichment, got {err:?}"
    );
    tx2.rollback().await.unwrap();

    let mut verify_tx = Database::begin_transaction(db).await.unwrap();
    let stored = verify_tx.get_saga(&operation_id).await.unwrap().unwrap();
    assert_eq!(stored.finalization_data(), Some(&first_data));
    verify_tx.commit().await.unwrap();
}
