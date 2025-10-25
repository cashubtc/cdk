use tracing::instrument;

use crate::nuts::nut00::ProofsMethods;
use crate::nuts::State;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Total unspent balance of wallet
    #[instrument(skip(self))]
    pub async fn total_balance(&self) -> Result<Amount, Error> {
        // Use the efficient balance query instead of fetching all proofs
        let balance = self
            .localstore
            .get_balance(
                Some(self.mint_url.clone()),
                Some(self.unit.clone()),
                Some(vec![State::Unspent]),
            )
            .await?;
        Ok(Amount::from(balance))
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
