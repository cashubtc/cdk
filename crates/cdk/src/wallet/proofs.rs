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

    /// Select proofs to send or swap for a specific amount.
    /// If inactive keys are allowed via [`SelectProofsOptions`], they will be selected first in largest order.
    #[instrument(skip_all)]
    pub async fn select_proofs(
        &self,
        amount: Amount,
        proofs: Proofs,
        opts: SelectProofsOptions,
    ) -> Result<Proofs, Error> {
        let mut selected_proofs: Proofs = Vec::new();

        let active_keyset_id = self.get_active_mint_keyset().await?.id;

        let (mut active_proofs, mut inactive_proofs): (Proofs, Proofs) = proofs
            .into_iter()
            .partition(|p| p.keyset_id == active_keyset_id);

        if opts.prefer_inactive_keys {
            sort_proofs(&mut inactive_proofs, ProofSelectionMethod::Largest, amount);

            for inactive_proof in inactive_proofs {
                selected_proofs.push(inactive_proof);
                let selected_total = Amount::try_sum(selected_proofs.iter().map(|p| p.amount))?;
                let fees = self.get_proofs_fee(&selected_proofs).await?;

                if selected_total >= amount + fees {
                    return Ok(selected_proofs);
                }
            }
        }

        if opts.method == ProofSelectionMethod::Least {
            let selected_amount = Amount::try_sum(selected_proofs.iter().map(|p| p.amount))?;
            let keyset_fees = if opts.include_fees {
                self.get_mint_keysets()
                    .await?
                    .into_iter()
                    .map(|k| (k.id, k.input_fee_ppk))
                    .collect()
            } else {
                HashMap::new()
            };
            if let Some(proofs) = select_least_proofs_over_amount(
                &active_proofs,
                amount.checked_sub(selected_amount).unwrap_or(Amount::ZERO),
                keyset_fees,
            ) {
                selected_proofs.extend(proofs);
                return Ok(selected_proofs);
            }
        }

        sort_proofs(&mut active_proofs, opts.method, amount);

        for active_proof in active_proofs {
            selected_proofs.push(active_proof);
            let selected_total = Amount::try_sum(selected_proofs.iter().map(|p| p.amount))?;
            let fees = if opts.include_fees {
                self.get_proofs_fee(&selected_proofs).await?
            } else {
                Amount::ZERO
            };

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
        ProofSelectionMethod::Closest => proofs.sort_by_key(|p| {
            if p.amount > amount {
                p.amount - amount
            } else {
                amount - p.amount
            }
        }),
        ProofSelectionMethod::Smallest => proofs.sort(),
    }
}

fn select_least_proofs_over_amount(
    proofs: &Proofs,
    amount: Amount,
    fees: HashMap<Id, u64>,
) -> Option<Vec<Proof>> {
    tracing::trace!(
        "Selecting LEAST proofs over amount {} with fees {:?}",
        amount,
        fees
    );
    let max_sum = Amount::try_sum(proofs.iter().map(|p| p.amount))
        .ok()?
        .checked_add(1.into())?;
    if max_sum < amount || proofs.is_empty() || amount == Amount::ZERO {
        return None;
    }
    let table_len = u64::from(max_sum + 1.into()) as usize;
    let mut dp = vec![None; table_len];
    let mut paths = vec![Vec::<Proof>::new(); table_len];

    dp[0] = Some(Amount::ZERO);

    // Fill DP table and track paths
    for proof in proofs {
        let max_other_amounts = u64::from(max_sum - proof.amount) as usize;
        for t in (0..=max_other_amounts).rev() {
            // Double check bounds
            if t >= dp.len() || t >= paths.len() {
                continue;
            }

            if let Some(current_sum) = dp[t as usize] {
                let new_sum = current_sum + proof.amount;
                let target_index = (t as u64 + u64::from(proof.amount)) as usize;

                // Double check new bounds
                if target_index >= dp.len() || target_index >= paths.len() {
                    continue;
                }

                // If this sum has not been reached yet, or if the new sum is smaller, or if the new path is shorter
                if dp[target_index].is_none()
                    || dp[target_index].expect("None checked") > new_sum
                    || paths[target_index].len() > paths[t].len() + 1
                {
                    tracing::trace!("Updating DP table: {} -> {}", target_index, new_sum);
                    dp[target_index] = Some(new_sum);
                    paths[target_index] = paths[t].clone();
                    paths[target_index].push(proof.clone());
                    tracing::trace!("Path: {:?}", paths[target_index]);
                }
            }
        }
    }

    // Find the smallest sum greater than or equal to the target amount
    for t in u64::from(amount)..=u64::from(max_sum) {
        let idx = t as usize;
        if idx >= dp.len() || idx >= paths.len() {
            continue;
        }

        if let Some(proofs_amount) = dp[idx] {
            let proofs = &paths[idx];
            let proofs_sum =
                Amount::try_sum(proofs.iter().map(|p| p.amount)).unwrap_or(Amount::ZERO);
            if proofs_sum != proofs_amount {
                tracing::error!("Proofs sum does not match DP table sum");
                continue;
            }
            let mut proofs_count = HashMap::new();
            for proof in proofs {
                proofs_count
                    .entry(proof.keyset_id)
                    .and_modify(|count| *count += 1)
                    .or_insert(1);
            }
            let fee = calculate_fee(&proofs_count, &fees).unwrap_or(Amount::ZERO);

            if proofs_amount >= amount + fee {
                let proofs = paths[idx].clone();
                tracing::trace!(
                    "Selected proofs for amount {} with fee {}: {:?}",
                    amount,
                    fee,
                    proofs
                );
                return Some(proofs);
            }
        }
    }

    tracing::trace!("No proofs found for amount {}", amount);
    None
}

/// Select proofs options
pub struct SelectProofsOptions {
    /// Allow inactive keys (if `true`, inactive keys will be selected first in largest order)
    pub prefer_inactive_keys: bool,
    /// Include fees to add to the selection amount
    pub include_fees: bool,
    /// Proof selection method
    pub method: ProofSelectionMethod,
}

impl SelectProofsOptions {
    /// Create new [`SelectProofsOptions`]
    pub fn new(
        allow_inactive_keys: bool,
        include_fees: bool,
        method: ProofSelectionMethod,
    ) -> Self {
        Self {
            prefer_inactive_keys: allow_inactive_keys,
            include_fees,
            method,
        }
    }

    /// Allow inactive keys (if `true`, inactive keys will be selected first in largest order)
    pub fn allow_inactive_keys(mut self, allow_inactive_keys: bool) -> Self {
        self.prefer_inactive_keys = allow_inactive_keys;
        self
    }

    /// Include fees to add to the selection amount
    pub fn include_fees(mut self, include_fees: bool) -> Self {
        self.include_fees = include_fees;
        self
    }

    /// Proof selection method
    pub fn method(mut self, method: ProofSelectionMethod) -> Self {
        self.method = method;
        self
    }
}

impl Default for SelectProofsOptions {
    fn default() -> Self {
        Self {
            prefer_inactive_keys: true,
            include_fees: true,
            method: ProofSelectionMethod::Largest,
        }
    }
}

/// Select proofs method
#[derive(Debug, Default, Clone, Copy, Hash, PartialEq, Eq)]
pub enum ProofSelectionMethod {
    /// The largest value proofs first
    #[default]
    Largest,
    /// The closest in value to the amount first
    Closest,
    /// The smallest value proofs first
    Smallest,
    /// Select the least value of proofs equal to or over the specified amount
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
        let keyset_id = Id::random();
        let c_1 = PublicKey::random();
        let proofs = vec![
            Proof {
                amount: 1.into(),
                keyset_id,
                secret: Secret::generate(),
                c: c_1,
                witness: None,
                dleq: None,
            },
            Proof {
                amount: 1.into(),
                keyset_id,
                secret: Secret::generate(),
                c: c_1,
                witness: None,
                dleq: None,
            },
            Proof {
                amount: 2.into(),
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

        fn assert_amounts(proofs: &mut [Proof], amounts: &mut [u64]) {
            println!("{:?}", proofs);
            println!("{:?}", amounts);
            assert_eq!(proofs.len(), amounts.len());
            proofs.sort_by(|a, b| a.amount.cmp(&b.amount));
            amounts.sort();
            for (p, a) in proofs.iter().zip(amounts.iter()) {
                assert_eq!(p.amount, Amount::from(*a));
            }
        }

        let mut selected_proofs =
            select_least_proofs_over_amount(&proofs, Amount::from(1025), HashMap::new()).unwrap();
        assert_amounts(&mut selected_proofs, &mut [1024, 1]);

        let mut selected_proofs =
            select_least_proofs_over_amount(&proofs, Amount::from(1), HashMap::new()).unwrap();
        assert_amounts(&mut selected_proofs, &mut [1]);

        let mut selected_proofs =
            select_least_proofs_over_amount(&proofs, Amount::from(2), HashMap::new()).unwrap();
        assert_amounts(&mut selected_proofs, &mut [2]);

        let mut selected_proofs =
            select_least_proofs_over_amount(&proofs, Amount::from(1284), HashMap::new()).unwrap();
        assert_amounts(&mut selected_proofs, &mut [1024, 256, 2, 1, 1]);

        // Edge cases
        assert!(
            select_least_proofs_over_amount(&proofs, Amount::from(2048), HashMap::new()).is_none()
        );
        assert!(
            select_least_proofs_over_amount(&proofs, Amount::from(0), HashMap::new()).is_none()
        );
        assert!(
            select_least_proofs_over_amount(&vec![], Amount::from(1), HashMap::new()).is_none()
        );
    }
}
