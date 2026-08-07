//! Swap module for the wallet.
//!
//! This module provides functionality for swapping proofs.

use cdk_common::amount::FeeAndAmounts;
use cdk_common::Id;
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::fees::ProofsFeeBreakdown;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{PreMintSecrets, PreSwap, Proofs, PublicKey, SpendingConditions, SwapRequest};
use crate::{Amount, Error, Wallet};

pub(crate) mod saga;

use saga::SwapSaga;

/// Controls whether swap operations should reserve proofs in the database.
///
/// When a swap is performed as a nested operation within a parent saga
/// (send, melt, receive), the parent has already reserved the proofs.
/// Passing [`ProofReservation::Skip`] avoids a double-reservation conflict
/// that would otherwise fail with `ProofNotUnspent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProofReservation {
    /// Reserve proofs as part of the swap (default for standalone swaps).
    Reserve,
    /// Skip reservation because a parent saga already reserved these proofs.
    Skip,
}

impl Wallet {
    /// Swap proofs using the saga pattern.
    ///
    /// This method reserves the input proofs before performing the swap,
    /// ensuring they cannot be used by concurrent operations.
    #[instrument(skip(self, input_proofs))]
    pub async fn swap(
        &self,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        input_proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
        include_fees: bool,
        use_p2bk: bool,
    ) -> Result<Option<Proofs>, Error> {
        self.swap_internal(
            amount,
            amount_split_target,
            input_proofs,
            spending_conditions,
            include_fees,
            use_p2bk,
            ProofReservation::Reserve,
        )
        .await
    }

    /// Swap proofs without reserving them first.
    ///
    /// This is intended for internal use by parent sagas (send, melt, receive)
    /// that have already reserved the proofs. Calling this on unreserved proofs
    /// bypasses the reservation safety check.
    #[instrument(skip(self, input_proofs))]
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn swap_no_reserve(
        &self,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        input_proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
        include_fees: bool,
        use_p2bk: bool,
    ) -> Result<Option<Proofs>, Error> {
        self.swap_internal(
            amount,
            amount_split_target,
            input_proofs,
            spending_conditions,
            include_fees,
            use_p2bk,
            ProofReservation::Skip,
        )
        .await
    }

    /// Internal swap implementation with explicit proof reservation control.
    #[allow(clippy::too_many_arguments)]
    async fn swap_internal(
        &self,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        input_proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
        include_fees: bool,
        use_p2bk: bool,
        proof_reservation: ProofReservation,
    ) -> Result<Option<Proofs>, Error> {
        tracing::info!("Swapping");

        self.retry_on_inactive_keyset(|| async {
            let saga = SwapSaga::new(self);
            let saga = saga
                .prepare(
                    amount,
                    amount_split_target.clone(),
                    input_proofs.clone(),
                    spending_conditions.clone(),
                    use_p2bk,
                    include_fees,
                    proof_reservation,
                )
                .await?;
            let saga = saga.execute().await?;
            Ok(saga.into_send_proofs())
        })
        .await
    }

    /// Create Swap Payload
    #[instrument(skip(self, proofs))]
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_swap(
        &self,
        operation_id: &uuid::Uuid,
        active_keyset_id: Id,
        fee_and_amounts: &FeeAndAmounts,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
        include_fees: bool,
        use_p2bk: bool,
        proofs_fee_breakdown: &ProofsFeeBreakdown,
        proof_reservation: ProofReservation,
    ) -> Result<PreSwap, Error> {
        tracing::info!("Creating swap");

        // Desired amount is either amount passed or value of all proof
        let proofs_total = proofs.total_amount()?;

        if proof_reservation == ProofReservation::Reserve {
            let ys: Vec<PublicKey> = proofs.ys()?;
            self.localstore.reserve_proofs(ys, operation_id).await?;
        }

        let total_to_subtract = amount
            .unwrap_or(Amount::ZERO)
            .checked_add(proofs_fee_breakdown.total)
            .ok_or(Error::AmountOverflow)?;

        let change_amount: Amount = proofs_total
            .checked_sub(total_to_subtract)
            .ok_or(Error::InsufficientFunds)?;

        let (send_amount, change_amount) = match include_fees {
            true => {
                let split_count = amount
                    .unwrap_or(Amount::ZERO)
                    .split_targeted(&SplitTarget::default(), fee_and_amounts)?
                    .len();

                let fee_to_redeem = self
                    .get_keyset_count_fee(&active_keyset_id, split_count as u64)
                    .await?;

                (
                    amount
                        .map(|a| a.checked_add(fee_to_redeem).ok_or(Error::AmountOverflow))
                        .transpose()?,
                    change_amount
                        .checked_sub(fee_to_redeem)
                        .ok_or(Error::InsufficientFunds)?,
                )
            }
            false => (amount, change_amount),
        };

        // If a non None split target is passed use that
        // else use state refill
        let change_split_target = match amount_split_target {
            SplitTarget::None => {
                self.determine_split_target_values(change_amount, fee_and_amounts)
                    .await?
            }
            s => s,
        };

        let derived_secret_count;

        // Calculate total secrets needed and atomically reserve counter range
        let total_secrets_needed = match spending_conditions {
            Some(_) => {
                // For spending conditions, we only need to count change secrets
                change_amount
                    .split_targeted(&change_split_target, fee_and_amounts)?
                    .len() as u32
            }
            None => {
                // For no spending conditions, count both send and change secrets
                let send_count = send_amount
                    .unwrap_or(Amount::ZERO)
                    .split_targeted(&SplitTarget::default(), fee_and_amounts)?
                    .len() as u32;
                let change_count = change_amount
                    .split_targeted(&change_split_target, fee_and_amounts)?
                    .len() as u32;
                send_count + change_count
            }
        };

        // Atomically get the counter range we need
        let starting_counter = if total_secrets_needed > 0 {
            tracing::debug!(
                "Incrementing keyset {} counter by {}",
                active_keyset_id,
                total_secrets_needed
            );

            let new_counter = self
                .localstore
                .increment_keyset_counter(&active_keyset_id, total_secrets_needed)
                .await?;

            new_counter - total_secrets_needed
        } else {
            0
        };

        let mut count = starting_counter;

        let has_spending_conditions = spending_conditions.is_some();
        let mut p2bk_ephemeral_key = None;
        let (mut desired_messages, mut change_messages) = match spending_conditions {
            Some(conditions) => {
                let change_premint_secrets = PreMintSecrets::from_seed(
                    active_keyset_id,
                    count,
                    &self.seed,
                    change_amount,
                    &change_split_target,
                    fee_and_amounts,
                )?;

                derived_secret_count = change_premint_secrets.len();

                let (send_secrets, ephemeral_key) = if use_p2bk {
                    if let SpendingConditions::P2PKConditions { data, conditions } = conditions {
                        let is_sig_all = conditions
                            .as_ref()
                            .is_some_and(|c| c.sig_flag == crate::nuts::nut11::SigFlag::SigAll);
                        let amount_split = send_amount
                            .unwrap_or(Amount::ZERO)
                            .split_targeted(&SplitTarget::default(), fee_and_amounts)?;
                        let keys_count = if is_sig_all { 1 } else { amount_split.len() };
                        let ephemeral_keys: Vec<_> = (0..keys_count)
                            .map(|_| crate::nuts::nut01::SecretKey::generate())
                            .collect();
                        (
                            PreMintSecrets::with_p2bk(
                                active_keyset_id,
                                send_amount.unwrap_or(Amount::ZERO),
                                &SplitTarget::default(),
                                data,
                                conditions,
                                &ephemeral_keys,
                                fee_and_amounts,
                            )?,
                            Some(ephemeral_keys),
                        )
                    } else {
                        return Err(Error::Custom("P2BK requires P2PK conditions".to_string()));
                    }
                } else {
                    (
                        PreMintSecrets::with_conditions(
                            active_keyset_id,
                            send_amount.unwrap_or(Amount::ZERO),
                            &SplitTarget::default(),
                            &conditions,
                            fee_and_amounts,
                        )?,
                        None,
                    )
                };

                p2bk_ephemeral_key = ephemeral_key;
                (send_secrets, change_premint_secrets)
            }
            None => {
                let premint_secrets = PreMintSecrets::from_seed(
                    active_keyset_id,
                    count,
                    &self.seed,
                    send_amount.unwrap_or(Amount::ZERO),
                    &SplitTarget::default(),
                    fee_and_amounts,
                )?;

                count += premint_secrets.len() as u32;

                let change_premint_secrets = PreMintSecrets::from_seed(
                    active_keyset_id,
                    count,
                    &self.seed,
                    change_amount,
                    &change_split_target,
                    fee_and_amounts,
                )?;

                derived_secret_count = change_premint_secrets.len() + premint_secrets.len();

                (premint_secrets, change_premint_secrets)
            }
        };

        if has_spending_conditions {
            self.add_nut342_metadata(active_keyset_id, starting_counter, &mut change_messages)
                .await?;
        } else {
            self.add_nut342_metadata(active_keyset_id, starting_counter, &mut desired_messages)
                .await?;
            self.add_nut342_metadata(
                active_keyset_id,
                starting_counter.saturating_add(desired_messages.len() as u32),
                &mut change_messages,
            )
            .await?;
        }

        // Combine the BlindedMessages totaling the desired amount with change
        desired_messages.combine(change_messages);
        // Sort the premint secrets to avoid finger printing
        desired_messages.sort_secrets();

        let swap_request = SwapRequest::new(proofs, desired_messages.blinded_messages());

        Ok(PreSwap {
            pre_mint_secrets: desired_messages,
            swap_request,
            derived_secret_count: derived_secret_count as u32,
            fee: proofs_fee_breakdown.total,
            p2bk_secret_keys: p2bk_ephemeral_key,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk_common::wallet::{KeysetLoadPolicy, ProofInfo};
    use cdk_common::CurrencyUnit;

    use crate::amount::SplitTarget;
    use crate::nuts::State;
    use crate::wallet::test_utils::{
        create_test_db, create_test_wallet_with_mock, make_inactive_keyset, test_keyset,
        test_keyset_id, test_mint_url, test_proof, MockMintConnector,
    };
    use crate::Error;

    /// When the mint returns InactiveKeyset on a swap and the active keyset
    /// has rotated, the wallet should retry the swap with the new keyset.
    #[tokio::test]
    async fn swap_retries_on_inactive_keyset_after_rotation() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        db.add_mint(mint_url.clone(), None).await.unwrap();

        let mock = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), mock.clone()).await;

        // Prime cache with keyset A as active
        wallet.keysets(KeysetLoadPolicy::Refresh).await.unwrap();

        let keyset_a_id = test_keyset_id();

        // Store proofs in the DB so reserve_proofs can find them.
        // Use amounts 1+2=3 so that after fee (1 sat for 2 inputs), the output
        // amount of 2 is expressible in keyset B's shifted denominations (min 2).
        let proof1 = test_proof(keyset_a_id, 1);
        let proof2 = test_proof(keyset_a_id, 2);
        let pi1 = ProofInfo::new(
            proof1.clone(),
            mint_url.clone(),
            State::Unspent,
            CurrencyUnit::Sat,
        )
        .unwrap();
        let pi2 =
            ProofInfo::new(proof2.clone(), mint_url, State::Unspent, CurrencyUnit::Sat).unwrap();
        db.update_proofs(vec![pi1, pi2], vec![]).await.unwrap();

        // Rotate keysets on the mock: A becomes inactive, B becomes active
        let mut old = test_keyset();
        old.active = Some(false);
        let mut rotated = make_inactive_keyset();
        rotated.active = Some(true);
        let keyset_b_id = rotated.id;
        mock.set_mint_keys_response(Ok(vec![old, rotated]));

        // First post_swap → InactiveKeyset, second → TokenAlreadySpent
        // (we can't construct valid blind signatures, so use a different error
        // to prove the retry happened)
        mock.push_post_swap_response(Err(Error::InactiveKeyset));
        mock.push_post_swap_response(Err(Error::TokenAlreadySpent));

        let result = wallet
            .swap(
                None,
                SplitTarget::default(),
                vec![proof1, proof2],
                None,
                false,
                false,
            )
            .await;

        // The second attempt's error should surface, not InactiveKeyset
        assert!(
            matches!(result, Err(Error::TokenAlreadySpent)),
            "expected TokenAlreadySpent from retry, got: {result:?}"
        );

        let requests = mock.captured_swap_requests();
        assert_eq!(requests.len(), 2, "post_swap should be called twice");

        // First attempt targeted keyset A
        assert!(
            requests[0]
                .outputs()
                .iter()
                .all(|o| o.keyset_id == keyset_a_id),
            "first swap attempt should target keyset A"
        );
        // Second attempt targeted keyset B
        assert!(
            requests[1]
                .outputs()
                .iter()
                .all(|o| o.keyset_id == keyset_b_id),
            "second swap attempt should target keyset B"
        );
    }

    /// When the mint returns InactiveKeyset but the active keyset hasn't
    /// changed, the wallet should NOT retry and return the error immediately.
    #[tokio::test]
    async fn swap_does_not_retry_when_keyset_unchanged() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        db.add_mint(mint_url.clone(), None).await.unwrap();

        let mock = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), mock.clone()).await;

        // Prime cache with keyset A as active
        wallet.keysets(KeysetLoadPolicy::Refresh).await.unwrap();

        // Store proofs in the DB
        let proof1 = test_proof(test_keyset_id(), 1);
        let proof2 = test_proof(test_keyset_id(), 2);
        let pi1 = ProofInfo::new(
            proof1.clone(),
            mint_url.clone(),
            State::Unspent,
            CurrencyUnit::Sat,
        )
        .unwrap();
        let pi2 =
            ProofInfo::new(proof2.clone(), mint_url, State::Unspent, CurrencyUnit::Sat).unwrap();
        db.update_proofs(vec![pi1, pi2], vec![]).await.unwrap();

        // Don't rotate keysets — mock still returns keyset A as active
        mock.push_post_swap_response(Err(Error::InactiveKeyset));

        let result = wallet
            .swap(
                None,
                SplitTarget::default(),
                vec![proof1, proof2],
                None,
                false,
                false,
            )
            .await;

        assert!(
            matches!(result, Err(Error::InactiveKeyset)),
            "expected InactiveKeyset without rotation, got: {result:?}"
        );

        let requests = mock.captured_swap_requests();
        assert_eq!(
            requests.len(),
            1,
            "post_swap should be called only once (no retry)"
        );
    }
}
