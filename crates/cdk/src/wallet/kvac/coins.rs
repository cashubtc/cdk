//! Operations on KVAC coins
use cashu_kvac::secp::Scalar;
use cdk_common::kvac::KvacCoin;
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

    /// Get this wallet's [`KvacCoin`]s that match the args
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
    pub async fn unreserve_kvac_coins(&self, ts: &[Scalar]) -> Result<(), Error> {
        Ok(self.localstore.set_unspent_kvac_coins(ts).await?)
    }

    /*
    /// Reclaim unspent proofs
    ///
    /// Checks the stats of [`Proofs`] swapping for a new [`Proof`] if unspent
    #[instrument(skip(self, proofs))]
    pub async fn reclaim_unspent(&self, proofs: Proofs) -> Result<(), Error> {
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

    /// NUT-07 Check the state of a [`Proof`] with the mint
    #[instrument(skip(self, proofs))]
    pub async fn check_proofs_spent(&self, proofs: Proofs) -> Result<Vec<ProofState>, Error> {
        let spendable = self
            .client
            .post_check_state(CheckStateRequest { ys: proofs.ys()? })
            .await?;
        let spent_ys: Vec<_> = spendable
            .states
            .iter()
            .filter_map(|p| match p.state {
                State::Spent => Some(p.y),
                _ => None,
            })
            .collect();

        self.localstore.update_proofs(vec![], spent_ys).await?;

        Ok(spendable.states)
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
