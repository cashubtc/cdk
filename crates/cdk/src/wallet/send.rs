use std::collections::HashMap;
use std::fmt::Debug;

use cdk_common::nut02::KeySetInfosMethods;
use cdk_common::util::unix_time;
use cdk_common::wallet::{Transaction, TransactionDirection};
use tracing::instrument;

use super::SendKind;
use crate::amount::SplitTarget;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{Proofs, SpendingConditions, State, Token};
use crate::{Amount, Error, Wallet};

impl Wallet {
    /// Prepare A Send Transaction
    ///
    /// This function prepares a send transaction by selecting proofs to send and proofs to swap.
    /// By doing so, it ensures that the wallet user is able to view the fees associated with the send transaction.
    ///
    /// ```no_compile
    /// let send = wallet.prepare_send(Amount::from(10), SendOptions::default()).await?;
    /// assert!(send.fee() <= Amount::from(1));
    /// let token = send.confirm(None).await?;
    /// ```
    #[instrument(skip(self), err)]
    pub async fn prepare_send(
        &self,
        amount: Amount,
        opts: SendOptions,
    ) -> Result<PreparedSend, Error> {
        tracing::info!("Preparing send");

        // If online send check mint for current keysets fees
        if opts.send_kind.is_online() {
            if let Err(e) = self.refresh_keysets().await {
                tracing::error!("Error refreshing keysets: {:?}. Using stored keysets", e);
            }
        }

        // Get keyset fees from localstore
        let keyset_fees = self.get_keyset_fees_and_amounts().await?;

        // Get available proofs matching conditions
        let mut available_proofs = self
            .get_proofs_with(
                None,
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
                        Some(vec![]),
                    )
                    .await?
                    .into_iter()
                    .map(|p| p.proof)
                    .collect();
            }
        }

        // Select proofs
        let active_keyset_ids = self
            .get_mint_keysets()
            .await?
            .active()
            .map(|k| k.id)
            .collect();

        let selected_proofs = Wallet::select_proofs(
            amount,
            available_proofs,
            &active_keyset_ids,
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
                .internal_prepare_send(amount, opts, selected_proofs, force_swap)
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
            if selected_total - amount > tolerance && opts.send_kind.is_offline() {
                return Err(Error::InsufficientFunds);
            }
        }

        self.internal_prepare_send(amount, opts, selected_proofs, force_swap)
            .await
    }

    async fn internal_prepare_send(
        &self,
        amount: Amount,
        opts: SendOptions,
        proofs: Proofs,
        force_swap: bool,
    ) -> Result<PreparedSend, Error> {
        // Split amount with fee if necessary
        let active_keyset_id = self.get_active_keyset().await?.id;
        let fee_and_amounts = self
            .get_keyset_fees_and_amounts_by_id(active_keyset_id)
            .await?;
        let (send_amounts, send_fee) = if opts.include_fee {
            tracing::debug!("Keyset fee per proof: {:?}", fee_and_amounts.fee());
            let send_split = amount.split_with_fee(&fee_and_amounts)?;
            let send_fee = self
                .get_proofs_fee_by_count(
                    vec![(active_keyset_id, send_split.len() as u64)]
                        .into_iter()
                        .collect(),
                )
                .await?;
            (send_split, send_fee)
        } else {
            let send_split = amount.split(&fee_and_amounts);
            let send_fee = Amount::ZERO;
            (send_split, send_fee)
        };
        tracing::debug!("Send amounts: {:?}", send_amounts);
        tracing::debug!("Send fee: {:?}", send_fee);

        let mut tx = self.localstore.begin_db_transaction().await?;

        // Reserve proofs
        tx.update_proofs_state(proofs.ys()?, State::Reserved)
            .await?;

        // Check if proofs are exact send amount (and does not exceed max_proofs)
        let mut exact_proofs = proofs.total_amount()? == amount + send_fee;
        if let Some(max_proofs) = opts.max_proofs {
            exact_proofs &= proofs.len() <= max_proofs;
        }

        // Split proofs to swap and send
        let mut proofs_to_swap = Proofs::new();
        let mut proofs_to_send = Proofs::new();
        if force_swap {
            proofs_to_swap = proofs;
        } else if exact_proofs || opts.send_kind.is_offline() || opts.send_kind.has_tolerance() {
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

        tx.commit().await?;

        // Return prepared send
        Ok(PreparedSend {
            wallet: self.clone(),
            amount,
            options: opts,
            proofs_to_swap,
            swap_fee,
            proofs_to_send,
            send_fee,
        })
    }
}

/// Prepared send
pub struct PreparedSend {
    wallet: Wallet,
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

    /// Proofs to swap (i.e., proofs that need to be swapped before constructing the token)
    pub fn proofs_to_swap(&self) -> &Proofs {
        &self.proofs_to_swap
    }

    /// Swap fee
    pub fn swap_fee(&self) -> Amount {
        self.swap_fee
    }

    /// Proofs to send (i.e., proofs that will be included in the token)
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

    /// Confirm the prepared send and create a token
    #[instrument(skip(self), err)]
    pub async fn confirm(self, memo: Option<SendMemo>) -> Result<Token, Error> {
        tracing::info!("Confirming prepared send");
        let total_send_fee = self.fee();
        let mut proofs_to_send = self.proofs_to_send;

        // Get active keyset ID
        let active_keyset_id = self.wallet.fetch_active_keyset().await?.id;
        tracing::debug!("Active keyset ID: {:?}", active_keyset_id);

        // Get keyset fees
        let keyset_fee_ppk = self
            .wallet
            .get_keyset_fees_and_amounts_by_id(active_keyset_id)
            .await?;
        tracing::debug!("Keyset fees: {:?}", keyset_fee_ppk);

        // Calculate total send amount
        let total_send_amount = self.amount + self.send_fee;
        tracing::debug!("Total send amount: {}", total_send_amount);

        // Swap proofs if necessary
        if !self.proofs_to_swap.is_empty() {
            let swap_amount = total_send_amount - proofs_to_send.total_amount()?;
            tracing::debug!("Swapping proofs; swap_amount={:?}", swap_amount);
            if let Some(proofs) = self
                .wallet
                .swap(
                    Some(swap_amount),
                    SplitTarget::None,
                    self.proofs_to_swap,
                    self.options.conditions.clone(),
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
        if self.amount > proofs_to_send.total_amount()? {
            return Err(Error::InsufficientFunds);
        }

        let mut tx = self.wallet.localstore.begin_db_transaction().await?;

        // Check if proofs are reserved or unspent
        let sendable_proof_ys = self
            .wallet
            .get_proofs_with(
                Some(&mut tx),
                Some(vec![State::Reserved, State::Unspent]),
                self.options.conditions.clone().map(|c| vec![c]),
            )
            .await?
            .ys()?;
        if proofs_to_send
            .ys()?
            .iter()
            .any(|y| !sendable_proof_ys.contains(y))
        {
            tracing::warn!("Proofs to send are not reserved or unspent");
            return Err(Error::UnexpectedProofState);
        }

        // Update proofs state to pending spent
        tracing::debug!(
            "Updating proofs state to pending spent: {:?}",
            proofs_to_send.ys()?
        );

        tx.update_proofs_state(proofs_to_send.ys()?, State::PendingSpent)
            .await?;

        // Include token memo
        let send_memo = self.options.memo.or(memo);
        let memo = send_memo.and_then(|m| if m.include_memo { Some(m.memo) } else { None });

        // Add transaction to store
        tx.add_transaction(Transaction {
            mint_url: self.wallet.mint_url.clone(),
            direction: TransactionDirection::Outgoing,
            amount: self.amount,
            fee: total_send_fee,
            unit: self.wallet.unit.clone(),
            ys: proofs_to_send.ys()?,
            timestamp: unix_time(),
            memo: memo.clone(),
            metadata: self.options.metadata,
            quote_id: None,
            payment_request: None,
            payment_proof: None,
        })
        .await?;

        tx.commit().await?;

        // Create and return token
        Ok(Token::new(
            self.wallet.mint_url.clone(),
            proofs_to_send,
            memo,
            self.wallet.unit.clone(),
        ))
    }

    /// Cancel the prepared send
    pub async fn cancel(self) -> Result<(), Error> {
        tracing::info!("Cancelling prepared send");

        let mut tx = self.wallet.localstore.begin_db_transaction().await?;

        // Double-check proofs state
        let reserved_proofs = self
            .wallet
            .get_proofs_with(Some(&mut tx), Some(vec![State::Reserved]), None)
            .await?
            .ys()?;

        if !self
            .proofs()
            .ys()?
            .iter()
            .all(|y| reserved_proofs.contains(y))
        {
            return Err(Error::UnexpectedProofState);
        }

        tx.update_proofs_state(self.proofs().ys()?, State::Unspent)
            .await?;

        tx.commit().await?;

        Ok(())
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
    pub memo: Option<SendMemo>,
    /// Spending conditions
    pub conditions: Option<SpendingConditions>,
    /// Amount split target
    pub amount_split_target: SplitTarget,
    /// Send kind
    pub send_kind: SendKind,
    /// Include fee
    ///
    /// When this is true the token created will include the amount of fees needed to redeem the token (amount + fee_to_redeem)
    pub include_fee: bool,
    /// Maximum number of proofs to include in the token
    /// Default is `None`, which means all selected proofs will be included.
    pub max_proofs: Option<usize>,
    /// Metadata
    pub metadata: HashMap<String, String>,
}

/// Send memo
#[derive(Debug, Clone)]
pub struct SendMemo {
    /// Memo
    pub memo: String,
    /// Include memo in token
    pub include_memo: bool,
}

impl SendMemo {
    /// Create a new send memo
    pub fn for_token(memo: &str) -> Self {
        Self {
            memo: memo.to_string(),
            include_memo: true,
        }
    }
}
