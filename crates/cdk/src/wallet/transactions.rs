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
    pub async fn list_transactions(
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
    pub async fn get_transaction(&self, id: TransactionId) -> Result<Option<Transaction>, Error> {
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
    pub(crate) async fn update_transaction_status_by_saga_id(
        &self,
        saga_id: uuid::Uuid,
        status: TransactionStatus,
    ) -> Result<bool, Error> {
        let transaction = self
            .localstore
            .list_transactions(Some(self.mint_url.clone()), None, Some(self.unit.clone()))
            .await?
            .into_iter()
            .find(|transaction| transaction.saga_id == Some(saga_id));

        let Some(mut transaction) = transaction else {
            return Ok(false);
        };

        transaction.status = status;
        self.localstore.add_transaction(transaction).await?;
        Ok(true)
    }

    /// Get proofs for a transaction by transaction ID
    ///
    /// This retrieves all proofs associated with a transaction by looking up
    /// the transaction's Y values and fetching the corresponding proofs.
    pub async fn get_proofs_for_transaction(&self, id: TransactionId) -> Result<Proofs, Error> {
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

    /// Revert a transaction by reclaiming unspent proofs.
    ///
    /// For transactions created by the saga pattern (with `saga_id` set), this
    /// function loads the associated send saga and calls `revoke()` on it, which
    /// properly handles the saga lifecycle including state transitions and cleanup.
    ///
    /// For legacy transactions (without `saga_id`), this function checks the proofs
    /// with the mint and marks any spent proofs accordingly. Unspent proofs are
    /// left in their current state for manual recovery.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The transaction is not found
    /// - The transaction is not outgoing
    /// - The saga is not in a revocable state (e.g., already completed)
    /// - The token has already been claimed by the recipient
    pub async fn revert_transaction(&self, id: TransactionId) -> Result<(), Error> {
        let tx = self
            .get_transaction(id)
            .await?
            .ok_or(Error::TransactionNotFound)?;

        if tx.direction != TransactionDirection::Outgoing {
            return Err(Error::InvalidTransactionDirection);
        }

        // Check if this is a saga-managed transaction
        if let Some(saga_id) = &tx.saga_id {
            // Use the existing revoke_send method which properly handles the saga
            // Discard the returned amount - we just care about success/failure
            let _ = self.revoke_send(*saga_id).await?;
            Ok(())
        } else {
            // Legacy transaction without saga - check proofs and mark spent ones
            // We don't attempt to swap for legacy transactions to avoid
            // interfering with any potential in-flight operations
            let pending_spent_proofs: Proofs = self
                .get_pending_spent_proofs()
                .await?
                .into_iter()
                .filter(|p| match p.y() {
                    Ok(y) => tx.ys.contains(&y),
                    Err(_) => false,
                })
                .collect();

            if pending_spent_proofs.is_empty() {
                return Ok(());
            }

            // Just check and mark spent - don't attempt swap for legacy transactions
            self.check_proofs_spent(pending_spent_proofs).await?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::str::FromStr;

    use cdk_common::mint_url::MintUrl;
    use cdk_common::nuts::{CurrencyUnit, State};
    use cdk_common::wallet::{ProofInfo, Transaction, TransactionDirection, TransactionStatus};
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
}
