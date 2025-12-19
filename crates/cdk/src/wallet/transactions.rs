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

    /// Revert a transaction
    pub async fn revert_transaction(&self, id: TransactionId) -> Result<(), Error> {
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

        self.reclaim_unspent(pending_spent_proofs).await?;
        Ok(())
    }
}
