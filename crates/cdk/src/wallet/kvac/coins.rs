//! Operations on KVAC coins
use cashu_kvac::secp::GroupElement;
use cdk_common::kvac::{KvacCheckStateRequest, KvacCoin, KvacCoinState, KvacRandomizedCoin};
use tracing::instrument;

use crate::nuts::State;
use crate::{Error, Wallet};

impl Wallet {
    /// Get unspent proofs for mint
    #[instrument(skip(self))]
    pub async fn get_unspent_kvac_coins(&self) -> Result<Vec<KvacCoin>, Error> {
        self.get_kvac_coins_with(Some(vec![State::Unspent])).await
    }

    /// Get pending [`Proofs`]
    #[instrument(skip(self))]
    pub async fn get_pending_kvac_coins(&self) -> Result<Vec<KvacCoin>, Error> {
        self.get_kvac_coins_with(Some(vec![State::Pending])).await
    }

    /// Get reserved [`Proofs`]
    #[instrument(skip(self))]
    pub async fn get_reserved_kvac_coins(&self) -> Result<Vec<KvacCoin>, Error> {
        self.get_kvac_coins_with(Some(vec![State::Reserved])).await
    }

    /// Get this wallet's [`KvacCoin`]s that match the state
    pub async fn get_kvac_coins_with(
        &self,
        state: Option<Vec<State>>,
    ) -> Result<Vec<KvacCoin>, Error> {
        Ok(self
            .localstore
            .get_kvac_coins(
                Some(self.mint_url.clone()),
                Some(self.unit.clone()),
                state,
                None,
            )
            .await?
            .into_iter()
            .map(|c| c.coin)
            .collect())
    }

    /// Return proofs to unspent allowing them to be selected and spent
    #[instrument(skip(self))]
    pub async fn unreserve_kvac_coins(&self, nullifiers: &[GroupElement]) -> Result<(), Error> {
        Ok(self.localstore.set_unspent_kvac_coins(nullifiers).await?)
    }

    /// Check the state of a [`KvacCoin`] with the mint
    #[instrument(skip(self, coins))]
    pub async fn check_coins_spent(
        &self,
        coins: Vec<KvacCoin>,
    ) -> Result<Vec<KvacCoinState>, Error> {
        // Get the randomized coins
        let randomized_coins: Vec<KvacRandomizedCoin> =
            coins.iter().map(|c| KvacRandomizedCoin::from(c)).collect();

        // Get the nullifiers
        let nullifiers = randomized_coins.iter().map(|c| c.get_nullifier()).collect();

        // Call the endpoint
        let result = self
            .client
            .post_kvac_check_state(KvacCheckStateRequest { nullifiers })
            .await?;

        // Filter spent nullifiers
        let spent_nullifiers: Vec<_> = result
            .states
            .iter()
            .filter_map(|s| match s.state {
                State::Spent => Some(s.nullifier.clone()),
                _ => None,
            })
            .collect();

        self.localstore
            .update_kvac_coins(vec![], spent_nullifiers)
            .await?;

        Ok(result.states)
    }
    /*
    /// Reclaim unspent KVAC coins
    ///
    /// Checks the stats of [`KvacCoin`] swapping for a new [`KvacCoin`] if unspent
    #[instrument(skip(self, proofs))]
    pub async fn reclaim_unspent_coins(&self, proofs: Proofs) -> Result<(), Error> {
        let proof_ys = proofs.ys()?;

        let spendable = self
            .client
            .post_check_state(CheckStateRequest { ys: proof_ys })
            .await?
            .states;

        let unspent: Proofs = proofs
            .into_iter()
            .zip(spendable)
            .filter_map(|(p, s)| (s.state == State::Unspent).then_some(p))
            .collect();

        self.swap(None, SplitTarget::default(), unspent, None, false)
            .await?;

        Ok(())
    }

    /// Checks pending proofs for spent status
    #[instrument(skip(self))]
    pub async fn check_all_pending_proofs(&self) -> Result<Amount, Error> {
        let mut balance = Amount::ZERO;

        let proofs = self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit.clone()),
                Some(vec![State::Pending, State::Reserved]),
                None,
            )
            .await?;

        if proofs.is_empty() {
            return Ok(Amount::ZERO);
        }

        let states = self
            .check_proofs_spent(proofs.clone().into_iter().map(|p| p.proof).collect())
            .await?;

        // Both `State::Pending` and `State::Unspent` should be included in the pending
        // table. This is because a proof that has been crated to send will be
        // stored in the pending table in order to avoid accidentally double
        // spending but to allow it to be explicitly reclaimed
        let pending_states: HashSet<PublicKey> = states
            .into_iter()
            .filter(|s| s.state.ne(&State::Spent))
            .map(|s| s.y)
            .collect();

        let (pending_proofs, non_pending_proofs): (Vec<ProofInfo>, Vec<ProofInfo>) = proofs
            .into_iter()
            .partition(|p| pending_states.contains(&p.y));

        let amount = Amount::try_sum(pending_proofs.iter().map(|p| p.proof.amount))?;

        self.localstore
            .update_proofs(
                vec![],
                non_pending_proofs.into_iter().map(|p| p.y).collect(),
            )
            .await?;

        balance += amount;

        Ok(balance)
    }
    */
}
