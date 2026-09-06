use serde::{Deserialize, Serialize};
#[cfg(any(feature = "nwc", test))]
use tracing::instrument;

use super::{WalletIdentity, WalletManager};
use crate::nuts::{CurrencyUnit, State};
use crate::{Amount, Error, Wallet};

#[cfg(any(feature = "nwc", test))]
impl Wallet {
    /// Total unspent balance of wallet
    #[cfg(any(feature = "nwc", test))]
    #[instrument(skip(self))]
    pub(crate) async fn total_balance(&self) -> Result<Amount, Error> {
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

    /// Total reserved balance
    #[cfg(test)]
    #[instrument(skip(self))]
    pub(crate) async fn total_reserved_balance(&self) -> Result<Amount, Error> {
        let balance = self
            .localstore
            .get_balance(
                Some(self.mint_url.clone()),
                Some(self.unit.clone()),
                Some(vec![State::Reserved]),
            )
            .await?;
        Ok(Amount::from(balance))
    }
}

/// Balances grouped by spendability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WalletBalance {
    /// Funds that can be spent immediately.
    pub available: Amount,
    /// Funds committed to an in-flight payment or an unclaimed send.
    pub pending: Amount,
    /// Funds held by a prepared operation awaiting confirmation or cancellation.
    pub reserved: Amount,
}

impl Wallet {
    /// Read available, pending, and reserved balances from one local snapshot.
    pub async fn balance(&self) -> Result<WalletBalance, Error> {
        let proofs = self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit.clone()),
                Some(vec![
                    State::Unspent,
                    State::Reserved,
                    State::Pending,
                    State::PendingSpent,
                ]),
                None,
            )
            .await?;

        let mut balance = WalletBalance::default();
        for proof in proofs {
            let bucket = match proof.state {
                State::Unspent => &mut balance.available,
                State::Reserved => &mut balance.reserved,
                State::Pending | State::PendingSpent => &mut balance.pending,
                State::Spent => continue,
            };
            *bucket = bucket
                .checked_add(proof.proof.amount)
                .ok_or(Error::AmountOverflow)?;
        }
        Ok(balance)
    }
}

impl WalletManager {
    /// Read balances for every configured mint wallet.
    pub async fn balances(&self) -> Result<Vec<(WalletIdentity, WalletBalance)>, Error> {
        let wallets = self.get_wallets().await;
        let mut balances = Vec::with_capacity(wallets.len());
        for wallet in wallets {
            balances.push((wallet.identity(), wallet.balance().await?));
        }
        Ok(balances)
    }

    /// Read immediately spendable balances keyed by wallet identity.
    pub async fn available_balances(
        &self,
    ) -> Result<std::collections::BTreeMap<WalletIdentity, Amount>, Error> {
        Ok(self
            .balances()
            .await?
            .into_iter()
            .map(|(identity, balance)| (identity, balance.available))
            .collect())
    }

    /// Sum available balances across mints, grouped by currency unit.
    pub async fn balance_totals(
        &self,
    ) -> Result<std::collections::BTreeMap<CurrencyUnit, Amount>, Error> {
        let mut totals = std::collections::BTreeMap::new();
        for (identity, balance) in self.balances().await? {
            let total = totals.entry(identity.unit).or_insert(Amount::ZERO);
            *total = total
                .checked_add(balance.available)
                .ok_or(Error::AmountOverflow)?;
        }
        Ok(totals)
    }
}
