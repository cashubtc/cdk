use cdk_common::wallet::{Transaction, TransactionDirection, TransactionId};

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

    /// Revert a transaction
    pub async fn revert_transaction(&self, id: TransactionId) -> Result<bool, Error> {
        let tx = self
            .localstore
            .get_transaction(id)
            .await?
            .ok_or(Error::TransactionNotFound)?;

        if tx.direction != TransactionDirection::Outgoing {
            return Err(Error::InvalidTransactionDirection);
        }

        let pending_spent_proofs = self
            .get_pending_spent_proofs()
            .await?
            .into_iter()
            .filter(|p| match p.y() {
                Ok(y) => tx.ys.contains(&y),
                Err(_) => false,
            })
            .collect::<Vec<_>>();

        Ok(self.reclaim_unspent(pending_spent_proofs).await.is_ok())
    }
}
