use cdk_common::wallet::{Transaction, TransactionDirection, TransactionId, TransactionStatus};
use cdk_common::Proofs;

use crate::{Error, Wallet};

impl Wallet {
    fn transaction_matches_wallet(&self, transaction: &Transaction) -> bool {
        transaction.matches_conditions(
            &Some(self.mint_url.clone()),
            &None,
            &Some(self.unit.clone()),
        )
    }

    /// List transactions
    pub(crate) async fn list_transactions(
        &self,
        direction: Option<TransactionDirection>,
    ) -> Result<Vec<Transaction>, Error> {
        let mut transactions = self
            .localstore
            .list_transactions(
                Some(self.mint_url.clone()),
                direction,
                Some(self.unit.clone()),
            )
            .await?;

        transactions.sort();

        Ok(transactions)
    }

    /// Get transaction by ID
    pub(crate) async fn get_transaction(
        &self,
        id: TransactionId,
    ) -> Result<Option<Transaction>, Error> {
        let transaction = self.localstore.get_transaction(id).await?;

        Ok(transaction.filter(|transaction| self.transaction_matches_wallet(transaction)))
    }

    /// Store a transaction without changing its original creation timestamp.
    pub(crate) async fn upsert_transaction(
        &self,
        mut transaction: Transaction,
    ) -> Result<(), Error> {
        if let Some(existing) = self.localstore.get_transaction(transaction.id()).await? {
            transaction.timestamp = existing.timestamp;
        }

        self.localstore.add_transaction(transaction).await?;
        Ok(())
    }

    /// Update the status of the transaction associated with a saga.
    ///
    /// Returns `true` when the requested status was applied to at least one
    /// matching transaction, or a matching transaction already had it.
    /// Returns `false` when no transaction matches the saga, or every match
    /// was already in a terminal state and the update was ignored.
    pub(crate) async fn update_transaction_status_by_saga_id(
        &self,
        saga_id: uuid::Uuid,
        status: TransactionStatus,
    ) -> Result<bool, Error> {
        let transactions = self.transactions_for_operation(saga_id).await?;

        if transactions.is_empty() {
            return Ok(false);
        }

        let mut applied = false;
        for mut transaction in transactions {
            if transaction.status == status {
                applied = true;
                continue;
            }

            if transaction.status != TransactionStatus::Pending {
                tracing::warn!(
                    saga_id = %saga_id,
                    current_status = %transaction.status,
                    requested_status = %status,
                    "Ignoring transaction status change from terminal state"
                );
                continue;
            }

            transaction.status = status;
            self.localstore.add_transaction(transaction).await?;
            applied = true;
        }
        Ok(applied)
    }

    /// Load a workflow's transactions, including legacy proof-derived IDs.
    pub(crate) async fn transactions_for_operation(
        &self,
        operation_id: uuid::Uuid,
    ) -> Result<Vec<Transaction>, Error> {
        if let Some(transaction) = self
            .get_transaction(TransactionId::from_saga_id(operation_id))
            .await?
            .filter(|transaction| transaction.saga_id == Some(operation_id))
        {
            return Ok(vec![transaction]);
        }

        Ok(self
            .list_transactions(None)
            .await?
            .into_iter()
            .filter(|transaction| transaction.saga_id == Some(operation_id))
            .collect())
    }

    /// Mark a saga transaction as failed before compensating it.
    ///
    /// Persistence errors are propagated so compensation cannot delete the
    /// saga before its transaction reaches a durable terminal state.
    pub(crate) async fn mark_transaction_failed(&self, saga_id: uuid::Uuid) -> Result<(), Error> {
        self.update_transaction_status_by_saga_id(saga_id, TransactionStatus::Failed)
            .await?;
        Ok(())
    }

    /// Get proofs for a transaction by transaction ID
    ///
    /// This retrieves all proofs associated with a transaction by looking up
    /// the transaction's Y values and fetching the corresponding proofs.
    pub(crate) async fn get_proofs_for_transaction(
        &self,
        id: TransactionId,
    ) -> Result<Proofs, Error> {
        let transaction = self
            .get_transaction(id)
            .await?
            .ok_or(Error::TransactionNotFound)?;

        let mint_url = Some(self.mint_url.clone());
        let unit = Some(self.unit.clone());

        let proofs = self
            .localstore
            .get_proofs_by_ys(transaction.ys)
            .await?
            .into_iter()
            .filter(|proof_info| proof_info.matches_conditions(&mint_url, &unit, &None, &None))
            .map(|p| p.proof)
            .collect();

        Ok(proofs)
    }

    /// Reconcile an outgoing transaction and return reclaimed saga value.
    ///
    /// Saga-backed sends are reclaimed through their durable operation. Legacy
    /// transactions only have their pending proofs checked with the mint; this
    /// intentionally preserves the historical non-swapping behavior.
    pub(crate) async fn recover_outgoing_transaction(
        &self,
        id: TransactionId,
    ) -> Result<Option<crate::Amount>, Error> {
        let transaction = self
            .get_transaction(id)
            .await?
            .ok_or(Error::TransactionNotFound)?;

        if transaction.direction != TransactionDirection::Outgoing {
            return Err(Error::InvalidTransactionDirection);
        }

        match transaction.saga_id {
            Some(saga_id) => self.revoke_send(saga_id).await.map(Some),
            None => {
                let pending = self
                    .get_proofs_with(Some(vec![crate::nuts::State::PendingSpent]), None)
                    .await?
                    .into_iter()
                    .filter(|proof| {
                        proof
                            .y()
                            .map(|y| transaction.ys.contains(&y))
                            .unwrap_or(false)
                    })
                    .collect::<Proofs>();
                if !pending.is_empty() {
                    self.check_proofs_spent(pending).await?;
                }
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::str::FromStr;

    use cdk_common::mint_url::MintUrl;
    use cdk_common::nuts::{CurrencyUnit, State};
    use cdk_common::wallet::{
        ProofInfo, Transaction, TransactionDirection, TransactionId, TransactionStatus,
    };
    use cdk_common::Amount;

    use crate::wallet::test_utils::{
        create_test_db, create_test_wallet, test_keyset_id, test_proof,
    };

    #[tokio::test]
    async fn get_proofs_for_transaction_does_not_leak_other_mints_proofs() {
        let db = create_test_db().await;
        let wallet = create_test_wallet(db.clone()).await;

        let mint_b =
            MintUrl::from_str("https://other-mint.example.com").expect("mint URL should be valid");
        let proof_b = test_proof(test_keyset_id(), 100);
        let proof_b_y = proof_b.y().expect("test proof should derive a Y value");
        let proof_info_b =
            ProofInfo::new(proof_b, mint_b.clone(), State::Unspent, CurrencyUnit::Sat)
                .expect("proof info should be valid");
        db.update_proofs(vec![proof_info_b], vec![])
            .await
            .expect("proof should be stored");

        let tx_b = Transaction {
            mint_url: mint_b,
            direction: TransactionDirection::Outgoing,
            amount: Amount::from(100_u64),
            fee: Amount::from(0_u64),
            unit: CurrencyUnit::Sat,
            ys: vec![proof_b_y],
            timestamp: 0,
            memo: None,
            metadata: HashMap::new(),
            quote_id: None,
            payment_request: None,
            payment_proof: None,
            payment_method: None,
            saga_id: None,
            status: TransactionStatus::Completed,
        };
        let tx_b_id = tx_b.id();
        db.add_transaction(tx_b)
            .await
            .expect("transaction should be stored");

        let returned = wallet.get_proofs_for_transaction(tx_b_id).await;

        assert!(
            matches!(returned, Err(crate::Error::TransactionNotFound)),
            "wallet returned proofs for another mint's transaction: {:?}",
            returned.map(|proofs| proofs.len())
        );
    }

    #[tokio::test]
    async fn terminal_transaction_status_cannot_be_overwritten() {
        let db = create_test_db().await;
        let wallet = create_test_wallet(db.clone()).await;
        let saga_id = uuid::Uuid::new_v4();

        let transaction = Transaction {
            mint_url: wallet.mint_url.clone(),
            direction: TransactionDirection::Incoming,
            amount: Amount::from(100_u64),
            fee: Amount::ZERO,
            unit: wallet.unit.clone(),
            ys: vec![],
            timestamp: 0,
            memo: None,
            metadata: HashMap::new(),
            quote_id: None,
            payment_request: None,
            payment_proof: None,
            payment_method: None,
            saga_id: Some(saga_id),
            status: TransactionStatus::Pending,
        };
        db.add_transaction(transaction)
            .await
            .expect("transaction should be stored");

        assert!(wallet
            .update_transaction_status_by_saga_id(saga_id, TransactionStatus::Completed)
            .await
            .expect("pending status should update"));
        assert!(!wallet
            .update_transaction_status_by_saga_id(saga_id, TransactionStatus::Failed)
            .await
            .expect("terminal status update should be ignored"));

        let transaction = db
            .get_transaction(TransactionId::from_saga_id(saga_id))
            .await
            .expect("transaction lookup should succeed")
            .expect("transaction should exist");
        assert_eq!(transaction.status, TransactionStatus::Completed);
    }

    fn saga_transaction(
        wallet: &crate::Wallet,
        saga_id: uuid::Uuid,
        status: TransactionStatus,
        batch_quote_id: Option<&str>,
    ) -> Transaction {
        let mut metadata = HashMap::new();
        if let Some(quote_id) = batch_quote_id {
            metadata.insert("batch_quote_id".to_string(), quote_id.to_string());
        }

        Transaction {
            mint_url: wallet.mint_url.clone(),
            direction: TransactionDirection::Incoming,
            amount: Amount::from(100_u64),
            fee: Amount::ZERO,
            unit: wallet.unit.clone(),
            ys: vec![],
            timestamp: 0,
            memo: None,
            metadata,
            quote_id: None,
            payment_request: None,
            payment_proof: None,
            payment_method: None,
            saga_id: Some(saga_id),
            status,
        }
    }

    #[tokio::test]
    async fn status_update_returns_false_for_unknown_saga() {
        let db = create_test_db().await;
        let wallet = create_test_wallet(db.clone()).await;

        assert!(!wallet
            .update_transaction_status_by_saga_id(
                uuid::Uuid::new_v4(),
                TransactionStatus::Completed
            )
            .await
            .expect("update for unknown saga should succeed"));
    }

    #[tokio::test]
    async fn batch_quote_transactions_are_updated_via_saga_fallback() {
        let db = create_test_db().await;
        let wallet = create_test_wallet(db.clone()).await;
        let saga_id = uuid::Uuid::new_v4();

        // Batch-quote transactions derive their ID from the saga ID and quote
        // ID, so the direct `from_saga_id` lookup misses and the saga-ID
        // fallback scan must find them.
        for quote_id in ["quote-a", "quote-b"] {
            let transaction =
                saga_transaction(&wallet, saga_id, TransactionStatus::Pending, Some(quote_id));
            assert_ne!(
                transaction.id(),
                TransactionId::from_saga_id(saga_id),
                "batch quote transaction must not use the plain saga ID"
            );
            db.add_transaction(transaction)
                .await
                .expect("transaction should be stored");
        }

        assert!(wallet
            .update_transaction_status_by_saga_id(saga_id, TransactionStatus::Completed)
            .await
            .expect("batch quote transactions should update"));

        for quote_id in ["quote-a", "quote-b"] {
            let transaction = db
                .get_transaction(TransactionId::from_batch_quote(saga_id, quote_id))
                .await
                .expect("transaction lookup should succeed")
                .expect("transaction should exist");
            assert_eq!(transaction.status, TransactionStatus::Completed);
        }
    }

    #[tokio::test]
    async fn mixed_terminal_and_pending_transactions_update_only_pending() {
        let db = create_test_db().await;
        let wallet = create_test_wallet(db.clone()).await;
        let saga_id = uuid::Uuid::new_v4();

        db.add_transaction(saga_transaction(
            &wallet,
            saga_id,
            TransactionStatus::Completed,
            Some("quote-done"),
        ))
        .await
        .expect("transaction should be stored");
        db.add_transaction(saga_transaction(
            &wallet,
            saga_id,
            TransactionStatus::Pending,
            Some("quote-pending"),
        ))
        .await
        .expect("transaction should be stored");

        assert!(wallet
            .update_transaction_status_by_saga_id(saga_id, TransactionStatus::Failed)
            .await
            .expect("pending transaction should update"));

        let terminal = db
            .get_transaction(TransactionId::from_batch_quote(saga_id, "quote-done"))
            .await
            .expect("transaction lookup should succeed")
            .expect("transaction should exist");
        assert_eq!(
            terminal.status,
            TransactionStatus::Completed,
            "terminal transaction must not be overwritten"
        );

        let pending = db
            .get_transaction(TransactionId::from_batch_quote(saga_id, "quote-pending"))
            .await
            .expect("transaction lookup should succeed")
            .expect("transaction should exist");
        assert_eq!(pending.status, TransactionStatus::Failed);
    }

    #[tokio::test]
    async fn status_update_returns_false_when_all_matches_are_terminal() {
        let db = create_test_db().await;
        let wallet = create_test_wallet(db.clone()).await;
        let saga_id = uuid::Uuid::new_v4();

        for quote_id in ["quote-a", "quote-b"] {
            db.add_transaction(saga_transaction(
                &wallet,
                saga_id,
                TransactionStatus::Completed,
                Some(quote_id),
            ))
            .await
            .expect("transaction should be stored");
        }

        assert!(!wallet
            .update_transaction_status_by_saga_id(saga_id, TransactionStatus::Failed)
            .await
            .expect("terminal status updates should be ignored"));
    }

    #[tokio::test]
    async fn status_update_to_current_status_reports_applied() {
        let db = create_test_db().await;
        let wallet = create_test_wallet(db.clone()).await;
        let saga_id = uuid::Uuid::new_v4();

        db.add_transaction(saga_transaction(
            &wallet,
            saga_id,
            TransactionStatus::Completed,
            None,
        ))
        .await
        .expect("transaction should be stored");

        assert!(wallet
            .update_transaction_status_by_saga_id(saga_id, TransactionStatus::Completed)
            .await
            .expect("matching status should report applied"));

        let transaction = db
            .get_transaction(TransactionId::from_saga_id(saga_id))
            .await
            .expect("transaction lookup should succeed")
            .expect("transaction should exist");
        assert_eq!(transaction.status, TransactionStatus::Completed);
    }
}
