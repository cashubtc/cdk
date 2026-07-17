use std::collections::{HashMap, HashSet};

use cdk_common::amount::KeysetFeeAndAmounts;
use cdk_common::wallet::ProofInfo;
use cdk_common::Id;
use tracing::instrument;

use crate::fees::calculate_fee;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{
    CheckStateRequest, Proof, ProofState, Proofs, PublicKey, SpendingConditions, State,
};
use crate::{ensure_cdk, Amount, Error, Wallet};

impl Wallet {
    pub(crate) async fn unspent_proof_derivation_indices(
        &self,
    ) -> Result<HashMap<Proof, u32>, Error> {
        Ok(self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit.clone()),
                Some(vec![State::Unspent]),
                None,
            )
            .await?
            .into_iter()
            .filter_map(|proof| proof.derivation_index.map(|index| (proof.proof, index)))
            .collect())
    }

    /// Get unspent proofs for mint
    #[instrument(skip(self))]
    pub async fn get_unspent_proofs(&self) -> Result<Proofs, Error> {
        self.get_proofs_with(Some(vec![State::Unspent]), None).await
    }

    /// Get pending [`Proofs`]
    #[instrument(skip(self))]
    pub async fn get_pending_proofs(&self) -> Result<Proofs, Error> {
        self.get_proofs_with(Some(vec![State::Pending]), None).await
    }

    /// Get reserved [`Proofs`]
    #[instrument(skip(self))]
    pub async fn get_reserved_proofs(&self) -> Result<Proofs, Error> {
        self.get_proofs_with(Some(vec![State::Reserved]), None)
            .await
    }

    /// Get pending spent [`Proofs`]
    #[instrument(skip(self))]
    pub async fn get_pending_spent_proofs(&self) -> Result<Proofs, Error> {
        self.get_proofs_with(Some(vec![State::PendingSpent]), None)
            .await
    }

    /// Get proofs filtered by states
    #[instrument(skip(self))]
    pub async fn get_proofs_by_states(&self, states: Vec<State>) -> Result<Proofs, Error> {
        self.get_proofs_with(Some(states), None).await
    }

    /// Get this wallet's [Proofs] that match the args
    pub async fn get_proofs_with(
        &self,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Proofs, Error> {
        Ok(self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit.clone()),
                state,
                spending_conditions,
            )
            .await?
            .into_iter()
            .map(|p| p.proof)
            .collect())
    }

    /// Return proofs to unspent allowing them to be selected and spent
    #[instrument(skip(self))]
    pub async fn unreserve_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Error> {
        self.localstore
            .update_proofs_state(ys, State::Unspent)
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

    /// Checks pending proofs for spent status and marks spent proofs accordingly.
    ///
    /// # Legacy Recovery Function
    ///
    /// This function is intended for recovering **orphaned proofs** that were stuck
    /// in `Pending`, `Reserved`, or `PendingSpent` state before the saga pattern
    /// was implemented, or proofs whose saga was deleted without proper cleanup.
    ///
    /// **Important**: This function only operates on proofs that are NOT currently
    /// associated with an active saga operation (i.e., `used_by_operation` is `None`).
    /// Proofs managed by active sagas are skipped to avoid interfering with in-flight
    /// operations.
    ///
    /// For proofs that are part of active sagas, use the appropriate saga recovery
    /// mechanism (`recover_incomplete_sagas`) or saga-specific methods like
    /// `SendSaga::revoke()`.
    ///
    /// # Returns
    ///
    /// The total amount of orphaned proofs that remain pending (not spent by the mint)
    /// after checking.
    #[instrument(skip(self))]
    pub async fn check_all_pending_proofs(&self) -> Result<Amount, Error> {
        let mut balance = Amount::ZERO;

        let proofs = self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit.clone()),
                Some(vec![State::Pending, State::Reserved, State::PendingSpent]),
                None,
            )
            .await?;

        // Filter out proofs that are managed by active sagas
        let orphaned_proofs: Vec<ProofInfo> = proofs
            .into_iter()
            .filter(|p| p.used_by_operation.is_none())
            .collect();

        if orphaned_proofs.is_empty() {
            return Ok(Amount::ZERO);
        }

        let states = self
            .check_proofs_spent(
                orphaned_proofs
                    .clone()
                    .into_iter()
                    .map(|p| p.proof)
                    .collect(),
            )
            .await?;

        // Both `State::Pending` and `State::Unspent` should be included in the pending
        // table. This is because a proof that has been created to send will be
        // stored in the pending table in order to avoid accidentally double
        // spending but to allow it to be explicitly reclaimed
        let pending_states: HashSet<PublicKey> = states
            .into_iter()
            .filter(|s| s.state.ne(&State::Spent))
            .map(|s| s.y)
            .collect();

        let (pending_proofs, non_pending_proofs): (Vec<ProofInfo>, Vec<ProofInfo>) =
            orphaned_proofs
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

    /// Select exact proofs
    ///
    /// This function is similar to `select_proofs` but it the selected proofs will not exceed the
    /// requested Amount, it will include a Proof and the exacto amount needed form that Proof to
    /// perform a swap.
    ///
    /// The intent is to perform a swap with info, or include the Proof as part of the return if the
    /// swap is not needed or if the swap failed.
    pub fn select_exact_proofs(
        amount: Amount,
        proofs: Proofs,
        active_keyset_ids: &Vec<Id>,
        fees_and_keyset_amounts: &KeysetFeeAndAmounts,
        include_fees: bool,
    ) -> Result<(Proofs, Option<(Proof, Amount)>), Error> {
        let mut input_proofs = Self::select_proofs(
            amount,
            proofs,
            active_keyset_ids,
            fees_and_keyset_amounts,
            include_fees,
        )?;
        let mut exchange = None;

        // How much amounts do we have selected in our proof sets?
        let total_for_proofs = input_proofs.total_amount().unwrap_or_default();

        if total_for_proofs > amount {
            // If the selected proofs' total amount is more than the needed amount with fees,
            // consider swapping if it makes sense to avoid locking large tokens. Instead, make the
            // exact amount of tokens for the melting, even if that means paying more fees.
            //
            // If the fees would make it more expensive than it is already, it makes no sense, so
            // skip it.
            //
            // The first step is to sort the proofs, select the one with the biggest amount, and
            // perform a swap requesting the exact amount (covering the swap fees).
            input_proofs.sort_by_key(|a| a.amount);

            if let Some(proof_to_exchange) = input_proofs.pop() {
                let fee_ppk = fees_and_keyset_amounts
                    .get(&proof_to_exchange.keyset_id)
                    .map(|fee_and_amounts| fee_and_amounts.fee())
                    .unwrap_or_default();
                // Convert fee from ppk (parts per thousand) to sats per NUT-02 spec:
                // fee = ceil(fee_ppk / 1000) for 1 proof
                let fee: Amount = fee_ppk.div_ceil(1000).into();

                if let Some(exact_amount_to_melt) = total_for_proofs
                    .checked_sub(proof_to_exchange.amount)
                    .and_then(|a| a.checked_add(fee))
                    .and_then(|b| amount.checked_sub(b))
                {
                    exchange = Some((proof_to_exchange, exact_amount_to_melt));
                } else {
                    // failed for some reason
                    input_proofs.push(proof_to_exchange);
                }
            }
        }

        Ok((input_proofs, exchange))
    }

    /// Select proofs
    #[instrument(skip_all)]
    pub fn select_proofs(
        amount: Amount,
        proofs: Proofs,
        active_keyset_ids: &Vec<Id>,
        fees_and_keyset_amounts: &KeysetFeeAndAmounts,
        include_fees: bool,
    ) -> Result<Proofs, Error> {
        Self::select_proofs_with_derivation_indices(
            amount,
            proofs,
            active_keyset_ids,
            fees_and_keyset_amounts,
            include_fees,
            &HashMap::new(),
        )
    }

    /// Select proofs, preferring older deterministic proofs when choices are otherwise equivalent.
    pub(crate) fn select_proofs_with_derivation_indices(
        amount: Amount,
        proofs: Proofs,
        active_keyset_ids: &Vec<Id>,
        fees_and_keyset_amounts: &KeysetFeeAndAmounts,
        include_fees: bool,
        derivation_indices: &HashMap<Proof, u32>,
    ) -> Result<Proofs, Error> {
        if amount == Amount::ZERO {
            return Ok(vec![]);
        }
        ensure_cdk!(proofs.total_amount()? >= amount, Error::InsufficientFunds);

        // Sort proofs in descending order
        let mut proofs = proofs;
        Self::sort_proofs_by_amount_and_age(&mut proofs, derivation_indices);

        // Track selected proofs and remaining amounts (include all inactive proofs first)
        let inactive_proofs: Proofs = proofs
            .iter()
            .filter(|p| !p.is_active(active_keyset_ids))
            .cloned()
            .collect();
        let mut selected_proofs: HashSet<Proof> = inactive_proofs.iter().cloned().collect();
        if selected_proofs.total_amount()? >= amount {
            tracing::debug!("All inactive proofs are sufficient");
            // Still need to filter to minimum set, not return all of them
            let mut inactive_selected = selected_proofs.into_iter().collect::<Vec<_>>();
            Self::sort_proofs_by_amount_and_age(&mut inactive_selected, derivation_indices);
            return Self::select_least_amount_over(inactive_selected, amount, derivation_indices);
        }
        let mut remaining_amounts: Vec<Amount> = Vec::new();

        // Select proof with the exact amount and not already selected
        let mut select_proof = |proofs: &Proofs, amount: Amount, exact: bool| -> bool {
            let mut last_proof = None;
            for proof in proofs.iter() {
                if !selected_proofs.contains(proof) {
                    if proof.amount == amount {
                        selected_proofs.insert(proof.clone());
                        return true;
                    } else if !exact && proof.amount > amount {
                        last_proof = Some(proof.clone());
                    } else if proof.amount < amount {
                        break;
                    }
                }
            }
            if let Some(proof) = last_proof {
                selected_proofs.insert(proof);
                true
            } else {
                false
            }
        };

        // Get fee_and_amounts for the first active keyset (use for optimal amount splitting)
        // We only need to split once - iterating over all keysets would cause duplicate selections
        let fee_and_amounts = active_keyset_ids
            .iter()
            .find_map(|id| fees_and_keyset_amounts.get(id))
            .or_else(|| fees_and_keyset_amounts.values().next());

        // Select proofs with the optimal amounts (only split once, not per keyset)
        if let Some(fee_and_amounts) = fee_and_amounts {
            for optimal_amount in amount.split(fee_and_amounts)? {
                if !select_proof(&proofs, optimal_amount, true) {
                    // Add the remaining amount to the remaining amounts because proof with the optimal amount was not found
                    remaining_amounts.push(optimal_amount);
                }
            }
        }

        // If all the optimal amounts are selected, return the selected proofs
        if remaining_amounts.is_empty() {
            let result: Proofs = selected_proofs.into_iter().collect();
            tracing::debug!(
                "All optimal amounts are selected, returning {} proofs with total {}",
                result.len(),
                result.total_amount().unwrap_or_default()
            );
            if include_fees {
                return Self::include_fees(
                    amount,
                    proofs,
                    result,
                    active_keyset_ids,
                    fees_and_keyset_amounts,
                    derivation_indices,
                );
            } else {
                return Ok(result);
            }
        }

        // Select proofs with the remaining amounts by checking for 2 of the half amount, 4 of the quarter amount, etc.
        tracing::debug!("Selecting proofs with the remaining amounts");
        for remaining_amount in remaining_amounts {
            // Number of proofs to select
            let mut n = 2;

            let mut target_amount = remaining_amount;
            let mut found = false;
            while let Some(curr_amount) = target_amount.checked_div(Amount::from(2)) {
                if curr_amount == Amount::ZERO {
                    break;
                }

                // Select proofs with the current amount
                let mut count = 0;
                for _ in 0..n {
                    if select_proof(&proofs, curr_amount, true) {
                        count += 1;
                    } else {
                        break;
                    }
                }
                n -= count;

                // All proofs with the current amount are selected
                if n == 0 {
                    found = true;
                    break;
                }

                // Try to find double the number of the next amount
                n *= 2;
                target_amount = curr_amount;
            }

            // Find closest amount over the remaining amount
            if !found {
                select_proof(&proofs, remaining_amount, false);
            }
        }

        // Check if the selected proofs total amount is equal to the amount else filter out unnecessary proofs
        let mut selected_proofs = selected_proofs.into_iter().collect::<Vec<_>>();
        let total_amount = selected_proofs.total_amount()?;
        if total_amount != amount && selected_proofs.len() > 1 {
            Self::sort_proofs_by_amount_and_age(&mut selected_proofs, derivation_indices);
            selected_proofs =
                Self::select_least_amount_over(selected_proofs, amount, derivation_indices)?;
        }

        if include_fees {
            return Self::include_fees(
                amount,
                proofs,
                selected_proofs,
                active_keyset_ids,
                fees_and_keyset_amounts,
                derivation_indices,
            );
        }

        Ok(selected_proofs)
    }

    fn select_least_amount_over(
        proofs: Proofs,
        amount: Amount,
        derivation_indices: &HashMap<Proof, u32>,
    ) -> Result<Vec<Proof>, Error> {
        let total_amount = proofs.total_amount()?;
        if total_amount < amount {
            return Err(Error::InsufficientFunds);
        }
        if proofs.len() == 1 {
            return Ok(proofs);
        }

        for i in 1..proofs.len() {
            let (left, right) = proofs.split_at(i);
            let left = left.to_vec();
            let right = right.to_vec();
            let left_amount = left.total_amount()?;
            let right_amount = right.total_amount()?;

            if left_amount >= amount && right_amount >= amount {
                match (
                    Self::select_least_amount_over(left, amount, derivation_indices),
                    Self::select_least_amount_over(right, amount, derivation_indices),
                ) {
                    (Ok(left_proofs), Ok(right_proofs)) => {
                        let left_total_amount = left_proofs.total_amount()?;
                        let right_total_amount = right_proofs.total_amount()?;
                        if left_total_amount < right_total_amount
                            || (left_total_amount == right_total_amount
                                && Self::is_older_selection(
                                    &left_proofs,
                                    &right_proofs,
                                    derivation_indices,
                                ))
                        {
                            return Ok(left_proofs);
                        } else {
                            return Ok(right_proofs);
                        }
                    }
                    (Ok(left_proofs), Err(_)) => return Ok(left_proofs),
                    (Err(_), Ok(right_proofs)) => return Ok(right_proofs),
                    (Err(_), Err(_)) => return Err(Error::InsufficientFunds),
                }
            } else if left_amount >= amount {
                return Self::select_least_amount_over(left, amount, derivation_indices);
            } else if right_amount >= amount {
                return Self::select_least_amount_over(right, amount, derivation_indices);
            }
        }

        Ok(proofs)
    }

    fn include_fees(
        amount: Amount,
        proofs: Proofs,
        mut selected_proofs: Proofs,
        active_keyset_ids: &Vec<Id>,
        fees_and_keyset_amounts: &KeysetFeeAndAmounts,
        derivation_indices: &HashMap<Proof, u32>,
    ) -> Result<Proofs, Error> {
        tracing::debug!("Including fees");
        let fee_breakdown = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &fees_and_keyset_amounts
                .iter()
                .map(|(key, values)| (*key, values.fee()))
                .collect(),
        )?;
        let net_amount = selected_proofs.total_amount()? - fee_breakdown.total;
        tracing::debug!(
            "Net amount={}, fee={}, total amount={}",
            net_amount,
            fee_breakdown.total,
            selected_proofs.total_amount()?
        );
        if net_amount >= amount {
            tracing::debug!(
                "Selected proofs: {:?}",
                selected_proofs
                    .iter()
                    .map(|p| p.amount.into())
                    .collect::<Vec<u64>>(),
            );
            return Ok(selected_proofs);
        }

        let keyset_fees: HashMap<Id, u64> = fees_and_keyset_amounts
            .iter()
            .map(|(key, values)| (*key, values.fee()))
            .collect();

        let mut remaining_proofs: Proofs = proofs
            .into_iter()
            .filter(|p| !selected_proofs.contains(p))
            .collect();

        loop {
            let fee = calculate_fee(&selected_proofs.count_by_keyset(), &keyset_fees)?.total;
            let total = selected_proofs.total_amount()?;
            let net_amount = total - fee;

            tracing::debug!(
                "Net amount={}, fee={}, total amount={}",
                net_amount,
                fee,
                total
            );

            if net_amount >= amount {
                tracing::debug!(
                    "Selected proofs: {:?}",
                    selected_proofs
                        .iter()
                        .map(|p| p.amount.into())
                        .collect::<Vec<u64>>(),
                );
                return Ok(selected_proofs);
            }

            if remaining_proofs.is_empty() {
                return Err(Error::InsufficientFunds);
            }

            let shortfall = amount - net_amount;
            tracing::debug!("Net amount is less than required, shortfall={}", shortfall);

            let additional = Wallet::select_proofs_with_derivation_indices(
                shortfall,
                remaining_proofs.clone(),
                active_keyset_ids,
                fees_and_keyset_amounts,
                false,
                derivation_indices,
            )?;

            if additional.is_empty() {
                return Err(Error::InsufficientFunds);
            }

            remaining_proofs.retain(|p| !additional.contains(p));
            selected_proofs.extend(additional);
        }
    }

    fn sort_proofs_by_amount_and_age(
        proofs: &mut Proofs,
        derivation_indices: &HashMap<Proof, u32>,
    ) {
        if derivation_indices.is_empty() {
            // Preserve the selector's legacy ordering when no recovery metadata is available.
            proofs.sort_by(|a, b| a.cmp(b).reverse());
            return;
        }
        proofs.sort_by(|a, b| {
            b.amount.cmp(&a.amount).then_with(|| {
                derivation_indices
                    .get(a)
                    .copied()
                    .unwrap_or(u32::MAX)
                    .cmp(&derivation_indices.get(b).copied().unwrap_or(u32::MAX))
            })
        });
    }

    fn is_older_selection(
        left: &Proofs,
        right: &Proofs,
        derivation_indices: &HashMap<Proof, u32>,
    ) -> bool {
        if derivation_indices.is_empty() {
            return false;
        }
        let mut left_indices = left
            .iter()
            .map(|proof| derivation_indices.get(proof).copied().unwrap_or(u32::MAX))
            .collect::<Vec<_>>();
        let mut right_indices = right
            .iter()
            .map(|proof| derivation_indices.get(proof).copied().unwrap_or(u32::MAX))
            .collect::<Vec<_>>();
        left_indices.sort_unstable();
        right_indices.sort_unstable();
        left_indices < right_indices
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use cdk_common::amount::KeysetFeeAndAmounts;
    use cdk_common::secret::Secret;
    use cdk_common::{Amount, Id, Proof, Proofs, ProofsMethods, PublicKey};

    use crate::Wallet;

    fn id() -> Id {
        Id::from_bytes(&[0; 8]).unwrap()
    }

    fn proof(amount: u64) -> Proof {
        Proof::new(
            Amount::from(amount),
            id(),
            Secret::generate(),
            PublicKey::from_hex(
                "03deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            )
            .unwrap(),
        )
    }

    fn indexed_proof(amount: u64, index: u32) -> Proof {
        Proof::new(
            Amount::from(amount),
            id(),
            Secret::new(format!("proof-{index:08}")),
            PublicKey::from_hex(
                "03deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            )
            .unwrap(),
        )
    }

    fn fee_and_amounts() -> KeysetFeeAndAmounts {
        HashMap::from([(
            id(),
            (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        )])
    }

    #[test]
    fn test_select_proofs_prefers_oldest_equal_amount() {
        let old = indexed_proof(8, 4);
        let middle = indexed_proof(8, 40);
        let young = indexed_proof(8, 400);
        let indices = HashMap::from([(old.clone(), 4), (middle.clone(), 40), (young.clone(), 400)]);

        let selected = Wallet::select_proofs_with_derivation_indices(
            Amount::from(8),
            vec![young, middle, old.clone()],
            &vec![id()],
            &fee_and_amounts(),
            false,
            &indices,
        )
        .unwrap();

        assert_eq!(selected, vec![old]);
    }

    #[test]
    fn test_select_proofs_without_indices_preserves_legacy_order() {
        let proofs = vec![proof(8), proof(8), proof(8)];
        let expected = proofs[0].clone();

        let selected = Wallet::select_proofs(
            Amount::from(8),
            proofs,
            &vec![id()],
            &fee_and_amounts(),
            false,
        )
        .unwrap();

        assert_eq!(selected, vec![expected]);
    }

    #[test]
    fn test_select_proofs_keeps_exact_amount_priority_over_age() {
        let old = indexed_proof(16, 4);
        let young = indexed_proof(8, 400);
        let indices = HashMap::from([(old.clone(), 4), (young.clone(), 400)]);

        let selected = Wallet::select_proofs_with_derivation_indices(
            Amount::from(8),
            vec![old, young.clone()],
            &vec![id()],
            &fee_and_amounts(),
            false,
            &indices,
        )
        .unwrap();

        assert_eq!(selected, vec![young]);
    }

    struct GapSimulationResult {
        final_gap: u32,
        maximum_gap: u32,
        shrink_events: u32,
        samples: u32,
        average_gap: u64,
    }

    fn run_gap_simulation(
        prefer_oldest: bool,
        target_index: u32,
        run_nonce: u64,
    ) -> GapSimulationResult {
        let mut proofs = Vec::new();
        let mut indices = HashMap::new();
        let mut next_index = 0_u32;
        let mut rng = run_nonce;
        let mut previous_gap = 0_u32;
        let mut maximum_gap = 0_u32;
        let mut shrink_events = 0_u32;
        let mut samples = 0_u32;
        let mut gap_sum = 0_u64;

        let add_outputs = |amount: u64,
                           retain: bool,
                           proofs: &mut Proofs,
                           indices: &mut HashMap<Proof, u32>,
                           next_index: &mut u32| {
            for bit in 0..64 {
                let denomination = 1_u64 << bit;
                if amount & denomination == 0 {
                    continue;
                }
                let proof = Proof::new(
                    Amount::from(denomination),
                    id(),
                    Secret::new(format!("simulation-{run_nonce:016x}-{next_index:08}")),
                    PublicKey::from_hex(
                        "03deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
                    )
                    .unwrap(),
                );
                if retain {
                    indices.insert(proof.clone(), *next_index);
                    proofs.push(proof);
                }
                *next_index = next_index.saturating_add(1);
            }
        };

        add_outputs(65_535, true, &mut proofs, &mut indices, &mut next_index);

        while next_index < target_index {
            let balance: u64 = proofs.total_amount().unwrap().into();
            if balance < 32_768_u64 {
                add_outputs(32_768, true, &mut proofs, &mut indices, &mut next_index);
            }

            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let spend = 1 + (rng >> 32) % 4_096;
            let ranking = if prefer_oldest {
                indices
                    .iter()
                    .map(|(proof, index)| (proof.clone(), *index))
                    .collect()
            } else {
                HashMap::new()
            };
            let selected = Wallet::select_proofs_with_derivation_indices(
                Amount::from(spend),
                proofs.clone(),
                &vec![id()],
                &fee_and_amounts(),
                false,
                &ranking,
            )
            .unwrap();
            let selected_total: u64 = selected.total_amount().unwrap().into();
            proofs.retain(|proof| !selected.contains(proof));
            for proof in &selected {
                indices.remove(proof);
            }

            // A normal online send derives both the outgoing proofs and change. The outgoing
            // proofs are considered claimed immediately; retained change remains active.
            add_outputs(spend, false, &mut proofs, &mut indices, &mut next_index);
            add_outputs(
                selected_total - spend,
                true,
                &mut proofs,
                &mut indices,
                &mut next_index,
            );

            let first_active = indices.values().copied().min().unwrap_or(next_index);
            let gap = next_index.saturating_sub(first_active);
            if samples > 0 && gap < previous_gap {
                shrink_events += 1;
            }
            previous_gap = gap;
            maximum_gap = maximum_gap.max(gap);
            gap_sum += u64::from(gap);
            samples += 1;
        }

        GapSimulationResult {
            final_gap: previous_gap,
            maximum_gap,
            shrink_events,
            samples,
            average_gap: gap_sum / u64::from(samples),
        }
    }

    #[test]
    #[ignore = "long-horizon NUT-342 diagnostic simulation"]
    fn benchmark_legacy_vs_oldest_first_dynamic_gap() {
        let target_index = std::env::var("CDK_GAP_SIM_TARGET_INDEX")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(50_000);
        let trials = std::env::var("CDK_GAP_SIM_TRIALS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1);

        for trial in 1..=trials {
            let run_nonce = rand::random::<u64>();
            let legacy = run_gap_simulation(false, target_index, run_nonce);
            let oldest_first = run_gap_simulation(true, target_index, run_nonce);

            println!("trial={trial} nonce={run_nonce:016x} target_index={target_index}");
            println!(
                "legacy: final={}, max={}, shrinks={}, swaps={}, average={}",
                legacy.final_gap,
                legacy.maximum_gap,
                legacy.shrink_events,
                legacy.samples,
                legacy.average_gap
            );
            println!(
                "oldest-first: final={}, max={}, shrinks={}, swaps={}, average={}",
                oldest_first.final_gap,
                oldest_first.maximum_gap,
                oldest_first.shrink_events,
                oldest_first.samples,
                oldest_first.average_gap
            );
            assert_eq!(oldest_first.samples, legacy.samples);
        }
    }

    #[test]
    fn test_select_proofs_empty() {
        let active_id = id();
        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(
            active_id,
            (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );
        let proofs = vec![];
        let selected_proofs = Wallet::select_proofs(
            0.into(),
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            false,
        )
        .unwrap();
        assert_eq!(selected_proofs.len(), 0);
    }

    #[test]
    fn test_select_proofs_insufficient() {
        let active_id = id();
        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(
            active_id,
            (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );
        let proofs = vec![proof(1), proof(2), proof(4)];
        let selected_proofs = Wallet::select_proofs(
            8.into(),
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            false,
        );
        assert!(selected_proofs.is_err());
    }

    #[test]
    fn test_select_proofs_exact() {
        let proofs = vec![
            proof(1),
            proof(2),
            proof(4),
            proof(8),
            proof(16),
            proof(32),
            proof(64),
        ];

        let active_id = id();
        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(
            active_id,
            (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );

        let mut selected_proofs = Wallet::select_proofs(
            77.into(),
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            false,
        )
        .unwrap();
        selected_proofs.sort();
        assert_eq!(selected_proofs.len(), 4);
        assert_eq!(selected_proofs[0].amount, 1.into());
        assert_eq!(selected_proofs[1].amount, 4.into());
        assert_eq!(selected_proofs[2].amount, 8.into());
        assert_eq!(selected_proofs[3].amount, 64.into());
    }

    #[test]
    fn test_select_proofs_over() {
        let active_id = id();
        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(
            active_id,
            (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );
        let proofs = vec![proof(1), proof(2), proof(4), proof(8), proof(32), proof(64)];
        let selected_proofs = Wallet::select_proofs(
            31.into(),
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            false,
        )
        .unwrap();
        assert_eq!(selected_proofs.len(), 1);
        assert_eq!(selected_proofs[0].amount, 32.into());
    }

    #[test]
    fn test_select_proofs_smaller_over() {
        let proofs = vec![proof(8), proof(16), proof(32)];
        let active_id = id();
        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(
            active_id,
            (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );

        let selected_proofs = Wallet::select_proofs(
            23.into(),
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            false,
        )
        .unwrap();
        assert_eq!(selected_proofs.len(), 2);
        assert_eq!(selected_proofs[0].amount, 16.into());
        assert_eq!(selected_proofs[1].amount, 8.into());
    }

    #[test]
    fn test_select_proofs_many_ones() {
        let active_id = id();
        let mut fee_and_keyset_amounts = HashMap::new();
        fee_and_keyset_amounts.insert(
            active_id,
            (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );
        let proofs = (0..1024).map(|_| proof(1)).collect::<Vec<_>>();
        let selected_proofs = Wallet::select_proofs(
            1024.into(),
            proofs,
            &vec![active_id],
            &fee_and_keyset_amounts,
            false,
        )
        .unwrap();
        assert_eq!(selected_proofs.len(), 1024);
        selected_proofs
            .iter()
            .for_each(|proof| assert_eq!(proof.amount, Amount::ONE));
    }

    #[test]
    fn test_select_proof_change() {
        let active_id = id();
        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(
            active_id,
            (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );
        let proofs = vec![proof(64), proof(4), proof(32)];
        let (selected_proofs, exchange) = Wallet::select_exact_proofs(
            97.into(),
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            false,
        )
        .unwrap();
        assert!(exchange.is_some());
        let (proof_to_exchange, amount) = exchange.unwrap();

        assert_eq!(selected_proofs.len(), 2);
        assert_eq!(proof_to_exchange.amount, 64.into());
        assert_eq!(amount, 61.into());
    }

    #[test]
    fn test_select_proofs_huge_proofs() {
        let active_id = id();
        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(
            active_id,
            (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );
        let proofs = (0..32)
            .flat_map(|i| (0..5).map(|_| proof(1 << i)).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        let mut selected_proofs = Wallet::select_proofs(
            ((1u64 << 32) - 1).into(),
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            false,
        )
        .unwrap();
        selected_proofs.sort();
        assert_eq!(selected_proofs.len(), 32);

        selected_proofs
            .iter()
            .enumerate()
            .for_each(|(i, proof)| assert_eq!(proof.amount, (1 << i).into()));
    }

    #[test]
    fn test_select_proofs_with_fees() {
        let proofs = vec![proof(64), proof(4), proof(32)];
        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(id(), (100, (0..32).map(|x| 2u64.pow(x)).collect()).into());
        let selected_proofs = Wallet::select_proofs(
            10.into(),
            proofs,
            &vec![id()],
            &keyset_fee_and_amounts,
            false,
        )
        .unwrap();
        assert_eq!(selected_proofs.len(), 1);
        assert_eq!(selected_proofs[0].amount, 32.into());
    }

    #[test]
    fn test_select_proofs_include_fees_accounts_for_additional_proof_fees() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(
            active_id,
            (100, (0..32).map(|x| 2u64.pow(x)).collect()).into(),
        );

        let proofs = vec![
            proof(512),
            proof(256),
            proof(128),
            proof(64),
            proof(32),
            proof(16),
            proof(8),
            proof(4),
            proof(2),
            proof(1),
        ];

        let amount: Amount = 1010.into();
        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert!(
            net >= amount,
            "Net amount {} should be >= requested amount {} (total={}, fee={})",
            net,
            amount,
            total,
            fee
        );
    }

    #[test]
    fn test_select_proofs_include_fees_iterates_until_stable() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(
            active_id,
            (100, (0..32).map(|x| 2u64.pow(x)).collect()).into(),
        );

        let mut proofs = Vec::new();
        for i in 0..10 {
            proofs.push(proof(1 << i));
        }
        proofs.push(proof(2));
        proofs.push(proof(4));

        let amount: Amount = 1010.into();
        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert!(
            net >= amount,
            "Net amount {} should be >= requested amount {} (total={}, fee={}, num_proofs={})",
            net,
            amount,
            total,
            fee,
            selected_proofs.len()
        );
    }

    // ========================================================================
    // Fee-Aware Proof Selection Tests (fee_ppk = 200)
    // ========================================================================

    fn keyset_fee_and_amounts_with_fee(
        fee_ppk: u64,
    ) -> HashMap<Id, cdk_common::amount::FeeAndAmounts> {
        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(
            id(),
            (fee_ppk, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );
        keyset_fee_and_amounts
    }

    fn standard_proofs() -> Vec<Proof> {
        vec![
            proof(1),
            proof(2),
            proof(4),
            proof(8),
            proof(16),
            proof(32),
            proof(64),
            proof(128),
            proof(256),
            proof(512),
            proof(1024),
            proof(2048),
            proof(4096),
        ]
    }

    fn fragmented_proofs() -> Vec<Proof> {
        let mut proofs = Vec::new();
        for _ in 0..10 {
            proofs.push(proof(1));
        }
        for _ in 0..8 {
            proofs.push(proof(2));
        }
        for _ in 0..6 {
            proofs.push(proof(4));
        }
        for _ in 0..5 {
            proofs.push(proof(8));
        }
        for _ in 0..4 {
            proofs.push(proof(16));
        }
        for _ in 0..3 {
            proofs.push(proof(32));
        }
        for _ in 0..2 {
            proofs.push(proof(64));
        }
        for _ in 0..2 {
            proofs.push(proof(128));
        }
        for _ in 0..2 {
            proofs.push(proof(256));
        }
        for _ in 0..2 {
            proofs.push(proof(512));
        }
        for _ in 0..2 {
            proofs.push(proof(1024));
        }
        for _ in 0..2 {
            proofs.push(proof(2048));
        }
        proofs
    }

    fn large_proofs() -> Vec<Proof> {
        vec![
            proof(4096),
            proof(2048),
            proof(1024),
            proof(512),
            proof(256),
        ]
    }

    fn mixed_proofs() -> Vec<Proof> {
        vec![
            proof(4096),
            proof(1024),
            proof(256),
            proof(256),
            proof(128),
            proof(64),
            proof(32),
            proof(16),
            proof(8),
            proof(4),
            proof(2),
            proof(1),
            proof(1),
        ]
    }

    #[test]
    fn test_select_proofs_with_fees_single_proof_exact() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(200);

        let proofs = vec![proof(4096)];
        let amount: Amount = 4095.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert_eq!(selected_proofs.len(), 1);
        assert_eq!(selected_proofs[0].amount, 4096.into());
        assert!(net >= amount, "4096 - 1 (fee) = 4095 >= 4095");
    }

    #[test]
    fn test_select_proofs_with_fees_single_proof_insufficient() {
        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(200);

        let proofs = vec![proof(4096)];
        let amount: Amount = 4096.into();

        let result = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        );

        assert!(result.is_err(), "4096 - 1 (fee) = 4095 < 4096, should fail");
    }

    #[test]
    fn test_select_proofs_with_fees_two_proofs_fee_threshold() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(200);

        let proofs = vec![proof(4096), proof(1024)];
        let amount: Amount = 5000.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert!(net >= amount, "5120 - 1 = 5119 >= 5000");
    }

    #[test]
    fn test_select_proofs_with_fees_iterative_fee_adjustment() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(200);

        let proofs = vec![
            proof(4096),
            proof(1024),
            proof(512),
            proof(256),
            proof(128),
            proof(8),
        ];
        let amount: Amount = 5000.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert!(
            net >= amount,
            "Net amount {} should be >= requested amount {} (total={}, fee={})",
            net,
            amount,
            total,
            fee
        );
    }

    #[test]
    fn test_select_proofs_with_fees_fee_increases_with_proofs() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(200);

        let proofs = vec![
            proof(1024),
            proof(1024),
            proof(1024),
            proof(1024),
            proof(1024),
        ];
        let amount: Amount = 5000.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert!(
            net >= amount,
            "Net amount {} should be >= requested amount {}",
            net,
            amount
        );
    }

    #[test]
    fn test_select_proofs_with_fees_standard_proofs() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(200);

        let proofs = standard_proofs();
        let amount: Amount = 5000.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert!(
            net >= amount,
            "Standard proofs: net {} should be >= {} (total={}, fee={}, num_proofs={})",
            net,
            amount,
            total,
            fee,
            selected_proofs.len()
        );
    }

    #[test]
    fn test_select_proofs_with_fees_mixed_proofs() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(200);

        let proofs = mixed_proofs();
        let amount: Amount = 5000.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert!(
            net >= amount,
            "Mixed proofs: net {} should be >= {} (total={}, fee={}, num_proofs={})",
            net,
            amount,
            total,
            fee,
            selected_proofs.len()
        );
    }

    // ========================================================================
    // High Fee Tests (fee_ppk = 1000, i.e., 1 sat per proof)
    // ========================================================================

    #[test]
    fn test_select_proofs_high_fees_one_sat_per_proof() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(1000);

        let proofs = vec![
            proof(4096),
            proof(4096),
            proof(4096),
            proof(4096),
            proof(4096),
            proof(4096),
            proof(4096),
            proof(4096),
            proof(4096),
            proof(4096),
            proof(4096),
            proof(512),
            proof(1024),
            proof(1024),
            proof(1024),
            proof(1024),
            proof(1024),
            proof(1024),
            proof(1024),
            proof(1024),
            proof(512),
            proof(512),
            proof(512),
            proof(512),
            proof(512),
            proof(512),
            proof(256),
            proof(256),
            proof(256),
            proof(256),
            proof(256),
            proof(256),
            proof(128),
            proof(128),
            proof(128),
            proof(128),
            proof(128),
            proof(128),
            proof(128),
            proof(8),
            proof(8),
            proof(8),
            proof(8),
            proof(8),
            proof(8),
            proof(8),
            proof(4),
            proof(4),
            proof(4),
            proof(4),
            proof(4),
            proof(4),
            proof(4),
            proof(2),
            proof(2),
            proof(2),
            proof(2),
            proof(2),
            proof(2),
            proof(1),
            proof(1),
            proof(1),
            proof(1),
            proof(1),
            proof(4096),
        ];
        let amount: Amount = 5000.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;

        let net = total - fee;

        assert!(
            net == amount,
            "Selected proofs should cover amount after fees"
        );
        assert!(fee == Amount::from(selected_proofs.len() as u64));
        assert!(fee > Amount::ZERO);
    }

    #[test]
    fn test_select_proofs_high_fees_prefers_larger_proofs() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(1000);

        let mut proofs = Vec::new();
        for _ in 0..100 {
            proofs.push(proof(64));
        }
        proofs.push(proof(4096));
        proofs.push(proof(1024));

        let amount: Amount = 5000.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert!(net >= amount, "Net amount {} should be >= {}", net, amount);
    }

    #[test]
    fn test_select_proofs_high_fees_exact_with_fee() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(1000);

        let proofs = vec![proof(4096), proof(1024)];
        let amount: Amount = 5118.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert_eq!(selected_proofs.len(), 2);
        assert_eq!(net, 5118.into(), "5120 - 2 = 5118");
    }

    #[test]
    fn test_select_proofs_high_fees_large_proofs() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(1000);

        let proofs = large_proofs();
        let amount: Amount = 5000.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert!(
            net >= amount,
            "Large proofs: net {} should be >= {} (total={}, fee={}, num_proofs={})",
            net,
            amount,
            total,
            fee,
            selected_proofs.len()
        );
    }

    // ========================================================================
    // Edge Case Tests
    // ========================================================================

    #[test]
    fn test_select_proofs_with_fees_zero_amount() {
        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(200);

        let proofs = standard_proofs();
        let amount: Amount = 0.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        assert_eq!(
            selected_proofs.len(),
            0,
            "Zero amount should return empty selection"
        );
    }

    #[test]
    fn test_select_proofs_with_fees_empty_proofs() {
        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(200);

        let proofs: Vec<Proof> = vec![];
        let amount: Amount = 5000.into();

        let result = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        );

        assert!(
            result.is_err(),
            "Empty proofs should return InsufficientFunds"
        );
    }

    #[test]
    fn test_select_proofs_with_fees_all_proofs_same_size() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(200);

        let proofs = vec![
            proof(1024),
            proof(1024),
            proof(1024),
            proof(1024),
            proof(1024),
            proof(1024),
        ];
        let amount: Amount = 5000.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert!(net >= amount, "Net {} should be >= {}", net, amount);
    }

    #[test]
    fn test_select_proofs_with_fees_fee_exceeds_small_proof() {
        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(1000);

        let proofs = vec![proof(1)];
        let amount: Amount = 1.into();

        let result = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        );

        assert!(
            result.is_err(),
            "1 sat proof with 1 sat fee is uneconomical"
        );
    }

    #[test]
    fn test_select_proofs_with_fees_barely_sufficient() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(200);

        let proofs = vec![
            proof(4096),
            proof(1024),
            proof(512),
            proof(256),
            proof(128),
            proof(8),
            proof(1),
        ];
        let amount: Amount = 5000.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert!(
            net >= amount,
            "Barely sufficient: net {} should be >= {} (total={}, fee={})",
            net,
            amount,
            total,
            fee
        );
    }

    // ========================================================================
    // Stress Tests
    // ========================================================================

    #[test]
    fn test_select_proofs_many_small_proofs_with_fees() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(100);

        let mut proofs: Vec<Proof> = (0..500).map(|_| proof(16)).collect();
        proofs.extend((0..200).map(|_| proof(8)));
        proofs.extend((0..100).map(|_| proof(4)));

        let amount: Amount = 5000.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert!(
            net >= amount,
            "Net {} should be >= {} (total={}, fee={}, num_proofs={})",
            net,
            amount,
            total,
            fee,
            selected_proofs.len()
        );
    }

    #[test]
    fn test_select_proofs_fee_convergence_with_many_proofs() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(100);

        let proofs: Vec<Proof> = (0..600).map(|_| proof(16)).collect();
        let amount: Amount = 5000.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert!(
            net >= amount,
            "Fee convergence should work: net={}, amount={}, total={}, fee={}, proofs={}",
            net,
            amount,
            total,
            fee,
            selected_proofs.len()
        );
    }

    #[test]
    fn test_select_proofs_fragmented_proofs_with_fees() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(200);

        let proofs = fragmented_proofs();
        let amount: Amount = 5000.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert!(
            net >= amount,
            "Fragmented proofs: net {} should be >= {} (total={}, fee={}, num_proofs={})",
            net,
            amount,
            total,
            fee,
            selected_proofs.len()
        );
    }

    // ========================================================================
    // Regression Tests
    // ========================================================================

    #[test]
    fn test_regression_swap_insufficient_small_proof() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(200);

        let proofs = vec![
            proof(4096),
            proof(1024),
            proof(512),
            proof(256),
            proof(128),
            proof(8),
        ];
        let amount: Amount = 5000.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert!(
            net >= amount,
            "Regression: should handle small proofs correctly. Net={}, expected >= {}",
            net,
            amount
        );
    }

    #[test]
    fn test_regression_fragmented_proofs_with_fees() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(200);

        let mut proofs = Vec::new();
        for _ in 0..20 {
            proofs.push(proof(1));
        }
        for _ in 0..15 {
            proofs.push(proof(2));
        }
        for _ in 0..12 {
            proofs.push(proof(4));
        }
        for _ in 0..10 {
            proofs.push(proof(8));
        }
        for _ in 0..8 {
            proofs.push(proof(16));
        }
        for _ in 0..6 {
            proofs.push(proof(32));
        }
        for _ in 0..5 {
            proofs.push(proof(64));
        }
        for _ in 0..4 {
            proofs.push(proof(128));
        }
        for _ in 0..3 {
            proofs.push(proof(256));
        }
        for _ in 0..3 {
            proofs.push(proof(512));
        }
        for _ in 0..2 {
            proofs.push(proof(1024));
        }
        for _ in 0..2 {
            proofs.push(proof(2048));
        }

        let amount: Amount = 5000.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert!(
            net >= amount,
            "Fragmented proofs should work: net={}, amount={}",
            net,
            amount
        );
    }

    #[test]
    fn test_regression_exact_amount_with_multiple_denominations() {
        use cdk_common::nuts::nut00::ProofsMethods;

        use crate::fees::calculate_fee;

        let active_id = id();
        let keyset_fee_and_amounts = keyset_fee_and_amounts_with_fee(200);

        let proofs = vec![
            proof(4096),
            proof(1024),
            proof(512),
            proof(256),
            proof(128),
            proof(8),
            proof(4),
            proof(2),
            proof(1),
        ];
        let amount: Amount = 5007.into();

        let selected_proofs = Wallet::select_proofs(
            amount,
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            true,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &keyset_fee_and_amounts
                .iter()
                .map(|(k, v)| (*k, v.fee()))
                .collect(),
        )
        .unwrap()
        .total;
        let net = total - fee;

        assert!(
            net >= amount,
            "Exact amount with multiple denominations: net {} should be >= {} (total={}, fee={})",
            net,
            amount,
            total,
            fee
        );
    }

    // ========================================================================
    // Inactive Keyset Tests
    // ========================================================================

    fn inactive_id() -> Id {
        Id::from_bytes(&[0x00, 1, 1, 1, 1, 1, 1, 1]).unwrap()
    }

    fn proof_with_keyset(amount: u64, keyset_id: Id) -> Proof {
        Proof::new(
            Amount::from(amount),
            keyset_id,
            Secret::generate(),
            PublicKey::from_hex(
                "03deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            )
            .unwrap(),
        )
    }

    #[test]
    fn test_select_proofs_inactive_keyset_exact_amount() {
        use cdk_common::nuts::nut00::ProofsMethods;

        let inactive = inactive_id();
        let active = id();

        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(
            active,
            (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );

        let proofs = vec![
            proof_with_keyset(1, inactive),
            proof_with_keyset(1, inactive),
            proof_with_keyset(2, inactive),
            proof_with_keyset(4, inactive),
            proof_with_keyset(8, inactive),
            proof_with_keyset(16, inactive),
        ];

        let selected_proofs = Wallet::select_proofs(
            4.into(),
            proofs,
            &vec![active],
            &keyset_fee_and_amounts,
            false,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        assert_eq!(
            total,
            4.into(),
            "Should select exactly 4 sats worth of proofs from inactive keyset, got {}",
            total
        );
    }

    #[test]
    fn test_select_proofs_inactive_keyset_minimum_over() {
        use cdk_common::nuts::nut00::ProofsMethods;

        let inactive = inactive_id();
        let active = id();

        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(
            active,
            (0, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );

        let proofs = vec![
            proof_with_keyset(8, inactive),
            proof_with_keyset(16, inactive),
            proof_with_keyset(32, inactive),
        ];

        let selected_proofs = Wallet::select_proofs(
            5.into(),
            proofs,
            &vec![active],
            &keyset_fee_and_amounts,
            false,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        assert_eq!(
            total,
            8.into(),
            "Should select minimum amount (8) that covers 5 sats, got {}",
            total
        );
        assert_eq!(selected_proofs.len(), 1, "Should select only 1 proof");
    }

    #[test]
    fn test_select_proofs_active_keyset_exact_4_sats_with_fee() {
        use cdk_common::nuts::nut00::ProofsMethods;

        let active = id();

        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(
            active,
            (100, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );

        let proofs = vec![
            proof(1),
            proof(1),
            proof(1),
            proof(1),
            proof(2),
            proof(2),
            proof(2),
            proof(2),
            proof(4),
            proof(4),
            proof(4),
            proof(4),
            proof(8),
            proof(8),
            proof(8),
            proof(16),
            proof(16),
            proof(16),
        ];

        let selected_proofs = Wallet::select_proofs(
            4.into(),
            proofs,
            &vec![active],
            &keyset_fee_and_amounts,
            false,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        assert_eq!(
            total,
            4.into(),
            "Should select exactly 4 sats worth of proofs, got {}",
            total
        );
        assert_eq!(
            selected_proofs.len(),
            1,
            "Should select only 1 proof (the 4-sat one)"
        );
    }

    #[test]
    fn test_select_proofs_multiple_keysets_does_not_double_select() {
        use cdk_common::nuts::nut00::ProofsMethods;

        let active = id();
        let other_keyset = inactive_id();

        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(
            active,
            (100, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );
        keyset_fee_and_amounts.insert(
            other_keyset,
            (100, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );

        let proofs = vec![
            proof(1),
            proof(1),
            proof(1),
            proof(1),
            proof(2),
            proof(2),
            proof(2),
            proof(2),
            proof(4),
            proof(4),
            proof(4),
            proof(4),
            proof(8),
            proof(8),
            proof(8),
            proof(16),
            proof(16),
            proof(16),
        ];

        let selected_proofs = Wallet::select_proofs(
            4.into(),
            proofs,
            &vec![active],
            &keyset_fee_and_amounts,
            false,
        )
        .unwrap();

        let total = selected_proofs.total_amount().unwrap();
        assert_eq!(
            total,
            4.into(),
            "Should select exactly 4 sats worth even with multiple keysets in fee map, got {}",
            total
        );
        assert_eq!(selected_proofs.len(), 1, "Should select only 1 proof");
    }

    // ========================================================================
    // select_exact_proofs Fee Tests (NUT-02 compliance)
    // ========================================================================

    /// Test select_exact_proofs with fee_ppk=100 (0.1 sat per proof)
    ///
    /// Per NUT-02 spec: fee = ceil(sum(input_fee_ppk) / 1000)
    /// For 1 proof with fee_ppk=100: fee = ceil(100/1000) = 1 sat
    #[test]
    fn test_select_exact_proofs_with_fee_ppk_100() {
        let active_id = id();
        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(
            active_id,
            (100, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );
        // Proofs: 64 + 32 + 4 = 100 sats total
        let proofs = vec![proof(64), proof(4), proof(32)];

        // Request 97 sats
        let (selected_proofs, exchange) = Wallet::select_exact_proofs(
            97.into(),
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            false,
        )
        .unwrap();

        assert!(exchange.is_some(), "Should have a proof to exchange");
        let (proof_to_exchange, exact_amount_to_melt) = exchange.unwrap();

        // selected_proofs should be [32, 4] = 36 sats
        assert_eq!(selected_proofs.len(), 2);
        let selected_total: u64 = selected_proofs.iter().map(|p| u64::from(p.amount)).sum();
        assert_eq!(selected_total, 36, "Selected proofs should total 36 sats");

        // proof_to_exchange should be the 64 sat proof
        assert_eq!(proof_to_exchange.amount, 64.into());

        // Per NUT-02: fee for 1 proof with fee_ppk=100 is ceil(100/1000) = 1 sat
        // exact_amount_to_melt = amount - (selected_total + fee)
        //                      = 97 - (36 + 1) = 60 sats
        assert_eq!(
            exact_amount_to_melt,
            60.into(),
            "exact_amount_to_melt should be 60 (97 - 36 - 1 fee)"
        );
    }

    /// Test select_exact_proofs with fee_ppk=1000 (1 sat per proof)
    ///
    /// Per NUT-02 spec: fee = ceil(sum(input_fee_ppk) / 1000)
    /// For 1 proof with fee_ppk=1000: fee = ceil(1000/1000) = 1 sat
    #[test]
    fn test_select_exact_proofs_with_fee_ppk_1000() {
        let active_id = id();
        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(
            active_id,
            (1000, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );
        let proofs = vec![proof(64), proof(4), proof(32)];

        let (selected_proofs, exchange) = Wallet::select_exact_proofs(
            97.into(),
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            false,
        )
        .unwrap();

        assert!(exchange.is_some(), "Should have a proof to exchange");
        let (proof_to_exchange, exact_amount_to_melt) = exchange.unwrap();

        assert_eq!(proof_to_exchange.amount, 64.into());

        let selected_total: u64 = selected_proofs.iter().map(|p| u64::from(p.amount)).sum();
        assert_eq!(selected_total, 36);

        // Per NUT-02: fee for 1 proof with fee_ppk=1000 is ceil(1000/1000) = 1 sat
        // exact_amount_to_melt = 97 - (36 + 1) = 60 sats
        assert_eq!(
            exact_amount_to_melt,
            60.into(),
            "exact_amount_to_melt should be 60 (97 - 36 - 1 fee)"
        );
    }

    /// Test select_exact_proofs with fee_ppk=200 (0.2 sat per proof)
    ///
    /// Per NUT-02 spec: fee = ceil(sum(input_fee_ppk) / 1000)
    /// For 1 proof with fee_ppk=200: fee = ceil(200/1000) = 1 sat
    #[test]
    fn test_select_exact_proofs_with_fee_ppk_200() {
        let active_id = id();
        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(
            active_id,
            (200, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );
        let proofs = vec![proof(64), proof(4), proof(32)];

        let (selected_proofs, exchange) = Wallet::select_exact_proofs(
            97.into(),
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            false,
        )
        .unwrap();

        assert!(exchange.is_some());
        let (proof_to_exchange, exact_amount_to_melt) = exchange.unwrap();

        assert_eq!(proof_to_exchange.amount, 64.into());

        let selected_total: u64 = selected_proofs.iter().map(|p| u64::from(p.amount)).sum();
        assert_eq!(selected_total, 36);

        // fee = ceil(200/1000) = 1 sat
        // exact_amount_to_melt = 97 - 36 - 1 = 60
        assert_eq!(
            exact_amount_to_melt,
            60.into(),
            "exact_amount_to_melt should be 60 (97 - 36 - 1 fee)"
        );
    }

    /// Test select_exact_proofs with fee_ppk=2000 (2 sats per proof)
    ///
    /// Per NUT-02 spec: fee = ceil(sum(input_fee_ppk) / 1000)
    /// For 1 proof with fee_ppk=2000: fee = ceil(2000/1000) = 2 sats
    #[test]
    fn test_select_exact_proofs_with_fee_ppk_2000() {
        let active_id = id();
        let mut keyset_fee_and_amounts = HashMap::new();
        keyset_fee_and_amounts.insert(
            active_id,
            (2000, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
        );
        let proofs = vec![proof(64), proof(4), proof(32)];

        let (selected_proofs, exchange) = Wallet::select_exact_proofs(
            97.into(),
            proofs,
            &vec![active_id],
            &keyset_fee_and_amounts,
            false,
        )
        .unwrap();

        assert!(exchange.is_some());
        let (proof_to_exchange, exact_amount_to_melt) = exchange.unwrap();

        assert_eq!(proof_to_exchange.amount, 64.into());

        let selected_total: u64 = selected_proofs.iter().map(|p| u64::from(p.amount)).sum();
        assert_eq!(selected_total, 36);

        // fee = ceil(2000/1000) = 2 sats
        // exact_amount_to_melt = 97 - 36 - 2 = 59
        assert_eq!(
            exact_amount_to_melt,
            59.into(),
            "exact_amount_to_melt should be 59 (97 - 36 - 2 fee)"
        );
    }

    /// Test that select_exact_proofs correctly handles the fee calculation
    /// by verifying the math: exact_amount = amount - selected_total - fee
    #[test]
    fn test_select_exact_proofs_fee_calculation_math() {
        let active_id = id();

        // Test multiple fee_ppk values
        let test_cases: Vec<(u64, u64)> = vec![
            (100, 1),    // ceil(100/1000) = 1
            (500, 1),    // ceil(500/1000) = 1
            (1000, 1),   // ceil(1000/1000) = 1
            (1001, 2),   // ceil(1001/1000) = 2
            (1500, 2),   // ceil(1500/1000) = 2
            (2000, 2),   // ceil(2000/1000) = 2
            (5000, 5),   // ceil(5000/1000) = 5
            (10000, 10), // ceil(10000/1000) = 10
        ];

        for (fee_ppk, expected_fee) in test_cases {
            let mut keyset_fee_and_amounts = HashMap::new();
            keyset_fee_and_amounts.insert(
                active_id,
                (fee_ppk, (0..32).map(|x| 2u64.pow(x)).collect::<Vec<_>>()).into(),
            );

            // Use proofs that will definitely trigger the exchange path
            // 128 + 64 + 32 = 224, request 200
            let proofs = vec![proof(128), proof(64), proof(32)];
            let amount: Amount = 200.into();

            let (selected_proofs, exchange) = Wallet::select_exact_proofs(
                amount,
                proofs,
                &vec![active_id],
                &keyset_fee_and_amounts,
                false,
            )
            .unwrap();

            if let Some((proof_to_exchange, exact_amount_to_melt)) = exchange {
                let selected_total: u64 = selected_proofs.iter().map(|p| u64::from(p.amount)).sum();

                // Per NUT-02 spec:
                // exact_amount_to_melt = amount - selected_total - fee
                let expected_exact = u64::from(amount) - selected_total - expected_fee;

                assert_eq!(
                    u64::from(exact_amount_to_melt),
                    expected_exact,
                    "fee_ppk={}: exact_amount_to_melt should be {} (amount {} - selected {} - fee {}), got {}",
                    fee_ppk,
                    expected_exact,
                    u64::from(amount),
                    selected_total,
                    expected_fee,
                    u64::from(exact_amount_to_melt)
                );

                // Also verify the proof to exchange has enough to cover exact_amount_to_melt
                assert!(
                    proof_to_exchange.amount >= exact_amount_to_melt,
                    "Proof to exchange ({}) must be >= exact_amount_to_melt ({})",
                    proof_to_exchange.amount,
                    exact_amount_to_melt
                );
            }
        }
    }
}
