//! Swap Saga - Type State Pattern Implementation
//!
//! This module implements the saga pattern for swap operations using the typestate
//! pattern to enforce valid state transitions at compile-time.
//!
//! # State Flow
//!
//! ```text
//! [saga created] ──► ProofsReserved ──► SwapRequested ──► [completed]
//!                         │                   │
//!                         │                   ├─ replay succeeds ───► [completed]
//!                         │                   ├─ proofs spent ──────► [completed] (via /restore)
//!                         │                   ├─ proofs not spent ──► [compensated]
//!                         │                   └─ mint unreachable ──► [skipped]
//!                         │
//!                         └─ recovery ─────────────────────────────► [compensated]
//! ```
//!
//! # States
//!
//! | State | Description |
//! |-------|-------------|
//! | `ProofsReserved` | Input proofs reserved, swap request prepared, ready to execute |
//! | `SwapRequested` | Swap request sent to mint, awaiting signatures for new proofs |
//!
//! # Recovery Outcomes
//!
//! | Outcome | Description |
//! |---------|-------------|
//! | `[completed]` | Swap succeeded, new proofs saved to wallet |
//! | `[compensated]` | Swap rolled back, input proofs released back to wallet |
//! | `[skipped]` | Recovery deferred (mint unreachable), will retry on next recovery |

use cdk_common::wallet::{
    OperationData, ProofInfo, SwapOperationData, SwapSagaState, WalletSaga, WalletSagaState,
};
use tracing::instrument;

use self::state::{Finalized, Initial, Prepared};
use crate::amount::SplitTarget;
use crate::dhke::construct_proofs;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{nut10, Proofs, SpendingConditions, State};
use crate::wallet::saga::{
    add_compensation, clear_compensations, execute_compensations, new_compensations, Compensations,
    RevertProofReservation as RevertSwapProofReservation,
};
use crate::{Amount, Error, Wallet};

pub(crate) mod resume;
pub(crate) mod state;

/// Swap saga using typestate pattern for compile-time state transition safety.
pub(crate) struct SwapSaga<'a, S> {
    /// Wallet reference
    wallet: &'a Wallet,
    /// Compensating actions in LIFO order (most recent first)
    compensations: Compensations,
    /// State-specific data
    state_data: S,
}

impl<'a> SwapSaga<'a, Initial> {
    /// Create a new swap saga in the Initial state.
    pub fn new(wallet: &'a Wallet) -> Self {
        let operation_id = uuid::Uuid::new_v4();

        Self {
            wallet,
            compensations: new_compensations(),
            state_data: Initial { operation_id },
        }
    }

    /// Prepare the swap operation.
    ///
    /// Gets the active keyset, calculates fees, creates the swap request
    /// (reserving proofs and incrementing counter), and persists saga state
    /// for crash recovery.
    ///
    /// # Compensation
    ///
    /// On failure, reverts proof reservation and deletes the saga.
    #[instrument(skip_all)]
    pub async fn prepare(
        mut self,
        amount: Option<Amount>,
        amount_split_target: SplitTarget,
        input_proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
        include_fees: bool,
    ) -> Result<SwapSaga<'a, Prepared>, Error> {
        tracing::info!(
            "Preparing swap with operation {}",
            self.state_data.operation_id
        );

        let active_keyset_id = self.wallet.fetch_active_keyset().await?.id;
        let fee_and_amounts = self
            .wallet
            .get_keyset_fees_and_amounts_by_id(active_keyset_id)
            .await?;

        let fee_breakdown = self.wallet.get_proofs_fee(&input_proofs).await?;

        let input_ys = input_proofs.ys()?;

        let pre_swap = self
            .wallet
            .create_swap(
                active_keyset_id,
                &fee_and_amounts,
                amount,
                amount_split_target.clone(),
                input_proofs.clone(),
                spending_conditions.clone(),
                include_fees,
                &fee_breakdown,
            )
            .await?;

        let fee = pre_swap.fee;
        let input_amount = input_proofs.total_amount()?;

        let counter_end = self
            .wallet
            .localstore
            .increment_keyset_counter(&active_keyset_id, 0)
            .await?;
        let counter_start = counter_end.saturating_sub(pre_swap.derived_secret_count);
        let output_amount = input_amount
            .checked_sub(fee)
            .ok_or(Error::InsufficientFunds)?;

        let saga = WalletSaga::new(
            self.state_data.operation_id,
            WalletSagaState::Swap(SwapSagaState::ProofsReserved),
            input_amount,
            self.wallet.mint_url.clone(),
            self.wallet.unit.clone(),
            OperationData::Swap(SwapOperationData {
                input_amount,
                output_amount,
                counter_start: Some(counter_start),
                counter_end: Some(counter_end),
                blinded_messages: None,
            }),
        );

        self.wallet.localstore.add_saga(saga.clone()).await?;

        add_compensation(
            &mut self.compensations,
            Box::new(RevertSwapProofReservation {
                localstore: self.wallet.localstore.clone(),
                proof_ys: input_ys.clone(),
                saga_id: self.state_data.operation_id,
            }),
        )
        .await;

        Ok(SwapSaga {
            wallet: self.wallet,
            compensations: self.compensations,
            state_data: Prepared {
                operation_id: self.state_data.operation_id,
                amount,
                amount_split_target,
                input_ys,
                spending_conditions,
                pre_swap,
                saga,
            },
        })
    }
}

impl<'a> SwapSaga<'a, Prepared> {
    /// Execute the swap operation.
    ///
    /// Updates saga state for recovery, posts swap to mint, constructs new
    /// proofs from response, updates database, and deletes saga record.
    #[instrument(skip_all)]
    pub async fn execute(mut self) -> Result<SwapSaga<'a, Finalized>, Error> {
        tracing::info!(
            "Executing swap for operation {}",
            self.state_data.operation_id
        );

        let mint_url = &self.wallet.mint_url;
        let unit = &self.wallet.unit;
        let operation_id = self.state_data.operation_id;

        let mut saga = self.state_data.saga.clone();
        saga.update_state(WalletSagaState::Swap(SwapSagaState::SwapRequested));
        if let OperationData::Swap(ref mut data) = saga.data {
            data.blinded_messages = Some(self.state_data.pre_swap.swap_request.outputs().clone());
        }

        if !self.wallet.localstore.update_saga(saga).await? {
            return Err(Error::ConcurrentUpdate);
        }

        let swap_response = match self
            .wallet
            .client
            .post_swap(self.state_data.pre_swap.swap_request.clone())
            .await
        {
            Ok(response) => response,
            Err(err) => {
                if err.is_definitive_failure() {
                    tracing::error!("Failed to post swap request (definitive): {}", err);
                    execute_compensations(&mut self.compensations).await?;
                } else {
                    tracing::warn!("Failed to post swap request (ambiguous): {}.", err,);
                }
                return Err(err);
            }
        };

        let active_keyset_id = self.state_data.pre_swap.pre_mint_secrets.keyset_id;
        let active_keys = self.wallet.load_keyset_keys(active_keyset_id).await?;

        let post_swap_proofs = construct_proofs(
            swap_response.signatures,
            self.state_data.pre_swap.pre_mint_secrets.rs(),
            self.state_data.pre_swap.pre_mint_secrets.secrets(),
            &active_keys,
        )?;

        let mut added_proofs = Vec::new();
        let change_proofs;
        let send_proofs;

        let fee_and_amounts = self
            .wallet
            .get_keyset_fees_and_amounts_by_id(active_keyset_id)
            .await?;

        match self.state_data.amount {
            Some(amount) => {
                let (proofs_with_condition, proofs_without_condition): (Proofs, Proofs) =
                    post_swap_proofs.into_iter().partition(|p| {
                        let nut10_secret: Result<nut10::Secret, _> = p.secret.clone().try_into();
                        nut10_secret.is_ok()
                    });

                let (proofs_to_send, proofs_to_keep) = match &self.state_data.spending_conditions {
                    Some(_) => (proofs_with_condition, proofs_without_condition),
                    None => {
                        let mut all_proofs = proofs_without_condition;
                        all_proofs.reverse();

                        let mut proofs_to_send = Proofs::new();
                        let mut proofs_to_keep = Proofs::new();
                        let mut amount_split = amount.split_targeted(
                            &self.state_data.amount_split_target,
                            &fee_and_amounts,
                        )?;

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

        self.wallet
            .localstore
            .update_proofs(added_proofs, self.state_data.input_ys.clone())
            .await?;

        clear_compensations(&mut self.compensations).await;

        if let Err(e) = self.wallet.localstore.delete_saga(&operation_id).await {
            tracing::warn!(
                "Failed to delete swap saga {}: {}. Will be cleaned up on recovery.",
                operation_id,
                e
            );
        }

        Ok(SwapSaga {
            wallet: self.wallet,
            compensations: self.compensations,
            state_data: Finalized { send_proofs },
        })
    }
}

impl<'a> SwapSaga<'a, Finalized> {
    /// Consume the saga and return the send proofs
    pub fn into_send_proofs(self) -> Option<Proofs> {
        self.state_data.send_proofs
    }
}

impl<S: std::fmt::Debug> std::fmt::Debug for SwapSaga<'_, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SwapSaga")
            .field("state_data", &self.state_data)
            .finish_non_exhaustive()
    }
}
