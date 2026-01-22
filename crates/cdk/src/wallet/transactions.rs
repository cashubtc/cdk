use cdk_common::wallet::{Transaction, TransactionDirection, TransactionId};
use cdk_common::Proofs;

use crate::{Error, Wallet};

impl Wallet {
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

        Ok(transaction)
    }

    /// Get proofs for a transaction by transaction ID
    ///
    /// This retrieves all proofs associated with a transaction by looking up
    /// the transaction's Y values and fetching the corresponding proofs.
    pub async fn get_proofs_for_transaction(&self, id: TransactionId) -> Result<Proofs, Error> {
        let transaction = self
            .localstore
            .get_transaction(id)
            .await?
            .ok_or(Error::TransactionNotFound)?;

        let proofs = self
            .localstore
            .get_proofs_by_ys(transaction.ys)
            .await?
            .into_iter()
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
            .localstore
            .get_transaction(id)
            .await?
            .ok_or(Error::TransactionNotFound)?;

        if tx.direction != TransactionDirection::Outgoing {
            return Err(Error::InvalidTransactionDirection);
        }

        // Check if this is a saga-managed transaction
        if let Some(saga_id_str) = &tx.saga_id {
            let saga_id = uuid::Uuid::parse_str(saga_id_str)
                .map_err(|e| Error::Custom(format!("Invalid saga ID: {}", e)))?;

            // Use the existing revoke_send method which properly handles the saga
            // Discard the returned amount - we just care about success/failure
            let _ = self.revoke_send(saga_id).await?;
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
