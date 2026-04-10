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
use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::str::FromStr;

use cdk_common::util::unix_time;
use cdk_common::wallet::{MeltQuote, Transaction, TransactionDirection};
use cdk_common::{Error, MeltQuoteState, PaymentMethod, ProofsMethods, State};
use tracing::instrument;
use uuid::Uuid;

use crate::nuts::nut00::KnownMethod;
use crate::nuts::{MeltOptions, Proofs, Token};
use crate::types::FinalizedMelt;
use crate::wallet::subscription::NotificationPayload;
use crate::wallet::WalletSubscription;
use crate::{ensure_cdk, Amount, Wallet};

mod bolt11;
mod bolt12;
mod custom;
#[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
mod melt_bip353;
#[cfg(feature = "wallet")]
mod melt_lightning_address;
mod onchain;
pub(crate) mod saga;

use saga::state::Prepared;
use saga::{MeltSaga, MeltSagaResult};

/// Outcome of a melt operation using async support (NUT-05).
#[derive(Debug)]
pub enum MeltOutcome<'a> {
    /// Melt completed immediately
    Paid(FinalizedMelt),
    /// Melt is pending - can be awaited or dropped to poll elsewhere
    Pending(PendingMelt<'a>),
}

/// A pending melt operation that can be awaited.
#[derive(Debug)]
pub struct PendingMelt<'a> {
    saga: Box<MeltSaga<'a, saga::state::PaymentPending>>,
    metadata: HashMap<String, String>,
}

impl<'a> PendingMelt<'a> {
    /// Wait for the melt to complete by polling the mint.
    async fn wait(self) -> Result<FinalizedMelt, Error> {
        let quote_id = self.saga.quote().id.clone();
        let wallet = self.saga.wallet;
        let operation_id = self.saga.state_data.operation_id;

        let subscribe_result = match self.saga.quote().payment_method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                wallet
                    .subscribe(WalletSubscription::Bolt11MeltQuoteState(vec![
                        quote_id.clone()
                    ]))
                    .await
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                wallet
                    .subscribe(WalletSubscription::Bolt12MeltQuoteState(vec![
                        quote_id.clone()
                    ]))
                    .await
            }
            PaymentMethod::Known(KnownMethod::Onchain) => {
                wallet
                    .subscribe(WalletSubscription::MeltQuoteOnchainState(vec![
                        quote_id.clone()
                    ]))
                    .await
            }
            PaymentMethod::Custom(ref method) => {
                wallet
                    .subscribe(WalletSubscription::MeltQuoteCustom(
                        method.to_string(),
                        vec![quote_id.clone()],
                    ))
                    .await
            }
        };

        let mut subscription = match subscribe_result {
            Ok(subscription) => subscription,
            Err(err) => {
                return wallet.recover_failed_melt_confirm(operation_id, err).await;
            }
        };

        loop {
            match subscription.recv().await {
                Some(event) => {
                    let notification = event.into_inner();

                    // `payment_proof` is the method-specific settlement
                    // artifact: Lightning preimage for Bolt11/Bolt12/Custom,
                    // broadcast outpoint (`txid:vout`) for Onchain. Either
                    // presence signals an irreversible mint-side action and
                    // is used by the failure path below to block proof
                    // reversion.
                    let (response_quote_id, state, payment_proof, change) = match notification {
                        NotificationPayload::MeltQuoteBolt11Response(response) => (
                            response.quote,
                            response.state,
                            response.payment_preimage,
                            response.change,
                        ),
                        NotificationPayload::MeltQuoteBolt12Response(response) => (
                            response.quote,
                            response.state,
                            response.payment_preimage,
                            response.change,
                        ),
                        NotificationPayload::CustomMeltQuoteResponse(_, response) => (
                            response.quote,
                            response.state,
                            response.payment_preimage,
                            response.change,
                        ),
                        NotificationPayload::MeltQuoteOnchainResponse(response) => {
                            // Onchain melts never return NUT-08 change outputs;
                            // the outpoint is surfaced as the payment proof.
                            (response.quote, response.state, response.outpoint, None)
                        }
                        _ => continue,
                    };

                    if response_quote_id != quote_id {
                        continue;
                    }

                    match state {
                        MeltQuoteState::Paid => {
                            // TODO: Remove this workaround once Nutshell 0.18.3+ is widely deployed
                            //
                            // Per NUT-05, mints SHOULD include change in WebSocket notifications when
                            // available. However, Nutshell 0.18.2 and below have a bug where change
                            // is omitted from WS notifications even when proofs were provided.
                            //
                            // Workaround: When WS shows Paid but has no change, we make an extra HTTP
                            // request to get the full response with change. This adds latency and
                            // unnecessary network traffic for the common case where change exists.
                            //
                            // Impact: One extra HTTP request per melt until Nutshell versions < 0.18.3
                            // are no longer widely used.
                            let change = if change.is_none() {
                                tracing::debug!("Received WS with no change checking with HTTP");

                                match self.saga.wallet.internal_check_melt_status(&quote_id).await {
                                    Ok(response) => response.change(),
                                    Err(e) => {
                                        tracing::warn!(
                                            "Failed to check melt status via HTTP: {}",
                                            e
                                        );
                                        None
                                    }
                                }
                            } else {
                                change
                            };

                            match self
                                .saga
                                .finalize(state, payment_proof, change, self.metadata)
                                .await
                            {
                                Ok(finalized) => {
                                    return Ok(FinalizedMelt::new(
                                        finalized.quote_id().to_string(),
                                        finalized.state(),
                                        finalized.payment_proof().map(|s| s.to_string()),
                                        finalized.amount(),
                                        finalized.fee_paid(),
                                        finalized.into_change(),
                                    ));
                                }
                                Err(err) => {
                                    return wallet
                                        .recover_failed_melt_confirm(operation_id, err)
                                        .await;
                                }
                            }
                        }
                        MeltQuoteState::Failed
                        | MeltQuoteState::Unpaid
                        | MeltQuoteState::Unknown => {
                            // Safety: if the mint has emitted a payment
                            // proof (Lightning preimage or Onchain
                            // outpoint), an irreversible settlement
                            // artifact exists. Do not revert proofs —
                            // continue waiting for the next subscription
                            // event or reconciliation to resolve the
                            // state.
                            if payment_proof.is_some() {
                                tracing::warn!(
                                    "Melt quote {} reported {:?} via WS but \
                                     carries a payment proof; continuing to \
                                     wait to avoid proof loss",
                                    quote_id,
                                    state
                                );
                                continue;
                            }
                            self.saga.handle_failure().await;
                            return Err(Error::PaymentFailed);
                        }
                        MeltQuoteState::Pending => continue,
                    }
                }
                None => {
                    let err = Error::Custom("Subscription closed".to_string());
                    return wallet.recover_failed_melt_confirm(operation_id, err).await;
                }
            }
        }
    }
}

impl<'a> IntoFuture for PendingMelt<'a> {
    type Output = Result<FinalizedMelt, Error>;

    #[cfg(not(target_arch = "wasm32"))]
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;

    #[cfg(target_arch = "wasm32")]
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.wait())
    }
}

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
    /// Onchain response
    Onchain(cdk_common::MeltQuoteOnchainResponse<String>),
    /// Custom payment method response
    Custom(cdk_common::MeltQuoteCustomResponse<String>),
}

impl MeltQuoteStatusResponse {
    /// Get the quote state
    pub fn state(&self) -> MeltQuoteState {
        match self {
            Self::Standard(r) => r.state,
            Self::Bolt12(r) => r.state,
            Self::Onchain(r) => r.state,
            Self::Custom(r) => r.state,
        }
    }

    /// Get the payment proof.
    ///
    /// For Bolt11/Bolt12/Custom methods this is the Lightning payment preimage.
    /// For Onchain, the "proof" is the broadcast outpoint (`txid:vout`) — it
    /// plays the same role: it is the canonical, method-specific artifact that
    /// proves the mint executed the payment. Callers that persist
    /// `payment_proof` on a `MeltQuote` will keep the txid reference alongside
    /// other methods' preimages.
    pub fn payment_proof(&self) -> Option<String> {
        match self {
            Self::Standard(r) => r.payment_preimage.clone(),
            Self::Bolt12(r) => r.payment_preimage.clone(),
            Self::Onchain(r) => r.outpoint.clone(),
            Self::Custom(r) => r.payment_preimage.clone(),
        }
    }

    /// Get the change signatures
    ///
    /// Onchain melts never return NUT-08 change outputs.
    pub fn change(&self) -> Option<Vec<crate::nuts::BlindSignature>> {
        match self {
            Self::Standard(r) => r.change.clone(),
            Self::Bolt12(r) => r.change.clone(),
            Self::Onchain(_) => None,
            Self::Custom(r) => r.change.clone(),
        }
    }

    /// Convert to standard response (for Bolt11).
    ///
    /// Also supports the Onchain variant by synthesizing a standard-shaped
    /// response: the broadcast outpoint (`txid:vout`) is used as the
    /// `payment_preimage` because onchain treats the outpoint as its
    /// payment proof (the on-wire artifact proving the mint executed the
    /// payment), analogous to the Lightning preimage. Returns error for
    /// Custom payment methods and Bolt12 (since their types differ
    /// meaningfully).
    pub fn into_standard(self) -> Result<cdk_common::MeltQuoteBolt11Response<String>, Error> {
        match self {
            Self::Standard(r) => Ok(r),
            Self::Onchain(r) => Ok(cdk_common::MeltQuoteBolt11Response {
                quote: r.quote,
                amount: r.amount,
                fee_reserve: r.fee,
                state: r.state,
                expiry: r.expiry,
                // Onchain uses `outpoint` as payment proof; surface it here
                // via the `payment_preimage` slot for parity with Bolt11/Bolt12.
                payment_preimage: r.outpoint,
                // Onchain melts never return NUT-08 change.
                change: None,
                request: Some(r.request),
                unit: Some(r.unit),
            }),
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
#[must_use = "must be confirmed or canceled; confirm auto-recovers reserved proofs on failure"]
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
    ///
    /// This method waits for the payment to complete and returns the finalized melt.
    /// If the mint supports async payments (NUT-05), this may complete faster by
    /// not blocking on the payment processing.
    ///
    /// If the confirm path fails before returning a [`FinalizedMelt`], the wallet
    /// runs melt saga recovery using the persisted saga state. If recovery shows
    /// the melt actually completed, this method still returns the recovered melt.
    /// Otherwise, the original confirm error or a recovery error is returned.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use std::collections::HashMap;
    /// # async fn example(wallet: &cdk::wallet::Wallet) -> anyhow::Result<()> {
    /// use cdk::nuts::PaymentMethod;
    ///
    /// let quote = wallet
    ///     .melt_quote(PaymentMethod::BOLT11, "lnbc...", None, None)
    ///     .await?;
    ///
    /// // Prepare the melt
    /// let prepared = wallet.prepare_melt(&quote.id, HashMap::new()).await?;
    ///
    /// // Confirm and wait for completion
    /// let finalized = prepared.confirm().await?;
    ///
    /// println!(
    ///     "Melt completed: state={:?}, amount={}, fee_paid={}",
    ///     finalized.state(),
    ///     finalized.amount(),
    ///     finalized.fee_paid()
    /// );
    /// # Ok(())
    /// # }
    /// ```
    pub async fn confirm(self) -> Result<FinalizedMelt, Error> {
        self.confirm_with_options(MeltConfirmOptions::default())
            .await
    }

    /// Confirm the prepared melt with custom options.
    ///
    /// This method waits for the payment to complete and returns the finalized melt.
    /// If the mint supports async payments (NUT-05), this may complete faster by
    /// not blocking on the payment processing.
    ///
    /// If the confirm path fails before returning a [`FinalizedMelt`], the wallet
    /// runs melt saga recovery using the persisted saga state so proofs do not
    /// remain stuck in an intermediate state. If recovery determines the melt
    /// actually completed, this method returns the recovered melt. Recovery
    /// errors are surfaced directly.
    pub async fn confirm_with_options(
        self,
        options: MeltConfirmOptions,
    ) -> Result<FinalizedMelt, Error> {
        self.saga
            .wallet
            .confirm_prepared_melt_with_options(
                self.saga.operation_id(),
                self.saga.quote().clone(),
                self.saga.proofs().clone(),
                self.saga.proofs_to_swap().clone(),
                self.saga.input_fee(),
                self.saga.input_fee_without_swap(),
                self.metadata,
                options,
            )
            .await
    }

    /// Confirm the prepared melt using async support (NUT-05).
    ///
    /// Sends the melt request with a `Prefer: respond-async` header and waits for the
    /// mint's response. Returns `Paid` if the payment completed immediately, or
    /// `Pending` if the mint accepted the async request and will process it in the
    /// background.
    ///
    /// Note: This waits for the mint's initial response, which may block if the mint
    /// does not support async payments. Only returns `Pending` if the mint explicitly
    /// supports and accepts async melt requests.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # async fn example(wallet: &cdk::wallet::Wallet) -> anyhow::Result<()> {
    /// use std::collections::HashMap;
    ///
    /// use cdk::nuts::PaymentMethod;
    /// use cdk::wallet::MeltOutcome;
    ///
    /// let quote = wallet
    ///     .melt_quote(PaymentMethod::BOLT11, "lnbc...", None, None)
    ///     .await?;
    ///
    /// // Prepare the melt
    /// let prepared = wallet.prepare_melt(&quote.id, HashMap::new()).await?;
    ///
    /// // Confirm with async preference
    /// match prepared.confirm_prefer_async().await? {
    ///     MeltOutcome::Paid(finalized) => {
    ///         println!(
    ///             "Melt completed immediately: state={:?}, amount={}, fee_paid={}",
    ///             finalized.state(),
    ///             finalized.amount(),
    ///             finalized.fee_paid()
    ///         );
    ///     }
    ///     MeltOutcome::Pending(pending) => {
    ///         // You can await the pending melt directly
    ///         let finalized = pending.await?;
    ///         println!(
    ///             "Melt completed after waiting: state={:?}, amount={}, fee_paid={}",
    ///             finalized.state(),
    ///             finalized.amount(),
    ///             finalized.fee_paid()
    ///         );
    ///
    ///         // Alternative: Instead of awaiting, you could:
    ///         // 1. Store the quote ID and check status later with:
    ///         //    wallet.check_melt_quote_status(&quote.id).await?
    ///         // 2. Let the wallet's background task handle it via:
    ///         //    wallet.finalize_pending_melts().await?
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn confirm_prefer_async(self) -> Result<MeltOutcome<'a>, Error> {
        self.confirm_prefer_async_with_options(MeltConfirmOptions::default())
            .await
    }

    /// Confirm with async support and custom options.
    ///
    /// Sends the melt request with a `Prefer: respond-async` header and waits for the
    /// mint's response. Returns `Paid` if the payment completed immediately, or
    /// `Pending` if the mint accepted the async request and will process it in the
    /// background.
    ///
    /// Note: This waits for the mint's initial response, which may block if the mint
    /// does not support async payments. Only returns `Pending` if the mint explicitly
    /// supports and accepts async melt requests.
    ///
    /// If confirm fails before returning a [`MeltOutcome`], this method runs melt
    /// saga recovery using the persisted saga state so proofs do not stay stuck
    /// in an intermediate state. If recovery determines the melt actually
    /// completed, this method returns `MeltOutcome::Paid`. Recovery errors are
    /// surfaced directly.
    pub async fn confirm_prefer_async_with_options(
        self,
        options: MeltConfirmOptions,
    ) -> Result<MeltOutcome<'a>, Error> {
        let operation_id = self.saga.operation_id();
        let wallet = self.saga.wallet;
        let metadata = self.metadata;

        let melt_requested = match self.saga.request_melt_with_options(options).await {
            Ok(melt_requested) => melt_requested,
            Err(err) => {
                let finalized = wallet
                    .recover_failed_melt_confirm(operation_id, err)
                    .await?;
                return Ok(MeltOutcome::Paid(finalized));
            }
        };

        let result = match melt_requested.execute_async(metadata.clone()).await {
            Ok(result) => result,
            Err(err) => {
                let finalized = wallet
                    .recover_failed_melt_confirm(operation_id, err)
                    .await?;
                return Ok(MeltOutcome::Paid(finalized));
            }
        };

        match result {
            MeltSagaResult::Finalized(finalized) => Ok(MeltOutcome::Paid(FinalizedMelt::new(
                finalized.quote_id().to_string(),
                finalized.state(),
                finalized.payment_proof().map(|s| s.to_string()),
                finalized.amount(),
                finalized.fee_paid(),
                finalized.into_change(),
            ))),
            MeltSagaResult::Pending(pending_saga) => Ok(MeltOutcome::Pending(PendingMelt {
                saga: pending_saga,
                metadata,
            })),
        }
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

    /// Prepare a melt operation from an encoded token.
    ///
    /// Decodes the token, validates unit and mint URL, extracts proofs,
    /// and delegates to [`prepare_melt_proofs`](Wallet::prepare_melt_proofs).
    #[instrument(skip(self, encoded_token, metadata))]
    pub async fn prepare_melt_token(
        &self,
        quote_id: &str,
        encoded_token: &str,
        metadata: HashMap<String, String>,
    ) -> Result<PreparedMelt<'_>, Error> {
        let token = Token::from_str(encoded_token)?;

        let unit = token.unit().unwrap_or_default();
        ensure_cdk!(unit == self.unit, Error::UnsupportedUnit);
        ensure_cdk!(self.mint_url == token.mint_url()?, Error::IncorrectMint);

        let keysets_info = self.load_mint_keysets().await?;
        println!("{:?}", keysets_info);
        let proofs = token.proofs(&keysets_info)?;

        self.prepare_melt_proofs(quote_id, proofs, metadata).await
    }

    /// Finalize pending melt operations.
    #[instrument(skip_all)]
    pub async fn finalize_pending_melts(&self) -> Result<Vec<FinalizedMelt>, Error> {
        use cdk_common::wallet::{MeltSagaState, WalletSagaState};

        let sagas = self.localstore.get_incomplete_sagas().await?;

        // Filter to only melt sagas for this wallet in states that need checking
        let melt_sagas: Vec<_> = sagas
            .into_iter()
            .filter(|s| {
                s.mint_url == self.mint_url
                    && s.unit == self.unit
                    && matches!(
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

    /// Internal method called by `PreparedMelt::confirm` with cached data.
    ///
    /// Not intended for direct use - use [`PreparedMelt::confirm`] instead.
    #[doc(hidden)]
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

    /// Internal method called by `PreparedMelt::confirm_with_options` with cached data.
    ///
    /// Not intended for direct use - use [`PreparedMelt::confirm_with_options`] instead.
    #[doc(hidden)]
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

        let melt_requested = match saga.request_melt_with_options(options).await {
            Ok(melt_requested) => melt_requested,
            Err(err) => return self.recover_failed_melt_confirm(operation_id, err).await,
        };

        let result = match melt_requested.execute_async(metadata.clone()).await {
            Ok(result) => result,
            Err(err) => return self.recover_failed_melt_confirm(operation_id, err).await,
        };

        match result {
            MeltSagaResult::Finalized(finalized) => Ok(FinalizedMelt::new(
                finalized.quote_id().to_string(),
                finalized.state(),
                finalized.payment_proof().map(|s| s.to_string()),
                finalized.amount(),
                finalized.fee_paid(),
                finalized.into_change(),
            )),
            MeltSagaResult::Pending(pending_saga) => {
                let pending = PendingMelt {
                    saga: pending_saga,
                    metadata,
                };
                pending.wait().await
            }
        }
    }

    /// Run melt recovery after a failed confirm path.
    ///
    /// This uses the persisted saga state as the source of truth, matching crash
    /// recovery semantics. If recovery proves the melt actually completed, the
    /// recovered [`FinalizedMelt`] is returned. If recovery compensates or leaves
    /// the saga pending, the original confirm error is returned. Recovery errors
    /// are surfaced directly so callers know cleanup did not complete.
    #[instrument(skip(self))]
    async fn recover_failed_melt_confirm(
        &self,
        operation_id: Uuid,
        original_err: Error,
    ) -> Result<FinalizedMelt, Error> {
        let saga = match self.localstore.get_saga(&operation_id).await? {
            Some(saga) => saga,
            None => return Err(original_err),
        };

        match self.resume_melt_saga(&saga).await? {
            Some(finalized) if finalized.state() == MeltQuoteState::Paid => {
                tracing::info!(
                    "Melt operation {} recovered to Paid after confirm error",
                    operation_id
                );
                Ok(finalized)
            }
            Some(_) | None => Err(original_err),
        }
    }

    /// Internal method called by `PreparedMelt::cancel` with cached data.
    ///
    /// Not intended for direct use - use [`PreparedMelt::cancel`] instead.
    #[doc(hidden)]
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
            let current = self.localstore.get_proofs_by_ys(all_ys).await?;
            let ys_to_revert: Vec<_> = current
                .into_iter()
                .filter(|proof| proof.state == State::Reserved || proof.state == State::Pending)
                .map(|proof| proof.y)
                .collect();

            if !ys_to_revert.is_empty() {
                self.localstore
                    .update_proofs_state(ys_to_revert, State::Unspent)
                    .await?;
            }
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
                q.unit == self.unit
                    && (q.state == MeltQuoteState::Pending
                        || (q.state == MeltQuoteState::Unpaid && q.expiry > unix_time()))
            })
            .collect())
    }

    /// Get pending melt quotes
    pub async fn get_pending_melt_quotes(&self) -> Result<Vec<MeltQuote>, Error> {
        let quotes = self.localstore.get_melt_quotes().await?;
        Ok(quotes
            .into_iter()
            .filter(|q| q.unit == self.unit && q.state == MeltQuoteState::Pending)
            .collect())
    }

    pub(crate) async fn add_transaction_for_pending_melt(
        &self,
        quote: &MeltQuote,
        new_state: MeltQuoteState,
        amount: Amount,
        change_amount: Option<Amount>,
        payment_proof: Option<String>,
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
                        payment_proof,
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
    /// 2. Falls back to Lightning address only if BIP353 resolution fails
    /// 3. If BIP353 resolves but does not contain a usable BOLT12 offer, it does NOT fall back
    /// 4. If the mint doesn't support Bolt12, it tries Lightning address directly
    ///
    /// The `network` parameter is forwarded to the BIP353 resolver for on-chain address
    /// validation in the resolved URI.
    #[cfg(all(feature = "bip353", feature = "wallet", not(target_arch = "wasm32")))]
    pub async fn melt_human_readable_quote(
        &self,
        address: &str,
        amount_msat: impl Into<crate::Amount>,
        network: bitcoin::Network,
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
            match self.melt_bip353_quote(address, amount, network).await {
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

    /// Get a melt quote for a human-readable address (alias for `melt_human_readable_quote`)
    #[cfg(all(feature = "bip353", feature = "wallet", not(target_arch = "wasm32")))]
    pub async fn melt_human_readable(
        &self,
        address: &str,
        amount_msat: impl Into<crate::Amount>,
        network: bitcoin::Network,
    ) -> Result<MeltQuote, Error> {
        self.melt_human_readable_quote(address, amount_msat, network)
            .await
    }

    /// Melt quote for all payment methods
    ///
    /// Accepts `Bolt11Invoice`, `Offer`, `String`, or `&str` for the request parameter.
    ///
    /// # Onchain
    ///
    /// The onchain payment method is **not** reachable through this generic
    /// entry point: onchain melt quotes require a payout `amount` (the address
    /// alone is insufficient) and the mint returns an array of candidate fee
    /// tiers that must be selected explicitly. Callers needing onchain should
    /// use [`Wallet::quote_onchain_melt_options`] to fetch the candidate quotes
    /// and [`Wallet::select_onchain_melt_quote`] to persist the chosen one.
    /// Invoking `melt_quote` with [`KnownMethod::Onchain`] returns
    /// [`Error::UnsupportedPaymentMethod`].
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
            PaymentMethod::Known(KnownMethod::Onchain) => {
                // Onchain cannot be dispatched generically: the generic
                // signature lacks an explicit `amount` and the protocol
                // returns an array of candidate quotes that must be selected
                // by the caller. See the doc-comment above.
                tracing::debug!(
                    "melt_quote called with onchain method; callers must use \
                     quote_onchain_melt_options + select_onchain_melt_quote"
                );
                Err(Error::UnsupportedPaymentMethod)
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
        payment_proof: Option<String>,
    ) -> Result<(), Error> {
        if let Err(e) = self
            .add_transaction_for_pending_melt(
                quote,
                new_state,
                amount,
                change_amount,
                payment_proof.clone(),
            )
            .await
        {
            tracing::error!("Failed to add transaction for pending melt: {}", e);
        }

        quote.state = new_state;
        quote.payment_proof = payment_proof;

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
                    fresh_quote.payment_proof = quote.payment_proof.clone();

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
                let response = self
                    .client
                    .get_melt_quote_status(quote.payment_method.clone(), quote_id)
                    .await?;
                let response = match response {
                    cdk_common::MeltQuoteResponse::Bolt11(response) => response,
                    _ => return Err(Error::InvalidPaymentMethod),
                };
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
                let response = self
                    .client
                    .get_melt_quote_status(quote.payment_method.clone(), quote_id)
                    .await?;
                let response = match response {
                    cdk_common::MeltQuoteResponse::Bolt12(response) => response,
                    _ => return Err(Error::InvalidPaymentMethod),
                };
                self.update_melt_quote_state(
                    &mut quote,
                    response.state,
                    response.amount,
                    response.change_amount(),
                    response.payment_preimage,
                )
                .await?;
            }
            PaymentMethod::Custom(_) => {
                let response = self
                    .client
                    .get_melt_quote_status(quote.payment_method.clone(), quote_id)
                    .await?;
                let response = match response {
                    cdk_common::MeltQuoteResponse::Custom((_, response)) => response,
                    _ => return Err(Error::InvalidPaymentMethod),
                };
                let change_amount = response
                    .change
                    .as_ref()
                    .and_then(|change| Amount::try_sum(change.iter().map(|sig| sig.amount)).ok());
                self.update_melt_quote_state(
                    &mut quote,
                    response.state,
                    response.amount,
                    change_amount,
                    response.payment_preimage,
                )
                .await?;
            }
            PaymentMethod::Known(KnownMethod::Onchain) => {
                let response = self
                    .client
                    .get_melt_quote_status(quote.payment_method.clone(), quote_id)
                    .await?;
                let response = match response {
                    cdk_common::MeltQuoteResponse::Onchain(response) => response,
                    _ => return Err(Error::InvalidPaymentMethod),
                };
                // Onchain melts never return NUT-08 change outputs.
                self.update_melt_quote_state(
                    &mut quote,
                    response.state,
                    response.amount,
                    None,
                    response.outpoint.clone(),
                )
                .await?;
                quote.estimated_blocks = Some(response.estimated_blocks);
                self.localstore.add_melt_quote(quote.clone()).await?;
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
        let response = self
            .client
            .get_melt_quote_status(quote.payment_method.clone(), quote_id)
            .await?;

        let response = match response {
            cdk_common::MeltQuoteResponse::Bolt11(r) => MeltQuoteStatusResponse::Standard(r),
            cdk_common::MeltQuoteResponse::Bolt12(r) => MeltQuoteStatusResponse::Bolt12(r),
            cdk_common::MeltQuoteResponse::Onchain(r) => MeltQuoteStatusResponse::Onchain(r),
            cdk_common::MeltQuoteResponse::Custom((_, r)) => MeltQuoteStatusResponse::Custom(r),
        };

        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::str::FromStr;
    use std::sync::Arc;

    use cdk_common::nuts::{CurrencyUnit, State};
    use cdk_common::Id;

    use super::*;
    use crate::wallet::saga::test_utils::{
        create_test_db, test_keyset_id, test_mint_url, test_proof_info,
    };
    use crate::wallet::test_utils::{
        create_test_wallet_with_mock, test_melt_quote, test_proof, MockMintConnector,
    };

    #[tokio::test]
    async fn test_cancel_prepared_melt_reverts_reserved_proofs() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let operation_id = uuid::Uuid::new_v4();

        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Reserved);
        let proof_y = proof_info.y;
        let proof = proof_info.proof.clone();
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;

        wallet
            .cancel_prepared_melt(operation_id, vec![proof], vec![])
            .await
            .unwrap();

        let stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].state, State::Unspent);
    }

    #[tokio::test]
    async fn test_cancel_prepared_melt_reverts_pending_proofs() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let operation_id = uuid::Uuid::new_v4();

        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Pending);
        let proof_y = proof_info.y;
        let proof = proof_info.proof.clone();
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;

        wallet
            .cancel_prepared_melt(operation_id, vec![proof], vec![])
            .await
            .unwrap();

        let stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].state, State::Unspent);
    }

    #[tokio::test]
    async fn test_cancel_prepared_melt_preserves_spent_proofs() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let operation_id = uuid::Uuid::new_v4();

        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Spent);
        let proof_y = proof_info.y;
        let proof = proof_info.proof.clone();
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;

        wallet
            .cancel_prepared_melt(operation_id, vec![], vec![proof])
            .await
            .unwrap();

        let stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].state, State::Spent);
    }

    #[tokio::test]
    async fn test_cancel_prepared_melt_mixed_states_only_reverts_reserved_and_pending() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let operation_id = uuid::Uuid::new_v4();

        let reserved = test_proof_info(keyset_id, 100, mint_url.clone(), State::Reserved);
        let pending = test_proof_info(keyset_id, 200, mint_url.clone(), State::Pending);
        let spent = test_proof_info(keyset_id, 300, mint_url.clone(), State::Spent);

        let reserved_y = reserved.y;
        let pending_y = pending.y;
        let spent_y = spent.y;

        let reserved_proof = reserved.proof.clone();
        let pending_proof = pending.proof.clone();
        let spent_proof = spent.proof.clone();

        db.update_proofs(vec![reserved, pending, spent], vec![])
            .await
            .unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;

        wallet
            .cancel_prepared_melt(
                operation_id,
                vec![reserved_proof, pending_proof],
                vec![spent_proof],
            )
            .await
            .unwrap();

        let stored = db
            .get_proofs_by_ys(vec![reserved_y, pending_y, spent_y])
            .await
            .unwrap();
        let state_for = |y| {
            stored
                .iter()
                .find(|proof| proof.y == y)
                .map(|proof| proof.state)
        };
        assert_eq!(state_for(reserved_y), Some(State::Unspent));
        assert_eq!(state_for(pending_y), Some(State::Unspent));
        assert_eq!(state_for(spent_y), Some(State::Spent));
    }

    async fn create_test_wallet_with_quote() -> (Wallet, String) {
        let db = create_test_db().await;
        let quote = test_melt_quote();
        let quote_id = quote.id.clone();
        db.add_melt_quote(quote).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.reset_default_mint_state();
        let wallet = create_test_wallet_with_mock(db, mock_client).await;

        (wallet, quote_id)
    }

    fn build_token(mint_url: cdk_common::mint_url::MintUrl, unit: CurrencyUnit) -> String {
        let proofs = vec![test_proof(test_keyset_id(), 1000)];
        Token::new(mint_url, proofs, None, unit).to_string()
    }

    #[tokio::test]
    async fn test_prepare_melt_token_rejects_wrong_unit() {
        let (wallet, quote_id) = create_test_wallet_with_quote().await;
        let encoded_token = build_token(test_mint_url(), CurrencyUnit::Usd);

        let result = wallet
            .prepare_melt_token(&quote_id, &encoded_token, HashMap::new())
            .await;

        assert!(matches!(result, Err(Error::UnsupportedUnit)));
    }

    #[tokio::test]
    async fn test_prepare_melt_token_rejects_wrong_mint() {
        let (wallet, quote_id) = create_test_wallet_with_quote().await;
        let encoded_token = build_token(
            cdk_common::mint_url::MintUrl::from_str("https://other-mint.example.com").unwrap(),
            CurrencyUnit::Sat,
        );

        let result = wallet
            .prepare_melt_token(&quote_id, &encoded_token, HashMap::new())
            .await;

        assert!(matches!(result, Err(Error::IncorrectMint)));
    }

    #[tokio::test]
    async fn test_prepare_melt_token_accepts_valid_token() {
        let db = create_test_db().await;
        let quote = test_melt_quote();
        let quote_id = quote.id.clone();
        db.add_melt_quote(quote).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.reset_default_mint_state();
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;

        let proof = test_proof(Id::from_str("0094d5a774c40a32").unwrap(), 1010);
        let encoded_token =
            Token::new(test_mint_url(), vec![proof], None, CurrencyUnit::Sat).to_string();

        let prepared = wallet
            .prepare_melt_token(&quote_id, &encoded_token, HashMap::new())
            .await
            .unwrap();

        let reserved = db
            .get_reserved_proofs(&prepared.operation_id())
            .await
            .unwrap();

        assert_eq!(reserved.len(), 1);
        assert_eq!(reserved[0].state, State::Reserved);
        assert_eq!(reserved[0].proof.amount, Amount::from(1010_u64));
    }
}
