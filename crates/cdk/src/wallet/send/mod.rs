//! Send module providing [`Wallet::prepare_send`] for creating [`PreparedSend`] transactions.
//!
//! Use [`PreparedSend::confirm`] to complete the send or [`PreparedSend::cancel`] to release reserved proofs.

use std::collections::HashMap;
use std::fmt::Debug;

use cdk_common::Id;
use tracing::instrument;
use uuid::Uuid;

use super::SendKind;
use crate::amount::SplitTarget;
use crate::fees::calculate_fee;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{Proofs, SpendingConditions, Token};
use crate::{Amount, Error, Wallet};

pub(crate) mod saga;

use saga::SendSaga;

/// Prepared send transaction
///
/// Created by [`Wallet::prepare_send`]. Call [`confirm`](Self::confirm) to complete the send
/// and create a token, or [`cancel`](Self::cancel) to release reserved proofs.
pub struct PreparedSend<'a> {
    wallet: &'a Wallet,
    operation_id: Uuid,
    // Cached display and confirmation data
    amount: Amount,
    options: SendOptions,
    proofs_to_swap: Proofs,
    proofs_to_send: Proofs,
    swap_fee: Amount,
    send_fee: Amount,
}

impl PreparedSend<'_> {
    /// Operation ID for this prepared send
    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }

    /// Amount to send
    pub fn amount(&self) -> Amount {
        self.amount
    }

    /// Send options
    pub fn options(&self) -> &SendOptions {
        &self.options
    }

    /// Proofs that need to be swapped before sending
    pub fn proofs_to_swap(&self) -> &Proofs {
        &self.proofs_to_swap
    }

    /// Fee for the swap operation
    pub fn swap_fee(&self) -> Amount {
        self.swap_fee
    }

    /// Proofs that will be sent directly
    pub fn proofs_to_send(&self) -> &Proofs {
        &self.proofs_to_send
    }

    /// Fee the recipient will pay to redeem the token
    pub fn send_fee(&self) -> Amount {
        self.send_fee
    }

    /// All proofs (both to swap and to send)
    pub fn proofs(&self) -> Proofs {
        let mut proofs = self.proofs_to_swap.clone();
        proofs.extend(self.proofs_to_send.clone());
        proofs
    }

    /// Total fee (swap + send)
    pub fn fee(&self) -> Amount {
        self.swap_fee + self.send_fee
    }

    /// Confirm the prepared send and create a token
    pub async fn confirm(self, memo: Option<SendMemo>) -> Result<Token, Error> {
        self.wallet
            .confirm_send(
                self.operation_id,
                self.amount,
                self.options,
                self.proofs_to_swap,
                self.proofs_to_send,
                self.swap_fee,
                self.send_fee,
                memo,
            )
            .await
    }

    /// Cancel the prepared send and release reserved proofs
    pub async fn cancel(self) -> Result<(), Error> {
        self.wallet
            .cancel_send(self.operation_id, self.proofs_to_swap, self.proofs_to_send)
            .await
    }
}

impl Debug for PreparedSend<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedSend")
            .field("operation_id", &self.operation_id)
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

impl Wallet {
    /// Prepare a send transaction
    ///
    /// This function prepares a send transaction by selecting proofs to send and proofs to swap.
    /// By doing so, it ensures that the wallet user is able to view the fees associated with the
    /// send transaction before confirming.
    ///
    /// # Example
    /// ```no_run
    /// # use cdk::wallet::{Wallet, SendOptions};
    /// # use cdk::Amount;
    /// # async fn example(wallet: &Wallet) -> Result<(), Box<dyn std::error::Error>> {
    /// let prepared = wallet
    ///     .prepare_send(Amount::from(10), SendOptions::default())
    ///     .await?;
    /// println!("Fee: {}", prepared.fee());
    /// let token = prepared.confirm(None).await?;
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(skip(self), err)]
    pub async fn prepare_send(
        &self,
        amount: Amount,
        opts: SendOptions,
    ) -> Result<PreparedSend<'_>, Error> {
        let saga = SendSaga::new(self);
        let prepared_saga = saga.prepare(amount, opts).await?;

        // Extract data from the saga into PreparedSend
        let prepared = PreparedSend {
            wallet: self,
            operation_id: prepared_saga.operation_id(),
            amount: prepared_saga.amount(),
            options: prepared_saga.options().clone(),
            proofs_to_swap: prepared_saga.proofs_to_swap().clone(),
            proofs_to_send: prepared_saga.proofs_to_send().clone(),
            swap_fee: prepared_saga.swap_fee(),
            send_fee: prepared_saga.send_fee(),
        };

        Ok(prepared)
    }

    /// Called by `PreparedSend::confirm` with cached data.
    #[instrument(skip(self, options, proofs_to_swap, proofs_to_send))]
    #[allow(clippy::too_many_arguments)]
    pub async fn confirm_send(
        &self,
        operation_id: Uuid,
        amount: Amount,
        options: SendOptions,
        proofs_to_swap: Proofs,
        proofs_to_send: Proofs,
        swap_fee: Amount,
        send_fee: Amount,
        memo: Option<SendMemo>,
    ) -> Result<Token, Error> {
        let db_saga = self
            .localstore
            .get_saga(&operation_id)
            .await?
            .ok_or(Error::Custom("Saga not found".to_string()))?;

        let saga = SendSaga::from_prepared(
            self,
            operation_id,
            amount,
            options,
            proofs_to_swap,
            proofs_to_send,
            swap_fee,
            send_fee,
            db_saga,
        );
        let (token, _saga) = saga.confirm(memo).await?;
        Ok(token)
    }

    /// Called by `PreparedSend::cancel` with cached data.
    #[instrument(skip(self, proofs_to_swap, proofs_to_send))]
    pub async fn cancel_send(
        &self,
        operation_id: Uuid,
        proofs_to_swap: Proofs,
        proofs_to_send: Proofs,
    ) -> Result<(), Error> {
        let db_saga = self
            .localstore
            .get_saga(&operation_id)
            .await?
            .ok_or(Error::Custom("Saga not found".to_string()))?;

        let saga = SendSaga::from_prepared(
            self,
            operation_id,
            Amount::ZERO,           // Dummy
            SendOptions::default(), // Dummy
            proofs_to_swap,
            proofs_to_send,
            Amount::ZERO, // Dummy
            Amount::ZERO, // Dummy
            db_saga,
        );
        saga.cancel().await
    }

    /// Returns operation IDs for pending sends (tokens created but not claimed).
    #[instrument(skip(self))]
    pub async fn get_pending_sends(&self) -> Result<Vec<Uuid>, Error> {
        let incomplete = self.localstore.get_incomplete_sagas().await?;
        Ok(incomplete
            .into_iter()
            .filter_map(|s| {
                if s.mint_url != self.mint_url {
                    return None;
                }
                if let cdk_common::wallet::WalletSagaState::Send(
                    cdk_common::wallet::SendSagaState::TokenCreated,
                ) = s.state
                {
                    Some(s.id)
                } else {
                    None
                }
            })
            .collect())
    }

    /// Reclaims funds by swapping proofs back to the wallet.
    #[instrument(skip(self))]
    pub async fn revoke_send(&self, operation_id: Uuid) -> Result<Amount, Error> {
        let saga_record = self
            .localstore
            .get_saga(&operation_id)
            .await?
            .ok_or(Error::Custom("Saga not found".to_string()))?;

        if let cdk_common::wallet::WalletSagaState::Send(
            cdk_common::wallet::SendSagaState::TokenCreated,
        ) = saga_record.state
        {
            if let cdk_common::wallet::OperationData::Send(data) = saga_record.data.clone() {
                let proofs = data.proofs.ok_or(Error::Custom(
                    "No proofs found in pending send saga".to_string(),
                ))?;

                let saga = SendSaga {
                    wallet: self,
                    compensations: crate::wallet::saga::new_compensations(),
                    state_data: saga::state::TokenCreated {
                        operation_id,
                        proofs,
                        saga: saga_record,
                    },
                };

                return saga.revoke().await;
            }
        }

        Err(Error::Custom("Operation is not a pending send".to_string()))
    }

    /// Returns true if the token has been claimed by the recipient.
    #[instrument(skip(self))]
    pub async fn check_send_status(&self, operation_id: Uuid) -> Result<bool, Error> {
        let saga_record = self
            .localstore
            .get_saga(&operation_id)
            .await?
            .ok_or(Error::Custom("Saga not found".to_string()))?;

        // Report as pending during rollback to prevent race condition where swap
        // makes proofs appear spent before revocation completes.
        if let cdk_common::wallet::WalletSagaState::Send(
            cdk_common::wallet::SendSagaState::RollingBack,
        ) = saga_record.state
        {
            tracing::debug!(
                "Operation {} is rolling back - returning pending status",
                operation_id
            );
            return Ok(false);
        }

        if let cdk_common::wallet::WalletSagaState::Send(
            cdk_common::wallet::SendSagaState::TokenCreated,
        ) = saga_record.state
        {
            if let cdk_common::wallet::OperationData::Send(data) = saga_record.data.clone() {
                let proofs = data.proofs.ok_or(Error::Custom(
                    "No proofs found in pending send saga".to_string(),
                ))?;

                let saga = SendSaga {
                    wallet: self,
                    compensations: crate::wallet::saga::new_compensations(),
                    state_data: saga::state::TokenCreated {
                        operation_id,
                        proofs,
                        saga: saga_record,
                    },
                };

                return saga.check_status().await;
            }
        }

        Err(Error::Custom("Operation is not a pending send".to_string()))
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

/// Splits proofs between those that can be sent directly and those requiring swap.
pub(crate) fn split_proofs_for_send(
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
                // proofs_to_send already covers the full amount
                proofs_to_swap.clear();
            } else {
                // Ensure proofs_to_swap can cover the swap's input fee plus the needed output
                loop {
                    let swap_input_fee =
                        calculate_fee(&proofs_to_swap.count_by_keyset(), keyset_fees)?.total;
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

    let swap_fee = calculate_fee(&proofs_to_swap.count_by_keyset(), keyset_fees)?.total;

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

        assert!(!result.proofs_to_swap.is_empty());

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

        let send_amounts_result: Vec<u64> = result
            .proofs_to_send
            .iter()
            .map(|p| p.amount.into())
            .collect();
        assert!(!result.proofs_to_swap.is_empty());
        assert!(
            send_amounts_result.contains(&64)
                || send_amounts_result.contains(&32)
                || send_amounts_result.contains(&8)
        );
    }

    #[test]
    fn test_split_large_fragmented() {
        // 256×8, 128×4, 64×8, 32×4, 16×8, 8×4 = 3360
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
        let input_proofs = proofs(&[4096, 512, 256, 128, 64, 32, 16, 8]);
        let send_amounts = amounts(&[4096, 512, 256, 128, 64, 32]);
        let keyset_fees = keyset_fees_with_ppk(200);

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

        let swap_total: u64 = result
            .proofs_to_swap
            .iter()
            .map(|p| u64::from(p.amount))
            .sum();
        assert!(swap_total >= 50);
    }

    #[test]
    fn test_split_move_multiple_proofs() {
        let input_proofs = proofs(&[2048, 1024, 512, 256, 128, 64, 8, 4, 2, 1]);
        let send_amounts = amounts(&[2048, 1024, 512, 256, 128, 64]);
        let keyset_fees = keyset_fees_with_ppk(200);

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
        assert!(swap_total - swap_fee >= 100);
    }

    #[test]
    fn test_split_high_fee_many_proofs() {
        let input_proofs = proofs(&[1024, 512, 256, 128, 64, 32, 16, 8, 4, 4, 2, 2, 1, 1, 1, 1]);
        let send_amounts = amounts(&[1024, 512, 256, 128, 64, 32, 16, 8]);
        let keyset_fees = keyset_fees_with_ppk(1000);

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
        assert!(swap_total - swap_fee >= 5);
        assert!(swap_total > 10);
    }

    #[test]
    fn test_split_cascading_fee_increase() {
        let input_proofs = proofs(&[2048, 1024, 512, 256, 128, 64, 32, 16, 8, 4, 2, 1]);
        let send_amounts = amounts(&[2048, 1024, 512, 256, 128, 64]);
        let keyset_fees = keyset_fees_with_ppk(500);

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
        // [2048, 1024, 512, 256×2, 128×2, 64×4, 32×4, 16×4, 8] = 20 proofs
        let mut input_amounts = vec![2048, 1024, 512];
        input_amounts.extend(vec![256; 2]);
        input_amounts.extend(vec![128; 2]);
        input_amounts.extend(vec![64; 4]);
        input_amounts.extend(vec![32; 4]);
        input_amounts.extend(vec![16; 4]);
        input_amounts.push(8);
        let input_proofs = proofs(&input_amounts);
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
        assert!(
            send_amounts_result.contains(&2048)
                || send_amounts_result.contains(&1024)
                || send_amounts_result.contains(&512)
        );
        assert!(!result.proofs_to_swap.is_empty());
        assert_eq!(
            result.proofs_to_send.len() + result.proofs_to_swap.len(),
            20
        );
    }

    #[test]
    fn test_split_30_small_proofs() {
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
            Amount::from(6),
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
            Amount::from(8),
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

    // ========================================================================
    // Melt Use Case Tests
    // For melt: amount = inputs_needed (quote + fee_reserve),
    //           send_fee = target_fee (input fee for target proofs)
    // ========================================================================

    #[test]
    fn test_melt_exact_proofs_no_swap() {
        // Melt scenario: have exact proofs matching target denominations
        // quote_amount + fee_reserve = 100, target_fee = 2
        // Need proofs totaling 102
        let input_proofs = proofs(&[64, 32, 4, 2]);
        let target_amounts = amounts(&[64, 32, 4, 2]); // split of 102
        let keyset_fees = keyset_fees_with_ppk(500); // 0.5 sat per proof

        let result = split_proofs_for_send(
            input_proofs,
            &target_amounts,
            Amount::from(100), // inputs_needed_amount
            Amount::from(2),   // target_fee (4 proofs * 0.5)
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // All proofs match, no swap needed
        assert_eq!(result.proofs_to_send.len(), 4);
        assert!(result.proofs_to_swap.is_empty());
        assert_eq!(result.swap_fee, Amount::ZERO);
    }

    #[test]
    fn test_melt_excess_proofs_needs_swap() {
        // Melt scenario: have proofs totaling more than needed
        // Need 102 (100 + 2 fee), but have 128
        let input_proofs = proofs(&[128]);
        let target_amounts = amounts(&[64, 32, 4, 2]); // optimal split of 102
        let keyset_fees = keyset_fees_with_ppk(500);

        let result = split_proofs_for_send(
            input_proofs,
            &target_amounts,
            Amount::from(100),
            Amount::from(2),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // 128 doesn't match any target, needs swap
        assert!(result.proofs_to_send.is_empty());
        assert_eq!(result.proofs_to_swap.len(), 1);
        assert_eq!(result.proofs_to_swap[0].amount, Amount::from(128));
    }

    #[test]
    fn test_melt_partial_match_with_swap() {
        // Melt scenario: some proofs match, others need swap
        // Need 100 + 2 fee = 102, have [64, 32, 16, 8] = 120
        let input_proofs = proofs(&[64, 32, 16, 8]);
        let target_amounts = amounts(&[64, 32, 4, 2]); // optimal split of 102
        let keyset_fees = keyset_fees_with_ppk(500);

        let result = split_proofs_for_send(
            input_proofs,
            &target_amounts,
            Amount::from(100),
            Amount::from(2),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // 64 and 32 match, 16 and 8 go to swap
        let send_amounts: Vec<u64> = result
            .proofs_to_send
            .iter()
            .map(|p| p.amount.into())
            .collect();
        assert!(send_amounts.contains(&64));
        assert!(send_amounts.contains(&32));

        // 16 and 8 should be in swap to produce the remaining 6 (4+2)
        assert!(!result.proofs_to_swap.is_empty());
    }

    #[test]
    fn test_melt_with_exact_target_match() {
        // Melt scenario: all target amounts match input proofs exactly
        // When all targets are matched, unneeded proofs are dropped (not swapped)
        let input_proofs = proofs(&[64, 32, 8, 4, 2]);
        let target_amounts = amounts(&[64, 32, 8, 4, 2]); // exact match
        let keyset_fees = keyset_fees_with_ppk(1000);

        let result = split_proofs_for_send(
            input_proofs,
            &target_amounts,
            Amount::from(105), // amount
            Amount::from(5),   // target fee
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // All proofs match target amounts
        assert_eq!(result.proofs_to_send.len(), 5);
        // No swap needed when all targets matched
        assert!(result.proofs_to_swap.is_empty());
    }

    #[test]
    fn test_melt_swap_fee_calculated() {
        // Verify swap_fee is calculated correctly for melt
        let input_proofs = proofs(&[64, 32, 8, 4]); // 108 total
        let target_amounts = amounts(&[64, 32, 4]); // 100 split
        let keyset_fees = keyset_fees_with_ppk(1000); // 1 sat per proof

        let result = split_proofs_for_send(
            input_proofs,
            &target_amounts,
            Amount::from(98),
            Amount::from(2), // target fee for 3 proofs
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // 8 doesn't match, goes to swap
        // swap_fee should be 1 sat (1 proof * 1000 ppk / 1000)
        if !result.proofs_to_swap.is_empty() {
            assert_eq!(
                result.swap_fee,
                Amount::from(result.proofs_to_swap.len() as u64)
            );
        }
    }

    #[test]
    fn test_melt_large_quote_partial_match() {
        // Realistic melt: input proofs don't contain all target denominations
        // Input: [512, 256, 128, 64, 32, 16] = 1008
        // Target: [512, 256, 128, 64, 32, 8, 4, 2, 1] = 1007 (need 8, 4, 2, 1 from swap)
        let input_proofs = proofs(&[512, 256, 128, 64, 32, 16]);
        let target_amounts = amounts(&[512, 256, 128, 64, 32, 8, 4, 2, 1]);
        let keyset_fees = keyset_fees_with_ppk(375);

        let result = split_proofs_for_send(
            input_proofs,
            &target_amounts,
            Amount::from(1004),
            Amount::from(3),
            &keyset_fees,
            false,
            false,
        )
        .unwrap();

        // Check that matched proofs are in proofs_to_send
        let send_amounts: Vec<u64> = result
            .proofs_to_send
            .iter()
            .map(|p| p.amount.into())
            .collect();

        // These should match
        assert!(send_amounts.contains(&512));
        assert!(send_amounts.contains(&256));
        assert!(send_amounts.contains(&128));
        assert!(send_amounts.contains(&64));
        assert!(send_amounts.contains(&32));

        // 16 doesn't match any target, should be in swap to produce 8+4+2+1=15
        let swap_amounts: Vec<u64> = result
            .proofs_to_swap
            .iter()
            .map(|p| p.amount.into())
            .collect();
        assert!(swap_amounts.contains(&16));
    }
}
