use std::collections::HashSet;

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
        Ok(self.localstore.set_unspent_proofs(ys).await?)
    }

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

    /// Select proofs to send
    #[instrument(skip_all)]
    pub async fn select_proofs_to_send(
        &self,
        amount: Amount,
        proofs: Proofs,
        include_fees: bool,
    ) -> Result<Proofs, Error> {
        // TODO: Check all proofs are same unit

        if proofs.total_amount()? < amount {
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
                - selected_proofs.total_amount()?;
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
            let selected_total = selected_proofs.total_amount()?;
            let fees = self.get_proofs_fee(&selected_proofs).await?;

            if selected_total >= amount + fees {
                return Ok(selected_proofs);
            }
        }

        active_proofs.sort_by(|a: &Proof, b: &Proof| b.cmp(a));

        for active_proof in active_proofs {
            selected_proofs.push(active_proof);
            let selected_total = selected_proofs.total_amount()?;
            let fees = self.get_proofs_fee(&selected_proofs).await?;

            if selected_total >= amount + fees {
                return Ok(selected_proofs);
            }
        }

        Err(Error::InsufficientFunds)
    }

    /// Select proofs v2
    pub fn select_proofs_v2(amount: Amount, proofs: Proofs) -> Result<Proofs, Error> {
        // println!(
        //     "select_proofs_v2: amount={}, proofs={:?}",
        //     amount,
        //     proofs.iter().map(|p| p.amount).collect::<Vec<_>>()
        // );
        if amount == Amount::ZERO {
            return Ok(vec![]);
        }
        if proofs.total_amount()? < amount {
            return Err(Error::InsufficientFunds);
        }

        // Sort proofs in descending order
        let mut proofs = proofs;
        proofs.sort_by(|a, b| a.cmp(b).reverse());

        // Split the amount into optimal amounts
        let optimal_amounts = amount.split();

        // Track selected proofs and remaining amounts
        let mut selected_proofs: HashSet<Proof> = HashSet::new();
        let mut remaining_amounts: Vec<Amount> = Vec::new();

        // Select proof with the exact amount and not already selected
        let mut select_proof = |proofs: &Proofs, amount: Amount, exact: bool| -> bool {
            // print!("select_proof: amount={}, exact={}... ", amount, exact);
            let mut last_proof = None;
            for proof in proofs.iter() {
                if !selected_proofs.contains(proof) {
                    if proof.amount == amount {
                        selected_proofs.insert(proof.clone());
                        // println!("found");
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
                // println!("found");
                true
            } else {
                // println!("not found");
                false
            }
        };

        // Select proofs with the optimal amounts
        // println!("optimal_amounts={:?}", optimal_amounts);
        for optimal_amount in optimal_amounts {
            if !select_proof(&proofs, optimal_amount, true) {
                // Add the remaining amount to the remaining amounts because proof with the optimal amount was not found
                remaining_amounts.push(optimal_amount);
            }
        }

        // If all the optimal amounts are selected, return the selected proofs
        if remaining_amounts.is_empty() {
            return Ok(selected_proofs.into_iter().collect());
        }

        // Select proofs with the remaining amounts by checking for 2 of the half amount, 4 of the quarter amount, etc.
        // println!("remaining_amounts={:?}", remaining_amounts);
        for remaining_amount in remaining_amounts {
            // println!("remaining_amount={}", remaining_amount);
            // Number of proofs to select
            let mut n = 2;

            let mut target_amount = remaining_amount;
            let mut found = false;
            while let Some(curr_amount) = target_amount.checked_div(Amount::from(2)) {
                // println!("curr_amount={}, n={}", curr_amount, n);
                if curr_amount == Amount::ZERO {
                    break;
                }

                // Select proofs with the current amount
                for _ in 0..n {
                    if select_proof(&proofs, curr_amount, true) {
                        n -= 1;
                    }
                }

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
                // println!("find closest amount over the remaining amount");
                select_proof(&proofs, remaining_amount, false);
            }
        }

        // Check if the selected proofs total amount is equal to the amount else filter out unnecessary proofs
        let mut selected_proofs = selected_proofs.into_iter().collect::<Vec<_>>();
        let total_amount = selected_proofs.total_amount()?;
        if total_amount != amount {
            // println!("total_amount={}, amount={}", total_amount, amount);
            selected_proofs.sort_by(|a, b| a.cmp(b).reverse());
            for i in 1..selected_proofs.len() {
                let (left, right) = selected_proofs.split_at(i);
                let left_amount = Amount::try_sum(left.iter().map(|p| p.amount))?;
                let right_amount = Amount::try_sum(right.iter().map(|p| p.amount))?;
                // println!(
                //     "left_amount={}, right_amount={}, left={:?}, right={:?}",
                //     left_amount,
                //     right_amount,
                //     left.iter().map(|p| p.amount).collect::<Vec<_>>(),
                //     right.iter().map(|p| p.amount).collect::<Vec<_>>()
                // );
                if left_amount >= amount && right_amount >= amount {
                    if left_amount <= right_amount {
                        return Ok(left.to_vec());
                    } else {
                        return Ok(right.to_vec());
                    }
                } else if left_amount >= amount {
                    return Ok(left.to_vec());
                } else if right_amount >= amount {
                    return Ok(right.to_vec());
                }
            }
        }

        Ok(selected_proofs)
    }
}

#[cfg(test)]
mod tests {
    use cdk_common::{secret::Secret, Amount, Id, Proof, PublicKey};

    use crate::Wallet;

    fn proof(amount: u64) -> Proof {
        Proof::new(
            Amount::from(amount),
            Id::from_bytes(&[0; 8]).unwrap(),
            Secret::generate(),
            PublicKey::from_hex(
                "03deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            )
            .unwrap(),
        )
    }

    #[test]
    fn test_select_proofs_v2_exact() {
        let proofs = vec![
            proof(1),
            proof(2),
            proof(4),
            proof(8),
            proof(16),
            proof(32),
            proof(64),
        ];
        let mut selected_proofs = Wallet::select_proofs_v2(77.into(), proofs).unwrap();
        selected_proofs.sort();
        assert_eq!(selected_proofs.len(), 4);
        assert_eq!(selected_proofs[0].amount, 1.into());
        assert_eq!(selected_proofs[1].amount, 4.into());
        assert_eq!(selected_proofs[2].amount, 8.into());
        assert_eq!(selected_proofs[3].amount, 64.into());
    }

    #[test]
    fn test_select_proofs_v2_over() {
        let proofs = vec![proof(1), proof(2), proof(4), proof(8), proof(32), proof(64)];
        let selected_proofs = Wallet::select_proofs_v2(21.into(), proofs).unwrap();
        assert_eq!(selected_proofs.len(), 1);
        assert_eq!(selected_proofs[0].amount, 32.into());
    }

    #[test]
    fn test_select_proofs_v2_smaller_over() {
        let proofs = vec![proof(8), proof(16), proof(32)];
        let selected_proofs = Wallet::select_proofs_v2(23.into(), proofs).unwrap();
        assert_eq!(selected_proofs.len(), 2);
        assert_eq!(selected_proofs[0].amount, 16.into());
        assert_eq!(selected_proofs[1].amount, 8.into());
    }

    #[test]
    fn test_select_proofs_v2_many_ones() {
        let proofs = (0..1024).into_iter().map(|_| proof(1)).collect::<Vec<_>>();
        let selected_proofs = Wallet::select_proofs_v2(1024.into(), proofs).unwrap();
        assert_eq!(selected_proofs.len(), 1024);
        for i in 0..1024 {
            assert_eq!(selected_proofs[i].amount, 1.into());
        }
    }

    #[test]
    fn test_select_proofs_v2_huge_proofs() {
        let proofs = (0..32)
            .flat_map(|i| {
                (0..5)
                    .into_iter()
                    .map(|_| proof(1 << i))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let mut selected_proofs =
            Wallet::select_proofs_v2(((1u64 << 32) - 1).into(), proofs).unwrap();
        selected_proofs.sort();
        assert_eq!(selected_proofs.len(), 32);
        for i in 0..32 {
            assert_eq!(selected_proofs[i].amount, (1 << i).into());
        }
    }
}
