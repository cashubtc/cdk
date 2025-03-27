use cdk_common::{
    mint_url::MintUrl,
    wallet::{Transaction, TransactionDirection, TransactionId},
    CurrencyUnit,
};

use crate::{Error, Wallet};

impl Wallet {
    /// List transactions
    pub async fn list_transactions(
        &self,
        mint_url: Option<MintUrl>,
        direction: Option<TransactionDirection>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Vec<Transaction>, Error> {
        let transactions = self
            .localstore
            .list_transactions(mint_url, direction, unit)
            .await?;

        Ok(transactions)
    }

    /// Get transaction by ID
    pub async fn get_transaction(&self, id: TransactionId) -> Result<Option<Transaction>, Error> {
        let transaction = self.localstore.get_transaction(id).await?;

        Ok(transaction)
    }
}
