use std::collections::{HashMap, HashSet};

use cdk_common::amount::KeysetFeeAndAmounts;
use cdk_common::database::DynWalletDatabaseTransaction;
use cdk_common::wallet::TransactionId;
use cdk_common::Id;
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::fees::calculate_fee;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{
    CheckStateRequest, Proof, ProofState, Proofs, PublicKey, SpendingConditions, State,
};
use crate::types::ProofInfo;
use crate::{ensure_cdk, Amount, Error, Wallet};

impl Wallet {
    /// Get unspent proofs for mint
    #[instrument(skip(self))]
    pub async fn get_unspent_proofs(&self) -> Result<Proofs, Error> {
        self.get_proofs_with(None, Some(vec![State::Unspent]), None)
            .await
    }

    /// Get pending [`Proofs`]
    #[instrument(skip(self))]
    pub async fn get_pending_proofs(&self) -> Result<Proofs, Error> {
        self.get_proofs_with(None, Some(vec![State::Pending]), None)
            .await
    }

    /// Get reserved [`Proofs`]
    #[instrument(skip(self))]
    pub async fn get_reserved_proofs(&self) -> Result<Proofs, Error> {
        self.get_proofs_with(None, Some(vec![State::Reserved]), None)
            .await
    }

    /// Get pending spent [`Proofs`]
    #[instrument(skip(self))]
    pub async fn get_pending_spent_proofs(&self) -> Result<Proofs, Error> {
        self.get_proofs_with(None, Some(vec![State::PendingSpent]), None)
            .await
    }

    /// Get this wallet's [Proofs] that match the args
    pub async fn get_proofs_with(
        &self,
        tx: Option<&mut DynWalletDatabaseTransaction<'_>>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Proofs, Error> {
        Ok(if let Some(tx) = tx {
            tx.get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit.clone()),
                state,
                spending_conditions,
            )
            .await?
            .into_iter()
            .map(|p| p.proof)
            .collect()
        } else {
            self.localstore
                .get_proofs(
                    Some(self.mint_url.clone()),
                    Some(self.unit.clone()),
                    state,
                    spending_conditions,
                )
                .await?
                .into_iter()
                .map(|p| p.proof)
                .collect()
        })
    }

    /// Return proofs to unspent allowing them to be selected and spent
    #[instrument(skip(self))]
    pub async fn unreserve_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Error> {
        let mut tx = self.localstore.begin_db_transaction().await?;
        tx.update_proofs_state(ys, State::Unspent).await?;
        Ok(tx.commit().await?)
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
        let mut tx = self.localstore.begin_db_transaction().await?;
        let _ = tx
            .remove_transaction(transaction_id)
            .await
            .inspect_err(|err| {
                tracing::warn!("Failed to remove transaction: {:?}", err);
            });
        tx.commit().await?;

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

        let mut tx = self.localstore.begin_db_transaction().await?;
        tx.update_proofs(vec![], spent_ys).await?;
        tx.commit().await?;

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

        let mut tx = self.localstore.begin_db_transaction().await?;
        tx.update_proofs(
            vec![],
            non_pending_proofs.into_iter().map(|p| p.y).collect(),
        )
        .await?;
        tx.commit().await?;

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
            input_proofs.sort_by(|a, b| a.amount.cmp(&b.amount));

            if let Some(proof_to_exchange) = input_proofs.pop() {
                let fee_ppk = fees_and_keyset_amounts
                    .get(&proof_to_exchange.keyset_id)
                    .map(|fee_and_amounts| fee_and_amounts.fee())
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

    /// Select proofs
    #[instrument(skip_all)]
    pub fn select_proofs(
        amount: Amount,
        proofs: Proofs,
        active_keyset_ids: &Vec<Id>,
        fees_and_keyset_amounts: &KeysetFeeAndAmounts,
        include_fees: bool,
    ) -> Result<Proofs, Error> {
        tracing::debug!(
            "amount={}, proofs={:?}",
            amount,
            proofs.iter().map(|p| p.amount.into()).collect::<Vec<u64>>()
        );
        if amount == Amount::ZERO {
            return Ok(vec![]);
        }
        ensure_cdk!(proofs.total_amount()? >= amount, Error::InsufficientFunds);

        // Sort proofs in descending order
        let mut proofs = proofs;
        proofs.sort_by(|a, b| a.cmp(b).reverse());

        // Track selected proofs and remaining amounts (include all inactive proofs first)
        let mut selected_proofs: HashSet<Proof> = proofs
            .iter()
            .filter(|p| !p.is_active(active_keyset_ids))
            .cloned()
            .collect();
        if selected_proofs.total_amount()? >= amount {
            tracing::debug!("All inactive proofs are sufficient");
            return Ok(selected_proofs.into_iter().collect());
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

        // Select proofs with the optimal amounts
        for (_, fee_and_amounts) in fees_and_keyset_amounts.iter() {
            // Split the amount into optimal amounts
            for optimal_amount in amount.split(fee_and_amounts) {
                if !select_proof(&proofs, optimal_amount, true) {
                    // Add the remaining amount to the remaining amounts because proof with the optimal amount was not found
                    remaining_amounts.push(optimal_amount);
                }
            }
        }

        // If all the optimal amounts are selected, return the selected proofs
        if remaining_amounts.is_empty() {
            tracing::debug!("All optimal amounts are selected");
            if include_fees {
                return Self::include_fees(
                    amount,
                    proofs,
                    selected_proofs.into_iter().collect(),
                    active_keyset_ids,
                    fees_and_keyset_amounts,
                );
            } else {
                return Ok(selected_proofs.into_iter().collect());
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
            selected_proofs.sort_by(|a, b| a.cmp(b).reverse());
            selected_proofs = Self::select_least_amount_over(selected_proofs, amount)?;
        }

        if include_fees {
            return Self::include_fees(
                amount,
                proofs,
                selected_proofs,
                active_keyset_ids,
                fees_and_keyset_amounts,
            );
        }

        Ok(selected_proofs)
    }

    fn select_least_amount_over(proofs: Proofs, amount: Amount) -> Result<Vec<Proof>, Error> {
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
                    Self::select_least_amount_over(left, amount),
                    Self::select_least_amount_over(right, amount),
                ) {
                    (Ok(left_proofs), Ok(right_proofs)) => {
                        let left_total_amount = left_proofs.total_amount()?;
                        let right_total_amount = right_proofs.total_amount()?;
                        if left_total_amount < right_total_amount {
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
                return Self::select_least_amount_over(left, amount);
            } else if right_amount >= amount {
                return Self::select_least_amount_over(right, amount);
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
    ) -> Result<Proofs, Error> {
        tracing::debug!("Including fees");
        let fee = calculate_fee(
            &selected_proofs.count_by_keyset(),
            &fees_and_keyset_amounts
                .iter()
                .map(|(key, values)| (*key, values.fee()))
                .collect(),
        )
        .unwrap_or_default();
        let net_amount = selected_proofs.total_amount()? - fee;
        tracing::debug!(
            "Net amount={}, fee={}, total amount={}",
            net_amount,
            fee,
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

        tracing::debug!("Net amount is less than the required amount");
        let remaining_amount = amount - net_amount;
        let remaining_proofs = proofs
            .into_iter()
            .filter(|p| !selected_proofs.contains(p))
            .collect::<Proofs>();
        selected_proofs.extend(Wallet::select_proofs(
            remaining_amount,
            remaining_proofs,
            active_keyset_ids,
            &HashMap::new(), // Fees are already calculated
            false,
        )?);
        tracing::debug!(
            "Selected proofs: {:?}",
            selected_proofs
                .iter()
                .map(|p| p.amount.into())
                .collect::<Vec<u64>>(),
        );
        Ok(selected_proofs)
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
}
