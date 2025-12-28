//! Melt Module
//!
//! This module provides the melt functionality for the wallet.
//!
//! # Usage
//!
//! Use [`Wallet::prepare_melt`] to create a [`PreparedMelt`], then call
//! [`confirm`](PreparedMelt::confirm) to complete the melt or
//! [`cancel`](PreparedMelt::cancel) to release reserved proofs.
//!
//! ```rust,no_run
//! # async fn example(wallet: &cdk::wallet::Wallet) -> anyhow::Result<()> {
//! use std::collections::HashMap;
//! let quote = wallet.melt_quote("lnbc...".to_string(), None).await?;
//!
//! // Prepare the melt - proofs are reserved but payment not yet executed
//! let prepared = wallet.prepare_melt(&quote.id, HashMap::new()).await?;
//!
//! // Inspect the prepared melt
//! println!(
//!     "Amount: {}, Fee: {}",
//!     prepared.amount(),
//!     prepared.total_fee()
//! );
//!
//! // Either confirm or cancel
//! let confirmed = prepared.confirm().await?;
//! // Or: prepared.cancel().await?;
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::fmt::Debug;

use cdk_common::util::unix_time;
use cdk_common::wallet::{MeltQuote, Transaction, TransactionDirection};
use cdk_common::{
    Error, MeltQuoteBolt11Response, MeltQuoteState, PaymentMethod, ProofsMethods, State,
};
use tracing::instrument;
use uuid::Uuid;

use crate::nuts::nut00::KnownMethod;
use crate::nuts::{MeltOptions, Proofs};
use crate::types::FinalizedMelt;
use crate::{Amount, Wallet};

mod bolt11;
mod bolt12;
mod custom;
#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
mod melt_bip353;
#[cfg(feature = "wallet")]
mod melt_lightning_address;
pub(crate) mod saga;

use saga::state::Prepared;
use saga::MeltSaga;

/// Options for confirming a melt operation
#[derive(Debug, Clone, Default)]
pub struct MeltConfirmOptions {
    /// Skip the pre-melt swap and send proofs directly to melt.
    ///
    /// When `false` (default): Performs a swap first to get optimal denominations,
    /// then sends the swapped proofs to the melt. This ensures exact amounts
    /// but pays input fees twice (once for swap, once for melt).
    ///
    /// When `true`: Sends proofs directly to the melt without swapping first.
    /// The mint will return any change. This saves the swap input fees but
    /// may result in less optimal change denominations.
    pub skip_swap: bool,
}

impl MeltConfirmOptions {
    /// Create options with default settings (swap enabled)
    pub fn new() -> Self {
        Self::default()
    }

    /// Create options that skip the swap
    pub fn skip_swap() -> Self {
        Self { skip_swap: true }
    }
}

/// A prepared melt operation that can be confirmed or cancelled.
///
/// This is the result of calling [`Wallet::prepare_melt`]. The proofs are reserved
/// but the payment has not yet been executed.
///
/// Call [`confirm`](Self::confirm) to execute the melt, or [`cancel`](Self::cancel)
/// to release the reserved proofs.
pub struct PreparedMelt<'a> {
    /// The saga in the Prepared state
    saga: MeltSaga<'a, Prepared>,
    /// Metadata for the transaction
    metadata: HashMap<String, String>,
}

impl<'a> PreparedMelt<'a> {
    /// Get the operation ID
    pub fn operation_id(&self) -> Uuid {
        self.saga.operation_id()
    }

    /// Get the quote
    pub fn quote(&self) -> &MeltQuote {
        self.saga.quote()
    }

    /// Get the amount to be melted
    pub fn amount(&self) -> Amount {
        self.saga.quote().amount
    }

    /// Get the proofs that will be used
    pub fn proofs(&self) -> &Proofs {
        self.saga.proofs()
    }

    /// Get the proofs that need to be swapped
    pub fn proofs_to_swap(&self) -> &Proofs {
        self.saga.proofs_to_swap()
    }

    /// Get the swap fee
    pub fn swap_fee(&self) -> Amount {
        self.saga.swap_fee()
    }

    /// Get the input fee
    pub fn input_fee(&self) -> Amount {
        self.saga.input_fee()
    }

    /// Get the total fee (with swap, if applicable)
    pub fn total_fee(&self) -> Amount {
        self.saga.swap_fee() + self.saga.input_fee()
    }

    /// Returns true if a swap would be performed (proofs_to_swap is not empty)
    pub fn requires_swap(&self) -> bool {
        !self.saga.proofs_to_swap().is_empty()
    }

    /// Get the total fee if swap is performed (current default behavior)
    ///
    /// This is swap_fee + input_fee on optimized proofs.
    /// Same as [`total_fee()`](Self::total_fee).
    pub fn total_fee_with_swap(&self) -> Amount {
        self.saga.swap_fee() + self.saga.input_fee()
    }

    /// Get the input fee if swap is skipped (fee on all proofs sent directly)
    pub fn input_fee_without_swap(&self) -> Amount {
        self.saga.input_fee_without_swap()
    }

    /// Get the fee savings from skipping the swap
    ///
    /// Returns how much less you would pay in fees by using
    /// `confirm_with_options(MeltConfirmOptions::skip_swap())`.
    pub fn fee_savings_without_swap(&self) -> Amount {
        self.total_fee_with_swap()
            .checked_sub(self.input_fee_without_swap())
            .unwrap_or(Amount::ZERO)
    }

    /// Get the expected change amount if swap is skipped
    ///
    /// This is how much would be "overpaid" and returned as change from the melt.
    pub fn change_amount_without_swap(&self) -> Amount {
        let all_proofs_total = self.saga.proofs().total_amount().unwrap_or(Amount::ZERO)
            + self
                .saga
                .proofs_to_swap()
                .total_amount()
                .unwrap_or(Amount::ZERO);
        let quote = self.saga.quote();
        let needed = quote.amount + quote.fee_reserve + self.input_fee_without_swap();
        all_proofs_total.checked_sub(needed).unwrap_or(Amount::ZERO)
    }

    /// Confirm the prepared melt and execute the payment.
    ///
    /// This transitions the saga through: Prepared -> MeltRequested -> Finalized
    ///
    /// Uses default options (swap enabled if needed).
    pub async fn confirm(self) -> Result<FinalizedMelt, Error> {
        self.confirm_with_options(MeltConfirmOptions::default())
            .await
    }

    /// Confirm the prepared melt with custom options.
    ///
    /// This transitions the saga through: Prepared -> MeltRequested -> Finalized
    ///
    /// # Options
    ///
    /// - `skip_swap`: If true, skips the pre-melt swap and sends proofs directly
    ///   to the melt. This saves fees but may result in change being returned.
    pub async fn confirm_with_options(
        self,
        options: MeltConfirmOptions,
    ) -> Result<FinalizedMelt, Error> {
        // Transition to MeltRequested state (handles swap based on options)
        let melt_requested = self.saga.request_melt_with_options(options).await?;

        // Execute the melt request and get Finalized saga
        let finalized = melt_requested.execute(self.metadata).await?;

        Ok(FinalizedMelt::new(
            finalized.quote_id().to_string(),
            finalized.state(),
            finalized.payment_proof().map(|s| s.to_string()),
            finalized.amount(),
            finalized.fee_paid(),
            finalized.into_change(),
        ))
    }

    /// Cancel the prepared melt and release reserved proofs
    pub async fn cancel(self) -> Result<(), Error> {
        self.saga.cancel().await
    }
}

impl Debug for PreparedMelt<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedMelt")
            .field("operation_id", &self.saga.operation_id())
            .field("quote_id", &self.saga.quote().id)
            .field("amount", &self.saga.quote().amount)
            .field("total_fee", &self.total_fee())
            .finish()
    }
}

impl Wallet {
    /// Prepare a melt operation without executing it.
    ///
    /// This reserves the proofs needed for the melt but does not execute the payment.
    /// The returned `PreparedMelt` can be:
    /// - Confirmed with `confirm()` to execute the payment
    /// - Cancelled with `cancel()` to release the reserved proofs
    ///
    /// This is useful for:
    /// - Inspecting the fee before committing to the melt
    /// - Building UIs that show a confirmation step
    /// - Implementing custom retry/cancellation logic
    ///
    /// # Example
    /// ```rust,no_run
    /// # async fn example(wallet: &cdk::wallet::Wallet) -> anyhow::Result<()> {
    /// use std::collections::HashMap;
    /// let quote = wallet.melt_quote("lnbc...".to_string(), None).await?;
    ///
    /// let prepared = wallet.prepare_melt(&quote.id, HashMap::new()).await?;
    /// println!("Fee will be: {}", prepared.total_fee());
    ///
    /// // Decide whether to proceed
    /// let confirmed = prepared.confirm().await?;
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(skip(self, metadata))]
    pub async fn prepare_melt(
        &self,
        quote_id: &str,
        metadata: HashMap<String, String>,
    ) -> Result<PreparedMelt<'_>, Error> {
        let saga = MeltSaga::new(self);
        let prepared_saga = saga.prepare(quote_id, metadata.clone()).await?;

        Ok(PreparedMelt {
            saga: prepared_saga,
            metadata,
        })
    }

    /// Prepare a melt operation with specific proofs.
    ///
    /// Unlike `prepare_melt()`, this method uses the provided proofs directly
    /// without automatic proof selection. The caller is responsible for ensuring
    /// the proofs are sufficient to cover the quote amount plus fee reserve.
    ///
    /// This is useful when:
    /// - You have specific proofs you want to use (e.g., from a received token)
    /// - The proofs are external (not already in the wallet's database)
    ///
    /// # Example
    /// ```rust,no_run
    /// # async fn example(wallet: &cdk::wallet::Wallet, proofs: cdk::nuts::Proofs) -> anyhow::Result<()> {
    /// use std::collections::HashMap;
    /// let quote = wallet.melt_quote("lnbc...".to_string(), None).await?;
    ///
    /// let prepared = wallet.prepare_melt_proofs(&quote.id, proofs, HashMap::new()).await?;
    /// let confirmed = prepared.confirm().await?;
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(skip(self, proofs, metadata))]
    pub async fn prepare_melt_proofs(
        &self,
        quote_id: &str,
        proofs: crate::nuts::Proofs,
        metadata: HashMap<String, String>,
    ) -> Result<PreparedMelt<'_>, Error> {
        let saga = MeltSaga::new(self);
        let prepared_saga = saga
            .prepare_with_proofs(quote_id, proofs, metadata.clone())
            .await?;

        Ok(PreparedMelt {
            saga: prepared_saga,
            metadata,
        })
    }

    /// Finalize pending melt operations.
    ///
    /// This checks all incomplete melt sagas where payment may be pending,
    /// queries the mint for the current quote status, and:
    /// - **Paid**: Marks proofs as spent, recovers change, returns `FinalizedMelt` with state `Paid`
    /// - **Failed/Unpaid**: Compensates by releasing reserved proofs, returns `FinalizedMelt` with state `Unpaid`/`Failed`
    /// - **Pending/Unknown**: Skips (payment still in flight), not included in result
    ///
    /// Call this periodically or after receiving a notification that a
    /// pending payment may have settled.
    ///
    /// # Returns
    ///
    /// A vector of finalized melt results. Check the `state` field to determine
    /// if each melt succeeded (`Paid`) or failed (`Unpaid`/`Failed`).
    /// Melts that are still pending are not included in the result.
    #[instrument(skip_all)]
    pub async fn finalize_pending_melts(&self) -> Result<Vec<FinalizedMelt>, Error> {
        use cdk_common::wallet::{MeltSagaState, WalletSagaState};

        let sagas = self.localstore.get_incomplete_sagas().await?;

        // Filter to only melt sagas in states that need checking
        let melt_sagas: Vec<_> = sagas
            .into_iter()
            .filter(|s| {
                matches!(
                    &s.state,
                    WalletSagaState::Melt(
                        MeltSagaState::MeltRequested | MeltSagaState::PaymentPending
                    )
                )
            })
            .collect();

        if melt_sagas.is_empty() {
            return Ok(Vec::new());
        }

        tracing::info!("Found {} pending melt(s) to check", melt_sagas.len());

        let mut results = Vec::new();

        for saga in melt_sagas {
            match self.resume_melt_saga(&saga).await {
                Ok(Some(melted)) => {
                    tracing::info!("Melt {} finalized with state {:?}", saga.id, melted.state());
                    results.push(melted);
                }
                Ok(None) => {
                    tracing::debug!("Melt {} still pending or compensated early", saga.id);
                }
                Err(e) => {
                    tracing::error!("Failed to finalize melt {}: {}", saga.id, e);
                    // Continue with other sagas instead of failing entirely
                }
            }
        }

        Ok(results)
    }

    /// Confirm a prepared melt with already-reserved proofs.
    ///
    /// This is used by `MultiMintPreparedMelt::confirm` which holds an `Arc<Wallet>`
    /// and has already prepared/reserved proofs. For the normal API path, use
    /// `PreparedMelt::confirm()` which uses the typestate saga.
    ///
    /// The `operation_id` and `quote` must correspond to an existing prepared saga.
    #[instrument(skip(self, proofs, proofs_to_swap, metadata))]
    #[allow(clippy::too_many_arguments)]
    pub async fn confirm_prepared_melt(
        &self,
        operation_id: Uuid,
        quote: MeltQuote,
        proofs: Proofs,
        proofs_to_swap: Proofs,
        input_fee: Amount,
        input_fee_without_swap: Amount,
        metadata: HashMap<String, String>,
    ) -> Result<FinalizedMelt, Error> {
        self.confirm_prepared_melt_with_options(
            operation_id,
            quote,
            proofs,
            proofs_to_swap,
            input_fee,
            input_fee_without_swap,
            metadata,
            MeltConfirmOptions::default(),
        )
        .await
    }

    /// Confirm a prepared melt with already-reserved proofs and custom options.
    ///
    /// This is used by `MultiMintPreparedMelt::confirm_with_options` which holds an `Arc<Wallet>`
    /// and has already prepared/reserved proofs.
    ///
    /// # Options
    ///
    /// - `skip_swap`: If true, skips the pre-melt swap and sends proofs directly.
    #[instrument(skip(self, proofs, proofs_to_swap, metadata, options))]
    #[allow(clippy::too_many_arguments)]
    pub async fn confirm_prepared_melt_with_options(
        &self,
        operation_id: Uuid,
        quote: MeltQuote,
        proofs: Proofs,
        proofs_to_swap: Proofs,
        input_fee: Amount,
        input_fee_without_swap: Amount,
        metadata: HashMap<String, String>,
        options: MeltConfirmOptions,
    ) -> Result<FinalizedMelt, Error> {
        // Create a saga in Prepared state and continue from there
        // We reconstruct the Prepared state from the stored data
        let saga = MeltSaga::from_prepared(
            self,
            operation_id,
            quote,
            proofs,
            proofs_to_swap,
            input_fee,
            input_fee_without_swap,
        );

        let melt_requested = saga.request_melt_with_options(options).await?;
        let finalized = melt_requested.execute(metadata).await?;

        Ok(FinalizedMelt::new(
            finalized.quote_id().to_string(),
            finalized.state(),
            finalized.payment_proof().map(|s| s.to_string()),
            finalized.amount(),
            finalized.fee_paid(),
            finalized.into_change(),
        ))
    }

    /// Cancel a prepared melt and release reserved proofs.
    ///
    /// This is used by `MultiMintPreparedMelt::cancel` which holds an `Arc<Wallet>`.
    #[instrument(skip(self, proofs, proofs_to_swap))]
    pub async fn cancel_prepared_melt(
        &self,
        operation_id: Uuid,
        proofs: Proofs,
        proofs_to_swap: Proofs,
    ) -> Result<(), Error> {
        tracing::info!("Cancelling prepared melt for operation {}", operation_id);

        // Revert proof reservation
        let mut all_ys = proofs.ys()?;
        all_ys.extend(proofs_to_swap.ys()?);

        if !all_ys.is_empty() {
            self.localstore
                .update_proofs_state(all_ys, State::Unspent)
                .await?;
        }

        // Release quote reservation
        if let Err(e) = self.localstore.release_melt_quote(&operation_id).await {
            tracing::warn!(
                "Failed to release melt quote for operation {}: {}",
                operation_id,
                e
            );
        }

        // Delete saga record
        if let Err(e) = self.localstore.delete_saga(&operation_id).await {
            tracing::warn!(
                "Failed to delete melt saga {}: {}. Will be cleaned up on recovery.",
                operation_id,
                e
            );
        }

        Ok(())
    }

    /// Get all active melt quotes from the wallet
    pub async fn get_active_melt_quotes(&self) -> Result<Vec<MeltQuote>, Error> {
        let quotes = self.localstore.get_melt_quotes().await?;
        Ok(quotes
            .into_iter()
            .filter(|q| {
                q.state == MeltQuoteState::Pending
                    || (q.state == MeltQuoteState::Unpaid && q.expiry > unix_time())
            })
            .collect())
    }

    /// Get pending melt quotes
    pub async fn get_pending_melt_quotes(&self) -> Result<Vec<MeltQuote>, Error> {
        let quotes = self.localstore.get_melt_quotes().await?;
        Ok(quotes
            .into_iter()
            .filter(|q| q.state == MeltQuoteState::Pending)
            .collect())
    }

    pub(crate) async fn add_transaction_for_pending_melt(
        &self,
        quote: &MeltQuote,
        response: &MeltQuoteBolt11Response<String>,
    ) -> Result<(), Error> {
        if quote.state != response.state {
            tracing::info!(
                "Quote melt {} state changed from {} to {}",
                quote.id,
                quote.state,
                response.state
            );
            if response.state == MeltQuoteState::Paid {
                let pending_proofs = self
                    .get_proofs_with(Some(vec![State::Pending]), None)
                    .await?;
                let proofs_total = pending_proofs.total_amount().unwrap_or_default();
                let change_total = response.change_amount().unwrap_or_default();

                self.localstore
                    .add_transaction(Transaction {
                        mint_url: self.mint_url.clone(),
                        direction: TransactionDirection::Outgoing,
                        amount: response.amount,
                        fee: proofs_total
                            .checked_sub(response.amount)
                            .and_then(|amt| amt.checked_sub(change_total))
                            .unwrap_or_default(),
                        unit: quote.unit.clone(),
                        ys: pending_proofs.ys()?,
                        timestamp: unix_time(),
                        memo: None,
                        metadata: HashMap::new(),
                        quote_id: Some(quote.id.clone()),
                        payment_request: Some(quote.request.clone()),
                        payment_proof: response.payment_preimage.clone(),
                        payment_method: Some(quote.payment_method.clone()),
                    })
                    .await?;
            }
        }
        Ok(())
    }

    /// Get a melt quote for a human-readable address
    ///
    /// This method accepts a human-readable address that could be either a BIP353 address
    /// or a Lightning address. It intelligently determines which to try based on mint support:
    ///
    /// 1. If the mint supports Bolt12, it tries BIP353 first
    /// 2. Falls back to Lightning address only if BIP353 DNS resolution fails
    /// 3. If BIP353 resolves but fails at the mint, it does NOT fall back to Lightning address
    /// 4. If the mint doesn't support Bolt12, it tries Lightning address directly
    #[cfg(all(feature = "bip353", feature = "wallet", not(target_arch = "wasm32")))]
    pub async fn melt_human_readable_quote(
        &self,
        address: &str,
        amount_msat: impl Into<crate::Amount>,
    ) -> Result<MeltQuote, Error> {
        use cdk_common::nuts::PaymentMethod;

        let amount = amount_msat.into();

        // Get mint info from cache to check bolt12 support (no network call)
        let mint_info = &self
            .metadata_cache
            .load(&self.localstore, &self.client, {
                let ttl = self.metadata_cache_ttl.read();
                *ttl
            })
            .await?
            .mint_info;

        // Check if mint supports bolt12 by looking at nut05 methods
        let supports_bolt12 = mint_info
            .nuts
            .nut05
            .methods
            .iter()
            .any(|m| m.method == PaymentMethod::Known(KnownMethod::Bolt12));

        if supports_bolt12 {
            // Mint supports bolt12, try BIP353 first
            match self.melt_bip353_quote(address, amount).await {
                Ok(quote) => Ok(quote),
                Err(Error::Bip353Resolve(_)) => {
                    // DNS resolution failed, fall back to Lightning address
                    tracing::debug!(
                        "BIP353 DNS resolution failed for {}, trying Lightning address",
                        address
                    );
                    return self.melt_lightning_address_quote(address, amount).await;
                }
                Err(e) => {
                    // BIP353 resolved but failed for another reason (e.g., mint error)
                    // Don't fall back to Lightning address
                    Err(e)
                }
            }
        } else {
            // Mint doesn't support bolt12, use Lightning address directly
            self.melt_lightning_address_quote(address, amount).await
        }
    }
    /// Unified melt quote method for all payment methods
    ///
    /// Routes to the appropriate handler based on the payment method.
    /// For custom payment methods, you can pass extra JSON data that will be
    /// forwarded to the payment processor.
    ///
    /// # Arguments
    /// * `method` - Payment method to use (bolt11, bolt12, or custom)
    /// * `request` - Payment request string (invoice, offer, or custom format)
    /// * `options` - Optional melt options (MPP, amountless, etc.)
    /// * `extra` - Optional extra payment-method-specific data as JSON (for custom methods)
    pub async fn melt_quote_unified(
        &self,
        method: PaymentMethod,
        request: String,
        options: Option<MeltOptions>,
        extra: Option<serde_json::Value>,
    ) -> Result<MeltQuote, Error> {
        match method {
            PaymentMethod::Known(KnownMethod::Bolt11) => self.melt_quote(request, options).await,
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                self.melt_bolt12_quote(request, options).await
            }
            PaymentMethod::Custom(custom_method) => {
                self.melt_quote_custom(&custom_method, request, options, extra)
                    .await
            }
        }
    }
}
