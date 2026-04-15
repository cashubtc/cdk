//! Saga database tests

use crate::database::mint::{Database, Error};
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
    let saga = Saga {
        operation_id,
        operation_kind: OperationKind::Swap,
        state: SagaStateEnum::Swap(SwapSagaState::SetupComplete),
        quote_id: None,
        finalization_data: None,
        created_at: 1234567890,
        updated_at: 1234567890,
    };

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_saga(&operation_id).await.unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.operation_id, saga.operation_id);
    assert_eq!(retrieved.operation_kind, saga.operation_kind);
    assert_eq!(retrieved.state, saga.state);
    assert_eq!(retrieved.quote_id, saga.quote_id);
    assert_eq!(retrieved.finalization_data, saga.finalization_data);
    tx.commit().await.unwrap();
}

/// Test adding duplicate saga fails
pub async fn add_duplicate_saga<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();
    let saga = Saga {
        operation_id,
        operation_kind: OperationKind::Swap,
        state: SagaStateEnum::Swap(SwapSagaState::SetupComplete),
        quote_id: None,
        finalization_data: None,
        created_at: 1234567890,
        updated_at: 1234567890,
    };

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let result = tx.add_saga(&saga).await;
    assert!(result.is_err());
    tx.rollback().await.unwrap();
}

/// Test updating saga state
pub async fn update_saga_state<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();
    let saga = Saga {
        operation_id,
        operation_kind: OperationKind::Swap,
        state: SagaStateEnum::Swap(SwapSagaState::SetupComplete),
        quote_id: None,
        finalization_data: None,
        created_at: 1234567890,
        updated_at: 1234567890,
    };

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    let new_state = SagaStateEnum::Swap(SwapSagaState::Signed);
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.update_saga(&operation_id, new_state.clone())
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_saga(&operation_id).await.unwrap().unwrap();
    assert_eq!(retrieved.state, new_state);
    assert!(retrieved.updated_at >= saga.updated_at);
    tx.commit().await.unwrap();
}

/// Test updating saga state with persisted melt finalization data.
pub async fn update_saga_with_finalization_data<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();
    let saga = Saga {
        operation_id,
        operation_kind: OperationKind::Melt,
        state: SagaStateEnum::Melt(MeltSagaState::SetupComplete),
        quote_id: Some("test_quote_id".to_string()),
        finalization_data: None,
        created_at: 1234567890,
        updated_at: 1234567890,
    };

    let finalization_data = MeltFinalizationData {
        total_spent: Amount::from(9_123).with_unit(CurrencyUnit::Sat),
        payment_lookup_id: PaymentIdentifier::CustomId("lookup_123".to_string()),
        payment_proof: Some("proof_123".to_string()),
    };

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.update_saga_with_finalization_data(
        &operation_id,
        SagaStateEnum::Melt(MeltSagaState::Finalizing),
        Some(&finalization_data),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_saga(&operation_id).await.unwrap().unwrap();
    assert_eq!(
        retrieved.state,
        SagaStateEnum::Melt(MeltSagaState::Finalizing)
    );
    assert_eq!(retrieved.finalization_data, Some(finalization_data));
    tx.commit().await.unwrap();
}

/// Test updating saga state preserves existing finalization data.
pub async fn update_saga_preserves_finalization_data<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();
    let saga = Saga {
        operation_id,
        operation_kind: OperationKind::Melt,
        state: SagaStateEnum::Melt(MeltSagaState::SetupComplete),
        quote_id: Some("test_quote_id".to_string()),
        finalization_data: None,
        created_at: 1234567890,
        updated_at: 1234567890,
    };

    let finalization_data = MeltFinalizationData {
        total_spent: Amount::from(9_123).with_unit(CurrencyUnit::Sat),
        payment_lookup_id: PaymentIdentifier::CustomId("lookup_123".to_string()),
        payment_proof: Some("proof_123".to_string()),
    };

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.update_saga_with_finalization_data(
        &operation_id,
        SagaStateEnum::Melt(MeltSagaState::Finalizing),
        Some(&finalization_data),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.update_saga(
        &operation_id,
        SagaStateEnum::Melt(MeltSagaState::PaymentAttempted),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_saga(&operation_id).await.unwrap().unwrap();
    assert_eq!(
        retrieved.state,
        SagaStateEnum::Melt(MeltSagaState::PaymentAttempted)
    );
    assert_eq!(retrieved.finalization_data, Some(finalization_data));
    tx.commit().await.unwrap();
}

/// Test deleting saga
pub async fn delete_saga<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();
    let saga = Saga {
        operation_id,
        operation_kind: OperationKind::Swap,
        state: SagaStateEnum::Swap(SwapSagaState::SetupComplete),
        quote_id: None,
        finalization_data: None,
        created_at: 1234567890,
        updated_at: 1234567890,
    };

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
    let saga1 = Saga {
        operation_id: uuid::Uuid::new_v4(),
        operation_kind: OperationKind::Swap,
        state: SagaStateEnum::Swap(SwapSagaState::SetupComplete),
        quote_id: None,
        finalization_data: None,
        created_at: 1234567890,
        updated_at: 1234567890,
    };

    let saga2 = Saga {
        operation_id: uuid::Uuid::new_v4(),
        operation_kind: OperationKind::Swap,
        state: SagaStateEnum::Swap(SwapSagaState::Signed),
        quote_id: None,
        finalization_data: None,
        created_at: 1234567891,
        updated_at: 1234567891,
    };

    let saga3 = Saga {
        operation_id: uuid::Uuid::new_v4(),
        operation_kind: OperationKind::Melt,
        state: SagaStateEnum::Melt(MeltSagaState::SetupComplete),
        quote_id: Some("test_quote_id".to_string()),
        finalization_data: None,
        created_at: 1234567892,
        updated_at: 1234567892,
    };

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
    let quote_id = crate::QuoteId::new_uuid();
    let saga = Saga {
        operation_id: uuid::Uuid::new_v4(),
        operation_kind: OperationKind::Melt,
        state: SagaStateEnum::Melt(MeltSagaState::SetupComplete),
        quote_id: Some(quote_id.to_string()),
        finalization_data: None,
        created_at: 1234567890,
        updated_at: 1234567890,
    };

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    let retrieved = db.get_melt_saga_by_quote_id(&quote_id).await.unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.operation_id, saga.operation_id);
    assert_eq!(retrieved.quote_id, Some(quote_id.to_string()));
}

/// Test melt-specific quote lookup ignores non-melt sagas with the same quote id
pub async fn get_melt_saga_by_quote_id_filters_non_melt<DB>(db: DB)
where
    DB: Database<Error>,
{
    let quote_id = crate::QuoteId::new_uuid();
    let swap_saga = Saga {
        operation_id: uuid::Uuid::new_v4(),
        operation_kind: OperationKind::Swap,
        state: SagaStateEnum::Swap(SwapSagaState::SetupComplete),
        quote_id: Some(quote_id.to_string()),
        finalization_data: None,
        created_at: 1234567890,
        updated_at: 1234567890,
    };
    let melt_saga = Saga {
        operation_id: uuid::Uuid::new_v4(),
        operation_kind: OperationKind::Melt,
        state: SagaStateEnum::Melt(MeltSagaState::SetupComplete),
        quote_id: Some(quote_id.to_string()),
        finalization_data: None,
        created_at: 1234567891,
        updated_at: 1234567891,
    };

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&swap_saga).await.unwrap();
    tx.add_saga(&melt_saga).await.unwrap();
    tx.commit().await.unwrap();

    let retrieved = db.get_melt_saga_by_quote_id(&quote_id).await.unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.operation_id, melt_saga.operation_id);
    assert_eq!(retrieved.quote_id, Some(quote_id.to_string()));
}

/// Test getting incomplete sagas for melt operation
pub async fn get_incomplete_melt_sagas<DB>(db: DB)
where
    DB: Database<Error>,
{
    let saga1 = Saga {
        operation_id: uuid::Uuid::new_v4(),
        operation_kind: OperationKind::Melt,
        state: SagaStateEnum::Melt(MeltSagaState::SetupComplete),
        quote_id: Some("melt_quote_1".to_string()),
        finalization_data: None,
        created_at: 1234567890,
        updated_at: 1234567890,
    };

    let saga2 = Saga {
        operation_id: uuid::Uuid::new_v4(),
        operation_kind: OperationKind::Melt,
        state: SagaStateEnum::Melt(MeltSagaState::PaymentAttempted),
        quote_id: Some("melt_quote_2".to_string()),
        finalization_data: None,
        created_at: 1234567891,
        updated_at: 1234567891,
    };

    let saga3 = Saga {
        operation_id: uuid::Uuid::new_v4(),
        operation_kind: OperationKind::Swap,
        state: SagaStateEnum::Swap(SwapSagaState::SetupComplete),
        quote_id: None,
        finalization_data: None,
        created_at: 1234567892,
        updated_at: 1234567892,
    };

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

/// Test updating non-existent saga fails gracefully
pub async fn update_nonexistent_saga<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let saga = tx.get_saga(&operation_id).await.unwrap();
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
    let saga = Saga {
        operation_id,
        operation_kind: OperationKind::Melt,
        state: SagaStateEnum::Melt(MeltSagaState::SetupComplete),
        quote_id: Some(quote_id.to_string()),
        finalization_data: None,
        created_at: 1234567890,
        updated_at: 1234567890,
    };

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_saga(&operation_id).await.unwrap().unwrap();
    assert_eq!(retrieved.quote_id, Some(quote_id.to_string()));
    tx.commit().await.unwrap();
}

/// Test saga transaction rollback
pub async fn saga_transaction_rollback<DB>(db: DB)
where
    DB: Database<Error>,
{
    let operation_id = uuid::Uuid::new_v4();
    let saga = Saga {
        operation_id,
        operation_kind: OperationKind::Swap,
        state: SagaStateEnum::Swap(SwapSagaState::SetupComplete),
        quote_id: None,
        finalization_data: None,
        created_at: 1234567890,
        updated_at: 1234567890,
    };

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
        Saga {
            operation_id: uuid::Uuid::new_v4(),
            operation_kind: OperationKind::Swap,
            state: SagaStateEnum::Swap(SwapSagaState::SetupComplete),
            quote_id: None,
            finalization_data: None,
            created_at: 1234567890,
            updated_at: 1234567890,
        },
        Saga {
            operation_id: uuid::Uuid::new_v4(),
            operation_kind: OperationKind::Swap,
            state: SagaStateEnum::Swap(SwapSagaState::Signed),
            quote_id: None,
            finalization_data: None,
            created_at: 1234567891,
            updated_at: 1234567891,
        },
        Saga {
            operation_id: uuid::Uuid::new_v4(),
            operation_kind: OperationKind::Melt,
            state: SagaStateEnum::Melt(MeltSagaState::SetupComplete),
            quote_id: Some("quote1".to_string()),
            finalization_data: None,
            created_at: 1234567892,
            updated_at: 1234567892,
        },
        Saga {
            operation_id: uuid::Uuid::new_v4(),
            operation_kind: OperationKind::Melt,
            state: SagaStateEnum::Melt(MeltSagaState::PaymentAttempted),
            quote_id: Some("quote2".to_string()),
            finalization_data: None,
            created_at: 1234567893,
            updated_at: 1234567893,
        },
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
        assert_eq!(retrieved.operation_kind, saga.operation_kind);
        assert_eq!(retrieved.state, saga.state);
        assert_eq!(retrieved.quote_id, saga.quote_id);
        assert_eq!(retrieved.finalization_data, saga.finalization_data);
        tx.commit().await.unwrap();
    }
}
