use tracing::instrument;

use crate::amount::SplitTarget;
use crate::dhke::construct_proofs;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{
    nut10, PreMintSecrets, PreSwap, Proofs, PublicKey, SpendingConditions, State, SwapRequest,
};
use crate::types::ProofInfo;
use crate::{Amount, Error, Wallet};

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
        let mint_url = &self.mint_url;
        let unit = &self.unit;

        let pre_swap = self
            .create_swap(
                amount,
                amount_split_target,
                input_proofs.clone(),
                spending_conditions.clone(),
                include_fees,
            )
            .await?;

        let swap_response = self.client.post_swap(pre_swap.swap_request).await?;

        let active_keyset_id = pre_swap.pre_mint_secrets.keyset_id;

        let active_keys = self
            .localstore
            .get_keys(&active_keyset_id)
            .await?
            .ok_or(Error::NoActiveKeyset)?;

        let post_swap_proofs = construct_proofs(
            swap_response.signatures,
            pre_swap.pre_mint_secrets.rs(),
            pre_swap.pre_mint_secrets.secrets(),
            &active_keys,
        )?;

        self.localstore
            .increment_keyset_counter(&active_keyset_id, pre_swap.derived_secret_count)
            .await?;

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

                        let mut proofs_to_send: Proofs = Vec::new();
                        let mut proofs_to_keep = Vec::new();

                        for proof in all_proofs {
                            let proofs_to_send_amount = proofs_to_send.total_amount()?;
                            if proof.amount + proofs_to_send_amount <= amount + pre_swap.fee {
                                proofs_to_send.push(proof);
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

        self.localstore
            .update_proofs(added_proofs, deleted_ys)
            .await?;
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

        let (available_proofs, proofs_sum) = available_proofs.into_iter().map(|p| p.proof).fold(
            (Vec::new(), Amount::ZERO),
            |(mut acc1, mut acc2), p| {
                acc2 += p.amount;
                acc1.push(p);
                (acc1, acc2)
            },
        );

        if proofs_sum < amount {
            return Err(Error::InsufficientFunds);
        }

        let proofs = self.select_proofs_to_swap(amount, available_proofs).await?;

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
    #[instrument(skip(self, proofs))]
    pub async fn create_swap(
        &self,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
        include_fees: bool,
    ) -> Result<PreSwap, Error> {
        let active_keyset_id = self.get_active_mint_keyset().await?.id;

        // Desired amount is either amount passed or value of all proof
        let proofs_total = proofs.total_amount()?;

        let ys: Vec<PublicKey> = proofs.ys()?;
        self.localstore.set_pending_proofs(ys).await?;

        let fee = self.get_proofs_fee(&proofs).await?;

        let change_amount: Amount = proofs_total - amount.unwrap_or(Amount::ZERO) - fee;

        let (send_amount, change_amount) = match include_fees {
            true => {
                let split_count = amount
                    .unwrap_or(Amount::ZERO)
                    .split_targeted(&SplitTarget::default())
                    .unwrap()
                    .len();

                let fee_to_redeem = self
                    .get_keyset_count_fee(&active_keyset_id, split_count as u64)
                    .await?;

                (
                    amount.map(|a| a + fee_to_redeem),
                    change_amount - fee_to_redeem,
                )
            }
            false => (amount, change_amount),
        };

        // If a non None split target is passed use that
        // else use state refill
        let change_split_target = match amount_split_target {
            SplitTarget::None => self.determine_split_target_values(change_amount).await?,
            s => s,
        };

        let derived_secret_count;

        let count = self
            .localstore
            .get_keyset_counter(&active_keyset_id)
            .await?;

        let mut count = count.map_or(0, |c| c + 1);

        let (mut desired_messages, change_messages) = match spending_conditions {
            Some(conditions) => {
                let change_premint_secrets = PreMintSecrets::from_xpriv(
                    active_keyset_id,
                    count,
                    self.xpriv,
                    change_amount,
                    &change_split_target,
                )?;

                derived_secret_count = change_premint_secrets.len();

                (
                    PreMintSecrets::with_conditions(
                        active_keyset_id,
                        send_amount.unwrap_or(Amount::ZERO),
                        &SplitTarget::default(),
                        &conditions,
                    )?,
                    change_premint_secrets,
                )
            }
            None => {
                let premint_secrets = PreMintSecrets::from_xpriv(
                    active_keyset_id,
                    count,
                    self.xpriv,
                    send_amount.unwrap_or(Amount::ZERO),
                    &SplitTarget::default(),
                )?;

                count += premint_secrets.len() as u32;

                let change_premint_secrets = PreMintSecrets::from_xpriv(
                    active_keyset_id,
                    count,
                    self.xpriv,
                    change_amount,
                    &change_split_target,
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

        Ok(PreSwap {
            pre_mint_secrets: desired_messages,
            swap_request,
            derived_secret_count: derived_secret_count as u32,
            fee,
        })
    }
}
