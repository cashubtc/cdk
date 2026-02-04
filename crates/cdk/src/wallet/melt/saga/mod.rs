//! Melt Saga - Type State Pattern Implementation
//!
//! This module implements the saga pattern for melt operations using the typestate
//! pattern to enforce valid state transitions at compile-time.
//!
//! # State Flow
//!
//! ```text
//! [saga created] ──► ProofsReserved ──► MeltRequested ──► PaymentPending ──► [completed]
//!                         │                   │                 │
//!                         │                   └─────────────────┤
//!                         │                                     ├─ quote Paid ─────────► [completed]
//!                         │                                     ├─ quote Unpaid/Failed ► [compensated]
//!                         │                                     └─ quote Pending ──────► [skipped]
//!                         │
//!                         └─ recovery ────────────────────────────────────────────────► [compensated]
//! ```
//!
//! # States
//!
//! | State | Description |
//! |-------|-------------|
//! | `ProofsReserved` | Proofs reserved and quote locked, ready to initiate payment |
//! | `MeltRequested` | Melt request sent to mint, Lightning payment initiated |
//! | `PaymentPending` | Lightning payment in progress, awaiting confirmation from network |
//!
//! # Recovery Outcomes
//!
//! | Outcome | Description |
//! |---------|-------------|
//! | `[completed]` | Payment succeeded, proofs spent, change (if any) claimed |
//! | `[compensated]` | Payment failed/cancelled, proofs and quote released |
//! | `[skipped]` | Payment still pending, will retry on next recovery |

use std::collections::HashMap;

use cdk_common::amount::SplitTarget;
use cdk_common::dhke::construct_proofs;
use cdk_common::wallet::{
    MeltOperationData, MeltQuote, MeltSagaState, OperationData, ProofInfo, Transaction,
    TransactionDirection, Wallet as WalletTrait, WalletSaga, WalletSagaState,
};
use cdk_common::MeltQuoteState;
use tracing::instrument;
use uuid::Uuid;

use self::compensation::{ReleaseMeltQuote, RevertProofReservation};
use self::state::{Finalized, Initial, MeltRequested, PaymentPending, Prepared};
use super::MeltConfirmOptions;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::{MeltRequest, PreMintSecrets, Proofs, State};
use crate::util::unix_time;
use crate::wallet::mint_connector::MeltOptions;
use crate::wallet::saga::{add_compensation, new_compensations, Compensations};
use crate::{ensure_cdk, Amount, Error, Wallet};

pub(crate) mod compensation;
pub(crate) mod resume;
pub(crate) mod state;

/// Result of an async melt execution
pub enum MeltSagaResult<'a> {
    /// Melt finalized (paid)
    Finalized(MeltSaga<'a, Finalized>),
    /// Melt pending
    Pending(MeltSaga<'a, PaymentPending>),
}

/// Saga pattern implementation for melt operations.
///
/// Uses the typestate pattern to enforce valid state transitions at compile-time.
/// Each state (Initial, Prepared, Confirmed) is a distinct type, and operations
/// are only available on the appropriate type.
pub(crate) struct MeltSaga<'a, S> {
    /// Wallet reference
    pub(crate) wallet: &'a Wallet,
    /// Compensating actions in LIFO order (most recent first)
    pub(crate) compensations: Compensations,
    /// State-specific data
    pub(crate) state_data: S,
}

/// Shared helper function to perform the actual melt finalization.
/// Used by `execute_async` and `PaymentPending::finalize`.
#[allow(clippy::too_many_arguments)]
async fn finalize_melt_common<'a>(
    wallet: &'a Wallet,
    compensations: Compensations,
    operation_id: Uuid,
    quote_info: &MeltQuote,
    final_proofs: &Proofs,
    premint_secrets: &PreMintSecrets,
    state: MeltQuoteState,
    payment_preimage: Option<String>,
    change: Option<Vec<crate::nuts::BlindSignature>>,
    metadata: HashMap<String, String>,
) -> Result<MeltSaga<'a, Finalized>, Error> {
    let active_keyset_id = wallet.fetch_active_keyset().await?.id;
    let active_keys = wallet.load_keyset_keys(active_keyset_id).await?;

    let change_proofs = match change {
        Some(change) => {
            let num_change_proof = change.len();

            let num_change_proof = match (
                premint_secrets.len() < num_change_proof,
                premint_secrets.secrets().len() < num_change_proof,
            ) {
                (true, _) | (_, true) => {
                    tracing::error!("Mismatch in change promises to change");
                    premint_secrets.len()
                }
                _ => num_change_proof,
            };

            Some(construct_proofs(
                change,
                premint_secrets.rs()[..num_change_proof].to_vec(),
                premint_secrets.secrets()[..num_change_proof].to_vec(),
                &active_keys,
            )?)
        }
        None => None,
    };

    let proofs_total = final_proofs.total_amount()?;
    let change_total = change_proofs
        .as_ref()
        .map(|p| p.total_amount())
        .transpose()?
        .unwrap_or(Amount::ZERO);
    let fee = proofs_total - quote_info.amount - change_total;

    let mut updated_quote = quote_info.clone();
    updated_quote.state = state;
    wallet.localstore.add_melt_quote(updated_quote).await?;

    let change_proof_infos = match change_proofs.clone() {
        Some(change_proofs) => change_proofs
            .into_iter()
            .map(|proof| {
                ProofInfo::new(
                    proof,
                    wallet.mint_url.clone(),
                    State::Unspent,
                    quote_info.unit.clone(),
                )
            })
            .collect::<Result<Vec<ProofInfo>, _>>()?,
        None => Vec::new(),
    };

    // Add new (change) proofs to the database
    wallet
        .localstore
        .update_proofs(change_proof_infos, vec![])
        .await?;

    // Mark input proofs as Spent instead of deleting them
    let spent_ys = final_proofs.ys()?;
    wallet
        .localstore
        .update_proofs_state(spent_ys, State::Spent)
        .await?;

    wallet
        .localstore
        .add_transaction(Transaction {
            mint_url: wallet.mint_url.clone(),
            direction: TransactionDirection::Outgoing,
            amount: quote_info.amount,
            fee,
            unit: wallet.unit.clone(),
            ys: final_proofs.ys()?,
            timestamp: unix_time(),
            memo: None,
            metadata,
            quote_id: Some(quote_info.id.clone()),
            payment_request: Some(quote_info.request.clone()),
            payment_proof: payment_preimage.clone(),
            payment_method: Some(quote_info.payment_method.clone()),
            saga_id: Some(operation_id),
        })
        .await?;

    if let Err(e) = wallet.localstore.release_melt_quote(&operation_id).await {
        tracing::warn!(
            "Failed to release melt quote for operation {}: {}",
            operation_id,
            e
        );
    }

    if let Err(e) = wallet.localstore.delete_saga(&operation_id).await {
        tracing::warn!(
            "Failed to delete melt saga {}: {}. Will be cleaned up on recovery.",
            operation_id,
            e
        );
    }

    Ok(MeltSaga {
        wallet,
        compensations,
        state_data: Finalized {
            quote_id: quote_info.id.clone(),
            state,
            amount: quote_info.amount,
            fee,
            payment_proof: payment_preimage,
            change: change_proofs,
        },
    })
}

impl<'a> MeltSaga<'a, Initial> {
    /// Create a new melt saga in the Initial state.
    pub fn new(wallet: &'a Wallet) -> Self {
        let operation_id = uuid::Uuid::new_v4();

        Self {
            wallet,
            compensations: new_compensations(),
            state_data: Initial { operation_id },
        }
    }

    /// Initialize melt operation (common steps for prepare methods)
    async fn initialize_melt(&mut self, quote_id: &str) -> Result<MeltQuote, Error> {
        let quote_info = self
            .wallet
            .localstore
            .get_melt_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        ensure_cdk!(
            quote_info.expiry.gt(&unix_time()),
            Error::ExpiredQuote(quote_info.expiry, unix_time())
        );

        // Reserve the quote to prevent concurrent operations from using it
        self.wallet
            .localstore
            .reserve_melt_quote(quote_id, &self.state_data.operation_id)
            .await?;

        // Register compensation to release quote on failure
        add_compensation(
            &mut self.compensations,
            Box::new(ReleaseMeltQuote {
                localstore: self.wallet.localstore.clone(),
                operation_id: self.state_data.operation_id,
            }),
        )
        .await;

        Ok(quote_info)
    }

    /// Prepare the melt operation by selecting and reserving proofs.
    ///
    /// Loads the quote, selects and reserves proofs for the required amount.
    ///
    /// # Compensation
    ///
    /// Registers a compensation action that will revert proof reservation
    /// if later steps fail.
    #[instrument(skip_all)]
    pub async fn prepare(
        mut self,
        quote_id: &str,
        _metadata: HashMap<String, String>,
    ) -> Result<MeltSaga<'a, Prepared>, Error> {
        tracing::info!(
            "Preparing melt for quote {} with operation {}",
            quote_id,
            self.state_data.operation_id
        );

        let quote_info = self.initialize_melt(quote_id).await?;

        let inputs_needed_amount = quote_info.amount + quote_info.fee_reserve;

        let active_keyset_ids = self
            .wallet
            .get_mint_keysets()
            .await?
            .into_iter()
            .map(|k| k.id)
            .collect();
        let keyset_fees_and_amounts = self.wallet.get_keyset_fees_and_amounts().await?;

        let available_proofs = self.wallet.get_unspent_proofs().await?;

        let exact_input_proofs = Wallet::select_proofs(
            inputs_needed_amount,
            available_proofs.clone(),
            &active_keyset_ids,
            &keyset_fees_and_amounts,
            true,
        )?;
        let proofs_total = exact_input_proofs.total_amount()?;

        if proofs_total == inputs_needed_amount {
            let proof_ys = exact_input_proofs.ys()?;
            let operation_id = self.state_data.operation_id;

            self.wallet
                .localstore
                .update_proofs_state(proof_ys.clone(), State::Reserved)
                .await?;

            let saga = WalletSaga::new(
                operation_id,
                WalletSagaState::Melt(MeltSagaState::ProofsReserved),
                quote_info.amount,
                self.wallet.mint_url.clone(),
                self.wallet.unit.clone(),
                OperationData::Melt(MeltOperationData {
                    quote_id: quote_id.to_string(),
                    amount: quote_info.amount,
                    fee_reserve: quote_info.fee_reserve,
                    counter_start: None,
                    counter_end: None,
                    change_amount: None,
                    change_blinded_messages: None,
                }),
            );

            self.wallet.localstore.add_saga(saga.clone()).await?;

            add_compensation(
                &mut self.compensations,
                Box::new(RevertProofReservation {
                    localstore: self.wallet.localstore.clone(),
                    proof_ys,
                    saga_id: operation_id,
                }),
            )
            .await;

            let input_fee = self.wallet.get_proofs_fee(&exact_input_proofs).await?.total;

            return Ok(MeltSaga {
                wallet: self.wallet,
                compensations: self.compensations,
                state_data: Prepared {
                    operation_id: self.state_data.operation_id,
                    quote: quote_info,
                    proofs: exact_input_proofs,
                    proofs_to_swap: Proofs::new(),
                    swap_fee: Amount::ZERO,
                    input_fee,
                    input_fee_without_swap: input_fee,
                    saga,
                },
            });
        }

        let active_keyset_id = self.wallet.get_active_keyset().await?.id;
        let fee_and_amounts = self
            .wallet
            .get_keyset_fees_and_amounts_by_id(active_keyset_id)
            .await?;

        let estimated_output_count = inputs_needed_amount.split(&fee_and_amounts)?.len();
        let estimated_melt_fee = self
            .wallet
            .get_keyset_count_fee(&active_keyset_id, estimated_output_count as u64)
            .await?;

        let selection_amount = inputs_needed_amount + estimated_melt_fee;

        let input_proofs = Wallet::select_proofs(
            selection_amount,
            available_proofs,
            &active_keyset_ids,
            &keyset_fees_and_amounts,
            true,
        )?;

        let input_fee = estimated_melt_fee;

        let proofs_to_send = Proofs::new();
        let proofs_to_swap = input_proofs;
        let swap_fee = self.wallet.get_proofs_fee(&proofs_to_swap).await?.total;

        let proof_ys = proofs_to_swap.ys()?;
        let operation_id = self.state_data.operation_id;

        if !proof_ys.is_empty() {
            self.wallet
                .localstore
                .update_proofs_state(proof_ys.clone(), State::Reserved)
                .await?;
        }

        let saga = WalletSaga::new(
            operation_id,
            WalletSagaState::Melt(MeltSagaState::ProofsReserved),
            quote_info.amount,
            self.wallet.mint_url.clone(),
            self.wallet.unit.clone(),
            OperationData::Melt(MeltOperationData {
                quote_id: quote_id.to_string(),
                amount: quote_info.amount,
                fee_reserve: quote_info.fee_reserve,
                counter_start: None,
                counter_end: None,
                change_amount: None,
                change_blinded_messages: None, // Will be set when melt is requested
            }),
        );

        self.wallet.localstore.add_saga(saga.clone()).await?;

        add_compensation(
            &mut self.compensations,
            Box::new(RevertProofReservation {
                localstore: self.wallet.localstore.clone(),
                proof_ys,
                saga_id: operation_id,
            }),
        )
        .await;

        let input_fee_without_swap = swap_fee;

        Ok(MeltSaga {
            wallet: self.wallet,
            compensations: self.compensations,
            state_data: Prepared {
                operation_id: self.state_data.operation_id,
                quote: quote_info,
                proofs: proofs_to_send,
                proofs_to_swap,
                swap_fee,
                input_fee,
                input_fee_without_swap,
                saga,
            },
        })
    }

    /// Prepare the melt operation with specific proofs (no automatic selection).
    ///
    /// Uses the provided proofs directly without automatic proof selection.
    /// The caller must ensure the proofs cover the quote amount plus fee reserve.
    ///
    /// # Compensation
    ///
    /// Registers a compensation action that will revert proof state
    /// if later steps fail.
    #[instrument(skip_all)]
    pub async fn prepare_with_proofs(
        mut self,
        quote_id: &str,
        proofs: Proofs,
        _metadata: HashMap<String, String>,
    ) -> Result<MeltSaga<'a, Prepared>, Error> {
        tracing::info!(
            "Preparing melt with specific proofs for quote {} with operation {}",
            quote_id,
            self.state_data.operation_id
        );

        let quote_info = self.initialize_melt(quote_id).await?;

        let proofs_total = proofs.total_amount()?;
        let inputs_needed = quote_info.amount + quote_info.fee_reserve;
        if proofs_total < inputs_needed {
            return Err(Error::InsufficientFunds);
        }

        let operation_id = self.state_data.operation_id;
        let proof_ys = proofs.ys()?;

        // Since proofs may be external (not in our database), add them first
        // Set to Reserved state like the regular prepare() does
        let proofs_info = proofs
            .clone()
            .into_iter()
            .map(|p| {
                ProofInfo::new(
                    p,
                    self.wallet.mint_url.clone(),
                    State::Reserved,
                    self.wallet.unit.clone(),
                )
            })
            .collect::<Result<Vec<ProofInfo>, _>>()?;

        self.wallet
            .localstore
            .update_proofs(proofs_info, vec![])
            .await?;

        let saga = WalletSaga::new(
            operation_id,
            WalletSagaState::Melt(MeltSagaState::ProofsReserved),
            quote_info.amount,
            self.wallet.mint_url.clone(),
            self.wallet.unit.clone(),
            OperationData::Melt(MeltOperationData {
                quote_id: quote_id.to_string(),
                amount: quote_info.amount,
                fee_reserve: quote_info.fee_reserve,
                counter_start: None,
                counter_end: None,
                change_amount: None,
                change_blinded_messages: None,
            }),
        );

        self.wallet.localstore.add_saga(saga.clone()).await?;

        add_compensation(
            &mut self.compensations,
            Box::new(RevertProofReservation {
                localstore: self.wallet.localstore.clone(),
                proof_ys,
                saga_id: operation_id,
            }),
        )
        .await;

        let input_fee = self.wallet.get_proofs_fee(&proofs).await?.total;

        Ok(MeltSaga {
            wallet: self.wallet,
            compensations: self.compensations,
            state_data: Prepared {
                operation_id: self.state_data.operation_id,
                quote: quote_info,
                proofs,
                proofs_to_swap: Proofs::new(),
                swap_fee: Amount::ZERO,
                input_fee,
                input_fee_without_swap: input_fee,
                saga,
            },
        })
    }
}

impl<'a> MeltSaga<'a, Prepared> {
    /// Create a new melt saga directly in the Prepared state.
    ///
    /// This constructor is used by `confirm_prepared_melt` to reconstruct
    /// a saga from stored state when confirming an already-prepared melt.
    ///
    /// Note: This bypasses the normal `prepare()` flow and assumes the caller
    /// has already properly reserved the proofs.
    #[allow(clippy::too_many_arguments)]
    pub fn from_prepared(
        wallet: &'a Wallet,
        operation_id: uuid::Uuid,
        quote: MeltQuote,
        proofs: Proofs,
        proofs_to_swap: Proofs,
        input_fee: Amount,
        input_fee_without_swap: Amount,
        saga: WalletSaga,
    ) -> Self {
        Self {
            wallet,
            compensations: new_compensations(),
            state_data: Prepared {
                operation_id,
                quote,
                proofs,
                proofs_to_swap,
                swap_fee: Amount::ZERO,
                input_fee,
                input_fee_without_swap,
                saga,
            },
        }
    }

    /// Get the operation ID
    pub fn operation_id(&self) -> uuid::Uuid {
        self.state_data.operation_id
    }

    /// Get the quote
    pub fn quote(&self) -> &MeltQuote {
        &self.state_data.quote
    }

    /// Get the proofs that will be used
    pub fn proofs(&self) -> &Proofs {
        &self.state_data.proofs
    }

    /// Get the proofs that need to be swapped
    pub fn proofs_to_swap(&self) -> &Proofs {
        &self.state_data.proofs_to_swap
    }

    /// Get the swap fee
    pub fn swap_fee(&self) -> Amount {
        self.state_data.swap_fee
    }

    /// Get the input fee
    pub fn input_fee(&self) -> Amount {
        self.state_data.input_fee
    }

    /// Get the input fee if swap is skipped
    pub fn input_fee_without_swap(&self) -> Amount {
        self.state_data.input_fee_without_swap
    }

    /// Build the melt request with options and transition to MeltRequested state.
    ///
    /// Performs swap if needed, sets proofs to Pending, creates pre-mint secrets for change.
    ///
    /// # Options
    ///
    /// - `skip_swap`: If true, skips the pre-melt swap and sends proofs directly.
    ///
    /// # Compensation
    ///
    /// On failure, compensations revert proof states and release the quote.
    #[instrument(skip_all)]
    pub async fn request_melt_with_options(
        mut self,
        options: MeltConfirmOptions,
    ) -> Result<MeltSaga<'a, MeltRequested>, Error> {
        let operation_id = self.state_data.operation_id;
        let quote_info = self.state_data.quote.clone();
        let input_fee = self.state_data.input_fee;

        tracing::info!(
            "Building melt request for quote {} with operation {} (skip_swap: {})",
            quote_info.id,
            operation_id,
            options.skip_swap
        );

        let active_keyset_id = self.wallet.fetch_active_keyset().await?.id;
        let mut final_proofs = self.state_data.proofs.clone();

        // Handle proofs_to_swap based on skip_swap option
        if !self.state_data.proofs_to_swap.is_empty() {
            if options.skip_swap {
                // Skip swap: use proofs_to_swap directly
                // The mint will return change from the melt
                tracing::debug!(
                    "Skipping swap, using {} proofs directly (total: {})",
                    self.state_data.proofs_to_swap.len(),
                    self.state_data.proofs_to_swap.total_amount()?,
                );
                final_proofs.extend(self.state_data.proofs_to_swap.clone());
            } else {
                // Current behavior: swap first to get optimal denominations
                let target_swap_amount = quote_info.amount + quote_info.fee_reserve + input_fee;

                tracing::debug!(
                    "Swapping {} proofs (total: {}) for target amount {}",
                    self.state_data.proofs_to_swap.len(),
                    self.state_data.proofs_to_swap.total_amount()?,
                    target_swap_amount
                );

                if let Some(swapped) = self
                    .wallet
                    .swap(
                        Some(target_swap_amount),
                        SplitTarget::None,
                        self.state_data.proofs_to_swap.clone(),
                        None,
                        false,
                    )
                    .await?
                {
                    final_proofs.extend(swapped);
                }
            }
        }

        // Recalculate the actual input_fee based on final_proofs
        let actual_input_fee = self.wallet.get_proofs_fee(&final_proofs).await?.total;
        let inputs_needed_amount = quote_info.amount + quote_info.fee_reserve + actual_input_fee;

        let proofs_total = final_proofs.total_amount()?;
        if proofs_total < inputs_needed_amount {
            // Insufficient funds - execute compensations
            self.compensate().await;
            return Err(Error::InsufficientFunds);
        }

        // Set proofs to Pending state before making melt request
        let proofs_info = final_proofs
            .clone()
            .into_iter()
            .map(|p| {
                ProofInfo::new_with_operations(
                    p,
                    self.wallet.mint_url.clone(),
                    State::Pending,
                    self.wallet.unit.clone(),
                    Some(operation_id),
                    None,
                )
            })
            .collect::<Result<Vec<ProofInfo>, _>>()?;

        self.wallet
            .localstore
            .update_proofs(proofs_info, vec![])
            .await?;

        // Add compensation to revert the new proofs if the saga fails later
        add_compensation(
            &mut self.compensations,
            Box::new(RevertProofReservation {
                localstore: self.wallet.localstore.clone(),
                proof_ys: final_proofs.ys()?,
                saga_id: operation_id,
            }),
        )
        .await;

        // Calculate change accounting for input fees
        let change_amount = proofs_total - quote_info.amount - actual_input_fee;

        let premint_secrets = if change_amount <= Amount::ZERO {
            PreMintSecrets::new(active_keyset_id)
        } else {
            let num_secrets =
                ((u64::from(change_amount) as f64).log2().ceil() as u64).max(1) as u32;

            let new_counter = self
                .wallet
                .localstore
                .increment_keyset_counter(&active_keyset_id, num_secrets)
                .await?;

            let count = new_counter - num_secrets;

            PreMintSecrets::from_seed_blank(
                active_keyset_id,
                count,
                &self.wallet.seed,
                change_amount,
            )?
        };

        // Get counter range for recovery
        let counter_end = self
            .wallet
            .localstore
            .increment_keyset_counter(&active_keyset_id, 0)
            .await?;
        let counter_start = counter_end.saturating_sub(premint_secrets.secrets.len() as u32);

        let change_blinded_messages = if change_amount > Amount::ZERO {
            Some(premint_secrets.blinded_messages())
        } else {
            None
        };

        // Update saga state to MeltRequested BEFORE making the melt call
        let mut saga = self.state_data.saga.clone();
        saga.update_state(WalletSagaState::Melt(MeltSagaState::MeltRequested));
        if let OperationData::Melt(ref mut data) = saga.data {
            data.counter_start = Some(counter_start);
            data.counter_end = Some(counter_end);
            data.change_amount = if change_amount > Amount::ZERO {
                Some(change_amount)
            } else {
                None
            };
            data.change_blinded_messages = change_blinded_messages.clone();
        }

        if !self.wallet.localstore.update_saga(saga.clone()).await? {
            return Err(Error::ConcurrentUpdate);
        }

        Ok(MeltSaga {
            wallet: self.wallet,
            compensations: self.compensations,
            state_data: MeltRequested {
                operation_id,
                quote: quote_info,
                final_proofs,
                premint_secrets,
            },
        })
    }

    /// Execute compensations and cancel the melt.
    async fn compensate(self) {
        // Move compensations out of self to iterate while owning self
        let mut compensations = self.compensations;
        while let Some(action) = compensations.pop_front() {
            if let Err(e) = action.execute().await {
                tracing::warn!("Compensation {} failed: {}", action.name(), e);
            }
        }
    }

    /// Cancel the prepared melt and release reserved proofs.
    pub async fn cancel(self) -> Result<(), Error> {
        self.compensate().await;
        Ok(())
    }
}

impl std::fmt::Debug for MeltSaga<'_, Prepared> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeltSaga<Prepared>")
            .field("operation_id", &self.state_data.operation_id)
            .field("quote_id", &self.state_data.quote.id)
            .field("amount", &self.state_data.quote.amount)
            .field(
                "proofs",
                &self
                    .state_data
                    .proofs
                    .iter()
                    .map(|p| p.amount)
                    .collect::<Vec<_>>(),
            )
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
            .field("input_fee", &self.state_data.input_fee)
            .finish()
    }
}

impl std::fmt::Debug for MeltSaga<'_, PaymentPending> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeltSaga<PaymentPending>")
            .field("operation_id", &self.state_data.operation_id)
            .field("quote_id", &self.state_data.quote.id)
            .field("amount", &self.state_data.quote.amount)
            .finish()
    }
}

impl<'a> MeltSaga<'a, MeltRequested> {
    /// Execute the melt request with async support.
    #[instrument(skip_all)]
    pub async fn execute_async(
        self,
        metadata: HashMap<String, String>,
    ) -> Result<MeltSagaResult<'a>, Error> {
        let operation_id = self.state_data.operation_id;
        let quote_info = &self.state_data.quote;

        tracing::info!(
            "Executing async melt request for quote {} with operation {}",
            quote_info.id,
            operation_id
        );

        let request = MeltRequest::new(
            quote_info.id.clone(),
            self.state_data.final_proofs.clone(),
            Some(self.state_data.premint_secrets.blinded_messages()),
        );

        let melt_result = self
            .wallet
            .client
            .post_melt_with_options(
                &quote_info.payment_method,
                request,
                MeltOptions { async_melt: true },
            )
            .await;

        let melt_response = match melt_result {
            Ok(response) => response,
            Err(error) => {
                // Check for known terminal errors first
                if matches!(error, Error::RequestAlreadyPaid) {
                    tracing::info!("Invoice already paid by another wallet - releasing proofs");
                    self.handle_failure().await;
                    return Err(error);
                }

                // On HTTP error, check quote status to determine if payment failed
                tracing::warn!(
                    "Melt request failed with error: {}. Checking quote status...",
                    error
                );

                match self.wallet.internal_check_melt_status(&quote_info.id).await {
                    Ok(response) => match response.state() {
                        MeltQuoteState::Failed
                        | MeltQuoteState::Unknown
                        | MeltQuoteState::Unpaid => {
                            tracing::info!(
                                "Quote {} status is {:?} - releasing proofs",
                                quote_info.id,
                                response.state()
                            );
                            self.handle_failure().await;
                            return Err(Error::PaymentFailed);
                        }
                        MeltQuoteState::Paid => {
                            tracing::info!(
                                "Quote {} confirmed paid - finalizing with change",
                                quote_info.id
                            );
                            let standard_response = response.into_standard()?;
                            let finalized = finalize_melt_common(
                                self.wallet,
                                self.compensations,
                                self.state_data.operation_id,
                                &self.state_data.quote,
                                &self.state_data.final_proofs,
                                &self.state_data.premint_secrets,
                                standard_response.state,
                                standard_response.payment_preimage,
                                standard_response.change,
                                metadata,
                            )
                            .await?;
                            return Ok(MeltSagaResult::Finalized(finalized));
                        }
                        MeltQuoteState::Pending => {
                            tracing::info!(
                                "Quote {} status is Pending - keeping proofs pending",
                                quote_info.id
                            );
                            self.handle_pending().await;
                            return Ok(MeltSagaResult::Pending(MeltSaga {
                                wallet: self.wallet,
                                compensations: self.compensations,
                                state_data: PaymentPending {
                                    operation_id: self.state_data.operation_id,
                                    quote: self.state_data.quote,
                                    final_proofs: self.state_data.final_proofs.clone(),
                                    premint_secrets: self.state_data.premint_secrets.clone(),
                                },
                            }));
                        }
                    },
                    Err(check_err) => {
                        tracing::warn!(
                            "Failed to check quote {} status: {}. Keeping proofs pending.",
                            quote_info.id,
                            check_err
                        );
                        self.handle_pending().await;
                        return Ok(MeltSagaResult::Pending(MeltSaga {
                            wallet: self.wallet,
                            compensations: self.compensations,
                            state_data: PaymentPending {
                                operation_id: self.state_data.operation_id,
                                quote: self.state_data.quote,
                                final_proofs: self.state_data.final_proofs.clone(),
                                premint_secrets: self.state_data.premint_secrets.clone(),
                            },
                        }));
                    }
                }
            }
        };

        match melt_response.state {
            MeltQuoteState::Paid => {
                let finalized = finalize_melt_common(
                    self.wallet,
                    self.compensations,
                    self.state_data.operation_id,
                    &self.state_data.quote,
                    &self.state_data.final_proofs,
                    &self.state_data.premint_secrets,
                    melt_response.state,
                    melt_response.payment_preimage,
                    melt_response.change,
                    metadata,
                )
                .await?;
                Ok(MeltSagaResult::Finalized(finalized))
            }
            MeltQuoteState::Pending => {
                self.handle_pending().await;
                Ok(MeltSagaResult::Pending(MeltSaga {
                    wallet: self.wallet,
                    compensations: self.compensations,
                    state_data: PaymentPending {
                        operation_id: self.state_data.operation_id,
                        quote: self.state_data.quote,
                        final_proofs: self.state_data.final_proofs.clone(),
                        premint_secrets: self.state_data.premint_secrets.clone(),
                    },
                }))
            }
            MeltQuoteState::Failed => {
                self.handle_failure().await;
                Err(Error::PaymentFailed)
            }
            _ => {
                tracing::warn!(
                    "Melt quote {} returned unexpected state {:?}",
                    quote_info.id,
                    melt_response.state
                );
                let finalized = finalize_melt_common(
                    self.wallet,
                    self.compensations,
                    self.state_data.operation_id,
                    &self.state_data.quote,
                    &self.state_data.final_proofs,
                    &self.state_data.premint_secrets,
                    melt_response.state,
                    melt_response.payment_preimage,
                    melt_response.change,
                    metadata,
                )
                .await?;
                Ok(MeltSagaResult::Finalized(finalized))
            }
        }
    }

    /// Handle pending payment state.
    async fn handle_pending(&self) {
        let quote_info = &self.state_data.quote;

        tracing::info!(
            "Melt quote {} is pending - proofs kept in pending state",
            quote_info.id
        );
    }

    /// Handle failed payment - release proofs and clean up.
    async fn handle_failure(&self) {
        let operation_id = self.state_data.operation_id;
        let final_proofs = &self.state_data.final_proofs;

        if let Ok(all_ys) = final_proofs.ys() {
            let _ = self
                .wallet
                .localstore
                .update_proofs_state(all_ys, State::Unspent)
                .await;
        }
        let _ = self
            .wallet
            .localstore
            .release_melt_quote(&operation_id)
            .await;
        let _ = self.wallet.localstore.delete_saga(&operation_id).await;
    }
}

impl<'a> MeltSaga<'a, PaymentPending> {
    /// Get the quote
    pub fn quote(&self) -> &MeltQuote {
        &self.state_data.quote
    }

    /// Finalize the melt with the response from subscription
    pub async fn finalize(
        self,
        state: MeltQuoteState,
        payment_preimage: Option<String>,
        change: Option<Vec<crate::nuts::BlindSignature>>,
        metadata: HashMap<String, String>,
    ) -> Result<MeltSaga<'a, Finalized>, Error> {
        finalize_melt_common(
            self.wallet,
            self.compensations,
            self.state_data.operation_id,
            &self.state_data.quote,
            &self.state_data.final_proofs,
            &self.state_data.premint_secrets,
            state,
            payment_preimage,
            change,
            metadata,
        )
        .await
    }
    /// Handle failed payment - release proofs and clean up.
    pub async fn handle_failure(&self) {
        let operation_id = self.state_data.operation_id;
        let final_proofs = &self.state_data.final_proofs;

        tracing::info!(
            "Handling failure for melt operation {}. Restoring {} proofs. Total amount: {}",
            operation_id,
            final_proofs.len(),
            final_proofs.total_amount().unwrap_or(Amount::ZERO)
        );

        if let Ok(all_ys) = final_proofs.ys() {
            if let Err(e) = self
                .wallet
                .localstore
                .update_proofs_state(all_ys, State::Unspent)
                .await
            {
                tracing::error!("Failed to restore proofs for failed melt: {}", e);
            } else {
                tracing::info!("Successfully restored proofs to Unspent");
            }
        }
        let _ = self
            .wallet
            .localstore
            .release_melt_quote(&operation_id)
            .await;
        let _ = self.wallet.localstore.delete_saga(&operation_id).await;
    }
}

impl<'a> MeltSaga<'a, Finalized> {
    /// Get the quote ID
    pub fn quote_id(&self) -> &str {
        &self.state_data.quote_id
    }

    /// Get the melt quote state
    pub fn state(&self) -> MeltQuoteState {
        self.state_data.state
    }

    /// Get the amount that was melted
    pub fn amount(&self) -> Amount {
        self.state_data.amount
    }

    /// Get the fee paid
    pub fn fee_paid(&self) -> Amount {
        self.state_data.fee
    }

    /// Get the payment proof (e.g., Lightning preimage)
    pub fn payment_proof(&self) -> Option<&str> {
        self.state_data.payment_proof.as_deref()
    }

    /// Consume the saga and return the change proofs
    pub fn into_change(self) -> Option<Proofs> {
        self.state_data.change
    }
}
