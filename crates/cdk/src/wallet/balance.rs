use std::collections::HashMap;

use tracing::instrument;

use crate::{
    nuts::{CurrencyUnit, State},
    Amount, Error, Wallet,
};

impl Wallet {
    /// Total unspent balance of wallet
    #[instrument(skip(self))]
    pub async fn total_balance(&self) -> Result<Amount, Error> {
        let proofs = self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit),
                Some(vec![State::Unspent]),
                None,
            )
            .await?;
        let balance = Amount::try_sum(proofs.iter().map(|p| p.proof.amount))?;

        Ok(balance)
    }

    /// Total pending balance
    #[instrument(skip(self))]
    pub async fn total_pending_balance(&self) -> Result<HashMap<CurrencyUnit, Amount>, Error> {
        let proofs = self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit),
                Some(vec![State::Pending]),
                None,
            )
            .await?;

        let balances = proofs.iter().fold(HashMap::new(), |mut acc, proof| {
            *acc.entry(proof.unit).or_insert(Amount::ZERO) += proof.proof.amount;
            acc
        });

        Ok(balances)
    }

    /// Total reserved balance
    #[instrument(skip(self))]
    pub async fn total_reserved_balance(&self) -> Result<HashMap<CurrencyUnit, Amount>, Error> {
        let proofs = self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit),
                Some(vec![State::Reserved]),
                None,
            )
            .await?;

        let balances = proofs.iter().fold(HashMap::new(), |mut acc, proof| {
            *acc.entry(proof.unit).or_insert(Amount::ZERO) += proof.proof.amount;
            acc
        });

        Ok(balances)
    }
}
