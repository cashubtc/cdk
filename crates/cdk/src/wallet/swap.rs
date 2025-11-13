use cdk_common::database::DynWalletDatabaseTransaction;
use cdk_common::nut02::KeySetInfosMethods;
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::dhke::construct_proofs;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{
    nut10, PreMintSecrets, PreSwap, Proofs, PublicKey, SpendingConditions, State, SwapRequest,
};
use crate::types::ProofInfo;
use crate::{ensure_cdk, Amount, Error, Wallet};

impl Wallet {
    /// Swap
    #[instrument(skip(self, input_proofs))]
    pub async fn swap(
        &self,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        input_proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
        include_fees: bool,
    ) -> Result<Option<Proofs>, Error> {
        tracing::info!("Swapping");
        let mint_url = &self.mint_url;
        let unit = &self.unit;

        let pre_swap = self
            .create_swap(
                self.localstore.begin_db_transaction().await?,
                amount,
                amount_split_target.clone(),
                input_proofs.clone(),
                spending_conditions.clone(),
                include_fees,
            )
            .await?;

        let swap_response = self
            .try_proof_operation_or_reclaim(
                pre_swap.swap_request.inputs().clone(),
                self.client.post_swap(pre_swap.swap_request),
            )
            .await?;

        let active_keyset_id = pre_swap.pre_mint_secrets.keyset_id;
        let fee_and_amounts = self
            .get_keyset_fees_and_amounts_by_id(active_keyset_id)
            .await?;

        let active_keys = self.load_keyset_keys(active_keyset_id).await?;

        let post_swap_proofs = construct_proofs(
            swap_response.signatures,
            pre_swap.pre_mint_secrets.rs(),
            pre_swap.pre_mint_secrets.secrets(),
            &active_keys,
        )?;

        let mut added_proofs = Vec::new();
        let change_proofs;
        let send_proofs;
        match amount {
            Some(amount) => {
                let (proofs_with_condition, proofs_without_condition): (Proofs, Proofs) =
                    post_swap_proofs.into_iter().partition(|p| {
                        let nut10_secret: Result<nut10::Secret, _> = p.secret.clone().try_into();

                        nut10_secret.is_ok()
                    });

                let (proofs_to_send, proofs_to_keep) = match spending_conditions {
                    Some(_) => (proofs_with_condition, proofs_without_condition),
                    None => {
                        let mut all_proofs = proofs_without_condition;
                        all_proofs.reverse();

                        let mut proofs_to_send = Proofs::new();
                        let mut proofs_to_keep = Proofs::new();
                        let mut amount_split =
                            amount.split_targeted(&amount_split_target, &fee_and_amounts)?;

                        for proof in all_proofs {
                            if let Some(idx) = amount_split.iter().position(|&a| a == proof.amount)
                            {
                                proofs_to_send.push(proof);
                                amount_split.remove(idx);
                            } else {
                                proofs_to_keep.push(proof);
                            }
                        }

                        (proofs_to_send, proofs_to_keep)
                    }
                };

                let send_amount = proofs_to_send.total_amount()?;

                if send_amount.ne(&(amount + pre_swap.fee)) {
                    tracing::warn!(
                        "Send amount proofs is {:?} expected {:?}",
                        send_amount,
                        amount
                    );
                }

                let send_proofs_info = proofs_to_send
                    .clone()
                    .into_iter()
                    .map(|proof| {
                        ProofInfo::new(proof, mint_url.clone(), State::Reserved, unit.clone())
                    })
                    .collect::<Result<Vec<ProofInfo>, _>>()?;
                added_proofs = send_proofs_info;

                change_proofs = proofs_to_keep;
                send_proofs = Some(proofs_to_send);
            }
            None => {
                change_proofs = post_swap_proofs;
                send_proofs = None;
            }
        }

        let keep_proofs = change_proofs
            .into_iter()
            .map(|proof| ProofInfo::new(proof, mint_url.clone(), State::Unspent, unit.clone()))
            .collect::<Result<Vec<ProofInfo>, _>>()?;
        added_proofs.extend(keep_proofs);

        // Remove spent proofs used as inputs
        let deleted_ys = input_proofs
            .into_iter()
            .map(|proof| proof.y())
            .collect::<Result<Vec<PublicKey>, _>>()?;

        let mut tx = self.localstore.begin_db_transaction().await?;

        tx.update_proofs(added_proofs, deleted_ys).await?;
        tx.commit().await?;

        Ok(send_proofs)
    }

    /// Swap from unspent proofs in db
    #[instrument(skip(self))]
    pub async fn swap_from_unspent(
        &self,
        amount: Amount,
        conditions: Option<SpendingConditions>,
        include_fees: bool,
    ) -> Result<Proofs, Error> {
        let available_proofs = self
            .localstore
            .get_proofs(
                Some(self.mint_url.clone()),
                Some(self.unit.clone()),
                Some(vec![State::Unspent]),
                None,
            )
            .await?;

        let (available_proofs, proofs_sum) = available_proofs
            .into_iter()
            .map(|p| p.proof)
            .try_fold((Vec::new(), Amount::ZERO), |(mut acc1, acc2), p| {
                let new_sum = acc2.checked_add(p.amount).ok_or(Error::AmountOverflow)?;
                acc1.push(p);
                Ok::<_, Error>((acc1, new_sum))
            })?;

        ensure_cdk!(proofs_sum >= amount, Error::InsufficientFunds);

        let active_keyset_ids = self
            .get_mint_keysets()
            .await?
            .active()
            .map(|k| k.id)
            .collect();

        let keyset_fees = self.get_keyset_fees_and_amounts().await?;
        let proofs = Wallet::select_proofs(
            amount,
            available_proofs,
            &active_keyset_ids,
            &keyset_fees,
            true,
        )?;

        self.swap(
            Some(amount),
            SplitTarget::default(),
            proofs,
            conditions,
            include_fees,
        )
        .await?
        .ok_or(Error::InsufficientFunds)
    }

    /// Create Swap Payload
    #[instrument(skip(self, proofs, tx))]
    pub async fn create_swap(
        &self,
        mut tx: DynWalletDatabaseTransaction<'_>,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
        include_fees: bool,
    ) -> Result<PreSwap, Error> {
        tracing::info!("Creating swap");
        let active_keyset_id = self.fetch_active_keyset().await?.id;

        // Desired amount is either amount passed or value of all proof
        let proofs_total = proofs.total_amount()?;
        let fee = self.get_proofs_fee(&proofs).await?;
        let fee_and_amounts = self
            .get_keyset_fees_and_amounts_by_id(active_keyset_id)
            .await?;

        let ys: Vec<PublicKey> = proofs.ys()?;
        tx.update_proofs_state(ys, State::Reserved).await?;

        let total_to_subtract = amount
            .unwrap_or(Amount::ZERO)
            .checked_add(fee)
            .ok_or(Error::AmountOverflow)?;

        let change_amount: Amount = proofs_total
            .checked_sub(total_to_subtract)
            .ok_or(Error::InsufficientFunds)?;

        let (send_amount, change_amount) = match include_fees {
            true => {
                let split_count = amount
                    .unwrap_or(Amount::ZERO)
                    .split_targeted(&SplitTarget::default(), &fee_and_amounts)
                    .unwrap()
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
                self.determine_split_target_values(&mut tx, change_amount, &fee_and_amounts)
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
                    .split_targeted(&change_split_target, &fee_and_amounts)?
                    .len() as u32
            }
            None => {
                // For no spending conditions, count both send and change secrets
                let send_count = send_amount
                    .unwrap_or(Amount::ZERO)
                    .split_targeted(&SplitTarget::default(), &fee_and_amounts)?
                    .len() as u32;
                let change_count = change_amount
                    .split_targeted(&change_split_target, &fee_and_amounts)?
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

            let new_counter = tx
                .increment_keyset_counter(&active_keyset_id, total_secrets_needed)
                .await?;

            new_counter - total_secrets_needed
        } else {
            0 // No secrets needed, don't increment the counter
        };

        let mut count = starting_counter;

        let (mut desired_messages, change_messages) = match spending_conditions {
            Some(conditions) => {
                let change_premint_secrets = PreMintSecrets::from_seed(
                    active_keyset_id,
                    count,
                    &self.seed,
                    change_amount,
                    &change_split_target,
                    &fee_and_amounts,
                )?;

                derived_secret_count = change_premint_secrets.len();

                (
                    PreMintSecrets::with_conditions(
                        active_keyset_id,
                        send_amount.unwrap_or(Amount::ZERO),
                        &SplitTarget::default(),
                        &conditions,
                        &fee_and_amounts,
                    )?,
                    change_premint_secrets,
                )
            }
            None => {
                let premint_secrets = PreMintSecrets::from_seed(
                    active_keyset_id,
                    count,
                    &self.seed,
                    send_amount.unwrap_or(Amount::ZERO),
                    &SplitTarget::default(),
                    &fee_and_amounts,
                )?;

                count += premint_secrets.len() as u32;

                let change_premint_secrets = PreMintSecrets::from_seed(
                    active_keyset_id,
                    count,
                    &self.seed,
                    change_amount,
                    &change_split_target,
                    &fee_and_amounts,
                )?;

                derived_secret_count = change_premint_secrets.len() + premint_secrets.len();

                (premint_secrets, change_premint_secrets)
            }
        };

        // Combine the BlindedMessages totaling the desired amount with change
        desired_messages.combine(change_messages);
        // Sort the premint secrets to avoid finger printing
        desired_messages.sort_secrets();

        let swap_request = SwapRequest::new(proofs, desired_messages.blinded_messages());

        tx.commit().await?;

        Ok(PreSwap {
            pre_mint_secrets: desired_messages,
            swap_request,
            derived_secret_count: derived_secret_count as u32,
            fee,
        })
    }
}
