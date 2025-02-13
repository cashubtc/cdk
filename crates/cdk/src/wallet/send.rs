use std::collections::HashMap;

use cdk_common::Id;
use tracing::instrument;

use super::SendKind;
use crate::amount::SplitTarget;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{Proofs, SpendingConditions, State, Token};
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Prepare send
    #[instrument(skip(self))]
    pub async fn prepare_send(
        &self,
        amount: Amount,
        opts: SendOptions,
    ) -> Result<PreparedSend, Error> {
        tracing::info!("Preparing send");

        // If online send check mint for current keysets fees
        if opts.send_kind.is_online() {
            if let Err(e) = self.get_active_mint_keyset().await {
                tracing::error!(
                    "Error fetching active mint keyset: {:?}. Using stored keysets",
                    e
                );
            }
        }

        // Get keyset fees from localstore
        let keyset_fees = self.get_keyset_fees().await?;

        // Get available proofs matching conditions
        let mut available_proofs = self
            .get_proofs_with(
                Some(vec![State::Unspent]),
                opts.conditions.clone().map(|c| vec![c]),
            )
            .await?;

        // Check if sufficient proofs are available
        let available_sum = available_proofs.total_amount()?;
        if available_sum < amount {
            if opts.conditions.is_none() || opts.send_kind.is_offline() {
                return Err(Error::InsufficientFunds);
            } else {
                // Swap is required for send
                tracing::debug!("Insufficient proofs matching conditions");
                available_proofs = self
                    .localstore
                    .get_proofs(
                        Some(self.mint_url.clone()),
                        Some(self.unit.clone()),
                        Some(vec![State::Unspent]),
                        None,
                    )
                    .await?
                    .into_iter()
                    .filter_map(|p| {
                        if p.spending_condition.is_none() {
                            Some(p.proof)
                        } else {
                            None
                        }
                    })
                    .collect();
            }
        }

        // Select proofs (including fee)
        let active_keyset_id = self
            .get_active_mint_keysets()
            .await?
            .first()
            .ok_or(Error::NoActiveKeyset)?
            .id;
        let selected_proofs = if opts.include_fee {
            Wallet::select_proofs_v2(amount, available_proofs, active_keyset_id, &keyset_fees)?
        } else {
            Wallet::select_proofs_v2(amount, available_proofs, active_keyset_id, &HashMap::new())?
        };
        let selected_total = selected_proofs.total_amount()?;

        // Check if selected proofs are exact
        let send_fee = if opts.include_fee {
            self.get_proofs_fee(&selected_proofs).await?
        } else {
            Amount::ZERO
        };
        if selected_total == amount + send_fee {
            return self
                .internal_prepare_send(amount, opts, selected_proofs, active_keyset_id)
                .await;
        } else if opts.send_kind == SendKind::OfflineExact {
            return Err(Error::InsufficientFunds);
        }

        // Check if selected proofs are sufficient for tolerance
        let tolerance = match opts.send_kind {
            SendKind::OfflineTolerance(tolerance) => Some(tolerance),
            SendKind::OnlineTolerance(tolerance) => Some(tolerance),
            _ => None,
        };
        if let Some(tolerance) = tolerance {
            if selected_total - amount <= tolerance {
                return self
                    .internal_prepare_send(amount, opts, selected_proofs, active_keyset_id)
                    .await;
            } else if opts.send_kind.is_offline() {
                return Err(Error::InsufficientFunds);
            }
        }

        self.internal_prepare_send(amount, opts, selected_proofs, active_keyset_id)
            .await
    }

    async fn internal_prepare_send(
        &self,
        amount: Amount,
        opts: SendOptions,
        proofs: Proofs,
        active_keyset_id: Id,
    ) -> Result<PreparedSend, Error> {
        let keyset_fee_ppk = self.get_keyset_fees_by_id(active_keyset_id).await?;
        let mut send_split = amount.split_with_fee(keyset_fee_ppk)?;
        let send_fee = self
            .get_proofs_fee_by_count(
                vec![(active_keyset_id, send_split.len() as u64)]
                    .into_iter()
                    .collect(),
            )
            .await?;

        let mut swap_count = HashMap::new();
        for proof in &proofs {
            let keyset_id = proof.keyset_id;
            if let Some(idx) = send_split.iter().position(|a| a == &proof.amount) {
                send_split.remove(idx);
            } else {
                let count = swap_count.entry(keyset_id).or_insert(0);
                *count += 1;
            }
        }
        let swap_fee = self.get_proofs_fee_by_count(swap_count).await?;

        Ok(PreparedSend {
            amount,
            options: opts,
            proofs,
            swap_fee,
            send_fee,
        })
    }

    /// Send prepared send
    #[instrument(skip(self))]
    pub async fn send(&self, send: PreparedSend) -> Result<Token, Error> {
        tracing::info!("Sending prepared send");

        let active_keyset_id = self
            .get_active_mint_keysets()
            .await?
            .first()
            .ok_or(Error::NoActiveKeyset)?
            .id;
        let keyset_fee_ppk = self.get_keyset_fees_by_id(active_keyset_id).await?;
        let mut send_split = send.amount.split_with_fee(keyset_fee_ppk)?;

        let mut proofs_to_send = Proofs::new();
        let mut proofs_to_swap = Proofs::new();
        for proof in send.proofs {
            if let Some(idx) = send_split.iter().position(|a| a == &proof.amount) {
                send_split.remove(idx);
                proofs_to_send.push(proof);
            } else {
                proofs_to_swap.push(proof);
            }
        }

        if !proofs_to_swap.is_empty() {
            let swap_amount = send.amount - proofs_to_send.total_amount()?;
            tracing::debug!("Swapping proofs; swap_amount={}", swap_amount,);
            if let Some(proofs) = self
                .swap(
                    Some(swap_amount),
                    SplitTarget::None,
                    proofs_to_swap,
                    send.options.conditions,
                    send.options.include_fee,
                )
                .await?
            {
                proofs_to_send.extend(proofs);
            }
        }

        if send.amount > proofs_to_send.total_amount()? {
            return Err(Error::InsufficientFunds);
        }

        self.localstore
            .update_proofs_state(proofs_to_send.ys()?, State::PendingSpent)
            .await?;

        Ok(Token::new(
            self.mint_url.clone(),
            proofs_to_send,
            send.options.memo,
            self.unit.clone(),
        ))
    }

    /// Cancel prepared send
    pub async fn cancel_send(&self, send: PreparedSend) -> Result<(), Error> {
        tracing::info!("Cancelling prepared send");

        self.localstore
            .update_proofs_state(send.proofs.ys()?, State::Unspent)
            .await?;

        Ok(())
    }
}

#[derive(Debug)]
pub struct PreparedSend {
    amount: Amount,
    options: SendOptions,
    proofs: Proofs,
    swap_fee: Amount,
    send_fee: Amount,
    // nonce: u64,
    // proofs_to_swap: Proofs,
    // swap_fee: Amount,
    // proofs_to_send: Proofs,
    // send_fee: Amount,
}

impl PreparedSend {
    pub fn amount(&self) -> Amount {
        self.amount
    }

    pub fn options(&self) -> &SendOptions {
        &self.options
    }

    pub fn proofs(&self) -> &Proofs {
        &self.proofs
    }

    pub fn swap_fee(&self) -> Amount {
        self.swap_fee
    }

    pub fn send_fee(&self) -> Amount {
        self.send_fee
    }
}

/// Send options
#[derive(Debug, Clone, Default)]
pub struct SendOptions {
    /// Memo
    pub memo: Option<String>,
    /// Spending conditions
    pub conditions: Option<SpendingConditions>,
    /// Amount split target
    pub amount_split_target: SplitTarget,
    /// Send kind
    pub send_kind: SendKind,
    /// Include fee
    pub include_fee: bool,
}
