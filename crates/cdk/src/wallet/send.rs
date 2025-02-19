use std::fmt::Debug;

use cdk_common::Id;
use tracing::instrument;

use super::SendKind;
use crate::amount::SplitTarget;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{Proofs, SpendingConditions, State, Token};
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Prepare send
    #[instrument(skip(self), err)]
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
        let mut force_swap = false;
        let available_sum = available_proofs.total_amount()?;
        if available_sum < amount {
            if opts.conditions.is_none() || opts.send_kind.is_offline() {
                return Err(Error::InsufficientFunds);
            } else {
                // Swap is required for send
                tracing::debug!("Insufficient proofs matching conditions");
                force_swap = true;
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

        // Select proofs
        let active_keyset_id = self
            .get_active_mint_keysets()
            .await?
            .first()
            .ok_or(Error::NoActiveKeyset)?
            .id;
        let selected_proofs = Wallet::select_proofs(
            amount,
            available_proofs,
            active_keyset_id,
            &keyset_fees,
            opts.include_fee,
        )?;
        let selected_total = selected_proofs.total_amount()?;

        // Check if selected proofs are exact
        let send_fee = if opts.include_fee {
            self.get_proofs_fee(&selected_proofs).await?
        } else {
            Amount::ZERO
        };
        if selected_total == amount + send_fee {
            return self
                .internal_prepare_send(amount, opts, selected_proofs, active_keyset_id, force_swap)
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
                    .internal_prepare_send(
                        amount,
                        opts,
                        selected_proofs,
                        active_keyset_id,
                        force_swap,
                    )
                    .await;
            } else if opts.send_kind.is_offline() {
                return Err(Error::InsufficientFunds);
            }
        }

        self.internal_prepare_send(amount, opts, selected_proofs, active_keyset_id, force_swap)
            .await
    }

    async fn internal_prepare_send(
        &self,
        amount: Amount,
        opts: SendOptions,
        proofs: Proofs,
        active_keyset_id: Id,
        force_swap: bool,
    ) -> Result<PreparedSend, Error> {
        // Split amount with fee if necessary
        let (send_amounts, send_fee) = if opts.include_fee {
            let keyset_fee_ppk = self.get_keyset_fees_by_id(active_keyset_id).await?;
            tracing::debug!("Keyset fee per proof: {:?}", keyset_fee_ppk);
            let send_split = amount.split_with_fee(keyset_fee_ppk)?;
            let send_fee = self
                .get_proofs_fee_by_count(
                    vec![(active_keyset_id, send_split.len() as u64)]
                        .into_iter()
                        .collect(),
                )
                .await?;
            (send_split, send_fee)
        } else {
            let send_split = amount.split();
            let send_fee = Amount::ZERO;
            (send_split, send_fee)
        };
        tracing::debug!("Send amounts: {:?}", send_amounts);
        tracing::debug!("Send fee: {:?}", send_fee);

        // Check if proofs are exact send amount
        let proofs_exact_amount = proofs.total_amount()? == amount + send_fee;

        // Split proofs to swap and send
        let mut proofs_to_swap = Proofs::new();
        let mut proofs_to_send = Proofs::new();
        if force_swap {
            proofs_to_swap = proofs;
        } else if proofs_exact_amount
            || opts.send_kind.is_offline()
            || opts.send_kind.has_tolerance()
        {
            proofs_to_send = proofs;
        } else {
            let mut remaining_send_amounts = send_amounts.clone();
            for proof in proofs {
                if let Some(idx) = remaining_send_amounts
                    .iter()
                    .position(|a| a == &proof.amount)
                {
                    proofs_to_send.push(proof);
                    remaining_send_amounts.remove(idx);
                } else {
                    proofs_to_swap.push(proof);
                }
            }
        }

        // Calculate swap fee
        let swap_fee = self.get_proofs_fee(&proofs_to_swap).await?;

        // Return prepared send
        Ok(PreparedSend {
            amount,
            options: opts,
            proofs_to_swap,
            swap_fee,
            proofs_to_send,
            send_fee,
        })
    }

    /// Send prepared send
    #[instrument(skip(self), err)]
    pub async fn send(&self, send: PreparedSend, memo: Option<String>) -> Result<Token, Error> {
        tracing::info!("Sending prepared send");
        let mut proofs_to_send = send.proofs_to_send;

        // Get active keyset ID
        let active_keyset_id = self.get_active_mint_keyset().await?.id;
        tracing::debug!("Active keyset ID: {:?}", active_keyset_id);

        // Get keyset fees
        let keyset_fee_ppk = self.get_keyset_fees_by_id(active_keyset_id).await?;
        tracing::debug!("Keyset fees: {:?}", keyset_fee_ppk);

        // Calculate total send amount
        let total_send_amount = send.amount + send.send_fee;
        tracing::debug!("Total send amount: {}", total_send_amount);

        // Swap proofs if necessary
        if !send.proofs_to_swap.is_empty() {
            let swap_amount = total_send_amount - proofs_to_send.total_amount()?;
            tracing::debug!("Swapping proofs; swap_amount={:?}", swap_amount);
            if let Some(proofs) = self
                .swap(
                    Some(swap_amount),
                    SplitTarget::None,
                    send.proofs_to_swap,
                    send.options.conditions,
                    false, // already included in swap_amount
                )
                .await?
            {
                proofs_to_send.extend(proofs);
            }
        }
        tracing::debug!(
            "Proofs to send: {:?}",
            proofs_to_send.iter().map(|p| p.amount).collect::<Vec<_>>()
        );

        // Check if sufficient proofs are available
        if send.amount > proofs_to_send.total_amount()? {
            return Err(Error::InsufficientFunds);
        }

        // Update proofs state to pending spent
        self.localstore
            .update_proofs_state(proofs_to_send.ys()?, State::PendingSpent)
            .await?;

        // Include token memo
        let memo = if send.options.include_memo {
            memo.or(send.options.memo)
        } else {
            None
        };

        // Create and return token
        Ok(Token::new(
            self.mint_url.clone(),
            proofs_to_send,
            memo,
            self.unit.clone(),
        ))
    }

    /// Cancel prepared send
    pub async fn cancel_send(&self, send: PreparedSend) -> Result<(), Error> {
        tracing::info!("Cancelling prepared send");

        self.localstore
            .update_proofs_state(send.proofs().ys()?, State::Unspent)
            .await?;

        Ok(())
    }
}

/// Prepared send
pub struct PreparedSend {
    amount: Amount,
    options: SendOptions,
    proofs_to_swap: Proofs,
    swap_fee: Amount,
    proofs_to_send: Proofs,
    send_fee: Amount,
}

impl PreparedSend {
    /// Amount
    pub fn amount(&self) -> Amount {
        self.amount
    }

    /// Send options
    pub fn options(&self) -> &SendOptions {
        &self.options
    }

    /// Proofs to swap
    pub fn proofs_to_swap(&self) -> &Proofs {
        &self.proofs_to_swap
    }

    /// Swap fee
    pub fn swap_fee(&self) -> Amount {
        self.swap_fee
    }

    /// Proofs to send
    pub fn proofs_to_send(&self) -> &Proofs {
        &self.proofs_to_send
    }

    /// Send fee
    pub fn send_fee(&self) -> Amount {
        self.send_fee
    }

    /// All proofs
    pub fn proofs(&self) -> Proofs {
        let mut proofs = self.proofs_to_swap.clone();
        proofs.extend(self.proofs_to_send.clone());
        proofs
    }

    /// Total fee
    pub fn fee(&self) -> Amount {
        self.swap_fee + self.send_fee
    }
}

impl Debug for PreparedSend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedSend")
            .field("amount", &self.amount)
            .field("options", &self.options)
            .field(
                "proofs_to_swap",
                &self
                    .proofs_to_swap
                    .iter()
                    .map(|p| p.amount)
                    .collect::<Vec<_>>(),
            )
            .field("swap_fee", &self.swap_fee)
            .field(
                "proofs_to_send",
                &self
                    .proofs_to_send
                    .iter()
                    .map(|p| p.amount)
                    .collect::<Vec<_>>(),
            )
            .field("send_fee", &self.send_fee)
            .finish()
    }
}

/// Send options
#[derive(Debug, Clone, Default)]
pub struct SendOptions {
    /// Memo
    pub memo: Option<String>,
    /// Include memo in token
    pub include_memo: bool,
    /// Spending conditions
    pub conditions: Option<SpendingConditions>,
    /// Amount split target
    pub amount_split_target: SplitTarget,
    /// Send kind
    pub send_kind: SendKind,
    /// Include fee
    pub include_fee: bool,
}
