use std::collections::HashMap;

use tracing::instrument;

use super::SendKind;
use crate::amount::SplitTarget;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{Proofs, SpendingConditions, State, Token};
use crate::secp256k1::rand;
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
        let available_proofs = self
            .get_proofs_with(
                Some(vec![State::Unspent]),
                opts.conditions.clone().map(|c| vec![c]),
            )
            .await?;

        // Check if sufficient proofs are available
        let proofs_sum = available_proofs.total_amount()?;
        if proofs_sum < amount {
            if opts.conditions.is_none() || opts.send_kind.is_offline() {
                return Err(Error::InsufficientFunds);
            } else {
                // Swap is required for send
                tracing::debug!("Insufficient proofs matching conditions");
                let unspent_proofs = self.get_unspent_proofs().await?;
                let proofs_to_swap =
                    Wallet::select_proofs_v2(amount, unspent_proofs, &keyset_fees)?;
                let swap_fee = self.get_proofs_fee(&proofs_to_swap).await?;
                return self
                    .store_prepared_send(PreparedSend {
                        amount,
                        options: opts,
                        nonce: rand::random(),
                        proofs_to_swap,
                        swap_fee,
                        proofs_to_send: vec![],
                        send_fee: Amount::ZERO,
                    })
                    .await;
            }
        }

        let selected_proofs = if opts.include_fee {
            Wallet::select_proofs_v2(amount, available_proofs, &keyset_fees)?
        } else {
            Wallet::select_proofs_v2(amount, available_proofs, &HashMap::new())?
        };
        let selected_total = selected_proofs.total_amount()?;

        let send_fee = if opts.include_fee {
            self.get_proofs_fee(&selected_proofs).await?
        } else {
            Amount::ZERO
        };

        // TODO what is this?
        if opts.include_fee || selected_total == amount {
            return self
                .store_prepared_send(PreparedSend {
                    amount,
                    options: opts,
                    nonce: rand::random(),
                    proofs_to_swap: vec![],
                    swap_fee: Amount::ZERO,
                    proofs_to_send: selected_proofs,
                    send_fee,
                })
                .await;
        }

        let tolerance = match opts.send_kind {
            SendKind::OfflineTolerance(tolerance) => Some(tolerance),
            SendKind::OnlineTolerance(tolerance) => Some(tolerance),
            _ => None,
        };

        if opts.send_kind.is_offline() {
            if let Some(tolerance) = tolerance {
                if selected_total - amount <= tolerance {
                    self.store_prepared_send(PreparedSend {
                        amount,
                        options: opts,
                        nonce: rand::random(),
                        proofs_to_swap: vec![],
                        swap_fee: Amount::ZERO,
                        proofs_to_send: selected_proofs,
                        send_fee,
                    })
                    .await
                } else {
                    Err(Error::InsufficientFunds)
                }
            } else {
                Err(Error::InsufficientFunds)
            }
        } else {
            if let Some(tolerance) = tolerance {
                if selected_total - amount <= tolerance {
                    return self
                        .store_prepared_send(PreparedSend {
                            amount,
                            options: opts,
                            nonce: rand::random(),
                            proofs_to_swap: vec![],
                            swap_fee: Amount::ZERO,
                            proofs_to_send: selected_proofs,
                            send_fee,
                        })
                        .await;
                }
            }

            let mut optimal_by_keyset = HashMap::new();
            for (id, amount) in selected_proofs.sum_by_keyset() {
                let fee_ppk = keyset_fees.get(&id).ok_or(Error::KeysetUnknown(id))?;
                optimal_by_keyset.insert(id, amount.split_with_fee(*fee_ppk)?);
            }

            let mut proofs_to_send = vec![];
            let mut proofs_to_swap = vec![];
            for proof in selected_proofs {
                let keyset_id = proof.keyset_id;
                let optimal_amounts = optimal_by_keyset
                    .get_mut(&keyset_id)
                    .ok_or(Error::KeysetUnknown(keyset_id))?;
                if let Some(idx) = optimal_amounts.iter().position(|a| a == &proof.amount) {
                    proofs_to_send.push(proof);
                    optimal_amounts.remove(idx);
                } else {
                    proofs_to_swap.push(proof);
                }
            }

            let swap_fee = self.get_proofs_fee(&proofs_to_swap).await?;
            let send_fee = self.get_proofs_fee(&proofs_to_send).await?;

            self.store_prepared_send(PreparedSend {
                amount,
                options: opts,
                nonce: rand::random(),
                proofs_to_swap,
                swap_fee,
                proofs_to_send,
                send_fee,
            })
            .await
        }
    }

    async fn store_prepared_send(
        &self,
        prepared_send: PreparedSend,
    ) -> Result<PreparedSend, Error> {
        tracing::debug!("Storing prepared send");
        let mut guard = self.prepared_send.lock().await;
        match *guard {
            Some(_) => Err(Error::ActivePreparedSend),
            None => {
                let mut ys = prepared_send.proofs_to_send.ys()?;
                ys.extend(prepared_send.proofs_to_swap.ys()?);
                self.localstore
                    .update_proofs_state(ys, State::Reserved)
                    .await?;
                *guard = Some(prepared_send.nonce);
                tracing::info!("Prepared send stored");
                Ok(prepared_send)
            }
        }
    }

    /// Send prepared send
    #[instrument(skip(self))]
    pub async fn send(&self, send: PreparedSend) -> Result<Token, Error> {
        tracing::info!("Sending prepared send");
        let guard = self.prepared_send.lock().await;
        match *guard {
            Some(nonce) if nonce == send.nonce => {}
            _ => return Err(Error::InvalidPreparedSend),
        }

        let mut send_proofs = send.proofs_to_send;
        if !send.proofs_to_swap.is_empty() {
            let swap_amount = send.amount - send_proofs.total_amount()?;
            tracing::debug!("Swapping proofs; swap_amount={}", swap_amount,);
            if let Some(proofs) = self
                .swap(
                    Some(swap_amount),
                    SplitTarget::None,
                    send.proofs_to_swap,
                    send.options.conditions,
                    send.options.include_fee,
                )
                .await?
            {
                send_proofs.extend(proofs);
            }
        }

        if send.amount > send_proofs.total_amount()? {
            return Err(Error::InsufficientFunds);
        }

        self.localstore
            .update_proofs_state(send_proofs.ys()?, State::PendingSpent)
            .await?;

        Ok(Token::new(
            self.mint_url.clone(),
            send_proofs,
            send.options.memo,
            self.unit.clone(),
        ))
    }
}

#[derive(Debug)]
pub struct PreparedSend {
    amount: Amount,
    options: SendOptions,
    nonce: u64,
    proofs_to_swap: Proofs,
    swap_fee: Amount,
    proofs_to_send: Proofs,
    send_fee: Amount,
}

impl PreparedSend {
    pub fn amount(&self) -> Amount {
        self.amount
    }

    pub fn options(&self) -> &SendOptions {
        &self.options
    }

    pub fn proofs_to_swap(&self) -> &Proofs {
        &self.proofs_to_swap
    }

    pub fn swap_fee(&self) -> Amount {
        self.swap_fee
    }

    pub fn proofs_to_send(&self) -> &Proofs {
        &self.proofs_to_send
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
