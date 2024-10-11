use std::collections::HashSet;

use tracing::instrument;

use crate::{
    amount::SplitTarget,
    dhke::hash_to_curve,
    nuts::{Proof, ProofState, Proofs, PublicKey, State},
    types::ProofInfo,
    Amount, Error, Wallet,
};

impl Wallet {
    /// Get unspent proofs for mint
    #[instrument(skip(self))]
    pub async fn get_proofs(&self) -> Result<Proofs, Error> {
        Ok(self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit),
                Some(vec![State::Unspent]),
                None,
            )
            .await?
            .into_iter()
            .map(|p| p.proof)
            .collect())
    }

    /// Get pending [`Proofs`]
    #[instrument(skip(self))]
    pub async fn get_pending_proofs(&self) -> Result<Proofs, Error> {
        Ok(self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit),
                Some(vec![State::Pending]),
                None,
            )
            .await?
            .into_iter()
            .map(|p| p.proof)
            .collect())
    }

    /// Get reserved [`Proofs`]
    #[instrument(skip(self))]
    pub async fn get_reserved_proofs(&self) -> Result<Proofs, Error> {
        Ok(self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit),
                Some(vec![State::Reserved]),
                None,
            )
            .await?
            .into_iter()
            .map(|p| p.proof)
            .collect())
    }

    /// Return proofs to unspent allowing them to be selected and spent
    #[instrument(skip(self))]
    pub async fn unreserve_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Error> {
        Ok(self.localstore.set_unspent_proofs(ys).await?)
    }

    /// Reclaim unspent proofs
    ///
    /// Checks the stats of [`Proofs`] swapping for a new [`Proof`] if unspent
    #[instrument(skip(self, proofs))]
    pub async fn reclaim_unspent(&self, proofs: Proofs) -> Result<(), Error> {
        let proof_ys = proofs
            .iter()
            // Find Y for the secret
            .map(|p| hash_to_curve(p.secret.as_bytes()))
            .collect::<Result<Vec<PublicKey>, _>>()?;

        let spendable = self
            .client
            .post_check_state(self.mint_url.clone().try_into()?, proof_ys)
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
            .post_check_state(
                self.mint_url.clone().try_into()?,
                proofs
                    .iter()
                    // Find Y for the secret
                    .map(|p| hash_to_curve(p.secret.as_bytes()))
                    .collect::<Result<Vec<PublicKey>, _>>()?,
            )
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
                Some(self.unit),
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

    /// Select proofs to send
    #[instrument(skip_all)]
    pub async fn select_proofs_to_send(
        &self,
        amount: Amount,
        proofs: Proofs,
        include_fees: bool,
    ) -> Result<Proofs, Error> {
        // TODO: Check all proofs are same unit

        if Amount::try_sum(proofs.iter().map(|p| p.amount))? < amount {
            return Err(Error::InsufficientFunds);
        }

        let (mut proofs_larger, mut proofs_smaller): (Proofs, Proofs) =
            proofs.into_iter().partition(|p| p.amount > amount);

        let next_bigger_proof = proofs_larger.first().cloned();

        let mut selected_proofs: Proofs = Vec::new();
        let mut remaining_amount = amount;

        while remaining_amount > Amount::ZERO {
            proofs_larger.sort();
            // Sort smaller proofs in descending order
            proofs_smaller.sort_by(|a: &Proof, b: &Proof| b.cmp(a));

            let selected_proof = if let Some(next_small) = proofs_smaller.clone().first() {
                next_small.clone()
            } else if let Some(next_bigger) = proofs_larger.first() {
                next_bigger.clone()
            } else {
                break;
            };

            let proof_amount = selected_proof.amount;

            selected_proofs.push(selected_proof);

            let fees = match include_fees {
                true => self.get_proofs_fee(&selected_proofs).await?,
                false => Amount::ZERO,
            };

            if proof_amount >= remaining_amount + fees {
                remaining_amount = Amount::ZERO;
                break;
            }

            remaining_amount = amount.checked_add(fees).ok_or(Error::AmountOverflow)?
                - Amount::try_sum(selected_proofs.iter().map(|p| p.amount))?;
            (proofs_larger, proofs_smaller) = proofs_smaller
                .into_iter()
                .skip(1)
                .partition(|p| p.amount > remaining_amount);
        }

        if remaining_amount > Amount::ZERO {
            if let Some(next_bigger) = next_bigger_proof {
                return Ok(vec![next_bigger.clone()]);
            }

            return Err(Error::InsufficientFunds);
        }

        Ok(selected_proofs)
    }

    /// Select proofs to send
    #[instrument(skip_all)]
    pub async fn select_proofs_to_swap(
        &self,
        amount: Amount,
        proofs: Proofs,
    ) -> Result<Proofs, Error> {
        let active_keyset_id = self.get_active_mint_keyset().await?.id;

        let (mut active_proofs, mut inactive_proofs): (Proofs, Proofs) = proofs
            .into_iter()
            .partition(|p| p.keyset_id == active_keyset_id);

        let mut selected_proofs: Proofs = Vec::new();
        inactive_proofs.sort_by(|a: &Proof, b: &Proof| b.cmp(a));

        for inactive_proof in inactive_proofs {
            selected_proofs.push(inactive_proof);
            let selected_total = Amount::try_sum(selected_proofs.iter().map(|p| p.amount))?;
            let fees = self.get_proofs_fee(&selected_proofs).await?;

            if selected_total >= amount + fees {
                return Ok(selected_proofs);
            }
        }

        active_proofs.sort_by(|a: &Proof, b: &Proof| b.cmp(a));

        for active_proof in active_proofs {
            selected_proofs.push(active_proof);
            let selected_total = Amount::try_sum(selected_proofs.iter().map(|p| p.amount))?;
            let fees = self.get_proofs_fee(&selected_proofs).await?;

            if selected_total >= amount + fees {
                return Ok(selected_proofs);
            }
        }

        Err(Error::InsufficientFunds)
    }
}
