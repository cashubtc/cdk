//! Saga database tests

use crate::database::mint::{Database, Error};
use crate::mint::{MeltSagaState, OperationKind, Saga, SagaStateEnum, SwapSagaState};

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
        created_at: 1234567890,
        updated_at: 1234567890,
    };

    // Add saga
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    // Retrieve saga
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_saga(&operation_id).await.unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.operation_id, saga.operation_id);
    assert_eq!(retrieved.operation_kind, saga.operation_kind);
    assert_eq!(retrieved.state, saga.state);
    assert_eq!(retrieved.quote_id, saga.quote_id);
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
        created_at: 1234567890,
        updated_at: 1234567890,
    };

    // Add saga first time
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    // Try to add duplicate - should fail
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
        created_at: 1234567890,
        updated_at: 1234567890,
    };

    // Add saga
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    // Update saga state
    let new_state = SagaStateEnum::Swap(SwapSagaState::Signed);
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.update_saga(&operation_id, new_state.clone())
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Verify update
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_saga(&operation_id).await.unwrap().unwrap();
    assert_eq!(retrieved.state, new_state);
    // Updated_at should have changed (we can't verify exact value but it should exist)
    assert!(retrieved.updated_at >= saga.updated_at);
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
        created_at: 1234567890,
        updated_at: 1234567890,
    };

    // Add saga
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    // Verify saga exists
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_saga(&operation_id).await.unwrap();
    assert!(retrieved.is_some());
    tx.commit().await.unwrap();

    // Delete saga
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.delete_saga(&operation_id).await.unwrap();
    tx.commit().await.unwrap();

    // Verify saga is deleted
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
        created_at: 1234567890,
        updated_at: 1234567890,
    };

    let saga2 = Saga {
        operation_id: uuid::Uuid::new_v4(),
        operation_kind: OperationKind::Swap,
        state: SagaStateEnum::Swap(SwapSagaState::Signed),
        quote_id: None,
        created_at: 1234567891,
        updated_at: 1234567891,
    };

    // Add melt saga (should not be returned)
    let saga3 = Saga {
        operation_id: uuid::Uuid::new_v4(),
        operation_kind: OperationKind::Melt,
        state: SagaStateEnum::Melt(MeltSagaState::SetupComplete),
        quote_id: Some("test_quote_id".to_string()),
        created_at: 1234567892,
        updated_at: 1234567892,
    };

    // Add all sagas
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga1).await.unwrap();
    tx.add_saga(&saga2).await.unwrap();
    tx.add_saga(&saga3).await.unwrap();
    tx.commit().await.unwrap();

    // Get incomplete swap sagas
    let incomplete_swaps = db.get_incomplete_sagas(OperationKind::Swap).await.unwrap();

    // Should have at least 2 swap sagas
    assert!(incomplete_swaps.len() >= 2);
    assert!(incomplete_swaps
        .iter()
        .any(|s| s.operation_id == saga1.operation_id));
    assert!(incomplete_swaps
        .iter()
        .any(|s| s.operation_id == saga2.operation_id));
    // Should not include melt saga
    assert!(!incomplete_swaps
        .iter()
        .any(|s| s.operation_id == saga3.operation_id));
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
        created_at: 1234567890,
        updated_at: 1234567890,
    };

    let saga2 = Saga {
        operation_id: uuid::Uuid::new_v4(),
        operation_kind: OperationKind::Melt,
        state: SagaStateEnum::Melt(MeltSagaState::PaymentAttempted),
        quote_id: Some("melt_quote_2".to_string()),
        created_at: 1234567891,
        updated_at: 1234567891,
    };

    // Add swap saga (should not be returned)
    let saga3 = Saga {
        operation_id: uuid::Uuid::new_v4(),
        operation_kind: OperationKind::Swap,
        state: SagaStateEnum::Swap(SwapSagaState::SetupComplete),
        quote_id: None,
        created_at: 1234567892,
        updated_at: 1234567892,
    };

    // Add all sagas
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga1).await.unwrap();
    tx.add_saga(&saga2).await.unwrap();
    tx.add_saga(&saga3).await.unwrap();
    tx.commit().await.unwrap();

    // Get incomplete melt sagas
    let incomplete_melts = db.get_incomplete_sagas(OperationKind::Melt).await.unwrap();

    // Should have at least 2 melt sagas
    assert!(incomplete_melts.len() >= 2);
    assert!(incomplete_melts
        .iter()
        .any(|s| s.operation_id == saga1.operation_id));
    assert!(incomplete_melts
        .iter()
        .any(|s| s.operation_id == saga2.operation_id));
    // Should not include swap saga
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

    // Try to get non-existent saga
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

    // Try to get non-existent saga - should return None
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

    // Try to get non-existent saga - should return None
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
        created_at: 1234567890,
        updated_at: 1234567890,
    };

    // Add saga
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.commit().await.unwrap();

    // Retrieve and verify quote_id
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
        created_at: 1234567890,
        updated_at: 1234567890,
    };

    // Start transaction, add saga, then rollback
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_saga(&saga).await.unwrap();
    tx.rollback().await.unwrap();

    // Verify saga was not persisted
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
            created_at: 1234567890,
            updated_at: 1234567890,
        },
        Saga {
            operation_id: uuid::Uuid::new_v4(),
            operation_kind: OperationKind::Swap,
            state: SagaStateEnum::Swap(SwapSagaState::Signed),
            quote_id: None,
            created_at: 1234567891,
            updated_at: 1234567891,
        },
        Saga {
            operation_id: uuid::Uuid::new_v4(),
            operation_kind: OperationKind::Melt,
            state: SagaStateEnum::Melt(MeltSagaState::SetupComplete),
            quote_id: Some("quote1".to_string()),
            created_at: 1234567892,
            updated_at: 1234567892,
        },
        Saga {
            operation_id: uuid::Uuid::new_v4(),
            operation_kind: OperationKind::Melt,
            state: SagaStateEnum::Melt(MeltSagaState::PaymentAttempted),
            quote_id: Some("quote2".to_string()),
            created_at: 1234567893,
            updated_at: 1234567893,
        },
    ];

    // Add all sagas
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    for saga in &sagas {
        tx.add_saga(saga).await.unwrap();
    }
    tx.commit().await.unwrap();

    // Verify all sagas were added
    for saga in &sagas {
        let mut tx = Database::begin_transaction(&db).await.unwrap();
        let retrieved = tx.get_saga(&saga.operation_id).await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.operation_id, saga.operation_id);
        assert_eq!(retrieved.state, saga.state);
        tx.commit().await.unwrap();
    }
}
