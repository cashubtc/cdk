use std::collections::HashMap;

use tracing::instrument;

use crate::nuts::nut00::ProofsMethods;
use crate::nuts::CurrencyUnit;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Total unspent balance of wallet
    #[instrument(skip(self))]
    pub async fn total_balance(&self) -> Result<Amount, Error> {
        Ok(self.get_unspent_proofs().await?.total_amount()?)
    }

    /// Total pending balance
    #[instrument(skip(self))]
    pub async fn total_pending_balance(&self) -> Result<HashMap<CurrencyUnit, Amount>, Error> {
        let proofs = self.get_pending_proofs().await?;

        // TODO If only the proofs for this wallet's unit are retrieved, why build a map with key = unit?
        let balances = proofs.iter().fold(HashMap::new(), |mut acc, proof| {
            *acc.entry(self.unit.clone()).or_insert(Amount::ZERO) += proof.amount;
            acc
        });

        Ok(balances)
    }

    /// Total reserved balance
    #[instrument(skip(self))]
    pub async fn total_reserved_balance(&self) -> Result<HashMap<CurrencyUnit, Amount>, Error> {
        let proofs = self.get_reserved_proofs().await?;

        // TODO If only the proofs for this wallet's unit are retrieved, why build a map with key = unit?
        let balances = proofs.iter().fold(HashMap::new(), |mut acc, proof| {
            *acc.entry(self.unit.clone()).or_insert(Amount::ZERO) += proof.amount;
            acc
        });

        Ok(balances)
    }
}
