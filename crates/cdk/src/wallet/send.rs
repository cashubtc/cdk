use std::collections::HashMap;
use std::fmt::Debug;

use cdk_common::nut02::KeySetInfosMethods;
use cdk_common::util::unix_time;
use cdk_common::wallet::{Transaction, TransactionDirection};
use cdk_common::Id;
use tracing::instrument;

use super::SendKind;
use crate::amount::SplitTarget;
use crate::fees::calculate_fee;
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

        // When including fees, we need to account for both:
        // 1. Input fees (to spend the selected proofs)
        // 2. Output fees (send_fee - fee to redeem the token we create)
        //
        // If proofs don't exactly match the desired denominations, a swap is needed.
        // The swap consumes the input fee, and the outputs must cover amount + send_fee.
        // So we select proofs for (amount + send_fee) to ensure the swap can succeed.
        let active_keyset_id = self.get_active_keyset().await?.id;
        let fee_and_amounts = self
            .get_keyset_fees_and_amounts_by_id(active_keyset_id)
            .await?;

        let selection_amount = if opts.include_fee {
            let send_split = amount.split_with_fee(&fee_and_amounts)?;
            let send_fee = self
                .get_proofs_fee_by_count(
                    vec![(active_keyset_id, send_split.len() as u64)]
                        .into_iter()
                        .collect(),
                )
                .await?;
            amount + send_fee
        } else {
            amount
        };

        let selected_proofs = Wallet::select_proofs(
            selection_amount,
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

        // Reserve proofs
        self.localstore
            .update_proofs_state(proofs.ys()?, State::Reserved)
            .await?;

        // Check if proofs are exact send amount (and does not exceed max_proofs)
        let mut exact_proofs = proofs.total_amount()? == amount + send_fee;
        if let Some(max_proofs) = opts.max_proofs {
            exact_proofs &= proofs.len() <= max_proofs;
        }

        // Determine if we should send all proofs directly
        let is_exact_or_offline =
            exact_proofs || opts.send_kind.is_offline() || opts.send_kind.has_tolerance();

        // Get keyset fees for the split function
        let keyset_fees_and_amounts = self.get_keyset_fees_and_amounts().await?;
        let keyset_fees: HashMap<Id, u64> = keyset_fees_and_amounts
            .iter()
            .map(|(key, values)| (*key, values.fee()))
            .collect();

        // Split proofs between send and swap
        let split_result = split_proofs_for_send(
            proofs,
            &send_amounts,
            amount,
            send_fee,
            &keyset_fees,
            force_swap,
            is_exact_or_offline,
        )?;

        // Return prepared send
        Ok(PreparedSend {
            wallet: self.clone(),
            amount,
            options: opts,
            proofs_to_swap: split_result.proofs_to_swap,
            swap_fee: split_result.swap_fee,
            proofs_to_send: split_result.proofs_to_send,
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
            let swap_amount = total_send_amount
                .checked_sub(proofs_to_send.total_amount()?)
                .unwrap_or(Amount::ZERO);
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

        // Check if proofs are reserved or unspent
        let sendable_proof_ys = self
            .wallet
            .get_proofs_with(
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
        self.wallet
            .localstore
            .update_proofs_state(proofs_to_send.ys()?, State::PendingSpent)
            .await?;

        // Include token memo
        let send_memo = self.options.memo.or(memo);
        let memo = send_memo.and_then(|m| if m.include_memo { Some(m.memo) } else { None });

        // Add transaction to store
        self.wallet
            .localstore
            .add_transaction(Transaction {
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

        // Double-check proofs state
        let reserved_proofs = self.wallet.get_reserved_proofs().await?.ys()?;
        if !self
            .proofs()
            .ys()?
            .iter()
            .all(|y| reserved_proofs.contains(y))
        {
            return Err(Error::UnexpectedProofState);
        }

        self.wallet
            .localstore
            .update_proofs_state(self.proofs().ys()?, State::Unspent)
            .await?;

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

/// Result of splitting proofs for a send operation
#[derive(Debug, Clone)]
pub struct ProofSplitResult {
    /// Proofs that can be sent directly (matching desired denominations)
    pub proofs_to_send: Proofs,
    /// Proofs that need to be swapped first
    pub proofs_to_swap: Proofs,
    /// Fee required for the swap operation
    pub swap_fee: Amount,
}

/// Split proofs between those to send directly and those requiring swap.
///
/// This is a pure function that implements the core logic of `internal_prepare_send`:
/// 1. Match proofs to desired send amounts
/// 2. Ensure proofs_to_swap can cover swap fees plus needed output
/// 3. Move proofs from send to swap if needed to cover fees
///
/// # Arguments
/// * `proofs` - All selected proofs to split
/// * `send_amounts` - Desired output denominations
/// * `amount` - Amount to send
/// * `send_fee` - Fee the recipient will pay to redeem
/// * `keyset_fees` - Map of keyset ID to fee_ppk
/// * `force_swap` - If true, all proofs go to swap
/// * `is_exact_or_offline` - If true (exact match or offline mode), all proofs go to send
pub fn split_proofs_for_send(
    proofs: Proofs,
    send_amounts: &[Amount],
    amount: Amount,
    send_fee: Amount,
    keyset_fees: &HashMap<Id, u64>,
    force_swap: bool,
    is_exact_or_offline: bool,
) -> Result<ProofSplitResult, Error> {
    let mut proofs_to_swap = Proofs::new();
    let mut proofs_to_send = Proofs::new();

    if force_swap {
        proofs_to_swap = proofs;
    } else if is_exact_or_offline {
        proofs_to_send = proofs;
    } else {
        let mut remaining_send_amounts: Vec<Amount> = send_amounts.to_vec();
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

        // Check if swap is actually needed
        if !proofs_to_swap.is_empty() {
            let swap_output_needed = (amount + send_fee)
                .checked_sub(proofs_to_send.total_amount()?)
                .unwrap_or(Amount::ZERO);

            if swap_output_needed == Amount::ZERO {
                // proofs_to_send already covers the full amount, no swap needed
                // Clear proofs_to_swap - these are just leftover proofs that don't match
                // any send denomination but aren't needed for the send
                proofs_to_swap.clear();
            } else {
                // Ensure proofs_to_swap can cover the swap's input fee plus the needed output
                loop {
                    let swap_input_fee =
                        calculate_fee(&proofs_to_swap.count_by_keyset(), keyset_fees)?;
                    let swap_total = proofs_to_swap.total_amount()?;

                    let swap_can_produce = swap_total.checked_sub(swap_input_fee);

                    match swap_can_produce {
                        Some(can_produce) if can_produce >= swap_output_needed => {
                            break;
                        }
                        _ => {
                            if proofs_to_send.is_empty() {
                                return Err(Error::InsufficientFunds);
                            }

                            // Move the smallest proof from send to swap
                            proofs_to_send.sort_by(|a, b| a.amount.cmp(&b.amount));
                            let proof_to_move = proofs_to_send.remove(0);
                            proofs_to_swap.push(proof_to_move);
                        }
                    }
                }
            }
        }
    }

    let swap_fee = calculate_fee(&proofs_to_swap.count_by_keyset(), keyset_fees)?;

    Ok(ProofSplitResult {
        proofs_to_send,
        proofs_to_swap,
        swap_fee,
    })
}

#[cfg(test)]
mod tests {
    use cdk_common::secret::Secret;
    use cdk_common::{Amount, Id, Proof, PublicKey};

    use super::*;

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

    fn proofs(amounts: &[u64]) -> Proofs {
        amounts.iter().map(|&a| proof(a)).collect()
    }

    fn keyset_fees_with_ppk(fee_ppk: u64) -> HashMap<Id, u64> {
        let mut fees = HashMap::new();
        fees.insert(id(), fee_ppk);
        fees
    }

    fn amounts(values: &[u64]) -> Vec<Amount> {
        values.iter().map(|&v| Amount::from(v)).collect()
    }

    // ========================================================================
    // No Swap Needed (Exact Proofs) Tests
    // ========================================================================

    #[test]
    fn test_split_exact_match_simple() {
        let input_proofs = proofs(&[8, 2]);
        let send_amounts = amounts(&[8, 2]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(10),
            Amount::from(1),
            &keyset_fees,
            false,
            true, // exact match
        )
        .unwrap();

        assert_eq!(result.proofs_to_send.len(), 2);
        assert!(result.proofs_to_swap.is_empty());
        assert_eq!(result.swap_fee, Amount::ZERO);
    }

    #[test]
    fn test_split_exact_match_six_proofs() {
        let input_proofs = proofs(&[2048, 1024, 512, 256, 128, 32]);
        let send_amounts = amounts(&[2048, 1024, 512, 256, 128, 32]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(4000),
            Amount::from(2),
            &keyset_fees,
            false,
            true,
        )
        .unwrap();

        assert_eq!(result.proofs_to_send.len(), 6);
        assert!(result.proofs_to_swap.is_empty());
    }

    #[test]
    fn test_split_exact_match_ten_proofs() {
        let input_proofs = proofs(&[4096, 2048, 1024, 512, 256, 128, 64, 32, 16, 8]);
        let send_amounts = amounts(&[4096, 2048, 1024, 512, 256, 128, 64, 32, 16, 8]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(8000),
            Amount::from(2),
            &keyset_fees,
            false,
            true,
        )
        .unwrap();

        assert_eq!(result.proofs_to_send.len(), 10);
        assert!(result.proofs_to_swap.is_empty());
    }

    #[test]
    fn test_split_exact_match_powers_of_two() {
        let input_proofs = proofs(&[4096, 512, 256, 128, 8]);
        let send_amounts = amounts(&[4096, 512, 256, 128, 8]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(5000),
            Amount::from(1),
            &keyset_fees,
            false,
            true,
        )
        .unwrap();

        assert_eq!(result.proofs_to_send.len(), 5);
        assert!(result.proofs_to_swap.is_empty());
    }

    // ========================================================================
    // Swap Required - Partial Match Tests
    // ========================================================================

    #[test]
    fn test_split_single_mismatch() {
        let input_proofs = proofs(&[8, 4, 2, 1]);
        let send_amounts = amounts(&[8, 2]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(10),
            Amount::from(1),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        let send_amounts_result: Vec<u64> = result
            .proofs_to_send
            .iter()
            .map(|p| p.amount.into())
            .collect();
        let swap_amounts_result: Vec<u64> = result
            .proofs_to_swap
            .iter()
            .map(|p| p.amount.into())
            .collect();

        assert!(send_amounts_result.contains(&8));
        assert!(send_amounts_result.contains(&2));
        assert!(swap_amounts_result.contains(&4) || swap_amounts_result.contains(&1));
    }

    #[test]
    fn test_split_multiple_mismatches() {
        let input_proofs = proofs(&[4096, 1024, 512, 256, 64, 32, 16, 8]);
        let send_amounts = amounts(&[4096, 512, 256, 128, 8]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(5000),
            Amount::from(1),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        let send_amounts_result: Vec<u64> = result
            .proofs_to_send
            .iter()
            .map(|p| p.amount.into())
            .collect();

        // 4096, 512, 256, 8 should match; 128 not in input, 1024, 64, 32, 16 to swap
        assert!(send_amounts_result.contains(&4096));
        assert!(send_amounts_result.contains(&512));
        assert!(send_amounts_result.contains(&256));
        assert!(send_amounts_result.contains(&8));
        assert!(!result.proofs_to_swap.is_empty());
    }

    #[test]
    fn test_split_half_match() {
        let input_proofs = proofs(&[2048, 2048, 1024, 512, 256, 128, 64, 32]);
        let send_amounts = amounts(&[4096, 512, 256, 128, 8]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(5000),
            Amount::from(1),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        let send_amounts_result: Vec<u64> = result
            .proofs_to_send
            .iter()
            .map(|p| p.amount.into())
            .collect();

        // Only 512, 256, 128 should match (no 4096 or 8 in input)
        assert!(send_amounts_result.contains(&512));
        assert!(send_amounts_result.contains(&256));
        assert!(send_amounts_result.contains(&128));
        assert!(!result.proofs_to_swap.is_empty());
    }

    #[test]
    fn test_split_large_swap_set() {
        let input_proofs = proofs(&[1024, 1024, 1024, 1024, 1024, 512, 256, 128, 64, 32, 16, 8]);
        let send_amounts = amounts(&[4096, 512, 256, 128, 8]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(5000),
            Amount::from(1),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        let send_amounts_result: Vec<u64> = result
            .proofs_to_send
            .iter()
            .map(|p| p.amount.into())
            .collect();

        assert!(send_amounts_result.contains(&512));
        assert!(send_amounts_result.contains(&256));
        assert!(send_amounts_result.contains(&128));
        assert!(send_amounts_result.contains(&8));
        // All 1024s and 64, 32, 16 should be in swap
        assert!(result.proofs_to_swap.len() >= 5);
    }

    #[test]
    fn test_split_dense_small_proofs() {
        let input_proofs = proofs(&[
            512, 256, 256, 128, 128, 128, 64, 64, 64, 64, 32, 32, 16, 16, 8, 8, 4, 4, 2, 2,
        ]);
        let send_amounts = amounts(&[1024, 256, 128, 64, 16, 8, 4]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(1500),
            Amount::from(2),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // No 1024 in input, so swap needed
        assert!(!result.proofs_to_swap.is_empty());
        // Should have matched some proofs
        let send_amounts_result: Vec<u64> = result
            .proofs_to_send
            .iter()
            .map(|p| p.amount.into())
            .collect();
        assert!(
            send_amounts_result.contains(&256)
                || send_amounts_result.contains(&128)
                || send_amounts_result.contains(&64)
        );
    }

    // ========================================================================
    // Swap Required - No Match Tests
    // ========================================================================

    #[test]
    fn test_split_fragmented_no_match() {
        // 64×10, 32×5, 16×10, 8×5 = 640 + 160 + 160 + 40 = 1000
        let mut input_amounts = vec![];
        for _ in 0..10 {
            input_amounts.push(64);
        }
        for _ in 0..5 {
            input_amounts.push(32);
        }
        for _ in 0..10 {
            input_amounts.push(16);
        }
        for _ in 0..5 {
            input_amounts.push(8);
        }
        let input_proofs = proofs(&input_amounts);
        let send_amounts = amounts(&[512, 256, 128, 64, 32, 8]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(1000),
            Amount::from(2),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // Some proofs should match (64, 32, 8 exist in input)
        let send_amounts_result: Vec<u64> = result
            .proofs_to_send
            .iter()
            .map(|p| p.amount.into())
            .collect();
        // 512, 256, 128 don't exist so need swap
        assert!(!result.proofs_to_swap.is_empty());
        // But 64, 32, 8 should be in send
        assert!(
            send_amounts_result.contains(&64)
                || send_amounts_result.contains(&32)
                || send_amounts_result.contains(&8)
        );
    }

    #[test]
    fn test_split_large_fragmented() {
        // 256×8, 128×4, 64×8, 32×4, 16×8, 8×4 = 2048 + 512 + 512 + 128 + 128 + 32 = 3360
        let mut input_amounts = vec![];
        for _ in 0..8 {
            input_amounts.push(256);
        }
        for _ in 0..4 {
            input_amounts.push(128);
        }
        for _ in 0..8 {
            input_amounts.push(64);
        }
        for _ in 0..4 {
            input_amounts.push(32);
        }
        for _ in 0..8 {
            input_amounts.push(16);
        }
        for _ in 0..4 {
            input_amounts.push(8);
        }
        let input_proofs = proofs(&input_amounts);
        // Total = 8*256 + 4*128 + 8*64 + 4*32 + 8*16 + 4*8 = 2048+512+512+128+128+32 = 3360
        // Use send_amounts that DON'T all exist in input to force swap
        let send_amounts = amounts(&[512, 256, 128, 64, 32, 8]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(1000),
            Amount::from(2),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // 256, 128, 64, 32, 8 exist in input but 512 doesn't
        // proofs_to_send = [256, 128, 64, 32, 8] = 488
        // swap_output_needed = (1000 + 2) - 488 = 514
        let send_amounts_result: Vec<u64> = result
            .proofs_to_send
            .iter()
            .map(|p| p.amount.into())
            .collect();
        assert!(
            send_amounts_result.contains(&256)
                || send_amounts_result.contains(&128)
                || send_amounts_result.contains(&32)
        );
        // Most proofs need swapping since we need to produce 514 from swap
        assert!(result.proofs_to_swap.len() > 10);
    }

    // ========================================================================
    // Swap Fee Adjustment Tests
    // ========================================================================

    #[test]
    fn test_split_swap_sufficient() {
        let input_proofs = proofs(&[4096, 512, 256, 128, 8, 64, 32]);
        let send_amounts = amounts(&[4096, 512, 256, 128, 8]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(5000),
            Amount::from(1),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // 64, 32 go to swap (96 total), fee = 1, can produce 95 >= 0 needed
        let swap_amounts: Vec<u64> = result
            .proofs_to_swap
            .iter()
            .map(|p| p.amount.into())
            .collect();
        assert!(swap_amounts.contains(&64) || swap_amounts.contains(&32));
    }

    #[test]
    fn test_split_swap_barely_sufficient() {
        // Test where proofs_to_send doesn't fully cover amount+fee, requiring swap
        let input_proofs = proofs(&[2048, 1024, 256, 128, 32, 16, 8, 4, 2, 1]);
        // Note: removed 64 from input, so send_amounts won't fully match
        let send_amounts = amounts(&[2048, 1024, 256, 128, 64]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(3520),
            Amount::from(1),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // proofs_to_send = [2048, 1024, 256, 128] = 3456 (no 64 in input)
        // swap_output_needed = (3520 + 1) - 3456 = 65
        // proofs_to_swap = [32, 16, 8, 4, 2, 1] = 63, fee = 2, can produce 61 < 65
        // So swap needs more proofs moved from send
        assert!(!result.proofs_to_swap.is_empty());

        let swap_total: u64 = result
            .proofs_to_swap
            .iter()
            .map(|p| u64::from(p.amount))
            .sum();
        let swap_fee: u64 = result.swap_fee.into();
        assert!(swap_total - swap_fee >= 65);
    }

    #[test]
    fn test_split_move_one_proof() {
        // Scenario: to_send has [4096, 512, 256, 128, 64, 32], to_swap has [16, 8]
        // swap_output_needed = 50, swap can produce 24-1=23 < 50
        // Need to move 32 to swap: 24+32=56, fee=1, can produce 55 >= 50
        let input_proofs = proofs(&[4096, 512, 256, 128, 64, 32, 16, 8]);
        let send_amounts = amounts(&[4096, 512, 256, 128, 64, 32]);
        let keyset_fees = keyset_fees_with_ppk(200);

        // We need swap to produce 50 sats
        // send = 4096+512+256+128+64+32 = 5088, amount+fee = 5088+50 = 5138
        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(5088),
            Amount::from(50),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // Should have moved 32 (smallest) from send to swap
        let swap_total: u64 = result
            .proofs_to_swap
            .iter()
            .map(|p| u64::from(p.amount))
            .sum();
        // 16 + 8 + 32 = 56, or some variation
        assert!(swap_total >= 50);
    }

    #[test]
    fn test_split_move_multiple_proofs() {
        let input_proofs = proofs(&[2048, 1024, 512, 256, 128, 64, 8, 4, 2, 1]);
        let send_amounts = amounts(&[2048, 1024, 512, 256, 128, 64]);
        let keyset_fees = keyset_fees_with_ppk(200);

        // swap has [8,4,2,1] = 15, need output of 100
        // fee = 1, can produce 14 < 100
        // Need to move proofs
        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(4032),
            Amount::from(100),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        let swap_total: u64 = result
            .proofs_to_swap
            .iter()
            .map(|p| u64::from(p.amount))
            .sum();
        let swap_fee: u64 = result.swap_fee.into();
        // Should have moved enough to cover 100
        assert!(swap_total - swap_fee >= 100);
    }

    #[test]
    fn test_split_high_fee_many_proofs() {
        let input_proofs = proofs(&[1024, 512, 256, 128, 64, 32, 16, 8, 4, 4, 2, 2, 1, 1, 1, 1]);
        let send_amounts = amounts(&[1024, 512, 256, 128, 64, 32, 16, 8]);
        let keyset_fees = keyset_fees_with_ppk(1000); // 1 sat per proof

        // swap has [4,4,2,2,1,1,1,1] = 16, 8 proofs, fee = 8, can produce 8
        // Need to produce 10
        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(2040),
            Amount::from(10),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        let swap_total: u64 = result
            .proofs_to_swap
            .iter()
            .map(|p| u64::from(p.amount))
            .sum();
        let swap_fee: u64 = result.swap_fee.into();
        assert!(swap_total - swap_fee >= 10);
    }

    #[test]
    fn test_split_fee_eats_small_proofs() {
        let input_proofs = proofs(&[4096, 512, 256, 128, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
        let send_amounts = amounts(&[4096, 512, 256, 128]);
        let keyset_fees = keyset_fees_with_ppk(1000); // 1 sat per proof

        // swap has 10×1 = 10, fee = 10, can produce 0
        // Need to produce 5
        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(4992),
            Amount::from(5),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        let swap_total: u64 = result
            .proofs_to_swap
            .iter()
            .map(|p| u64::from(p.amount))
            .sum();
        let swap_fee: u64 = result.swap_fee.into();
        // Must have moved a larger proof (128) to swap
        assert!(swap_total - swap_fee >= 5);
        assert!(swap_total > 10); // More than just the 1s
    }

    #[test]
    fn test_split_cascading_fee_increase() {
        let input_proofs = proofs(&[2048, 1024, 512, 256, 128, 64, 32, 16, 8, 4, 2, 1]);
        let send_amounts = amounts(&[2048, 1024, 512, 256, 128, 64]);
        let keyset_fees = keyset_fees_with_ppk(500); // 0.5 sat per proof

        // swap has [32,16,8,4,2,1] = 63, 6 proofs, fee = 3, can produce 60
        // Need 80
        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(4032),
            Amount::from(80),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        let swap_total: u64 = result
            .proofs_to_swap
            .iter()
            .map(|p| u64::from(p.amount))
            .sum();
        let swap_fee: u64 = result.swap_fee.into();
        assert!(swap_total - swap_fee >= 80);
    }

    // ========================================================================
    // Complex Scenarios with Many Proofs
    // ========================================================================

    #[test]
    fn test_split_20_proofs_mixed() {
        // [2048, 1024, 512, 256×2, 128×2, 64×4, 32×4, 16×4]
        // Count: 1 + 1 + 1 + 2 + 2 + 4 + 4 + 4 = 19 proofs. Need one more for 20.
        let mut input_amounts = vec![2048, 1024, 512];
        input_amounts.extend(vec![256; 2]);
        input_amounts.extend(vec![128; 2]);
        input_amounts.extend(vec![64; 4]);
        input_amounts.extend(vec![32; 4]);
        input_amounts.extend(vec![16; 4]);
        input_amounts.push(8); // Add one more to make 20
        let input_proofs = proofs(&input_amounts);
        // Use send amounts that match proofs in input
        let send_amounts = amounts(&[2048, 1024, 512, 256, 128]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(3968), // 2048+1024+512+256+128 = 3968
            Amount::from(1),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // All send_amounts exist in input
        let send_amounts_result: Vec<u64> = result
            .proofs_to_send
            .iter()
            .map(|p| p.amount.into())
            .collect();
        // Check some proofs went to send
        assert!(
            send_amounts_result.contains(&2048)
                || send_amounts_result.contains(&1024)
                || send_amounts_result.contains(&512)
        );
        // Some proofs to swap (the extras)
        assert!(!result.proofs_to_swap.is_empty());
        // Total proofs preserved
        assert_eq!(
            result.proofs_to_send.len() + result.proofs_to_swap.len(),
            20
        );
    }

    #[test]
    fn test_split_30_small_proofs() {
        // [256×2, 128×4, 64×6, 32×6, 16×6, 8×6]
        let mut input_amounts = vec![];
        input_amounts.extend(vec![256; 2]);
        input_amounts.extend(vec![128; 4]);
        input_amounts.extend(vec![64; 6]);
        input_amounts.extend(vec![32; 6]);
        input_amounts.extend(vec![16; 6]);
        input_amounts.extend(vec![8; 6]);
        let input_proofs = proofs(&input_amounts);
        let send_amounts = amounts(&[1024, 512, 256, 128, 64, 8]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(2000),
            Amount::from(6), // 30 proofs = 6 sat fee @ 200ppk
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            result.proofs_to_send.len() + result.proofs_to_swap.len(),
            30
        );
    }

    #[test]
    fn test_split_15_proofs_high_fee() {
        // [4096, 1024×2, 512×2, 256×2, 128×2, 64×2, 32×2, 16×2]
        let mut input_amounts = vec![4096];
        input_amounts.extend(vec![1024; 2]);
        input_amounts.extend(vec![512; 2]);
        input_amounts.extend(vec![256; 2]);
        input_amounts.extend(vec![128; 2]);
        input_amounts.extend(vec![64; 2]);
        input_amounts.extend(vec![32; 2]);
        input_amounts.extend(vec![16; 2]);
        let input_proofs = proofs(&input_amounts);
        let send_amounts = amounts(&[4096, 2048, 1024, 512, 256, 64]);
        let keyset_fees = keyset_fees_with_ppk(500);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(8000),
            Amount::from(8), // 15 proofs = 8 sat fee @ 500ppk
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            result.proofs_to_send.len() + result.proofs_to_swap.len(),
            15
        );
    }

    #[test]
    fn test_split_uniform_25_proofs() {
        let input_proofs = proofs(&[256; 25]);
        let send_amounts = amounts(&[4096, 512, 256, 128, 8]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(5000),
            Amount::from(1),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // Only one 256 matches
        let send_count = result.proofs_to_send.len();
        let swap_count = result.proofs_to_swap.len();
        assert_eq!(send_count + swap_count, 25);
        assert_eq!(send_count, 1); // Only one 256 matches
    }

    #[test]
    fn test_split_tiered_18_proofs() {
        // [4096, 2048, 1024×2, 512×2, 256×4, 128×4, 64×4]
        let mut input_amounts = vec![4096, 2048];
        input_amounts.extend(vec![1024; 2]);
        input_amounts.extend(vec![512; 2]);
        input_amounts.extend(vec![256; 4]);
        input_amounts.extend(vec![128; 4]);
        input_amounts.extend(vec![64; 4]);
        let input_proofs = proofs(&input_amounts);
        let send_amounts = amounts(&[8192, 1024, 512, 256, 8]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(10000),
            Amount::from(4), // 18 proofs = 4 sat fee @ 200ppk
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            result.proofs_to_send.len() + result.proofs_to_swap.len(),
            18
        );
    }

    #[test]
    fn test_split_dust_consolidation() {
        // [16×50, 8×50, 4×50, 2×50, 1×50] = 250 proofs
        let mut input_amounts = vec![];
        input_amounts.extend(vec![16; 50]);
        input_amounts.extend(vec![8; 50]);
        input_amounts.extend(vec![4; 50]);
        input_amounts.extend(vec![2; 50]);
        input_amounts.extend(vec![1; 50]);
        let input_proofs = proofs(&input_amounts);
        let send_amounts = amounts(&[1024, 256, 128, 64, 16, 8, 4]);
        let keyset_fees = keyset_fees_with_ppk(100);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(1500),
            Amount::from(25), // 250 proofs = 25 sat fee @ 100ppk
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // 16, 8, 4 exist and match
        let send_amounts_result: Vec<u64> = result
            .proofs_to_send
            .iter()
            .map(|p| p.amount.into())
            .collect();
        assert!(
            send_amounts_result.contains(&16)
                || send_amounts_result.contains(&8)
                || send_amounts_result.contains(&4)
        );
    }

    // ========================================================================
    // Force Swap Scenarios
    // ========================================================================

    #[test]
    fn test_split_force_swap_8_proofs() {
        let input_proofs = proofs(&[2048, 1024, 512, 256, 128, 64, 32, 16]);
        let send_amounts = amounts(&[2048, 1024, 512, 256, 128, 32]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(3000),
            Amount::from(2),
            &keyset_fees,
            true, // force_swap
            false,
        )
        .unwrap();

        assert!(result.proofs_to_send.is_empty());
        assert_eq!(result.proofs_to_swap.len(), 8);
    }

    #[test]
    fn test_split_force_swap_15_proofs() {
        let mut input_amounts = vec![];
        input_amounts.extend(vec![1024; 5]);
        input_amounts.extend(vec![512; 5]);
        input_amounts.extend(vec![256; 5]);
        let input_proofs = proofs(&input_amounts);
        let send_amounts = amounts(&[8000]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(8000),
            Amount::from(3),
            &keyset_fees,
            true, // force_swap
            false,
        )
        .unwrap();

        assert!(result.proofs_to_send.is_empty());
        assert_eq!(result.proofs_to_swap.len(), 15);
    }

    #[test]
    fn test_split_force_swap_fragmented() {
        // 64×10, 32×10, 16×10, 8×10 = 40 proofs
        let mut input_amounts = vec![];
        input_amounts.extend(vec![64; 10]);
        input_amounts.extend(vec![32; 10]);
        input_amounts.extend(vec![16; 10]);
        input_amounts.extend(vec![8; 10]);
        let input_proofs = proofs(&input_amounts);
        let send_amounts = amounts(&[2000]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(2000),
            Amount::from(8),
            &keyset_fees,
            true, // force_swap
            false,
        )
        .unwrap();

        assert!(result.proofs_to_send.is_empty());
        assert_eq!(result.proofs_to_swap.len(), 40);
    }

    // ========================================================================
    // Edge Cases
    // ========================================================================

    #[test]
    fn test_split_single_large_proof() {
        let input_proofs = proofs(&[8192]);
        let send_amounts = amounts(&[4096, 2048, 1024, 512, 256, 64]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(8000),
            Amount::from(1),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // 8192 doesn't match any send amount, goes to swap
        assert!(result.proofs_to_send.is_empty());
        assert_eq!(result.proofs_to_swap.len(), 1);
    }

    #[test]
    fn test_split_many_1sat_proofs() {
        let input_proofs = proofs(&[1; 100]);
        let send_amounts = amounts(&[32, 16, 2]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(50),
            Amount::from(20), // 100 proofs = 20 sat fee @ 200ppk
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // No proofs match (no 32, 16, or 2 individual proofs)
        assert!(result.proofs_to_send.is_empty());
        assert_eq!(result.proofs_to_swap.len(), 100);
    }

    #[test]
    fn test_split_all_same_denomination() {
        let input_proofs = proofs(&[512; 10]);
        let send_amounts = amounts(&[4096, 512, 256, 128, 8]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(4000),
            Amount::from(2),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // Only one 512 matches
        let send_count = result.proofs_to_send.len();
        assert_eq!(send_count, 1);
        assert_eq!(result.proofs_to_swap.len(), 9);
    }

    #[test]
    fn test_split_alternating_sizes() {
        let input_proofs = proofs(&[1024, 64, 1024, 64, 1024, 64, 1024, 64]);
        let send_amounts = amounts(&[4096, 256, 128]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(4000),
            Amount::from(2),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // No proofs match exactly
        assert!(result.proofs_to_send.is_empty());
        assert_eq!(result.proofs_to_swap.len(), 8);
    }

    #[test]
    fn test_split_power_of_two_boundary() {
        let input_proofs = proofs(&[2048, 1024, 512, 256, 128, 64, 32, 16, 8, 4, 2, 1]);
        let send_amounts = amounts(&[2048, 1024, 512, 256, 128, 64, 32, 16, 8, 4, 2, 1]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(4095),
            Amount::from(3), // 12 proofs = 3 sat fee @ 200ppk
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // All proofs match
        assert_eq!(result.proofs_to_send.len(), 12);
        assert!(result.proofs_to_swap.is_empty());
    }

    #[test]
    fn test_split_just_over_boundary() {
        // Total = 2048+1024+512+256+128+64+32+16+8+4+2+1+1 = 4096
        // With an extra proof to give some buffer for fees
        let input_proofs = proofs(&[2048, 1024, 512, 256, 128, 64, 32, 16, 8, 4, 2, 1, 1, 64]);
        // Total now = 4160
        let send_amounts = amounts(&[2048, 1024, 512, 1]);
        let keyset_fees = keyset_fees_with_ppk(200);

        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(3585), // 2048+1024+512+1 = 3585
            Amount::from(3),    // 14 proofs = 3 sat fee @ 200ppk
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // 2048, 1024, 512, 1 match
        let send_amounts_result: Vec<u64> = result
            .proofs_to_send
            .iter()
            .map(|p| p.amount.into())
            .collect();
        assert!(send_amounts_result.contains(&1) || send_amounts_result.contains(&2048));
        // Some proofs go to swap
        assert!(!result.proofs_to_swap.is_empty());
        // Total proofs preserved
        assert_eq!(
            result.proofs_to_send.len() + result.proofs_to_swap.len(),
            14
        );
    }

    // ========================================================================
    // Regression Tests
    // ========================================================================

    #[test]
    fn test_split_regression_insufficient_swap_fee() {
        // Scenario where initial swap proofs can't cover their own fee
        let input_proofs = proofs(&[4096, 512, 256, 128, 1, 1]);
        let send_amounts = amounts(&[4096, 512, 256, 128]);
        let keyset_fees = keyset_fees_with_ppk(1000); // 1 sat per proof

        // swap has [1,1] = 2, fee = 2, can produce 0
        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(4992),
            Amount::from(1),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // Should have moved proofs to make swap viable
        let swap_total: u64 = result
            .proofs_to_swap
            .iter()
            .map(|p| u64::from(p.amount))
            .sum();
        let swap_fee: u64 = result.swap_fee.into();
        // Must be able to produce at least 1
        assert!(swap_total > swap_fee || result.proofs_to_swap.is_empty());
    }

    #[test]
    fn test_split_regression_many_small_in_swap() {
        // Many small proofs in swap that individually have high fee overhead
        let mut input_amounts = vec![4096, 1024];
        input_amounts.extend(vec![1; 20]);
        let input_proofs = proofs(&input_amounts);
        let send_amounts = amounts(&[4096, 1024]);
        let keyset_fees = keyset_fees_with_ppk(500);

        // swap has 20×1 = 20, fee = 10, can produce 10
        // Need to produce something for change
        let result = split_proofs_for_send(
            input_proofs,
            &send_amounts,
            Amount::from(5120),
            Amount::from(5),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // Should handle this gracefully
        assert!(result.proofs_to_send.len() + result.proofs_to_swap.len() == 22);
    }
}
