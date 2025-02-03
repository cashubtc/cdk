use tracing::instrument;

use crate::nuts::nut00::ProofsMethods;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Total unspent balance of wallet
    #[instrument(skip(self))]
    pub async fn total_balance(&self) -> Result<Amount, Error> {
        Ok(self.get_unspent_proofs().await?.total_amount()?)
    }

    /// Total pending balance
    #[instrument(skip(self))]
    pub async fn total_pending_balance(&self) -> Result<Amount, Error> {
        Ok(self.get_pending_proofs().await?.total_amount()?)
    }

    /// Total reserved balance
    #[instrument(skip(self))]
    pub async fn total_reserved_balance(&self) -> Result<Amount, Error> {
        Ok(self.get_reserved_proofs().await?.total_amount()?)
    }
}
