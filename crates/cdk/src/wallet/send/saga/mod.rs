//! Send Saga - Type State Pattern Implementation
//!
//! This module implements the saga pattern for send operations using the typestate
//! pattern to enforce valid state transitions at compile-time.
//!
//! # State Flow
//!
//! ```text
//!                                  Normal Flow
//!                                  ===========
//!
//! [saga created] ──► ProofsReserved ──► TokenCreated ──► [recipient claims] ──► [completed]
//!                                            │
//!                                            └──► [user revokes]
//!                                                       │
//!                                                       ├─ proofs already spent ──► [completed] + error
//!                                                       │
//!                                                       └─ proofs not spent
//!                                                                 │
//!                                                                 ▼
//!                                                           RollingBack
//!                                                                 │
//!                                                       ┌─────────┴─────────┐
//!                                                       │                   │
//!                                                  swap succeeds       swap fails
//!                                                       │                   │
//!                                                       ▼                   ▼
//!                                                  [completed]        TokenCreated
//!                                                (proofs reclaimed)   (revert, retry)
//!
//!
//!                                  Recovery Flow
//!                                  =============
//!
//! ProofsReserved ─────────────────────────────────────────► [compensated]
//!
//! TokenCreated
//!     ├─ proofs spent ────────► [completed] (recipient claimed)
//!     ├─ proofs not spent ────► [completed] (saga deleted, token still valid)
//!     └─ mint unreachable ────► [skipped]
//!
//! RollingBack
//!     ├─ proofs spent ────────► [completed] (revoke swap succeeded)
//!     ├─ proofs not spent ────► TokenCreated (revert state, keep monitoring)
//!     └─ mint unreachable ────► [skipped]
//! ```
//!
//! # States
//!
//! | State | Description |
//! |-------|-------------|
//! | `ProofsReserved` | Proofs selected and reserved for sending, ready to create token |
//! | `TokenCreated` | Token created and ready to share, proofs marked as pending spent awaiting claim |
//! | `RollingBack` | Rollback in progress, reclaiming proofs via swap (transient state) |
//!
//! # Recovery Outcomes
//!
//! | Outcome | Description |
//! |---------|-------------|
//! | `[completed]` | Send finalized - either recipient claimed, or user successfully revoked |
//! | `[compensated]` | Send cancelled before token created, reserved proofs released |
//! | `[skipped]` | Recovery deferred (mint unreachable), will retry on next recovery |

use std::collections::HashMap;

use cdk_common::nut02::KeySetInfosMethods;
use cdk_common::util::unix_time;
use cdk_common::wallet::{
    OperationData, SendOperationData, SendSagaState, Transaction, TransactionDirection, WalletSaga,
    WalletSagaState,
};
use cdk_common::Id;
use tracing::instrument;

use self::state::{Initial, Prepared, TokenCreated};
use super::{split_proofs_for_send, SendMemo, SendOptions};
use crate::amount::SplitTarget;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{Proofs, State, Token};
use crate::wallet::saga::{
    add_compensation, execute_compensations, new_compensations, Compensations,
    RevertProofReservation,
};
use crate::wallet::SendKind;
use crate::{Amount, Error, Wallet};

pub(crate) mod resume;
pub(crate) mod state;

/// Saga pattern implementation for send operations.
///
/// Uses the typestate pattern to enforce valid state transitions at compile-time.
/// Each state (Initial, Prepared, Confirmed) is a distinct type, and operations
/// are only available on the appropriate type.
pub(crate) struct SendSaga<'a, S> {
    /// Wallet reference
    pub(crate) wallet: &'a Wallet,
    /// Compensating actions in LIFO order (most recent first)
    pub(crate) compensations: Compensations,
    /// State-specific data
    pub(crate) state_data: S,
}

impl<'a> SendSaga<'a, Initial> {
    /// Create a new send saga in the Initial state.
    pub fn new(wallet: &'a Wallet) -> Self {
        let operation_id = uuid::Uuid::new_v4();

        Self {
            wallet,
            compensations: new_compensations(),
            state_data: Initial { operation_id },
        }
    }

    /// Prepare the send operation by selecting and reserving proofs.
    ///
    /// Refreshes keysets (if online), selects and reserves proofs for the
    /// requested amount, and splits proofs between direct send and swap.
    ///
    /// Registers compensation to revert proof reservation on failure.
    #[instrument(skip_all)]
    pub async fn prepare(
        self,
        amount: Amount,
        opts: SendOptions,
    ) -> Result<SendSaga<'a, Prepared>, Error> {
        tracing::info!(
            "Preparing send for {} with operation {}",
            amount,
            self.state_data.operation_id
        );

        if opts.send_kind.is_online() {
            if let Err(e) = self.wallet.refresh_keysets().await {
                tracing::error!("Error refreshing keysets: {:?}. Using stored keysets", e);
            }
        }

        let keyset_fees = self.wallet.get_keyset_fees_and_amounts().await?;

        let mut available_proofs = self
            .wallet
            .get_proofs_with(
                Some(vec![State::Unspent]),
                opts.conditions.clone().map(|c| vec![c]),
            )
            .await?;

        let mut force_swap = false;
        let available_sum = available_proofs.total_amount()?;
        if available_sum < amount {
            if opts.conditions.is_none() || opts.send_kind.is_offline() {
                return Err(Error::InsufficientFunds);
            } else {
                tracing::debug!("Insufficient proofs matching conditions");
                force_swap = true;
                available_proofs = self
                    .wallet
                    .localstore
                    .get_proofs(
                        Some(self.wallet.mint_url.clone()),
                        Some(self.wallet.unit.clone()),
                        Some(vec![State::Unspent]),
                        Some(vec![]),
                    )
                    .await?
                    .into_iter()
                    .map(|p| p.proof)
                    .collect();
            }
        }

        let active_keyset_ids = self
            .wallet
            .get_mint_keysets()
            .await?
            .active()
            .map(|k| k.id)
            .collect();

        let active_keyset_id = self.wallet.get_active_keyset().await?.id;
        let fee_and_amounts = self
            .wallet
            .get_keyset_fees_and_amounts_by_id(active_keyset_id)
            .await?;

        let selection_amount = if opts.include_fee {
            let send_split = amount.split_with_fee(&fee_and_amounts)?;
            let send_fee = self
                .wallet
                .get_proofs_fee_by_count(
                    vec![(active_keyset_id, send_split.len() as u64)]
                        .into_iter()
                        .collect(),
                )
                .await?;
            amount + send_fee.total
        } else {
            amount
        };

        let selected_proofs = Wallet::select_proofs(
            selection_amount,
            available_proofs,
            &active_keyset_ids,
            &keyset_fees,
            opts.include_fee || force_swap,
        )?;
        let selected_total = selected_proofs.total_amount()?;

        let send_fee = if opts.include_fee {
            self.wallet.get_proofs_fee(&selected_proofs).await?.total
        } else {
            Amount::ZERO
        };

        if selected_total == amount + send_fee {
            return self
                .internal_prepare(amount, opts, selected_proofs, force_swap)
                .await;
        } else if opts.send_kind == SendKind::OfflineExact {
            return Err(Error::InsufficientFunds);
        }

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

        self.internal_prepare(amount, opts, selected_proofs, force_swap)
            .await
    }

    async fn internal_prepare(
        mut self,
        amount: Amount,
        opts: SendOptions,
        proofs: Proofs,
        force_swap: bool,
    ) -> Result<SendSaga<'a, Prepared>, Error> {
        let active_keyset_id = self.wallet.get_active_keyset().await?.id;
        let fee_and_amounts = self
            .wallet
            .get_keyset_fees_and_amounts_by_id(active_keyset_id)
            .await?;

        let (send_amounts, send_fee) = if opts.include_fee {
            let send_split = amount.split_with_fee(&fee_and_amounts)?;
            let send_fee = self
                .wallet
                .get_proofs_fee_by_count(
                    vec![(active_keyset_id, send_split.len() as u64)]
                        .into_iter()
                        .collect(),
                )
                .await?;
            (send_split, send_fee)
        } else {
            let send_split = amount.split(&fee_and_amounts)?;
            let send_fee = crate::fees::ProofsFeeBreakdown {
                total: Amount::ZERO,
                per_keyset: std::collections::HashMap::new(),
            };
            (send_split, send_fee)
        };

        let proof_ys = proofs.ys()?;

        self.wallet
            .localstore
            .update_proofs_state(proof_ys.clone(), State::Reserved)
            .await?;

        let memo_text = opts.memo.as_ref().map(|m| m.memo.clone());
        let saga = WalletSaga::new(
            self.state_data.operation_id,
            WalletSagaState::Send(SendSagaState::ProofsReserved),
            amount,
            self.wallet.mint_url.clone(),
            self.wallet.unit.clone(),
            OperationData::Send(SendOperationData {
                amount,
                memo: memo_text.clone(),
                counter_start: None,
                counter_end: None,
                token: None,
                proofs: None,
            }),
        );

        self.wallet.localstore.add_saga(saga.clone()).await?;

        add_compensation(
            &mut self.compensations,
            Box::new(RevertProofReservation {
                localstore: self.wallet.localstore.clone(),
                proof_ys,
                saga_id: self.state_data.operation_id,
            }),
        )
        .await;

        let mut exact_proofs = proofs.total_amount()? == amount + send_fee.total;
        if let Some(max_proofs) = opts.max_proofs {
            exact_proofs &= proofs.len() <= max_proofs;
        }

        let is_exact_or_offline =
            exact_proofs || opts.send_kind.is_offline() || opts.send_kind.has_tolerance();

        let keyset_fees_and_amounts = self.wallet.get_keyset_fees_and_amounts().await?;
        let keyset_fees: HashMap<Id, u64> = keyset_fees_and_amounts
            .iter()
            .map(|(key, values)| (*key, values.fee()))
            .collect();

        let split_result = split_proofs_for_send(
            proofs,
            &send_amounts,
            amount,
            send_fee.total,
            &keyset_fees,
            force_swap,
            is_exact_or_offline,
        )?;

        Ok(SendSaga {
            wallet: self.wallet,
            compensations: self.compensations,
            state_data: Prepared {
                operation_id: self.state_data.operation_id,
                amount,
                options: opts,
                proofs_to_swap: split_result.proofs_to_swap,
                swap_fee: split_result.swap_fee,
                proofs_to_send: split_result.proofs_to_send,
                send_fee: send_fee.total,
                saga,
            },
        })
    }
}

impl<'a> SendSaga<'a, Prepared> {
    /// Create a new send saga directly in the Prepared state.
    ///
    /// Used when reconstructing a saga from stored state for confirmation.
    #[allow(clippy::too_many_arguments)]
    pub fn from_prepared(
        wallet: &'a Wallet,
        operation_id: uuid::Uuid,
        amount: Amount,
        options: SendOptions,
        proofs_to_swap: Proofs,
        proofs_to_send: Proofs,
        swap_fee: Amount,
        send_fee: Amount,
        saga: WalletSaga,
    ) -> Self {
        Self {
            wallet,
            compensations: new_compensations(),
            state_data: Prepared {
                operation_id,
                amount,
                options,
                proofs_to_swap,
                proofs_to_send,
                swap_fee,
                send_fee,
                saga,
            },
        }
    }

    /// Get the operation ID
    pub fn operation_id(&self) -> uuid::Uuid {
        self.state_data.operation_id
    }

    /// Get the amount to be sent
    pub fn amount(&self) -> Amount {
        self.state_data.amount
    }

    /// Get the send options
    pub fn options(&self) -> &SendOptions {
        &self.state_data.options
    }

    /// Get the proofs that will be swapped
    pub fn proofs_to_swap(&self) -> &Proofs {
        self.state_data.proofs_to_swap.as_ref()
    }

    /// Get the swap fee
    pub fn swap_fee(&self) -> Amount {
        self.state_data.swap_fee
    }

    /// Get the proofs that will be sent directly
    pub fn proofs_to_send(&self) -> &Proofs {
        self.state_data.proofs_to_send.as_ref()
    }

    /// Get the send fee
    pub fn send_fee(&self) -> Amount {
        self.state_data.send_fee
    }

    /// Confirm the prepared send and create a token.
    ///
    /// Performs necessary swaps, marks proofs as pending spent, creates the
    /// token, and persists the saga in TokenCreated state.
    #[instrument(skip(self), err)]
    pub async fn confirm(
        mut self,
        memo: Option<SendMemo>,
    ) -> Result<(Token, SendSaga<'a, TokenCreated>), Error> {
        let operation_id = self.state_data.operation_id;
        let amount = self.state_data.amount;
        let options = self.state_data.options.clone();
        let proofs_to_swap = self.state_data.proofs_to_swap.clone();
        let proofs_to_send = self.state_data.proofs_to_send.clone();
        let swap_fee = self.state_data.swap_fee;
        let send_fee = self.state_data.send_fee;

        tracing::info!("Confirming prepared send for operation {}", operation_id);

        let logic_res = async {
            let total_send_fee = swap_fee + send_fee;
            let mut final_proofs_to_send = proofs_to_send.clone();

            let total_send_amount = amount + send_fee;

            let mut counter_start = None;
            let mut counter_end = None;

            if !proofs_to_swap.is_empty() {
                let swap_amount = total_send_amount
                    .checked_sub(final_proofs_to_send.total_amount()?)
                    .unwrap_or(Amount::ZERO);

                tracing::debug!("Swapping proofs; swap_amount={:?}", swap_amount);

                let keyset_id = self.wallet.fetch_active_keyset().await?.id;

                // Capture counter start before swap
                counter_start = Some(
                    self.wallet
                        .localstore
                        .increment_keyset_counter(&keyset_id, 0)
                        .await?,
                );

                if let Some(swapped_proofs) = self
                    .wallet
                    .swap(
                        Some(swap_amount),
                        SplitTarget::None,
                        proofs_to_swap,
                        options.conditions.clone(),
                        false,
                    )
                    .await?
                {
                    final_proofs_to_send.extend(swapped_proofs);
                }

                // Capture counter end after swap
                counter_end = Some(
                    self.wallet
                        .localstore
                        .increment_keyset_counter(&keyset_id, 0)
                        .await?,
                );
            }

            if amount > final_proofs_to_send.total_amount()? {
                return Err(Error::InsufficientFunds);
            }

            self.wallet
                .localstore
                .update_proofs_state(final_proofs_to_send.ys()?, State::PendingSpent)
                .await?;

            let send_memo = options.memo.clone().or(memo);
            let token_memo =
                send_memo.and_then(|m| if m.include_memo { Some(m.memo) } else { None });

            self.wallet
                .localstore
                .add_transaction(Transaction {
                    mint_url: self.wallet.mint_url.clone(),
                    direction: TransactionDirection::Outgoing,
                    amount,
                    fee: total_send_fee,
                    unit: self.wallet.unit.clone(),
                    ys: final_proofs_to_send.ys()?,
                    timestamp: unix_time(),
                    memo: token_memo.clone(),
                    metadata: options.metadata.clone(),
                    quote_id: None,
                    payment_request: None,
                    payment_proof: None,
                    payment_method: None,
                    saga_id: Some(operation_id),
                })
                .await?;

            let token = Token::new(
                self.wallet.mint_url.clone(),
                final_proofs_to_send.clone(),
                token_memo,
                self.wallet.unit.clone(),
            );

            let mut saga = self.state_data.saga.clone();
            saga.data = OperationData::Send(SendOperationData {
                amount,
                memo: options.memo.as_ref().map(|m| m.memo.clone()),
                counter_start,
                counter_end,
                token: Some(token.to_string()),
                proofs: Some(final_proofs_to_send.clone()),
            });
            saga.update_state(WalletSagaState::Send(SendSagaState::TokenCreated));

            if !self.wallet.localstore.update_saga(saga.clone()).await? {
                return Err(Error::ConcurrentUpdate);
            }

            Ok((token, final_proofs_to_send, saga))
        }
        .await;

        match logic_res {
            Ok((token, final_proofs_to_send, saga)) => {
                let send_saga = SendSaga {
                    wallet: self.wallet,
                    compensations: self.compensations,
                    state_data: TokenCreated {
                        operation_id,
                        proofs: final_proofs_to_send,
                        saga,
                    },
                };

                Ok((token, send_saga))
            }
            Err(e) => {
                if e.is_definitive_failure() {
                    tracing::warn!(
                        "Send saga confirmation failed (definitive): {}. Running compensations.",
                        e
                    );
                    execute_compensations(&mut self.compensations).await?;
                }
                Err(e)
            }
        }
    }

    /// Cancel the prepared send and release reserved proofs
    #[instrument(skip(self))]
    pub async fn cancel(self) -> Result<(), Error> {
        let operation_id = self.state_data.operation_id;
        tracing::info!("Cancelling prepared send for operation {}", operation_id);

        let mut all_ys = self.state_data.proofs_to_swap.ys()?;
        all_ys.extend(self.state_data.proofs_to_send.ys()?);

        self.wallet
            .localstore
            .update_proofs_state(all_ys, State::Unspent)
            .await?;

        if let Err(e) = self.wallet.localstore.delete_saga(&operation_id).await {
            tracing::warn!(
                "Failed to delete send saga {}: {}. Will be cleaned up on recovery.",
                operation_id,
                e
            );
        }

        Ok(())
    }
}

impl<'a> SendSaga<'a, TokenCreated> {
    /// Revoke the sent token if not yet claimed by recipient.
    ///
    /// Swaps proofs back to the wallet. On success, the saga is completed.
    pub async fn revoke(self) -> Result<Amount, Error> {
        tracing::info!("Revoking send operation {}", self.state_data.operation_id);

        // Check with mint if proofs are still unspent. Skip local check to force mint validation.
        let states = self
            .wallet
            .check_proofs_spent(self.state_data.proofs.clone())
            .await?;

        if states.iter().any(|s| s.state == State::Spent) {
            // Already spent by recipient
            tracing::info!("Cannot revoke: token already claimed by recipient");
            // We should finalize the saga as "Spent"
            self.finalize().await?;
            return Err(Error::Custom("Token already claimed".to_string()));
        }

        // Lock saga in RollingBack state to prevent proof watcher from treating swap as recipient claim
        let operation_id = self.state_data.operation_id;
        let mut saga = self.state_data.saga.clone();
        saga.update_state(WalletSagaState::Send(SendSagaState::RollingBack));
        if let OperationData::Send(ref mut data) = saga.data {
            data.proofs = Some(self.state_data.proofs.clone());
        }

        if !self.wallet.localstore.update_saga(saga).await? {
            return Err(Error::ConcurrentUpdate);
        }

        // Swap proofs back to wallet with fresh secrets
        let swap_result = self
            .wallet
            .swap(
                None, // Swap all
                SplitTarget::default(),
                self.state_data.proofs.clone(),
                None,
                false,
            )
            .await;

        match swap_result {
            Ok(swapped_proofs) => {
                let amount_recovered = match swapped_proofs {
                    Some(proofs) => proofs.total_amount()?,
                    None => {
                        // All proofs kept (refreshed). Recovered amount is input minus fees.
                        let input_amount = self.state_data.proofs.total_amount()?;
                        let fee = self
                            .wallet
                            .get_proofs_fee(&self.state_data.proofs)
                            .await?
                            .total;
                        input_amount.checked_sub(fee).unwrap_or(Amount::ZERO)
                    }
                };

                self.finalize().await?;

                Ok(amount_recovered)
            }
            Err(e) => {
                tracing::error!("Revoke swap failed: {}. Reverting lock.", e);

                // Revert state to TokenCreated and mark proofs as PendingSpent to resume monitoring.
                // Fetch fresh saga from DB since earlier update succeeded.
                let current_saga = self
                    .wallet
                    .localstore
                    .get_saga(&operation_id)
                    .await?
                    .ok_or(Error::Custom("Saga not found during revert".to_string()))?;

                let mut revert_saga = current_saga;
                revert_saga.update_state(WalletSagaState::Send(SendSagaState::TokenCreated));

                self.wallet.localstore.update_saga(revert_saga).await?;

                self.wallet
                    .localstore
                    .update_proofs_state(self.state_data.proofs.ys()?, State::PendingSpent)
                    .await?;

                Err(e)
            }
        }
    }

    /// Check the status of the sent token.
    ///
    /// Finalizes and removes the saga if the token has been claimed.
    /// Returns true if claimed, false if still pending.
    pub async fn check_status(self) -> Result<bool, Error> {
        let states = self
            .wallet
            .check_proofs_spent(self.state_data.proofs.clone())
            .await?;

        let all_spent = states.iter().all(|s| s.state == State::Spent);

        if all_spent {
            tracing::info!(
                "Token for operation {} has been claimed",
                self.state_data.operation_id
            );
            self.finalize().await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Finalize the saga (delete from DB)
    async fn finalize(self) -> Result<(), Error> {
        if let Err(e) = self
            .wallet
            .localstore
            .delete_saga(&self.state_data.operation_id)
            .await
        {
            tracing::warn!(
                "Failed to delete completed send saga {}: {}",
                self.state_data.operation_id,
                e
            );
        }
        Ok(())
    }
}

impl std::fmt::Debug for SendSaga<'_, Prepared> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SendSaga<Prepared>")
            .field("operation_id", &self.state_data.operation_id)
            .field("amount", &self.state_data.amount)
            .field("options", &self.state_data.options)
            .field(
                "proofs_to_swap",
                &self
                    .state_data
                    .proofs_to_swap
                    .iter()
                    .map(|p| p.amount)
                    .collect::<Vec<_>>(),
            )
            .field("swap_fee", &self.state_data.swap_fee)
            .field(
                "proofs_to_send",
                &self
                    .state_data
                    .proofs_to_send
                    .iter()
                    .map(|p| p.amount)
                    .collect::<Vec<_>>(),
            )
            .field("send_fee", &self.state_data.send_fee)
            .finish()
    }
}
