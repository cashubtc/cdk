use std::collections::{HashMap, HashSet};

use cdk_common::wallet::TransactionId;
use cdk_common::Id;
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{
    CheckStateRequest, Proof, ProofState, Proofs, PublicKey, SpendingConditions, State,
};
use crate::types::ProofInfo;
use crate::{Amount, Error, Wallet};

impl Wallet {
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
        Ok(self
            .localstore
            .update_proofs_state(ys, State::Unspent)
            .await?)
    }

    /// Reclaim unspent proofs
    ///
    /// Checks the stats of [`Proofs`] swapping for a new [`Proof`] if unspent
    #[instrument(skip(self, proofs))]
    pub async fn reclaim_unspent(&self, proofs: Proofs) -> Result<(), Error> {
        let proof_ys = proofs.ys()?;

        let transaction_id = TransactionId::new(proof_ys.clone());

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

        match self.localstore.remove_transaction(transaction_id).await {
            Ok(_) => (),
            Err(e) => {
                tracing::warn!("Failed to remove transaction: {:?}", e);
            }
        }

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
                Some(vec![State::Pending, State::Reserved, State::PendingSpent]),
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
        // table. This is because a proof that has been created to send will be
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
        keyset_fees: &HashMap<Id, u64>,
        include_fees: bool,
    ) -> Result<(Proofs, Option<(Proof, Amount)>), Error> {
        let mut input_proofs =
            Self::select_proofs(amount, proofs, active_keyset_ids, keyset_fees, include_fees)?;
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
            input_proofs.sort_by(|a, b| a.amount.cmp(&b.amount));

            if let Some(proof_to_exchange) = input_proofs.pop() {
                let fee_ppk = keyset_fees
                    .get(&proof_to_exchange.keyset_id)
                    .cloned()
                    .unwrap_or_default()
                    .into();

                if let Some(exact_amount_to_melt) = total_for_proofs
                    .checked_sub(proof_to_exchange.amount)
                    .and_then(|a| a.checked_add(fee_ppk))
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

    /// Select proofs using RGLI (Randomized Greedy with Local Improvements) algorithm
    /// Ported from cashu-ts selectProofsToSend implementation
    #[instrument(skip_all)]
    pub fn select_proofs(
        amount: Amount,
        proofs: Proofs,
        _active_keyset_ids: &Vec<Id>,
        keyset_fees: &HashMap<Id, u64>,
        include_fees: bool,
    ) -> Result<Proofs, Error> {
        use std::collections::HashSet;

        use rand::seq::SliceRandom;

        tracing::debug!(
            "amount={}, proofs={:?}",
            amount,
            proofs.iter().map(|p| p.amount.into()).collect::<Vec<u64>>()
        );

        if amount == Amount::ZERO {
            return Ok(vec![]);
        }

        // RGLI algorithm constants
        const MAX_TRIALS: usize = 60;
        const MAX_OVRPCT: f64 = 0.0; // Acceptable close match overage (percent)
        const MAX_OVRAMT: u64 = 0; // Acceptable close match overage (absolute)
        const MAX_P2SWAP: usize = 5000; // Max number of Phase 2 improvement swaps
        const EXACT_MATCH: bool = false; // Allows close match (> amount_to_send + fee)

        let mut best_subset: Option<Vec<ProofWithFee>> = None;
        let mut best_delta = f64::INFINITY;
        let mut best_amount = Amount::ZERO;
        let mut best_fee_ppk = 0u64;

        #[derive(Clone, Debug)]
        struct ProofWithFee {
            proof: Proof,
            ex_fee: Amount, // Amount after fees deducted
            ppk_fee: u64,
        }

        // Helper function to calculate net amount after fees
        let sum_ex_fees = |amount: Amount, fee_ppk: u64| -> Amount {
            if include_fees {
                amount
                    .checked_sub(Amount::from((fee_ppk + 999) / 1000))
                    .unwrap_or(Amount::ZERO) // Ceiling division
            } else {
                amount
            }
        };

        // Calculate delta: excess over amount_to_send including fees
        let calculate_delta = |amount: Amount, fee_ppk: u64| -> f64 {
            let net_sum = sum_ex_fees(amount, fee_ppk);
            if net_sum < amount {
                f64::INFINITY // Invalid solution
            } else {
                let excess = amount
                    .checked_add(Amount::from(fee_ppk / 1000))
                    .and_then(|total| total.checked_sub(amount))
                    .unwrap_or(Amount::ZERO);
                let excess_u64: u64 = excess.into();
                excess_u64 as f64
            }
        };

        // Get fee for proof
        let get_proof_fee_ppk =
            |proof: &Proof| -> u64 { keyset_fees.get(&proof.keyset_id).copied().unwrap_or(0) };

        // Pre-processing: create ProofWithFee objects and calculate totals
        let mut total_amount = Amount::ZERO;
        let mut total_fee_ppk = 0u64;
        let mut proof_with_fees: Vec<ProofWithFee> = Vec::new();

        for proof in proofs {
            let ppk_fee = get_proof_fee_ppk(&proof);
            let ex_fee = if include_fees {
                proof
                    .amount
                    .checked_sub(Amount::from(ppk_fee / 1000))
                    .unwrap_or(Amount::ZERO)
            } else {
                proof.amount
            };

            // Sum all economical proofs (filtered below)
            if !include_fees || ex_fee > Amount::ZERO {
                total_amount = total_amount
                    .checked_add(proof.amount)
                    .ok_or(Error::AmountOverflow)?;
                total_fee_ppk += ppk_fee;
            }

            proof_with_fees.push(ProofWithFee {
                proof,
                ex_fee,
                ppk_fee,
            });
        }

        // Filter uneconomical proofs
        let mut spendable_proofs: Vec<ProofWithFee> = if include_fees {
            proof_with_fees
                .into_iter()
                .filter(|p| p.ex_fee > Amount::ZERO)
                .collect()
        } else {
            proof_with_fees
        };

        // Sort by ex_fee ascending
        spendable_proofs.sort_by(|a, b| a.ex_fee.cmp(&b.ex_fee));

        // Remove proofs too large to be useful and adjust totals
        if !spendable_proofs.is_empty() {
            let end_index = if EXACT_MATCH {
                // Keep proofs where ex_fee <= amount
                spendable_proofs
                    .iter()
                    .position(|p| p.ex_fee > amount)
                    .unwrap_or(spendable_proofs.len())
            } else {
                // Find next bigger proof and keep all up to that amount
                match spendable_proofs.iter().position(|p| p.ex_fee >= amount) {
                    Some(bigger_index) => {
                        let next_bigger_ex_fee = spendable_proofs[bigger_index].ex_fee;
                        spendable_proofs
                            .iter()
                            .position(|p| p.ex_fee > next_bigger_ex_fee)
                            .unwrap_or(spendable_proofs.len())
                    }
                    None => spendable_proofs.len(),
                }
            };

            // Adjust totals for removed proofs
            for removed_proof in &spendable_proofs[end_index..] {
                total_amount = total_amount
                    .checked_sub(removed_proof.proof.amount)
                    .unwrap_or(Amount::ZERO);
                total_fee_ppk = total_fee_ppk.saturating_sub(removed_proof.ppk_fee);
            }
            spendable_proofs.truncate(end_index);
        }

        // Validate using precomputed totals
        let total_net_sum = sum_ex_fees(total_amount, total_fee_ppk);
        if amount <= Amount::ZERO || amount > total_net_sum {
            if amount > total_net_sum {
                return Err(Error::InsufficientFunds);
            }
            return Ok(vec![]);
        }

        // Max acceptable amount for non-exact matches
        let amount_val: u64 = amount.into();
        let max_over_amount = std::cmp::min(
            Amount::from((amount_val as f64 * (1.0 + MAX_OVRPCT / 100.0)).ceil() as u64),
            amount
                .checked_add(Amount::from(MAX_OVRAMT))
                .unwrap_or(amount),
        );
        let max_over_amount = std::cmp::min(max_over_amount, total_net_sum);

        // Binary search helper for sorted array
        let binary_search_index =
            |arr: &[ProofWithFee], value: Amount, less_or_equal: bool| -> Option<usize> {
                let mut left = 0;
                let mut right = arr.len();
                let mut result: Option<usize> = None;

                while left < right {
                    let mid = left + (right - left) / 2;
                    let mid_value = arr[mid].ex_fee;

                    if less_or_equal {
                        if mid_value <= value {
                            result = Some(mid);
                            left = mid + 1;
                        } else {
                            right = mid;
                        }
                    } else {
                        if mid_value >= value {
                            result = Some(mid);
                            right = mid;
                        } else {
                            left = mid + 1;
                        }
                    }
                }

                if less_or_equal {
                    result
                } else if left < arr.len() {
                    Some(left)
                } else {
                    None
                }
            };

        // Insert into sorted array
        let insert_sorted = |arr: &mut Vec<ProofWithFee>, obj: ProofWithFee| {
            let value = obj.ex_fee;
            let mut left = 0;
            let mut right = arr.len();

            while left < right {
                let mid = left + (right - left) / 2;
                if arr[mid].ex_fee < value {
                    left = mid + 1;
                } else {
                    right = mid;
                }
            }
            arr.insert(left, obj);
        };

        // RGLI algorithm: Run multiple trials
        let mut rng = rand::rng();
        for trial in 0..MAX_TRIALS {
            // PHASE 1: Randomized Greedy Selection
            let mut s: Vec<ProofWithFee> = Vec::new();
            let mut current_amount = Amount::ZERO;
            let mut current_fee_ppk = 0u64;

            let mut shuffled_proofs = spendable_proofs.clone();
            shuffled_proofs.shuffle(&mut rng);

            for obj in shuffled_proofs {
                let new_amount = current_amount
                    .checked_add(obj.proof.amount)
                    .ok_or(Error::AmountOverflow)?;
                let new_fee_ppk = current_fee_ppk + obj.ppk_fee;
                let net_sum = sum_ex_fees(new_amount, new_fee_ppk);

                if EXACT_MATCH && net_sum > amount {
                    break;
                }

                s.push(obj);
                current_amount = new_amount;
                current_fee_ppk = new_fee_ppk;

                if net_sum >= amount {
                    break;
                }
            }

            // PHASE 2: Local Improvement
            let s_set: HashSet<_> = s.iter().map(|p| &p.proof).collect();
            let mut others: Vec<ProofWithFee> = spendable_proofs
                .iter()
                .filter(|p| !s_set.contains(&p.proof))
                .cloned()
                .collect();

            let mut indices: Vec<usize> = (0..s.len()).collect();
            indices.shuffle(&mut rng);
            indices.truncate(std::cmp::min(MAX_P2SWAP, s.len()));

            for &i in &indices {
                let net_sum = sum_ex_fees(current_amount, current_fee_ppk);
                if net_sum == amount
                    || (!EXACT_MATCH && net_sum >= amount && net_sum <= max_over_amount)
                {
                    break;
                }

                let obj_p = s[i].clone(); // Clone to avoid borrowing issues
                let temp_amount = current_amount
                    .checked_sub(obj_p.proof.amount)
                    .unwrap_or(Amount::ZERO);
                let temp_fee_ppk = current_fee_ppk.saturating_sub(obj_p.ppk_fee);
                let temp_net_sum = sum_ex_fees(temp_amount, temp_fee_ppk);
                let target = amount.checked_sub(temp_net_sum).unwrap_or(Amount::ZERO);

                if let Some(q_index) = binary_search_index(&others, target, EXACT_MATCH) {
                    let obj_q = &others[q_index];
                    if !EXACT_MATCH || obj_q.ex_fee > obj_p.ex_fee {
                        if target <= Amount::ZERO || obj_q.ex_fee <= obj_p.ex_fee {
                            // Perform the swap
                            s[i] = obj_q.clone();
                            current_amount = temp_amount
                                .checked_add(obj_q.proof.amount)
                                .ok_or(Error::AmountOverflow)?;
                            current_fee_ppk = temp_fee_ppk + obj_q.ppk_fee;

                            others.remove(q_index);
                            insert_sorted(&mut others, obj_p.clone());
                        }
                    }
                }
            }

            // Update best solution
            let delta = calculate_delta(current_amount, current_fee_ppk);
            if delta < best_delta {
                tracing::debug!(
                    "Best solution found in trial {} - amount: {}, delta: {}",
                    trial,
                    current_amount,
                    delta
                );

                let mut sorted_s = s.clone();
                sorted_s.sort_by(|a, b| b.ex_fee.cmp(&a.ex_fee));
                best_subset = Some(sorted_s);
                best_delta = delta;
                best_amount = current_amount;
                best_fee_ppk = current_fee_ppk;

                // PHASE 3: Final optimization - remove unnecessary large proofs
                if let Some(ref mut temp_s) = best_subset {
                    while temp_s.len() > 1 && best_delta > 0.0 {
                        if let Some(obj_p) = temp_s.pop() {
                            let temp_amount = best_amount
                                .checked_sub(obj_p.proof.amount)
                                .unwrap_or(Amount::ZERO);
                            let temp_fee_ppk = best_fee_ppk.saturating_sub(obj_p.ppk_fee);
                            let temp_delta = calculate_delta(temp_amount, temp_fee_ppk);

                            if temp_delta == f64::INFINITY {
                                temp_s.push(obj_p); // Put it back
                                break;
                            }

                            if temp_delta < best_delta {
                                best_delta = temp_delta;
                                best_amount = temp_amount;
                                best_fee_ppk = temp_fee_ppk;
                            } else {
                                temp_s.push(obj_p); // Put it back
                                break;
                            }
                        }
                    }
                }
            }

            // Check if solution is acceptable
            if let Some(ref _subset) = best_subset {
                if best_delta < f64::INFINITY {
                    let best_sum = sum_ex_fees(best_amount, best_fee_ppk);
                    if best_sum == amount
                        || (!EXACT_MATCH && best_sum >= amount && best_sum <= max_over_amount)
                    {
                        break;
                    }
                }
            }
        }

        // Return result
        if let Some(subset) = best_subset {
            if best_delta < f64::INFINITY {
                let best_proofs: Proofs = subset.into_iter().map(|obj| obj.proof).collect();
                return Ok(best_proofs);
            }
        }

        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use cdk_common::secret::Secret;
    use cdk_common::{Amount, Id, Proof, PublicKey};

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

    #[test]
    fn test_select_proofs_empty() {
        let proofs = vec![];
        let selected_proofs =
            Wallet::select_proofs(0.into(), proofs, &vec![id()], &HashMap::new(), false).unwrap();
        assert_eq!(selected_proofs.len(), 0);
    }

    #[test]
    fn test_select_proofs_insufficient() {
        let proofs = vec![proof(1), proof(2), proof(4)];
        let selected_proofs =
            Wallet::select_proofs(8.into(), proofs, &vec![id()], &HashMap::new(), false);
        assert!(selected_proofs.is_err());
    }

    #[test]
    fn test_select_proofs_basic_functionality() {
        // Test that RGLI algorithm can find a solution for basic cases
        let proofs = vec![
            proof(1),
            proof(2),
            proof(4),
            proof(8),
            proof(16),
            proof(32),
            proof(64),
        ];
        let selected_proofs =
            Wallet::select_proofs(77.into(), proofs, &vec![id()], &HashMap::new(), false).unwrap();

        // Should select proofs that sum to at least 77
        let total: u64 = selected_proofs
            .iter()
            .map(|p| p.amount.to_i64().unwrap() as u64)
            .sum();
        assert!(total >= 77);
        assert!(!selected_proofs.is_empty());
    }

    #[test]
    fn test_select_proofs_single_proof_sufficient() {
        let proofs = vec![proof(1), proof(2), proof(4), proof(8), proof(32), proof(64)];
        let selected_proofs =
            Wallet::select_proofs(31.into(), proofs, &vec![id()], &HashMap::new(), false).unwrap();

        // Should find a valid solution (likely the 32 proof or combination)
        let total: u64 = selected_proofs
            .iter()
            .map(|p| p.amount.to_i64().unwrap() as u64)
            .sum();
        assert!(total >= 31);
        assert!(!selected_proofs.is_empty());
    }

    #[test]
    fn test_select_proofs_multiple_needed() {
        let proofs = vec![proof(8), proof(16), proof(32)];
        let selected_proofs =
            Wallet::select_proofs(23.into(), proofs, &vec![id()], &HashMap::new(), false).unwrap();

        // Should find a combination that covers 23
        let total: u64 = selected_proofs
            .iter()
            .map(|p| p.amount.to_i64().unwrap() as u64)
            .sum();
        assert!(total >= 23);
        assert!(!selected_proofs.is_empty());
    }

    #[test]
    fn test_select_proofs_many_ones() {
        let proofs = (0..1024).map(|_| proof(1)).collect::<Vec<_>>();
        let selected_proofs =
            Wallet::select_proofs(1024.into(), proofs, &vec![id()], &HashMap::new(), false)
                .unwrap();
        assert_eq!(selected_proofs.len(), 1024);
        selected_proofs
            .iter()
            .for_each(|proof| assert_eq!(proof.amount, Amount::ONE));
    }

    #[test]
    fn test_select_proof_change() {
        let proofs = vec![proof(64), proof(4), proof(32)];
        let (selected_proofs, exchange) =
            Wallet::select_exact_proofs(97.into(), proofs, &vec![id()], &HashMap::new(), false)
                .unwrap();
        assert!(exchange.is_some());
        let (proof_to_exchange, amount) = exchange.unwrap();

        assert_eq!(selected_proofs.len(), 2);
        assert_eq!(proof_to_exchange.amount, 64.into());
        assert_eq!(amount, 61.into());
    }

    #[test]
    fn test_select_proofs_optimal_large_set() {
        let proofs = (0..32)
            .flat_map(|i| (0..5).map(|_| proof(1 << i)).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        let selected_proofs = Wallet::select_proofs(
            ((1u64 << 32) - 1).into(),
            proofs,
            &vec![id()],
            &HashMap::new(),
            false,
        )
        .unwrap();

        // Should find a solution close to optimal (powers of 2)
        let total: u64 = selected_proofs
            .iter()
            .map(|p| p.amount.to_i64().unwrap() as u64)
            .sum();
        assert!(total >= (1u64 << 32) - 1);
        // The RGLI algorithm should find an efficient solution
        assert!(selected_proofs.len() <= 40); // Should be reasonably efficient
    }

    #[test]
    fn test_select_proofs_with_fees_basic() {
        let proofs = vec![proof(64), proof(4), proof(32)];
        let mut keyset_fees = HashMap::new();
        keyset_fees.insert(id(), 100);
        let selected_proofs =
            Wallet::select_proofs(10.into(), proofs, &vec![id()], &keyset_fees, false).unwrap();

        // Should find a valid solution
        let total: u64 = selected_proofs
            .iter()
            .map(|p| p.amount.to_i64().unwrap() as u64)
            .sum();
        assert!(total >= 10);
        assert!(!selected_proofs.is_empty());
    }
}
