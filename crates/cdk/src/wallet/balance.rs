use tracing::instrument;

use crate::nuts::nut00::ProofsMethods;
use crate::nuts::State;
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Total unspent balance of wallet
    ///
    /// Proofs on expired keysets are left out: the mint rejects them as inputs,
    /// so they cannot be spent. See
    /// [`total_expired_balance`](Self::total_expired_balance).
    #[instrument(skip(self))]
    pub async fn total_balance(&self) -> Result<Amount, Error> {
        if !self.expired_keyset_ids().await?.is_empty() {
            return Ok(self.get_unspent_proofs().await?.total_amount()?);
        }
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

    /// Total balance held on expired keysets
    ///
    /// The mint no longer accepts these proofs, so they are left out of
    /// [`total_balance`](Self::total_balance). This reports them separately so a
    /// caller can show funds that are stranded. It spans the unspent, pending
    /// and reserved states, so those three balances plus this one count every
    /// proof the wallet stores in one of them exactly once.
    #[instrument(skip(self))]
    pub async fn total_expired_balance(&self) -> Result<Amount, Error> {
        let expired = self.expired_keyset_ids().await?;
        if expired.is_empty() {
            return Ok(Amount::ZERO);
        }

        let proofs: Vec<_> = self
            .get_all_proofs_with(
                Some(vec![State::Unspent, State::Pending, State::Reserved]),
                None,
            )
            .await?
            .into_iter()
            .filter(|proof| expired.contains(&proof.keyset_id))
            .collect();

        Ok(proofs.total_amount()?)
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
