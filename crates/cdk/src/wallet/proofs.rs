use std::collections::{HashMap, HashSet};

use tracing::instrument;

use crate::{
    amount::SplitTarget,
    dhke::hash_to_curve,
    fees::calculate_fee,
    nuts::{Id, Proof, ProofState, Proofs, PublicKey, State},
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
    ///
    /// This method will first select inactive proofs and then active proofs.
    /// Inactive proofs are always sorted largest first.
    /// The active proofs are sorted by the [`SelectProofsMethod`] provided.
    #[instrument(skip_all)]
    pub async fn select_proofs_to_swap(
        &self,
        amount: Amount,
        proofs: Proofs,
        method: ProofSelectionMethod,
    ) -> Result<Proofs, Error> {
        let active_keyset_id = self.get_active_mint_keyset().await?.id;

        let (mut active_proofs, mut inactive_proofs): (Proofs, Proofs) = proofs
            .into_iter()
            .partition(|p| p.keyset_id == active_keyset_id);

        let mut selected_proofs: Proofs = Vec::new();
        sort_proofs(&mut inactive_proofs, ProofSelectionMethod::Largest, amount);

        for inactive_proof in inactive_proofs {
            selected_proofs.push(inactive_proof);
            let selected_total = Amount::try_sum(selected_proofs.iter().map(|p| p.amount))?;
            let fees = self.get_proofs_fee(&selected_proofs).await?;

            if selected_total >= amount + fees {
                return Ok(selected_proofs);
            }
        }

        if method == ProofSelectionMethod::Least {
            let selected_amount = Amount::try_sum(selected_proofs.iter().map(|p| p.amount))?;
            let keyset_fees = self
                .get_mint_keysets()
                .await?
                .into_iter()
                .map(|k| (k.id, k.input_fee_ppk))
                .collect();
            if let Some(proofs) = select_least_proofs_over_amount(
                &active_proofs,
                amount.checked_sub(selected_amount).unwrap_or(Amount::ZERO),
                keyset_fees,
            ) {
                selected_proofs.extend(proofs);
                return Ok(selected_proofs);
            }
        }

        sort_proofs(&mut active_proofs, method, amount);

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

fn sort_proofs(proofs: &mut Proofs, method: ProofSelectionMethod, amount: Amount) {
    match method {
        // Least fallback to largest
        ProofSelectionMethod::Largest | ProofSelectionMethod::Least => {
            proofs.sort_by(|a: &Proof, b: &Proof| b.cmp(a))
        }
        ProofSelectionMethod::Closest => proofs.sort_by(|a: &Proof, b: &Proof| {
            let a_diff = if a.amount > amount {
                a.amount - amount
            } else {
                amount - a.amount
            };
            let b_diff = if b.amount > amount {
                b.amount - amount
            } else {
                amount - b.amount
            };
            a_diff.cmp(&b_diff)
        }),
        ProofSelectionMethod::Smallest => proofs.sort(),
    }
}

fn select_least_proofs_over_amount(
    proofs: &Proofs,
    amount: Amount,
    fees: HashMap<Id, u64>,
) -> Option<Vec<Proof>> {
    let max_sum = Amount::try_sum(proofs.iter().map(|p| p.amount)).ok()? + 1.into();
    if max_sum < amount || proofs.is_empty() || amount == Amount::ZERO {
        return None;
    }
    let table_len = Into::<u64>::into(max_sum + 1.into()) as usize;
    let mut dp = vec![None; table_len];
    let mut paths = vec![Vec::<Proof>::new(); table_len];

    dp[0] = Some(Amount::ZERO);

    // Fill DP table and track paths
    for proof in proofs {
        let max_other_amounts: u64 = (max_sum - proof.amount).into();
        for t in (0..=max_other_amounts).rev() {
            if let Some(current_sum) = dp[t as usize] {
                let new_sum = current_sum + proof.amount;
                let target_index = (t + Into::<u64>::into(proof.amount)) as usize;

                // If we found a smaller sum or this sum has not been reached yet
                if dp[target_index].is_none() || dp[target_index].unwrap() > new_sum {
                    dp[target_index] = Some(new_sum);
                    paths[target_index] = paths[t as usize].clone();
                    paths[target_index].push(proof.clone());
                }
            }
        }
    }

    // Find the smallest sum greater than or equal to the target
    for t in Into::<u64>::into(amount)..=Into::<u64>::into(max_sum) {
        if let Some(proofs_amount) = dp[t as usize] {
            let proofs = &paths[t as usize];
            let mut proofs_count = HashMap::new();
            for proof in proofs {
                proofs_count
                    .entry(proof.keyset_id)
                    .and_modify(|count| *count += 1)
                    .or_insert(1);
            }
            let fee = calculate_fee(&proofs_count, &fees).unwrap_or(Amount::ZERO);

            if proofs_amount >= amount + fee {
                return Some(paths[t as usize].clone());
            }
        }
    }

    None
}

/// Select proofs method
#[derive(Debug, Default, Clone, Copy, Hash, PartialEq, Eq)]
pub enum ProofSelectionMethod {
    /// Select proofs with the largest amount first
    #[default]
    Largest,
    /// Select proofs closest to the amount first
    Closest,
    /// Select proofs with the smallest amount first
    Smallest,
    /// Select least total proof amount over the specified amount
    Least,
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::{
        nuts::{Id, Proof, PublicKey},
        secret::Secret,
        Amount,
    };

    use super::{select_least_proofs_over_amount, sort_proofs, ProofSelectionMethod};

    #[test]
    fn test_sort_proofs_by_method() {
        let amount = Amount::from(256);
        let keyset_id = Id::random();
        let mut proofs = vec![
            Proof {
                amount: 1.into(),
                keyset_id,
                secret: Secret::generate(),
                c: PublicKey::random(),
                witness: None,
                dleq: None,
            },
            Proof {
                amount: 256.into(),
                keyset_id,
                secret: Secret::generate(),
                c: PublicKey::random(),
                witness: None,
                dleq: None,
            },
            Proof {
                amount: 1024.into(),
                keyset_id,
                secret: Secret::generate(),
                c: PublicKey::random(),
                witness: None,
                dleq: None,
            },
        ];

        fn assert_proof_order(proofs: &[Proof], order: Vec<u64>) {
            for (p, a) in proofs.iter().zip(order.iter()) {
                assert_eq!(p.amount, Amount::from(*a));
            }
        }

        sort_proofs(&mut proofs, ProofSelectionMethod::Largest, amount);
        assert_proof_order(&proofs, vec![1024, 256, 1]);

        sort_proofs(&mut proofs, ProofSelectionMethod::Closest, amount);
        assert_proof_order(&proofs, vec![256, 1, 1024]);

        sort_proofs(&mut proofs, ProofSelectionMethod::Smallest, amount);
        assert_proof_order(&proofs, vec![1, 256, 1024]);

        // Least should fallback to largest
        sort_proofs(&mut proofs, ProofSelectionMethod::Least, amount);
        assert_proof_order(&proofs, vec![1024, 256, 1]);
    }

    #[test]
    fn test_select_least_proofs_over_amount() {
        let amount = Amount::from(1025);
        let keyset_id = Id::random();
        let proofs = vec![
            Proof {
                amount: 1.into(),
                keyset_id,
                secret: Secret::generate(),
                c: PublicKey::random(),
                witness: None,
                dleq: None,
            },
            Proof {
                amount: 256.into(),
                keyset_id,
                secret: Secret::generate(),
                c: PublicKey::random(),
                witness: None,
                dleq: None,
            },
            Proof {
                amount: 1024.into(),
                keyset_id,
                secret: Secret::generate(),
                c: PublicKey::random(),
                witness: None,
                dleq: None,
            },
        ];

        let selected_proofs =
            select_least_proofs_over_amount(&proofs, amount, HashMap::new()).unwrap();
        assert_eq!(selected_proofs.len(), 2);
        let amounts: Vec<u64> = selected_proofs.iter().map(|p| p.amount.into()).collect();
        assert!(amounts.contains(&1024));
        assert!(amounts.contains(&1));
    }
}
