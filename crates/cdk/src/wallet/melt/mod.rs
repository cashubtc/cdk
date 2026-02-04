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
//!
//! use cdk::nuts::PaymentMethod;
//! let quote = wallet
//!     .melt_quote(PaymentMethod::BOLT11, "lnbc...", None, None)
//!     .await?;
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
use cdk_common::{Error, MeltQuoteState, PaymentMethod, ProofsMethods, State};
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

/// Internal response type for melt quote status checking.
///
/// Wraps the different response types (Bolt11/Bolt12 vs Custom) that have
/// identical fields but different Rust types.
#[derive(Debug, Clone)]
pub(crate) enum MeltQuoteStatusResponse {
    /// Standard response (Bolt11)
    Standard(cdk_common::MeltQuoteBolt11Response<String>),
    /// Bolt12 response
    Bolt12(cdk_common::MeltQuoteBolt12Response<String>),
    /// Custom payment method response
    Custom(cdk_common::MeltQuoteCustomResponse<String>),
}

impl MeltQuoteStatusResponse {
    /// Get the quote state
    pub fn state(&self) -> MeltQuoteState {
        match self {
            Self::Standard(r) => r.state,
            Self::Bolt12(r) => r.state,
            Self::Custom(r) => r.state,
        }
    }

    /// Get the payment preimage
    pub fn payment_preimage(&self) -> Option<String> {
        match self {
            Self::Standard(r) => r.payment_preimage.clone(),
            Self::Bolt12(r) => r.payment_preimage.clone(),
            Self::Custom(r) => r.payment_preimage.clone(),
        }
    }

    /// Convert to standard response (for Bolt11).
    /// Returns error for Custom payment methods and Bolt12 (since types differ).
    pub fn into_standard(self) -> Result<cdk_common::MeltQuoteBolt11Response<String>, Error> {
        match self {
            Self::Standard(r) => Ok(r),
            _ => Err(Error::Custom(
                "Cannot convert response to standard bolt11 response".to_string(),
            )),
        }
    }
}

/// Options for confirming a melt operation
#[derive(Debug, Clone, Default)]
pub struct MeltConfirmOptions {
    /// Skip the pre-melt swap and send proofs directly to melt.
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
#[must_use = "must be confirmed or canceled to release reserved proofs"]
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
    pub fn total_fee_with_swap(&self) -> Amount {
        self.saga.swap_fee() + self.saga.input_fee()
    }

    /// Get the input fee if swap is skipped (fee on all proofs sent directly)
    pub fn input_fee_without_swap(&self) -> Amount {
        self.saga.input_fee_without_swap()
    }

    /// Get the fee savings from skipping the swap
    pub fn fee_savings_without_swap(&self) -> Amount {
        self.total_fee_with_swap()
            .checked_sub(self.input_fee_without_swap())
            .unwrap_or(Amount::ZERO)
    }

    /// Get the expected change amount if swap is skipped
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
    pub async fn confirm(self) -> Result<FinalizedMelt, Error> {
        self.confirm_with_options(MeltConfirmOptions::default())
            .await
    }

    /// Confirm the prepared melt with custom options.
    pub async fn confirm_with_options(
        self,
        options: MeltConfirmOptions,
    ) -> Result<FinalizedMelt, Error> {
        let melt_requested = self.saga.request_melt_with_options(options).await?;

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
        // Fetch saga from DB for optimistic locking
        let db_saga = self
            .localstore
            .get_saga(&operation_id)
            .await?
            .ok_or(Error::Custom("Saga not found".to_string()))?;

        let saga = MeltSaga::from_prepared(
            self,
            operation_id,
            quote,
            proofs,
            proofs_to_swap,
            input_fee,
            input_fee_without_swap,
            db_saga,
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

        let mut all_ys = proofs.ys()?;
        all_ys.extend(proofs_to_swap.ys()?);

        if !all_ys.is_empty() {
            self.localstore
                .update_proofs_state(all_ys, State::Unspent)
                .await?;
        }

        if let Err(e) = self.localstore.release_melt_quote(&operation_id).await {
            tracing::warn!(
                "Failed to release melt quote for operation {}: {}",
                operation_id,
                e
            );
        }

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
        new_state: MeltQuoteState,
        amount: Amount,
        change_amount: Option<Amount>,
        payment_preimage: Option<String>,
    ) -> Result<(), Error> {
        if quote.state != new_state {
            tracing::info!(
                "Quote melt {} state changed from {} to {}",
                quote.id,
                quote.state,
                new_state
            );
            if new_state == MeltQuoteState::Paid {
                let pending_proofs = self
                    .get_proofs_with(Some(vec![State::Pending]), None)
                    .await?;
                let proofs_total = pending_proofs.total_amount().unwrap_or_default();
                let change_total = change_amount.unwrap_or_default();

                self.localstore
                    .add_transaction(Transaction {
                        mint_url: self.mint_url.clone(),
                        direction: TransactionDirection::Outgoing,
                        amount,
                        fee: proofs_total
                            .checked_sub(amount)
                            .and_then(|amt| amt.checked_sub(change_total))
                            .unwrap_or_default(),
                        unit: quote.unit.clone(),
                        ys: pending_proofs.ys()?,
                        timestamp: unix_time(),
                        memo: None,
                        metadata: HashMap::new(),
                        quote_id: Some(quote.id.clone()),
                        payment_request: Some(quote.request.clone()),
                        payment_proof: payment_preimage,
                        payment_method: Some(quote.payment_method.clone()),
                        saga_id: quote
                            .used_by_operation
                            .as_ref()
                            .and_then(|id| Uuid::parse_str(id).ok()),
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
    /// Melt quote for all payment methods
    ///
    /// Accepts `Bolt11Invoice`, `Offer`, `String`, or `&str` for the request parameter.
    #[instrument(skip(self, request, options, extra))]
    pub async fn melt_quote<T, R>(
        &self,
        method: T,
        request: R,
        options: Option<MeltOptions>,
        extra: Option<String>,
    ) -> Result<MeltQuote, Error>
    where
        T: Into<PaymentMethod> + std::fmt::Debug,
        R: std::fmt::Display,
    {
        let method: PaymentMethod = method.into();
        let request_str = request.to_string();

        match method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                self.melt_bolt11_quote(request_str, options).await
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                self.melt_bolt12_quote(request_str, options).await
            }
            PaymentMethod::Custom(custom_method) => {
                let extra_json =
                    extra.map(|s| serde_json::from_str(&s).unwrap_or(serde_json::Value::Null));
                self.melt_quote_custom(&custom_method, request_str, options, extra_json)
                    .await
            }
        }
    }

    /// Update the state of a melt quote
    pub(crate) async fn update_melt_quote_state(
        &self,
        quote: &mut MeltQuote,
        new_state: MeltQuoteState,
        amount: Amount,
        change_amount: Option<Amount>,
        payment_preimage: Option<String>,
    ) -> Result<(), Error> {
        if let Err(e) = self
            .add_transaction_for_pending_melt(
                quote,
                new_state,
                amount,
                change_amount,
                payment_preimage.clone(),
            )
            .await
        {
            tracing::error!("Failed to add transaction for pending melt: {}", e);
        }

        quote.state = new_state;

        match self.localstore.add_melt_quote(quote.clone()).await {
            Ok(_) => Ok(()),
            Err(e) => {
                if matches!(e, cdk_common::database::Error::ConcurrentUpdate) {
                    tracing::debug!(
                        "Concurrent update detected for melt quote {}, retrying",
                        quote.id
                    );
                    let mut fresh_quote = self
                        .localstore
                        .get_melt_quote(&quote.id)
                        .await?
                        .ok_or(Error::UnknownQuote)?;

                    fresh_quote.state = new_state;

                    match self.localstore.add_melt_quote(fresh_quote.clone()).await {
                        Ok(_) => (),
                        Err(e) => {
                            if matches!(e, cdk_common::database::Error::ConcurrentUpdate) {
                                return Err(Error::ConcurrentUpdate);
                            }
                            return Err(Error::Database(e));
                        }
                    }

                    *quote = fresh_quote;
                    Ok(())
                } else {
                    Err(Error::Database(e))
                }
            }
        }
    }

    /// Check melt quote status
    #[instrument(skip(self, quote_id))]
    pub async fn check_melt_quote_status(&self, quote_id: &str) -> Result<MeltQuote, Error> {
        let mut quote = self
            .localstore
            .get_melt_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        // Check if there's an in-progress saga for this quote
        if let Some(ref operation_id_str) = quote.used_by_operation {
            if let Ok(operation_id) = uuid::Uuid::parse_str(operation_id_str) {
                match self.localstore.get_saga(&operation_id).await {
                    Ok(Some(saga)) => {
                        // Saga exists - try to complete it
                        tracing::info!(
                            "Melt quote {} has in-progress saga {}, attempting to complete",
                            quote_id,
                            operation_id
                        );

                        match self.resume_melt_saga(&saga).await? {
                            Some(_) => {
                                // Saga completed - re-fetch quote from DB
                                quote = self
                                    .localstore
                                    .get_melt_quote(quote_id)
                                    .await?
                                    .ok_or(Error::UnknownQuote)?;
                            }
                            None => {
                                // Saga still pending (payment in progress or mint unreachable)
                                // Return current quote state - no need to query mint again
                                // since resume_melt_saga already checked
                                return Ok(quote);
                            }
                        }
                    }
                    Ok(None) => {
                        // Orphaned reservation - release it
                        tracing::warn!(
                            "Melt quote {} has orphaned reservation for operation {}, releasing",
                            quote_id,
                            operation_id
                        );
                        if let Err(e) = self.localstore.release_melt_quote(&operation_id).await {
                            tracing::warn!("Failed to release orphaned melt quote: {}", e);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to check saga for melt quote {}: {}", quote_id, e);
                        return Err(Error::Database(e));
                    }
                }
            }
        }

        match &quote.payment_method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                let response = self.client.get_melt_quote_status(quote_id).await?;
                self.update_melt_quote_state(
                    &mut quote,
                    response.state,
                    response.amount,
                    response.change_amount(),
                    response.payment_preimage,
                )
                .await?;
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                let response = self.client.get_melt_bolt12_quote_status(quote_id).await?;
                self.update_melt_quote_state(
                    &mut quote,
                    response.state,
                    response.amount,
                    response.change_amount(),
                    response.payment_preimage,
                )
                .await?;
            }
            PaymentMethod::Custom(method) => {
                let response = self
                    .client
                    .get_melt_quote_custom_status(method, quote_id)
                    .await?;
                self.update_melt_quote_state(
                    &mut quote,
                    response.state,
                    response.amount,
                    response.change_amount(),
                    response.payment_preimage,
                )
                .await?;
            }
        };

        Ok(quote)
    }
    /// This returns the raw protocol response including change signatures,
    /// which is needed by saga recovery flows. For normal status checking,
    /// use `check_melt_quote_status()` instead.
    ///
    /// Routes to the correct client endpoint based on the payment method
    /// stored in the quote.
    #[instrument(skip(self, quote_id))]
    pub(crate) async fn internal_check_melt_status(
        &self,
        quote_id: &str,
    ) -> Result<MeltQuoteStatusResponse, Error> {
        let quote = self
            .localstore
            .get_melt_quote(quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        // Route to correct endpoint based on payment method
        let response = match &quote.payment_method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                let r = self.client.get_melt_quote_status(quote_id).await?;
                MeltQuoteStatusResponse::Standard(r)
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                let r = self.client.get_melt_bolt12_quote_status(quote_id).await?;
                MeltQuoteStatusResponse::Bolt12(r)
            }
            PaymentMethod::Custom(method) => {
                let r = self
                    .client
                    .get_melt_quote_custom_status(method, quote_id)
                    .await?;
                MeltQuoteStatusResponse::Custom(r)
            }
        };

        Ok(response)
    }
}
