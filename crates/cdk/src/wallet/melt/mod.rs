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
//!     prepared.total_fee()?
//! );
//!
//! // Either confirm or cancel
//! let confirmed = prepared.confirm().await?;
//! // Or: prepared.cancel().await?;
//! # Ok(())
//! # }
//! ```

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use cdk_common::util::unix_time;
use cdk_common::wallet::{
    CrossMintTransferOperation, CrossMintTransferQuote, MeltQuote, MeltSagaState, OperationData,
    PreparedMeltOperationData, PreparedMeltPurpose, Transaction, TransactionDirection,
    TransactionId, TransactionStatus, WalletSaga, WalletSagaState,
};
use cdk_common::{Error, MeltQuoteState, PaymentMethod, ProofsMethods, State};
use tracing::instrument;
use uuid::Uuid;

use crate::nuts::nut00::KnownMethod;
use crate::nuts::{MeltOptions, Proofs, Token};
use crate::types::FinalizedMelt;
use crate::wallet::recovery::recovery_is_deferred;
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

use saga::{MeltSaga, MeltSagaResult};

const CROSS_MINT_TRANSFER_KV_NAMESPACE: &str = "cdk_wallet";
const CROSS_MINT_TRANSFER_KV_SECONDARY_NAMESPACE: &str = "cross_mint_transfers";

/// Outcome of a melt operation using async support (NUT-05).
#[derive(Debug)]
pub enum MeltOutcome {
    /// Melt completed immediately
    Paid(FinalizedMelt),
    /// Melt is pending - can be awaited or dropped to poll elsewhere
    Pending(PendingMelt),
}

/// A durable pending melt operation that can be stored and awaited later.
#[derive(Debug, Clone)]
pub struct PendingMelt {
    wallet: Arc<Wallet>,
    operation_id: Uuid,
    quote_id: String,
    payment_method: PaymentMethod,
}

impl PendingMelt {
    /// Operation ID of the pending payment.
    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }

    /// Quote ID of the pending payment.
    pub fn quote_id(&self) -> &str {
        &self.quote_id
    }

    /// Payment method used by the pending quote.
    pub fn payment_method(&self) -> &PaymentMethod {
        &self.payment_method
    }

    /// Wait until the mint reports a terminal payment state.
    pub async fn wait(self) -> Result<FinalizedMelt, Error> {
        self.wallet.wait_pending_melt(self.operation_id).await
    }
}

impl IntoFuture for PendingMelt {
    type Output = Result<FinalizedMelt, Error>;

    #[cfg(not(target_arch = "wasm32"))]
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;

    #[cfg(target_arch = "wasm32")]
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output>>>;

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
                fee_reserve: r
                    .selected_fee_index
                    .and_then(|selected| {
                        r.fee_options
                            .iter()
                            .find(|option| option.fee_index == selected)
                    })
                    .or_else(|| r.fee_options.first())
                    .map(|option| option.fee_reserve)
                    .unwrap_or(Amount::ZERO),
                state: r.state,
                expiry: r.expiry,
                // Onchain uses `outpoint` as payment proof; surface it here
                // via the `payment_preimage` slot for parity with Bolt11/Bolt12.
                payment_preimage: r.outpoint,
                change: r.change,
                request: Some(r.request),
                unit: Some(r.unit),
                method: PaymentMethod::Known(KnownMethod::Onchain),
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
pub struct PreparedMelt {
    wallet: Wallet,
    operation_id: Uuid,
    plan: PreparedMeltOperationData,
}

impl PreparedMelt {
    fn from_saga(
        wallet: &Wallet,
        saga: MeltSaga<'_, saga::state::Prepared>,
    ) -> Result<Self, Error> {
        let operation_id = saga.operation_id();
        let OperationData::PreparedMelt(plan) = saga.state_data.saga.data.clone() else {
            return Err(Error::InvalidOperationState);
        };

        Ok(Self {
            wallet: wallet.clone(),
            operation_id,
            plan,
        })
    }

    /// Get the operation ID
    pub fn operation_id(&self) -> Uuid {
        self.operation_id
    }

    /// Get the quote
    pub fn quote(&self) -> &MeltQuote {
        &self.plan.quote
    }

    /// Get the amount to be melted
    pub fn amount(&self) -> Amount {
        self.plan.quote.amount
    }

    /// Get the proofs that will be used
    pub fn proofs(&self) -> &Proofs {
        &self.plan.proofs
    }

    /// Get the transaction metadata for this melt.
    pub fn metadata(&self) -> &HashMap<String, String> {
        &self.plan.metadata
    }

    /// Application-level purpose persisted with this operation.
    pub fn purpose(&self) -> &PreparedMeltPurpose {
        &self.plan.purpose
    }

    /// Get the proofs that need to be swapped
    pub fn proofs_to_swap(&self) -> &Proofs {
        &self.plan.proofs_to_swap
    }

    /// Get the swap fee
    pub fn swap_fee(&self) -> Amount {
        self.plan.swap_fee
    }

    /// Get the input fee
    pub fn input_fee(&self) -> Amount {
        self.plan.input_fee
    }

    /// Get the total fee (with swap, if applicable).
    pub fn total_fee(&self) -> Result<Amount, Error> {
        self.plan
            .swap_fee
            .checked_add(self.plan.input_fee)
            .ok_or(Error::AmountOverflow)
    }

    /// Returns true if a swap would be performed (proofs_to_swap is not empty)
    pub fn requires_swap(&self) -> bool {
        !self.plan.proofs_to_swap.is_empty()
    }

    /// Get the total fee if swap is performed (current default behavior).
    pub fn total_fee_with_swap(&self) -> Result<Amount, Error> {
        self.total_fee()
    }

    /// Get the input fee if swap is skipped (fee on all proofs sent directly)
    pub fn input_fee_without_swap(&self) -> Amount {
        self.plan.input_fee_without_swap
    }

    /// Get the fee savings from skipping the swap.
    pub fn fee_savings_without_swap(&self) -> Result<Amount, Error> {
        Ok(self
            .total_fee_with_swap()?
            .checked_sub(self.input_fee_without_swap())
            .unwrap_or(Amount::ZERO))
    }

    /// Get the expected change amount if swap is skipped.
    pub fn change_amount_without_swap(&self) -> Result<Amount, Error> {
        let all_proofs_total = self
            .plan
            .proofs
            .total_amount()?
            .checked_add(self.plan.proofs_to_swap.total_amount()?)
            .ok_or(Error::AmountOverflow)?;
        let quote = &self.plan.quote;
        let needed = quote
            .amount
            .checked_add(quote.fee_reserve)
            .and_then(|amount| amount.checked_add(self.input_fee_without_swap()))
            .ok_or(Error::AmountOverflow)?;
        Ok(all_proofs_total.checked_sub(needed).unwrap_or(Amount::ZERO))
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
        self.wallet
            .confirm_prepared_melt_with_options(self.operation_id, options)
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
    pub async fn confirm_prefer_async(self) -> Result<MeltOutcome, Error> {
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
    ) -> Result<MeltOutcome, Error> {
        self.wallet
            .confirm_prepared_melt_prefer_async_with_options(self.operation_id, options)
            .await
    }

    /// Cancel the prepared melt and release reserved proofs
    pub async fn cancel(self) -> Result<(), Error> {
        self.wallet.cancel_prepared_melt(self.operation_id).await
    }
}

impl Debug for PreparedMelt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedMelt")
            .field("operation_id", &self.operation_id)
            .field("quote_id", &self.plan.quote.id)
            .field("amount", &self.plan.quote.amount)
            .field("total_fee", &self.total_fee().ok())
            .finish()
    }
}

impl Wallet {
    /// Reconstruct a prepared melt from its durable operation ID.
    #[instrument(skip(self))]
    pub async fn prepared_melt(&self, operation_id: Uuid) -> Result<PreparedMelt, Error> {
        let saga = self
            .localstore
            .get_saga(&operation_id)
            .await?
            .ok_or(Error::OperationNotFound)?;
        self.ensure_melt_saga_state(&saga, MeltSagaState::Prepared)?;
        let OperationData::PreparedMelt(plan) = saga.data else {
            return Err(Error::InvalidOperationState);
        };

        Ok(PreparedMelt {
            wallet: self.clone(),
            operation_id,
            plan,
        })
    }

    /// Reconstruct a pending melt from its durable operation ID.
    #[instrument(skip(self))]
    pub async fn pending_melt(&self, operation_id: Uuid) -> Result<PendingMelt, Error> {
        let saga = self
            .localstore
            .get_saga(&operation_id)
            .await?
            .ok_or(Error::OperationNotFound)?;
        ensure_cdk!(
            saga.mint_url == self.mint_url && saga.unit == self.unit,
            Error::InvalidOperationState
        );
        ensure_cdk!(
            matches!(
                saga.state,
                WalletSagaState::Melt(MeltSagaState::MeltRequested | MeltSagaState::PaymentPending)
            ),
            Error::InvalidOperationState
        );
        let OperationData::Melt(data) = saga.data else {
            return Err(Error::InvalidOperationState);
        };
        let quote = self
            .localstore
            .get_melt_quote(&data.quote_id)
            .await?
            .ok_or(Error::UnknownQuote)?;

        Ok(PendingMelt {
            wallet: Arc::new(self.clone()),
            operation_id,
            quote_id: data.quote_id,
            payment_method: quote.payment_method,
        })
    }

    async fn persist_cross_mint_transfer_quote(
        &self,
        target_wallet: &Wallet,
        quote: CrossMintTransferQuote,
    ) -> Result<CrossMintTransferQuote, Error> {
        target_wallet
            .localstore
            .add_mint_quote(quote.mint_quote.clone())
            .await?;

        if let Err(error) = self
            .localstore
            .add_melt_quote(quote.melt_quote.clone())
            .await
        {
            if let Err(cleanup_error) = target_wallet
                .localstore
                .remove_mint_quote(&quote.mint_quote.id)
                .await
            {
                tracing::warn!(
                    "Failed to remove mint quote {} after melt quote persistence failed: {}",
                    quote.mint_quote.id,
                    cleanup_error
                );
            }

            return Err(Error::Database(error));
        }

        Ok(quote)
    }

    async fn persist_cross_mint_transfer_operation(
        &self,
        operation: &CrossMintTransferOperation,
    ) -> Result<(), Error> {
        let value = serde_json::to_vec(operation)?;
        self.localstore
            .kv_write(
                CROSS_MINT_TRANSFER_KV_NAMESPACE,
                CROSS_MINT_TRANSFER_KV_SECONDARY_NAMESPACE,
                &operation.operation_id.to_string(),
                &value,
            )
            .await?;
        Ok(())
    }

    /// Load a durable cross-mint transfer owned by this source wallet.
    #[instrument(skip(self))]
    pub async fn cross_mint_transfer_operation(
        &self,
        operation_id: Uuid,
    ) -> Result<Option<CrossMintTransferOperation>, Error> {
        let Some(value) = self
            .localstore
            .kv_read(
                CROSS_MINT_TRANSFER_KV_NAMESPACE,
                CROSS_MINT_TRANSFER_KV_SECONDARY_NAMESPACE,
                &operation_id.to_string(),
            )
            .await?
        else {
            return Ok(None);
        };
        let operation: CrossMintTransferOperation = serde_json::from_slice(&value)?;
        ensure_cdk!(
            operation.operation_id == operation_id,
            Error::InvalidOperationState
        );
        if operation.source_mint_url != self.mint_url || operation.source_unit != self.unit {
            return Ok(None);
        }
        Ok(Some(operation))
    }

    /// Remove a canceled cross-mint transfer record.
    #[instrument(skip(self))]
    pub async fn remove_cross_mint_transfer_operation(
        &self,
        operation_id: Uuid,
    ) -> Result<(), Error> {
        self.localstore
            .kv_remove(
                CROSS_MINT_TRANSFER_KV_NAMESPACE,
                CROSS_MINT_TRANSFER_KV_SECONDARY_NAMESPACE,
                &operation_id.to_string(),
            )
            .await?;
        Ok(())
    }

    /// Create quotes for transferring the maximum amount allowed by the source
    /// balance and both mints' advertised BOLT11 limits.
    ///
    /// The destination wallet creates a BOLT11 mint quote and this wallet asks
    /// its mint for the corresponding melt quote. Because the melt fee reserve
    /// can depend on the invoice amount, the method repeats this process until
    /// the destination amount fits the source balance after both the melt fee
    /// reserve and the input fee for all unspent source proofs.
    ///
    /// The returned plan assumes all currently unspent source proofs are used.
    /// To preserve that accounting, prepare the melt with
    /// [`Wallet::prepare_melt_proofs`] and confirm it with
    /// [`MeltConfirmOptions::skip_swap`].
    ///
    /// The melt fee is a reserve. If the actual Lightning fee is lower, the
    /// source mint can return the difference as change, so a successful melt
    /// may leave a small source balance.
    ///
    /// The returned plan covers one Lightning payment. If an advertised quote
    /// maximum is below the source balance, complete the plan and call this
    /// method again to transfer the remaining balance.
    ///
    /// # Remote side effects
    ///
    /// Finding the maximum can require several probes because the source mint's
    /// fee reserve is only known after the destination mint creates an invoice.
    /// Every probe creates a mint quote at the destination and a melt quote at
    /// the source. Only the returned pair is persisted locally; the other remote
    /// quotes cannot be cancelled and remain at their mints until they expire.
    /// Avoid calling this method speculatively or in an unbounded loop.
    #[instrument(skip_all)]
    pub async fn cross_mint_transfer_quote_max(
        &self,
        target_wallet: &Wallet,
    ) -> Result<CrossMintTransferQuote, Error> {
        const MAX_QUOTE_ATTEMPTS: usize = u64::BITS as usize;

        ensure_cdk!(self.unit == target_wallet.unit, Error::UnsupportedUnit);
        ensure_cdk!(
            self.mint_url != target_wallet.mint_url,
            Error::InvalidOperationState
        );

        let proofs = self.get_unspent_proofs().await?;
        let balance = proofs.total_amount()?;
        ensure_cdk!(balance > Amount::ZERO, Error::InsufficientFunds);

        let input_fee = self.get_proofs_fee(&proofs).await?.total;
        let available_for_payment = balance
            .checked_sub(input_fee)
            .ok_or(Error::InsufficientFunds)?;
        ensure_cdk!(
            available_for_payment > Amount::ZERO,
            Error::InsufficientFunds
        );

        let method = PaymentMethod::Known(KnownMethod::Bolt11);
        let source_mint_info = self.load_mint_info().await?;
        let target_mint_info = target_wallet.load_mint_info().await?;
        let source_settings = source_mint_info
            .nuts
            .nut05
            .get_settings(&self.unit, &method);
        let target_settings = target_mint_info
            .nuts
            .nut04
            .get_settings(&target_wallet.unit, &method);
        let minimum_amount = source_settings
            .as_ref()
            .and_then(|settings| settings.min_amount)
            .into_iter()
            .chain(
                target_settings
                    .as_ref()
                    .and_then(|settings| settings.min_amount),
            )
            .max()
            .unwrap_or(Amount::ONE)
            .max(Amount::ONE);
        let maximum_amount = source_settings
            .as_ref()
            .and_then(|settings| settings.max_amount)
            .into_iter()
            .chain(
                target_settings
                    .as_ref()
                    .and_then(|settings| settings.max_amount),
            )
            .fold(available_for_payment, Amount::min);
        ensure_cdk!(maximum_amount >= minimum_amount, Error::InsufficientFunds);

        let mut amount = maximum_amount;
        let mut attempted_amounts = HashSet::new();
        let mut best_feasible: Option<CrossMintTransferQuote> = None;
        let mut lowest_infeasible: Option<Amount> = None;

        for _ in 0..MAX_QUOTE_ATTEMPTS {
            attempted_amounts.insert(amount);

            let mint_quote = target_wallet
                .request_mint_quote(method.clone(), Some(amount), None, None)
                .await?;
            let melt_quote = self
                .request_melt_bolt11_quote(mint_quote.request.clone(), None)
                .await?;

            tracing::debug!(
                attempt = attempted_amounts.len(),
                amount = %amount,
                destination_mint_quote_id = %mint_quote.id,
                source_melt_quote_id = %melt_quote.id,
                "Created remote quote pair while searching for maximum cross-mint transfer"
            );

            let total_required = melt_quote
                .amount
                .checked_add(melt_quote.fee_reserve)
                .and_then(|required| required.checked_add(input_fee))
                .ok_or(Error::AmountOverflow)?;
            let plan = CrossMintTransferQuote {
                mint_quote,
                melt_quote,
                input_fee,
            };

            match total_required.cmp(&balance) {
                std::cmp::Ordering::Equal => {
                    return self
                        .persist_cross_mint_transfer_quote(target_wallet, plan)
                        .await;
                }
                std::cmp::Ordering::Less => {
                    if best_feasible
                        .as_ref()
                        .is_none_or(|best| plan.melt_quote.amount > best.melt_quote.amount)
                    {
                        best_feasible = Some(plan.clone());
                    }
                }
                std::cmp::Ordering::Greater => {
                    lowest_infeasible = Some(
                        lowest_infeasible
                            .map(|current| current.min(amount))
                            .unwrap_or(amount),
                    );
                }
            }

            if total_required <= balance && amount == maximum_amount {
                return self
                    .persist_cross_mint_transfer_quote(target_wallet, plan)
                    .await;
            }

            let estimated_amount = available_for_payment.checked_sub(plan.melt_quote.fee_reserve);
            let mut next_amount = estimated_amount
                .unwrap_or_else(|| Amount::from(amount.to_u64() / 2))
                .clamp(minimum_amount, maximum_amount);
            if let (Some(best), Some(upper)) = (&best_feasible, lowest_infeasible) {
                let lower = best.melt_quote.amount;
                let lower_value = lower.to_u64();
                let upper_value = upper.to_u64();
                if upper_value.saturating_sub(lower_value) <= 1 {
                    return self
                        .persist_cross_mint_transfer_quote(target_wallet, best.clone())
                        .await;
                }

                if next_amount <= lower
                    || next_amount >= upper
                    || attempted_amounts.contains(&next_amount)
                {
                    next_amount = Amount::from(lower_value + (upper_value - lower_value) / 2);
                }
            }

            ensure_cdk!(next_amount > Amount::ZERO, Error::InsufficientFunds);

            if attempted_amounts.contains(&next_amount) {
                return match best_feasible {
                    Some(best) => {
                        self.persist_cross_mint_transfer_quote(target_wallet, best)
                            .await
                    }
                    None => Err(Error::InsufficientFunds),
                };
            }

            amount = next_amount;
        }

        match best_feasible {
            Some(best) => {
                self.persist_cross_mint_transfer_quote(target_wallet, best)
                    .await
            }
            None => Err(Error::InsufficientFunds),
        }
    }

    /// Create a reviewable, durable plan that transfers the maximum available
    /// balance to another mint wallet using Lightning.
    #[instrument(skip_all)]
    pub async fn prepare_cross_mint_transfer(
        &self,
        target_wallet: &Wallet,
        metadata: HashMap<String, String>,
    ) -> Result<PreparedMelt, Error> {
        let transfer = self.cross_mint_transfer_quote_max(target_wallet).await?;
        let proofs = self.get_unspent_proofs().await?;
        let purpose = PreparedMeltPurpose::CrossMintTransfer {
            destination_mint_url: target_wallet.mint_url.clone(),
            destination_unit: target_wallet.unit.clone(),
            destination_quote_id: transfer.mint_quote.id.clone(),
        };
        let saga = MeltSaga::new(self);
        let operation_id = saga.operation_id();
        let saga = match saga
            .prepare_with_proofs_for(&transfer.melt_quote.id, proofs, metadata, purpose)
            .await
        {
            Ok(saga) => saga,
            Err(error) => {
                return Err(self.melt_preparation_error(operation_id, error).await);
            }
        };

        let prepared = match PreparedMelt::from_saga(self, saga) {
            Ok(prepared) => prepared,
            Err(error) => {
                return Err(self.melt_preparation_error(operation_id, error).await);
            }
        };
        let Some(maximum_fee) = prepared
            .quote()
            .fee_reserve
            .checked_add(prepared.input_fee_without_swap())
        else {
            if let Err(cleanup_error) = self.cancel_prepared_melt(operation_id).await {
                tracing::warn!(
                    "Could not cancel cross-mint transfer {} after fee overflow: {}",
                    operation_id,
                    cleanup_error
                );
            }
            return Err(Error::AmountOverflow);
        };
        let operation = CrossMintTransferOperation {
            operation_id,
            source_mint_url: self.mint_url.clone(),
            source_unit: self.unit.clone(),
            destination_mint_url: target_wallet.mint_url.clone(),
            destination_unit: target_wallet.unit.clone(),
            destination_quote_id: transfer.mint_quote.id,
            amount: prepared.amount(),
            maximum_fee,
        };
        if let Err(error) = self.persist_cross_mint_transfer_operation(&operation).await {
            if let Err(cleanup_error) = self.cancel_prepared_melt(operation_id).await {
                tracing::warn!(
                    "Could not cancel cross-mint transfer {} after persistence failed: {}",
                    operation_id,
                    cleanup_error
                );
            }
            return Err(error);
        }

        Ok(prepared)
    }

    async fn cleanup_failed_melt_preparation(&self, operation_id: Uuid) -> Result<(), Error> {
        // Keep the saga whenever either reservation cannot be released. A
        // partial cleanup with no durable operation would strand funds.
        self.localstore.release_proofs(&operation_id).await?;
        self.localstore.release_melt_quote(&operation_id).await?;
        self.localstore.delete_saga(&operation_id).await?;
        Ok(())
    }

    async fn melt_preparation_error(&self, operation_id: Uuid, error: Error) -> Error {
        match self.cleanup_failed_melt_preparation(operation_id).await {
            Ok(()) => error,
            Err(cleanup_error) => {
                tracing::error!(
                    "Melt preparation {} failed ({}), and cleanup also failed: {}",
                    operation_id,
                    error,
                    cleanup_error
                );
                cleanup_error
            }
        }
    }

    fn ensure_melt_saga_state(
        &self,
        saga: &WalletSaga,
        expected: MeltSagaState,
    ) -> Result<(), Error> {
        ensure_cdk!(
            saga.mint_url == self.mint_url && saga.unit == self.unit,
            Error::InvalidOperationState
        );

        let WalletSagaState::Melt(state) = saga.state else {
            return Err(Error::InvalidOperationState);
        };

        ensure_cdk!(state == expected, Error::InvalidOperationState);
        Ok(())
    }

    /// Prepare a melt operation without executing it.
    #[instrument(skip(self, metadata))]
    pub async fn prepare_melt(
        &self,
        quote_id: &str,
        metadata: HashMap<String, String>,
    ) -> Result<PreparedMelt, Error> {
        let saga = MeltSaga::new(self);
        let operation_id = saga.operation_id();
        let prepared_saga = match saga.prepare(quote_id, metadata).await {
            Ok(saga) => saga,
            Err(error) => {
                return Err(self.melt_preparation_error(operation_id, error).await);
            }
        };

        match PreparedMelt::from_saga(self, prepared_saga) {
            Ok(prepared) => Ok(prepared),
            Err(error) => Err(self.melt_preparation_error(operation_id, error).await),
        }
    }

    /// Prepare a melt operation with specific proofs.
    #[instrument(skip(self, proofs, metadata))]
    pub async fn prepare_melt_proofs(
        &self,
        quote_id: &str,
        proofs: crate::nuts::Proofs,
        metadata: HashMap<String, String>,
    ) -> Result<PreparedMelt, Error> {
        let saga = MeltSaga::new(self);
        let operation_id = saga.operation_id();
        let prepared_saga = match saga.prepare_with_proofs(quote_id, proofs, metadata).await {
            Ok(saga) => saga,
            Err(error) => {
                return Err(self.melt_preparation_error(operation_id, error).await);
            }
        };

        match PreparedMelt::from_saga(self, prepared_saga) {
            Ok(prepared) => Ok(prepared),
            Err(error) => Err(self.melt_preparation_error(operation_id, error).await),
        }
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
    ) -> Result<PreparedMelt, Error> {
        let token = Token::from_str(encoded_token)?;

        let unit = token.unit().unwrap_or_default();
        ensure_cdk!(unit == self.unit, Error::UnsupportedUnit);
        ensure_cdk!(self.mint_url == token.mint_url()?, Error::IncorrectMint);

        let proofs = self.token_proofs(&token).await?;

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

        for discovered_saga in melt_sagas {
            let _operation_guard = self.lock_operation(discovered_saga.id).await;
            let Some(saga) = self.localstore.get_saga(&discovered_saga.id).await? else {
                continue;
            };
            if recovery_is_deferred(&saga) {
                tracing::debug!(
                    "Melt {} was updated recently; deferring finalization while its operation lease is active",
                    saga.id
                );
                continue;
            }
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

    /// Confirm a prepared melt identified by its durable operation ID.
    ///
    /// Not intended for direct use - use [`PreparedMelt::confirm`] instead.
    #[doc(hidden)]
    #[instrument(skip(self))]
    pub async fn confirm_prepared_melt(&self, operation_id: Uuid) -> Result<FinalizedMelt, Error> {
        self.confirm_prepared_melt_with_options(operation_id, MeltConfirmOptions::default())
            .await
    }

    /// Confirm a prepared melt using its persisted operation plan.
    #[doc(hidden)]
    #[instrument(skip(self, options))]
    pub async fn confirm_prepared_melt_with_options(
        &self,
        operation_id: Uuid,
        options: MeltConfirmOptions,
    ) -> Result<FinalizedMelt, Error> {
        let operation_guard = self.lock_operation(operation_id).await;
        let db_saga = match self.localstore.get_saga(&operation_id).await? {
            Some(saga) => saga,
            None => {
                return self
                    .completed_melt_from_transaction(operation_id)
                    .await?
                    .ok_or(Error::OperationNotFound)
            }
        };
        self.ensure_melt_saga_state(&db_saga, MeltSagaState::Prepared)?;
        let OperationData::PreparedMelt(plan) = db_saga.data.clone() else {
            return Err(Error::InvalidOperationState);
        };
        let metadata = plan.metadata.clone();

        let saga = MeltSaga::from_prepared(
            self,
            operation_id,
            plan.quote,
            plan.proofs,
            plan.proofs_to_swap,
            plan.input_fee,
            db_saga,
        );

        let melt_requested = match saga.request_melt_with_options(options).await {
            Ok(melt_requested) => melt_requested,
            Err(Error::ConcurrentUpdate) => return Err(Error::ConcurrentUpdate),
            Err(err) => return self.recover_failed_melt_confirm(operation_id, err).await,
        };

        let result = match melt_requested.execute_async(metadata.clone()).await {
            Ok(result) => result,
            Err(Error::ConcurrentUpdate) => return Err(Error::ConcurrentUpdate),
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
                let quote = pending_saga.quote().clone();
                let pending = PendingMelt {
                    wallet: Arc::new(self.clone()),
                    operation_id: pending_saga.state_data.operation_id,
                    quote_id: quote.id,
                    payment_method: quote.payment_method,
                };
                drop(operation_guard);
                pending.wait().await
            }
        }
    }

    /// Confirm a prepared melt with async preference using persisted state.
    #[doc(hidden)]
    #[instrument(skip(self, options))]
    pub async fn confirm_prepared_melt_prefer_async_with_options(
        &self,
        operation_id: Uuid,
        options: MeltConfirmOptions,
    ) -> Result<MeltOutcome, Error> {
        let _operation_guard = self.lock_operation(operation_id).await;
        let db_saga = match self.localstore.get_saga(&operation_id).await? {
            Some(saga) => saga,
            None => {
                return self
                    .completed_melt_from_transaction(operation_id)
                    .await?
                    .map(MeltOutcome::Paid)
                    .ok_or(Error::OperationNotFound)
            }
        };
        self.ensure_melt_saga_state(&db_saga, MeltSagaState::Prepared)?;
        let OperationData::PreparedMelt(plan) = db_saga.data.clone() else {
            return Err(Error::InvalidOperationState);
        };
        let metadata = plan.metadata.clone();

        let saga = MeltSaga::from_prepared(
            self,
            operation_id,
            plan.quote,
            plan.proofs,
            plan.proofs_to_swap,
            plan.input_fee,
            db_saga,
        );

        let melt_requested = match saga.request_melt_with_options(options).await {
            Ok(melt_requested) => melt_requested,
            Err(Error::ConcurrentUpdate) => return Err(Error::ConcurrentUpdate),
            Err(err) => {
                let finalized = self.recover_failed_melt_confirm(operation_id, err).await?;
                return Ok(MeltOutcome::Paid(finalized));
            }
        };

        let result = match melt_requested.execute_async(metadata.clone()).await {
            Ok(result) => result,
            Err(Error::ConcurrentUpdate) => return Err(Error::ConcurrentUpdate),
            Err(err) => {
                let finalized = self.recover_failed_melt_confirm(operation_id, err).await?;
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
            MeltSagaResult::Pending(pending_saga) => {
                let quote = pending_saga.quote().clone();
                Ok(MeltOutcome::Pending(PendingMelt {
                    wallet: Arc::new(self.clone()),
                    operation_id: pending_saga.state_data.operation_id,
                    quote_id: quote.id,
                    payment_method: quote.payment_method,
                }))
            }
        }
    }

    /// Wait for a pending melt identified by persisted saga details.
    ///
    /// Uses mint notifications when available and bounded polling as a fallback.
    /// The handle can be dropped and reconstructed by operation ID.
    #[doc(hidden)]
    #[instrument(skip(self))]
    pub async fn wait_pending_melt(&self, operation_id: Uuid) -> Result<FinalizedMelt, Error> {
        use cdk_common::wallet::{MeltSagaState, OperationData, WalletSagaState};

        const MAX_RECONCILE_INTERVAL: Duration = Duration::from_secs(30);

        let pending = match self.pending_melt(operation_id).await {
            Ok(pending) => pending,
            Err(Error::OperationNotFound) => {
                return self
                    .completed_melt_from_transaction(operation_id)
                    .await?
                    .ok_or(Error::OperationNotFound)
            }
            Err(error) => return Err(error),
        };
        let quote_id = pending.quote_id;
        let payment_method = pending.payment_method;
        let mut subscription = match self
            .subscribe_pending_melt(&quote_id, &payment_method)
            .await
        {
            Ok(subscription) => Some(subscription),
            Err(error) => {
                tracing::warn!(
                    "Could not subscribe to pending melt {}: {}; using polling fallback",
                    operation_id,
                    error
                );
                None
            }
        };
        let mut retry_delay = Duration::from_secs(1);

        loop {
            let operation_guard = self.lock_operation(operation_id).await;
            let db_saga = match self.localstore.get_saga(&operation_id).await? {
                Some(saga) => saga,
                None => {
                    return self
                        .completed_melt_from_transaction(operation_id)
                        .await?
                        .ok_or(Error::OperationNotFound);
                }
            };

            ensure_cdk!(
                db_saga.mint_url == self.mint_url && db_saga.unit == self.unit,
                Error::Custom("Saga belongs to a different wallet".to_string())
            );

            let WalletSagaState::Melt(state) = &db_saga.state else {
                return Err(Error::Custom(format!(
                    "Invalid saga state type for melt saga {}",
                    operation_id
                )));
            };

            ensure_cdk!(
                matches!(
                    state,
                    MeltSagaState::MeltRequested | MeltSagaState::PaymentPending
                ),
                Error::InvalidOperationState
            );

            let OperationData::Melt(data) = &db_saga.data else {
                return Err(Error::Custom(format!(
                    "Invalid operation data type for melt saga {}",
                    operation_id
                )));
            };

            ensure_cdk!(data.quote_id == quote_id, Error::InvalidOperationState);

            let quote = self
                .localstore
                .get_melt_quote(&data.quote_id)
                .await?
                .ok_or(Error::UnknownQuote)?;

            ensure_cdk!(
                quote
                    .mint_url
                    .as_ref()
                    .is_none_or(|mint_url| mint_url == &self.mint_url)
                    && quote.unit == self.unit,
                Error::InvalidOperationState
            );
            ensure_cdk!(
                quote.payment_method == payment_method,
                Error::InvalidOperationState
            );

            if recovery_is_deferred(&db_saga) {
                tracing::debug!(
                    "Melt {} was updated recently; waiting for the active request before reconciliation",
                    operation_id
                );
            } else {
                match self.resume_melt_saga(&db_saga).await? {
                    Some(finalized) if finalized.state() == MeltQuoteState::Paid => {
                        return Ok(finalized);
                    }
                    Some(_) => return Err(Error::PaymentFailed),
                    None => {
                        if let Some(finalized) =
                            self.completed_melt_from_transaction(operation_id).await?
                        {
                            return Ok(finalized);
                        }
                    }
                }
            }

            drop(operation_guard);

            match subscription.as_mut() {
                Some(active) => {
                    match tokio::time::timeout(MAX_RECONCILE_INTERVAL, active.recv()).await {
                        Ok(Some(_)) => retry_delay = Duration::from_secs(1),
                        Ok(None) => subscription = None,
                        Err(_) => {}
                    }
                }
                None => {
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = retry_delay.saturating_mul(2).min(MAX_RECONCILE_INTERVAL);
                    subscription = self
                        .subscribe_pending_melt(&quote_id, &payment_method)
                        .await
                        .ok();
                }
            }
        }
    }

    async fn subscribe_pending_melt(
        &self,
        quote_id: &str,
        payment_method: &PaymentMethod,
    ) -> Result<crate::wallet::subscription::ActiveSubscription, Error> {
        let subscription = match payment_method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                WalletSubscription::Bolt11MeltQuoteState(vec![quote_id.to_owned()])
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                WalletSubscription::Bolt12MeltQuoteState(vec![quote_id.to_owned()])
            }
            PaymentMethod::Known(KnownMethod::Onchain) => {
                WalletSubscription::MeltQuoteOnchainState(vec![quote_id.to_owned()])
            }
            PaymentMethod::Custom(method) => {
                WalletSubscription::MeltQuoteCustom(method.to_string(), vec![quote_id.to_owned()])
            }
        };
        self.subscribe(subscription).await
    }

    async fn completed_melt_from_transaction(
        &self,
        operation_id: Uuid,
    ) -> Result<Option<FinalizedMelt>, Error> {
        let transaction_id = TransactionId::from_saga_id(operation_id);
        let Some(transaction) = self.localstore.get_transaction(transaction_id).await? else {
            return Ok(None);
        };
        if transaction.mint_url != self.mint_url
            || transaction.unit != self.unit
            || transaction.direction != TransactionDirection::Outgoing
            || transaction.status != TransactionStatus::Completed
        {
            return Ok(None);
        }
        let Some(quote_id) = transaction.quote_id else {
            return Ok(None);
        };

        Ok(Some(FinalizedMelt::new(
            quote_id,
            MeltQuoteState::Paid,
            transaction.payment_proof,
            transaction.amount,
            transaction.fee,
            None,
        )))
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

    /// Cancel a prepared melt identified by its durable operation ID.
    #[doc(hidden)]
    #[instrument(skip(self))]
    pub async fn cancel_prepared_melt(&self, operation_id: Uuid) -> Result<(), Error> {
        tracing::info!("Cancelling prepared melt for operation {}", operation_id);

        let _operation_guard = self.lock_operation(operation_id).await;

        let db_saga = self
            .localstore
            .get_saga(&operation_id)
            .await?
            .ok_or(Error::OperationNotFound)?;
        self.ensure_melt_saga_state(&db_saga, MeltSagaState::Prepared)?;
        let OperationData::PreparedMelt(plan) = &db_saga.data else {
            return Err(Error::InvalidOperationState);
        };
        let is_cross_mint_transfer =
            matches!(&plan.purpose, PreparedMeltPurpose::CrossMintTransfer { .. });

        let mut claimed_saga = db_saga.clone();
        claimed_saga.update_state(WalletSagaState::Melt(MeltSagaState::ProofsReserved));
        if !self.localstore.update_saga(claimed_saga).await? {
            return Err(Error::ConcurrentUpdate);
        }

        self.localstore.release_proofs(&operation_id).await?;

        // Keep the claimed saga if quote release fails so recovery can retry
        // instead of leaving an invisible orphaned reservation.
        self.localstore.release_melt_quote(&operation_id).await?;

        if is_cross_mint_transfer {
            self.remove_cross_mint_transfer_operation(operation_id)
                .await?;
        }
        self.localstore.delete_saga(&operation_id).await?;
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
                let Some(operation_id_str) = quote.used_by_operation.as_deref() else {
                    tracing::warn!(
                        "Skipping transaction for paid melt quote {} without operation id",
                        quote.id
                    );
                    return Ok(());
                };
                let operation_id = match Uuid::parse_str(operation_id_str) {
                    Ok(operation_id) => operation_id,
                    Err(err) => {
                        tracing::warn!(
                            "Skipping transaction for paid melt quote {} with invalid operation id {}: {}",
                            quote.id,
                            operation_id_str,
                            err
                        );
                        return Ok(());
                    }
                };
                let metadata = match self.localstore.get_saga(&operation_id).await? {
                    Some(saga) => match saga.data {
                        OperationData::Melt(data) => data.metadata,
                        _ => {
                            tracing::warn!(
                                "Skipping transaction metadata for paid melt quote {} with non-melt saga {}",
                                quote.id,
                                operation_id
                            );
                            HashMap::new()
                        }
                    },
                    None => {
                        tracing::warn!(
                            "Recording transaction for paid melt quote {} without saga metadata; saga {} not found",
                            quote.id,
                            operation_id
                        );
                        HashMap::new()
                    }
                };
                let pending_proofs: Proofs = self
                    .localstore
                    .get_reserved_proofs(&operation_id)
                    .await?
                    .into_iter()
                    .filter(|proof| proof.state == State::Pending)
                    .map(|proof| proof.proof)
                    .collect();
                let proofs_total = pending_proofs.total_amount()?;
                let change_total = change_amount.unwrap_or_default();
                let fee = proofs_total
                    .checked_sub(amount)
                    .and_then(|amount| amount.checked_sub(change_total))
                    .ok_or(Error::AmountOverflow)?;

                self.upsert_transaction(Transaction {
                    mint_url: self.mint_url.clone(),
                    direction: TransactionDirection::Outgoing,
                    amount,
                    fee,
                    unit: quote.unit.clone(),
                    ys: pending_proofs.ys()?,
                    timestamp: unix_time(),
                    memo: None,
                    metadata,
                    quote_id: Some(quote.id.clone()),
                    payment_request: Some(quote.request.clone()),
                    payment_proof,
                    payment_method: Some(quote.payment_method.clone()),
                    saga_id: Some(operation_id),
                    status: TransactionStatus::Completed,
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
            .load(&self.localstore, &self.client)
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
                let _operation_guard = self.lock_operation(operation_id).await;
                match self.localstore.get_saga(&operation_id).await {
                    Ok(Some(saga)) => {
                        if recovery_is_deferred(&saga) {
                            tracing::info!(
                                "Melt quote {} has active saga {}; deferring status recovery",
                                quote_id,
                                operation_id
                            );
                            return self
                                .localstore
                                .get_melt_quote(quote_id)
                                .await?
                                .ok_or(Error::UnknownQuote);
                        }

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
                    .map(|change| Amount::try_sum(change.iter().map(|sig| sig.amount)))
                    .transpose()?;
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
                let change_amount = response
                    .change
                    .as_ref()
                    .map(|change| Amount::try_sum(change.iter().map(|sig| sig.amount)))
                    .transpose()?;
                self.update_melt_quote_state(
                    &mut quote,
                    response.state,
                    response.amount,
                    change_amount,
                    response.outpoint.clone(),
                )
                .await?;
                quote.fee_index = response
                    .selected_fee_index
                    .or_else(|| response.fee_options.first().map(|option| option.fee_index));
                quote.estimated_blocks = response
                    .selected_fee_index
                    .and_then(|selected| {
                        response
                            .fee_options
                            .iter()
                            .find(|option| option.fee_index == selected)
                    })
                    .or_else(|| response.fee_options.first())
                    .map(|option| option.estimated_blocks);
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

    use bitcoin::hashes::sha256::Hash as Sha256Hash;
    use bitcoin::hashes::Hash;
    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    use cdk_common::nut23::QuoteState;
    use cdk_common::nuts::{CurrencyUnit, KeySet, MintQuoteBolt11Response, RestoreResponse, State};
    use cdk_common::wallet::{
        MeltOperationData, MeltSagaState, OperationData, WalletSaga, WalletSagaState,
    };
    use cdk_common::{
        Id, MeltQuoteBolt11Response, MeltQuoteCreateResponse, MeltQuoteResponse, MintQuoteRequest,
        MintQuoteResponse,
    };
    use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};

    use super::*;
    use crate::wallet::saga::test_utils::{
        create_test_db, test_keyset_id, test_mint_url, test_proof_info,
    };
    use crate::wallet::test_utils::{
        create_test_wallet_with_mock, create_test_wallet_with_mock_http_subscription,
        make_inactive_keyset, test_keyset, test_melt_quote, test_mint_info, test_proof,
        MockMintConnector,
    };

    type TestWalletDatabase =
        Arc<dyn cdk_common::database::WalletDatabase<cdk_common::database::Error> + Send + Sync>;

    struct CrossMintTransferTestFixture {
        source_db: TestWalletDatabase,
        target_db: TestWalletDatabase,
        source_wallet: Wallet,
        target_wallet: Wallet,
        source_connector: Arc<MockMintConnector>,
        target_connector: Arc<MockMintConnector>,
        source_url: crate::mint_url::MintUrl,
    }

    impl CrossMintTransferTestFixture {
        async fn new(source_unit: CurrencyUnit, target_unit: CurrencyUnit) -> Self {
            let source_db = create_test_db().await;
            let target_db = create_test_db().await;
            let source_connector = Arc::new(MockMintConnector::new());
            let target_connector = Arc::new(MockMintConnector::new());
            let source_url = crate::mint_url::MintUrl::from_str("https://source.example.com")
                .expect("valid source URL");
            let target_url = crate::mint_url::MintUrl::from_str("https://target.example.com")
                .expect("valid target URL");
            let seed = [42; 64];

            let source_wallet = crate::wallet::WalletBuilder::new()
                .mint_url(source_url.clone())
                .unit(source_unit)
                .localstore(source_db.clone())
                .seed(seed)
                .shared_client(source_connector.clone())
                .build()
                .expect("source wallet");
            let target_wallet = crate::wallet::WalletBuilder::new()
                .mint_url(target_url)
                .unit(target_unit)
                .localstore(target_db.clone())
                .seed(seed)
                .shared_client(target_connector.clone())
                .build()
                .expect("target wallet");

            Self {
                source_db,
                target_db,
                source_wallet,
                target_wallet,
                source_connector,
                target_connector,
                source_url,
            }
        }

        async fn set_source_proofs(&self, proofs: &[(Id, u64)]) {
            let proof_infos = proofs
                .iter()
                .map(|(keyset_id, amount)| {
                    test_proof_info(*keyset_id, *amount, self.source_url.clone(), State::Unspent)
                })
                .collect();
            self.source_db
                .update_proofs(proof_infos, vec![])
                .await
                .expect("store source proofs");
        }

        fn set_source_keysets(&self, keysets: Vec<KeySet>) {
            self.source_connector.set_mint_keys_response(Ok(keysets));
        }

        fn set_source_melt_limits(&self, min_amount: u64, max_amount: u64) {
            let mut mint_info = test_mint_info();
            let settings = mint_info
                .nuts
                .nut05
                .methods
                .iter_mut()
                .find(|settings| {
                    settings.method == PaymentMethod::Known(KnownMethod::Bolt11)
                        && settings.unit == CurrencyUnit::Sat
                })
                .expect("test mint info has BOLT11 sat melt settings");
            settings.min_amount = Some(Amount::from(min_amount));
            settings.max_amount = Some(Amount::from(max_amount));
            self.source_connector.set_mint_info_response(Ok(mint_info));
        }

        fn set_target_mint_limits(&self, min_amount: u64, max_amount: u64) {
            let mut mint_info = test_mint_info();
            let settings = mint_info
                .nuts
                .nut04
                .methods
                .iter_mut()
                .find(|settings| {
                    settings.method == PaymentMethod::Known(KnownMethod::Bolt11)
                        && settings.unit == CurrencyUnit::Sat
                })
                .expect("test mint info has BOLT11 sat mint settings");
            settings.min_amount = Some(Amount::from(min_amount));
            settings.max_amount = Some(Amount::from(max_amount));
            self.target_connector.set_mint_info_response(Ok(mint_info));
        }

        fn queue_quotes(&self, quotes: &[(u64, u64)]) {
            for (amount, fee_reserve) in quotes {
                self.target_connector
                    .push_post_mint_quote_response(Ok(mint_quote_response(Amount::from(*amount))));
                self.source_connector
                    .push_post_melt_quote_response(Ok(melt_quote_response(
                        Amount::from(*amount),
                        Amount::from(*fee_reserve),
                    )));
            }
        }
    }

    fn invoice_for_amount(amount: Amount) -> String {
        let private_key =
            SecretKey::from_slice(&[42; 32]).expect("valid fixed private key for test invoice");
        let payment_hash = Sha256Hash::hash(&amount.to_u64().to_be_bytes());
        let payment_secret = PaymentSecret([21; 32]);

        InvoiceBuilder::new(Currency::Bitcoin)
            .description("cross-mint transfer test".to_string())
            .payment_hash(payment_hash)
            .payment_secret(payment_secret)
            .amount_milli_satoshis(amount.to_u64() * 1_000)
            .current_timestamp()
            .min_final_cltv_expiry_delta(144)
            .build_signed(|hash| Secp256k1::new().sign_ecdsa_recoverable(hash, &private_key))
            .expect("test invoice should build")
            .to_string()
    }

    fn mint_quote_response(amount: Amount) -> MintQuoteResponse<String> {
        MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
            quote: format!("mint-quote-{}", amount),
            request: invoice_for_amount(amount),
            amount: Some(amount),
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            amount_paid: Amount::ZERO,
            amount_issued: Amount::ZERO,
            updated_at: 0,
            state: QuoteState::Unpaid,
            expiry: Some(2_000_000_000),
            pubkey: None,
        })
    }

    fn melt_quote_response(amount: Amount, fee_reserve: Amount) -> MeltQuoteCreateResponse<String> {
        MeltQuoteCreateResponse::Bolt11(MeltQuoteBolt11Response {
            quote: format!("melt-quote-{}", amount),
            amount,
            fee_reserve,
            state: MeltQuoteState::Unpaid,
            expiry: 2_000_000_000,
            payment_preimage: None,
            change: None,
            request: None,
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
        })
    }

    #[tokio::test]
    async fn melt_quote_refresh_defers_recent_preparation() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let operation_id = uuid::Uuid::now_v7();
        let mut quote = test_melt_quote();
        quote.mint_url = Some(mint_url.clone());
        quote.used_by_operation = Some(operation_id.to_string());
        db.add_melt_quote(quote.clone()).await.unwrap();

        let mut saga = WalletSaga::new(
            operation_id,
            WalletSagaState::Melt(MeltSagaState::Preparing),
            quote.amount,
            mint_url,
            quote.unit.clone(),
            OperationData::PreparedMelt(PreparedMeltOperationData {
                quote: quote.clone(),
                proofs: Vec::new(),
                proofs_to_swap: Vec::new(),
                swap_fee: Amount::ZERO,
                input_fee: Amount::ZERO,
                input_fee_without_swap: Amount::ZERO,
                metadata: HashMap::new(),
                purpose: PreparedMeltPurpose::Payment,
            }),
        );
        saga.update_state(WalletSagaState::Melt(MeltSagaState::Preparing));
        db.add_saga(saga).await.unwrap();

        // No status response is configured. A network call would therefore
        // fail this test; an active preparation must be returned from storage.
        let wallet =
            create_test_wallet_with_mock(db.clone(), Arc::new(MockMintConnector::new())).await;
        let refreshed = wallet.check_melt_quote_status(&quote.id).await.unwrap();

        assert_eq!(refreshed.id, quote.id);
        assert_eq!(refreshed.used_by_operation, Some(operation_id.to_string()));
        assert!(db.get_saga(&operation_id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn cross_mint_transfer_quote_max_converges_with_melt_and_input_fees() {
        let fixture = CrossMintTransferTestFixture::new(CurrencyUnit::Sat, CurrencyUnit::Sat).await;
        fixture
            .set_source_proofs(&[(crate::wallet::test_utils::test_keyset_id(), 1_000)])
            .await;
        fixture.queue_quotes(&[(999, 1_200), (499, 9), (990, 9)]);

        let quote = fixture
            .source_wallet
            .cross_mint_transfer_quote_max(&fixture.target_wallet)
            .await
            .expect("maximum cross-mint transfer quote");

        assert_eq!(quote.mint_quote.amount, Some(Amount::from(990)));
        assert_eq!(quote.melt_quote.amount, Amount::from(990));
        assert_eq!(quote.melt_quote.fee_reserve, Amount::from(9));
        assert_eq!(quote.input_fee, Amount::ONE);
        assert!(quote.mint_quote.secret_key.is_some());

        let stored_mint_quotes = fixture
            .target_db
            .get_mint_quotes()
            .await
            .expect("stored target mint quotes");
        assert_eq!(stored_mint_quotes, vec![quote.mint_quote.clone()]);
        let stored_melt_quotes = fixture
            .source_db
            .get_melt_quotes()
            .await
            .expect("stored source melt quotes");
        assert_eq!(stored_melt_quotes, vec![quote.melt_quote.clone()]);

        let requested_amounts = fixture
            .target_connector
            .post_mint_quote_requests()
            .into_iter()
            .map(|request| match request {
                MintQuoteRequest::Bolt11(request) => request.amount,
                _ => panic!("expected bolt11 mint quote"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            requested_amounts,
            vec![Amount::from(999), Amount::from(499), Amount::from(990)]
        );
        assert_eq!(fixture.source_connector.post_melt_quote_requests().len(), 3);

        let prepared = fixture
            .source_wallet
            .prepare_melt_proofs(
                &quote.melt_quote.id,
                fixture
                    .source_wallet
                    .get_unspent_proofs()
                    .await
                    .expect("source proofs"),
                HashMap::new(),
            )
            .await
            .expect("prepare maximum cross-mint transfer");
        assert_eq!(prepared.input_fee_without_swap(), Amount::ONE);
        assert_eq!(prepared.change_amount_without_swap().unwrap(), Amount::ZERO);
        prepared
            .cancel()
            .await
            .expect("cancel prepared cross-mint transfer");
    }

    #[tokio::test]
    async fn prepared_cross_mint_transfer_is_reconstructable() {
        let fixture = CrossMintTransferTestFixture::new(CurrencyUnit::Sat, CurrencyUnit::Sat).await;
        fixture
            .set_source_proofs(&[(crate::wallet::test_utils::test_keyset_id(), 1_000)])
            .await;
        fixture.queue_quotes(&[(999, 1_200), (499, 9), (990, 9)]);

        let prepared = fixture
            .source_wallet
            .prepare_cross_mint_transfer(&fixture.target_wallet, HashMap::new())
            .await
            .expect("prepare cross-mint transfer");
        let operation_id = prepared.operation_id();
        let operation = fixture
            .source_wallet
            .cross_mint_transfer_operation(operation_id)
            .await
            .expect("load durable transfer")
            .expect("transfer identity should be persisted");
        assert_eq!(operation.amount, Amount::from(990));
        assert_eq!(
            operation.destination_mint_url,
            fixture.target_wallet.mint_url
        );
        assert_eq!(operation.destination_quote_id, "mint-quote-990");
        assert!(matches!(
            prepared.purpose(),
            PreparedMeltPurpose::CrossMintTransfer {
                destination_mint_url,
                destination_unit: CurrencyUnit::Sat,
                destination_quote_id,
            } if destination_mint_url == &fixture.target_wallet.mint_url
                && destination_quote_id == "mint-quote-990"
        ));

        drop(prepared);
        let resumed = fixture
            .source_wallet
            .prepared_melt(operation_id)
            .await
            .expect("prepared transfer should be reconstructable by ID");
        assert_eq!(resumed.amount(), Amount::from(990));
        assert!(matches!(
            resumed.purpose(),
            PreparedMeltPurpose::CrossMintTransfer { .. }
        ));
        resumed
            .cancel()
            .await
            .expect("resumed transfer should cancel");
        assert!(fixture
            .source_db
            .get_saga(&operation_id)
            .await
            .unwrap()
            .is_none());
        assert!(fixture
            .source_wallet
            .cross_mint_transfer_operation(operation_id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn cross_mint_transfer_rejects_the_same_wallet_as_destination() {
        let fixture = CrossMintTransferTestFixture::new(CurrencyUnit::Sat, CurrencyUnit::Sat).await;

        let result = fixture
            .source_wallet
            .cross_mint_transfer_quote_max(&fixture.source_wallet)
            .await;

        assert!(matches!(result, Err(Error::InvalidOperationState)));
        assert!(fixture
            .source_connector
            .post_melt_quote_requests()
            .is_empty());
    }

    #[tokio::test]
    async fn cross_mint_transfer_quote_max_respects_source_melt_maximum() {
        let fixture = CrossMintTransferTestFixture::new(CurrencyUnit::Sat, CurrencyUnit::Sat).await;
        fixture.set_source_melt_limits(1, 600);
        fixture
            .set_source_proofs(&[(crate::wallet::test_utils::test_keyset_id(), 1_000)])
            .await;
        fixture.queue_quotes(&[(600, 5)]);

        let quote = fixture
            .source_wallet
            .cross_mint_transfer_quote_max(&fixture.target_wallet)
            .await
            .expect("quote capped by source melt maximum");

        assert_eq!(quote.melt_quote.amount, Amount::from(600));
        assert!(matches!(
            &fixture.target_connector.post_mint_quote_requests()[..],
            [MintQuoteRequest::Bolt11(request)] if request.amount == Amount::from(600)
        ));
    }

    #[tokio::test]
    async fn cross_mint_transfer_quote_max_respects_target_mint_maximum() {
        let fixture = CrossMintTransferTestFixture::new(CurrencyUnit::Sat, CurrencyUnit::Sat).await;
        fixture.set_target_mint_limits(1, 600);
        fixture
            .set_source_proofs(&[(crate::wallet::test_utils::test_keyset_id(), 1_000)])
            .await;
        fixture.queue_quotes(&[(600, 5)]);

        let quote = fixture
            .source_wallet
            .cross_mint_transfer_quote_max(&fixture.target_wallet)
            .await
            .expect("quote capped by target mint maximum");

        assert_eq!(quote.mint_quote.amount, Some(Amount::from(600)));
        assert!(matches!(
            &fixture.target_connector.post_mint_quote_requests()[..],
            [MintQuoteRequest::Bolt11(request)] if request.amount == Amount::from(600)
        ));
    }

    #[tokio::test]
    async fn cross_mint_transfer_quote_max_rejects_balance_below_quote_minimum() {
        let fixture = CrossMintTransferTestFixture::new(CurrencyUnit::Sat, CurrencyUnit::Sat).await;
        fixture.set_source_melt_limits(40, 500_000);
        fixture.set_target_mint_limits(100, 500_000);
        fixture
            .set_source_proofs(&[(crate::wallet::test_utils::test_keyset_id(), 90)])
            .await;

        let error = fixture
            .source_wallet
            .cross_mint_transfer_quote_max(&fixture.target_wallet)
            .await
            .expect_err("balance below quote minimum should be rejected");

        assert!(matches!(error, Error::InsufficientFunds));
        assert!(fixture
            .target_connector
            .post_mint_quote_requests()
            .is_empty());
    }

    #[tokio::test]
    async fn cross_mint_transfer_quote_max_does_not_persist_failed_search_probes() {
        let fixture = CrossMintTransferTestFixture::new(CurrencyUnit::Sat, CurrencyUnit::Sat).await;
        fixture
            .set_source_proofs(&[(crate::wallet::test_utils::test_keyset_id(), 1_000)])
            .await;
        fixture
            .target_connector
            .push_post_mint_quote_response(Ok(mint_quote_response(Amount::from(999))));
        fixture
            .source_connector
            .push_post_melt_quote_response(Ok(melt_quote_response(
                Amount::from(999),
                Amount::from(1_200),
            )));
        fixture
            .target_connector
            .push_post_mint_quote_response(Ok(mint_quote_response(Amount::from(499))));
        fixture
            .source_connector
            .push_post_melt_quote_response(Err(Error::Custom("test quote failure".to_string())));

        let error = fixture
            .source_wallet
            .cross_mint_transfer_quote_max(&fixture.target_wallet)
            .await
            .expect_err("quote search should fail");

        assert!(matches!(error, Error::Custom(message) if message == "test quote failure"));
        assert!(fixture
            .target_db
            .get_mint_quotes()
            .await
            .expect("stored target mint quotes")
            .is_empty());
        assert!(fixture
            .source_db
            .get_melt_quotes()
            .await
            .expect("stored source melt quotes")
            .is_empty());
    }

    #[tokio::test]
    async fn cross_mint_transfer_quote_max_rolls_back_partial_persistence() {
        let fixture = CrossMintTransferTestFixture::new(CurrencyUnit::Sat, CurrencyUnit::Sat).await;
        fixture
            .set_source_proofs(&[(crate::wallet::test_utils::test_keyset_id(), 100)])
            .await;
        fixture.queue_quotes(&[(99, 0)]);

        let mut conflicting_quote = test_melt_quote();
        conflicting_quote.id = "melt-quote-99".to_string();
        conflicting_quote.mint_url = Some(fixture.source_url.clone());
        fixture
            .source_db
            .add_melt_quote(conflicting_quote.clone())
            .await
            .expect("store conflicting melt quote");
        fixture
            .source_db
            .add_melt_quote(conflicting_quote)
            .await
            .expect("advance conflicting melt quote version");

        let error = fixture
            .source_wallet
            .cross_mint_transfer_quote_max(&fixture.target_wallet)
            .await
            .expect_err("selected melt quote persistence should conflict");

        assert!(matches!(
            error,
            Error::Database(cdk_common::database::Error::ConcurrentUpdate)
        ));
        assert!(fixture
            .target_db
            .get_mint_quotes()
            .await
            .expect("stored target mint quotes")
            .is_empty());
        assert_eq!(
            fixture
                .source_db
                .get_melt_quotes()
                .await
                .expect("stored source melt quotes")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn cross_mint_transfer_quote_max_returns_best_feasible_amount() {
        let fixture = CrossMintTransferTestFixture::new(CurrencyUnit::Sat, CurrencyUnit::Sat).await;
        fixture
            .set_source_proofs(&[(crate::wallet::test_utils::test_keyset_id(), 100)])
            .await;
        fixture.queue_quotes(&[(99, 2), (97, 1), (98, 2)]);

        let quote = fixture
            .source_wallet
            .cross_mint_transfer_quote_max(&fixture.target_wallet)
            .await
            .expect("best feasible cross-mint transfer quote");

        assert_eq!(quote.melt_quote.amount, Amount::from(97));
        assert_eq!(quote.melt_quote.fee_reserve, Amount::ONE);
        assert_eq!(quote.input_fee, Amount::ONE);

        let prepared = fixture
            .source_wallet
            .prepare_melt_proofs(
                &quote.melt_quote.id,
                fixture
                    .source_wallet
                    .get_unspent_proofs()
                    .await
                    .expect("source proofs"),
                HashMap::new(),
            )
            .await
            .expect("prepare best feasible cross-mint transfer");
        assert_eq!(prepared.change_amount_without_swap().unwrap(), Amount::ONE);
        prepared
            .cancel()
            .await
            .expect("cancel best feasible cross-mint transfer");
    }

    #[tokio::test]
    async fn cross_mint_transfer_quote_max_rejects_empty_balance() {
        let fixture = CrossMintTransferTestFixture::new(CurrencyUnit::Sat, CurrencyUnit::Sat).await;

        let error = fixture
            .source_wallet
            .cross_mint_transfer_quote_max(&fixture.target_wallet)
            .await
            .expect_err("empty balance should be rejected");

        assert!(matches!(error, Error::InsufficientFunds));
        assert!(fixture
            .target_connector
            .post_mint_quote_requests()
            .is_empty());
    }

    #[tokio::test]
    async fn cross_mint_transfer_quote_max_rejects_currency_unit_mismatch() {
        let fixture =
            CrossMintTransferTestFixture::new(CurrencyUnit::Sat, CurrencyUnit::Msat).await;

        let error = fixture
            .source_wallet
            .cross_mint_transfer_quote_max(&fixture.target_wallet)
            .await
            .expect_err("currency unit mismatch should be rejected");

        assert!(matches!(error, Error::UnsupportedUnit));
        assert!(fixture
            .target_connector
            .post_mint_quote_requests()
            .is_empty());
    }

    #[tokio::test]
    async fn cross_mint_transfer_quote_max_accounts_for_fees_across_keysets() {
        let fixture = CrossMintTransferTestFixture::new(CurrencyUnit::Sat, CurrencyUnit::Sat).await;
        let default_keyset = test_keyset();
        let mut expensive_keyset = make_inactive_keyset();
        expensive_keyset.input_fee_ppk = 900;
        expensive_keyset.id = Id::v2_from_data(
            &expensive_keyset.keys,
            &expensive_keyset.unit,
            expensive_keyset.input_fee_ppk,
            expensive_keyset.final_expiry,
        );
        fixture.set_source_keysets(vec![default_keyset.clone(), expensive_keyset.clone()]);
        fixture
            .set_source_proofs(&[(default_keyset.id, 600), (expensive_keyset.id, 400)])
            .await;
        fixture.queue_quotes(&[(998, 8), (990, 8)]);

        let quote = fixture
            .source_wallet
            .cross_mint_transfer_quote_max(&fixture.target_wallet)
            .await
            .expect("maximum cross-mint transfer quote across keysets");

        assert_eq!(quote.melt_quote.amount, Amount::from(990));
        assert_eq!(quote.melt_quote.fee_reserve, Amount::from(8));
        assert_eq!(quote.input_fee, Amount::from(2));

        let prepared = fixture
            .source_wallet
            .prepare_melt_proofs(
                &quote.melt_quote.id,
                fixture
                    .source_wallet
                    .get_unspent_proofs()
                    .await
                    .expect("source proofs"),
                HashMap::new(),
            )
            .await
            .expect("prepare maximum cross-mint transfer across keysets");
        assert_eq!(prepared.input_fee_without_swap(), Amount::from(2));
        assert_eq!(prepared.change_amount_without_swap().unwrap(), Amount::ZERO);
        prepared
            .cancel()
            .await
            .expect("cancel maximum cross-mint transfer across keysets");
    }

    #[tokio::test]
    async fn test_cancel_prepared_melt_reverts_reserved_proofs() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = crate::wallet::test_utils::test_keyset_id();

        let proof_info = test_proof_info(keyset_id, 1200, mint_url.clone(), State::Unspent);
        let proof_y = proof_info.y;
        let proof = proof_info.proof.clone();
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let quote = test_melt_quote();
        let quote_id = quote.id.clone();
        db.add_melt_quote(quote).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let prepared = wallet
            .prepare_melt_proofs(&quote_id, vec![proof], HashMap::new())
            .await
            .unwrap();
        let operation_id = prepared.operation_id();
        drop(prepared);

        let recovery = wallet.recover_incomplete_sagas().await.unwrap();
        assert_eq!(recovery.skipped, 1);
        assert_eq!(recovery.compensated, 0);
        assert!(db.get_saga(&operation_id).await.unwrap().is_some());

        let prepared = wallet
            .prepared_melt(operation_id)
            .await
            .expect("prepared melt should be reconstructable by ID");

        wallet
            .cancel_prepared_melt(prepared.operation_id())
            .await
            .unwrap();

        let stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].state, State::Unspent);
    }

    #[tokio::test]
    async fn test_cancel_prepared_melt_rejects_pending_saga() {
        let (db, proof_y, operation_id, _mock_client, pending) = pending_bolt11_melt(false).await;

        let result = pending.wallet.cancel_prepared_melt(operation_id).await;

        assert!(matches!(result, Err(Error::InvalidOperationState)));

        let stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].state, State::Pending);
        assert!(db.get_saga(&operation_id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_cancel_prepared_melt_reverts_operation_pending_proofs() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = crate::wallet::test_utils::test_keyset_id();

        let proof_info = test_proof_info(keyset_id, 1200, mint_url.clone(), State::Unspent);
        let proof_y = proof_info.y;
        let proof = proof_info.proof.clone();
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let quote = test_melt_quote();
        let quote_id = quote.id.clone();
        db.add_melt_quote(quote).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let prepared = wallet
            .prepare_melt_proofs(&quote_id, vec![proof], HashMap::new())
            .await
            .unwrap();
        let operation_id = prepared.operation_id();

        db.update_proofs_state(vec![proof_y], State::Pending)
            .await
            .unwrap();

        wallet.cancel_prepared_melt(operation_id).await.unwrap();

        let stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].state, State::Unspent);
        assert_eq!(stored[0].used_by_operation, None);
        assert!(db.get_saga(&operation_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_cancel_prepared_melt_rejects_melt_requested_saga() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let operation_id = uuid::Uuid::new_v4();

        let proof_info = test_proof_info(keyset_id, 100, mint_url.clone(), State::Pending);
        let proof_y = proof_info.y;
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let quote = test_melt_quote();
        let saga = WalletSaga::new(
            operation_id,
            WalletSagaState::Melt(MeltSagaState::MeltRequested),
            quote.amount,
            mint_url,
            CurrencyUnit::Sat,
            OperationData::Melt(MeltOperationData {
                quote_id: quote.id,
                amount: quote.amount,
                fee_reserve: quote.fee_reserve,
                counter_start: None,
                counter_end: None,
                change_amount: None,
                metadata: HashMap::new(),
                final_proof_ys: None,
                change_blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;

        let err = wallet
            .cancel_prepared_melt(operation_id)
            .await
            .expect_err("cancel should reject a melt that was already requested");

        assert!(matches!(err, Error::InvalidOperationState));
        let stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].state, State::Pending);
        assert!(db.get_saga(&operation_id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_cancel_prepared_melt_preserves_spent_proofs() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = crate::wallet::test_utils::test_keyset_id();

        let proof_info = test_proof_info(keyset_id, 1200, mint_url.clone(), State::Unspent);
        let proof_y = proof_info.y;
        let proof = proof_info.proof.clone();
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let quote = test_melt_quote();
        let quote_id = quote.id.clone();
        db.add_melt_quote(quote).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let prepared = wallet
            .prepare_melt_proofs(&quote_id, vec![proof], HashMap::new())
            .await
            .unwrap();
        db.update_proofs_state(vec![proof_y], State::Spent)
            .await
            .unwrap();

        wallet
            .cancel_prepared_melt(prepared.operation_id())
            .await
            .unwrap();

        let stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].state, State::Spent);
    }

    #[tokio::test]
    async fn test_cancel_prepared_melt_only_reverts_reserved_proofs() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = crate::wallet::test_utils::test_keyset_id();

        let reserved = test_proof_info(keyset_id, 1200, mint_url.clone(), State::Unspent);
        let pending = test_proof_info(keyset_id, 200, mint_url.clone(), State::Pending);
        let spent = test_proof_info(keyset_id, 300, mint_url.clone(), State::Spent);

        let reserved_y = reserved.y;
        let pending_y = pending.y;
        let spent_y = spent.y;

        let reserved_proof = reserved.proof.clone();
        db.update_proofs(vec![reserved, pending, spent], vec![])
            .await
            .unwrap();

        let quote = test_melt_quote();
        let quote_id = quote.id.clone();
        db.add_melt_quote(quote).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let prepared = wallet
            .prepare_melt_proofs(&quote_id, vec![reserved_proof], HashMap::new())
            .await
            .unwrap();

        wallet
            .cancel_prepared_melt(prepared.operation_id())
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
        assert_eq!(state_for(pending_y), Some(State::Pending));
        assert_eq!(state_for(spent_y), Some(State::Spent));
    }

    #[tokio::test]
    async fn test_add_transaction_for_pending_melt_uses_only_operation_pending_proofs() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let operation_a_id = uuid::Uuid::new_v4();
        let operation_b_id = uuid::Uuid::new_v4();

        let mut operation_a_pending =
            test_proof_info(keyset_id, 1200, mint_url.clone(), State::Pending);
        operation_a_pending.used_by_operation = Some(operation_a_id);
        let operation_a_pending_y = operation_a_pending.y;

        let mut operation_b_pending =
            test_proof_info(keyset_id, 700, mint_url.clone(), State::Pending);
        operation_b_pending.used_by_operation = Some(operation_b_id);
        let operation_b_pending_y = operation_b_pending.y;

        let mut operation_a_spent = test_proof_info(keyset_id, 300, mint_url.clone(), State::Spent);
        operation_a_spent.used_by_operation = Some(operation_a_id);
        let operation_a_spent_y = operation_a_spent.y;

        db.update_proofs(
            vec![operation_a_pending, operation_b_pending, operation_a_spent],
            vec![],
        )
        .await
        .unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;

        let mut quote = test_melt_quote();
        quote.used_by_operation = Some(operation_a_id.to_string());

        wallet
            .add_transaction_for_pending_melt(
                &quote,
                MeltQuoteState::Paid,
                Amount::from(1000),
                Some(Amount::from(150)),
                Some("payment-proof".to_string()),
            )
            .await
            .unwrap();

        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert_eq!(transactions.len(), 1);

        let tx = &transactions[0];
        assert_eq!(tx.ys, vec![operation_a_pending_y]);
        assert!(!tx.ys.contains(&operation_b_pending_y));
        assert!(!tx.ys.contains(&operation_a_spent_y));
        assert_eq!(tx.fee, Amount::from(50));
        assert_eq!(tx.saga_id, Some(operation_a_id));
    }

    #[tokio::test]
    async fn test_add_transaction_for_pending_melt_uses_saga_metadata() {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        let keyset_id = test_keyset_id();
        let operation_id = uuid::Uuid::new_v4();

        let mut pending = test_proof_info(keyset_id, 1200, mint_url.clone(), State::Pending);
        pending.used_by_operation = Some(operation_id);
        db.update_proofs(vec![pending], vec![]).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;

        let mut quote = test_melt_quote();
        quote.used_by_operation = Some(operation_id.to_string());
        let quote_id = quote.id.clone();

        let mut metadata = HashMap::new();
        metadata.insert("memo".to_string(), "pending metadata".to_string());

        let saga = WalletSaga::new(
            operation_id,
            WalletSagaState::Melt(MeltSagaState::PaymentPending),
            quote.amount,
            mint_url,
            CurrencyUnit::Sat,
            OperationData::Melt(MeltOperationData {
                quote_id: quote_id.clone(),
                amount: quote.amount,
                fee_reserve: quote.fee_reserve,
                counter_start: None,
                counter_end: None,
                change_amount: None,
                metadata: metadata.clone(),
                final_proof_ys: None,
                change_blinded_messages: None,
            }),
        );
        db.add_saga(saga).await.unwrap();

        wallet
            .add_transaction_for_pending_melt(
                &quote,
                MeltQuoteState::Paid,
                quote.amount,
                Some(Amount::from(150)),
                Some("payment-proof".to_string()),
            )
            .await
            .unwrap();

        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].metadata, metadata);
    }

    #[tokio::test]
    async fn test_add_transaction_for_pending_melt_skips_missing_or_invalid_operation_id() {
        let db = create_test_db().await;
        let mock_client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;

        let quote_without_operation = test_melt_quote();
        wallet
            .add_transaction_for_pending_melt(
                &quote_without_operation,
                MeltQuoteState::Paid,
                Amount::from(1000),
                None,
                None,
            )
            .await
            .unwrap();

        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert!(transactions.is_empty());

        let mut quote_with_invalid_operation = test_melt_quote();
        quote_with_invalid_operation.used_by_operation = Some("invalid-operation-id".to_string());
        wallet
            .add_transaction_for_pending_melt(
                &quote_with_invalid_operation,
                MeltQuoteState::Paid,
                Amount::from(1000),
                None,
                None,
            )
            .await
            .unwrap();

        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert!(transactions.is_empty());
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

    #[tokio::test]
    async fn test_prepared_melt_exposes_metadata() {
        let db = create_test_db().await;
        let quote = test_melt_quote();
        let quote_id = quote.id.clone();
        db.add_melt_quote(quote).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.reset_default_mint_state();
        let wallet = create_test_wallet_with_mock(db, mock_client).await;

        let mut metadata = HashMap::new();
        metadata.insert("memo".to_string(), "ffi metadata".to_string());

        let proof = test_proof(crate::wallet::test_utils::test_keyset_id(), 1200);
        let prepared = wallet
            .prepare_melt_proofs(&quote_id, vec![proof], metadata.clone())
            .await
            .unwrap();

        assert_eq!(prepared.metadata(), &metadata);
    }

    #[tokio::test]
    async fn failed_melt_preparation_releases_its_quote_reservation() {
        let db = create_test_db().await;
        let quote = test_melt_quote();
        let quote_id = quote.id.clone();
        db.add_melt_quote(quote).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.reset_default_mint_state();
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;

        let insufficient = test_proof(crate::wallet::test_utils::test_keyset_id(), 1);
        let result = wallet
            .prepare_melt_proofs(&quote_id, vec![insufficient], HashMap::new())
            .await;
        assert!(matches!(result, Err(Error::InsufficientFunds)));

        let stored_quote = db
            .get_melt_quote(&quote_id)
            .await
            .unwrap()
            .expect("quote should remain available");
        assert_eq!(stored_quote.used_by_operation, None);

        let sufficient = test_proof(crate::wallet::test_utils::test_keyset_id(), 1200);
        let prepared = wallet
            .prepare_melt_proofs(&quote_id, vec![sufficient], HashMap::new())
            .await
            .expect("the quote should be reusable after failed preparation");
        prepared.cancel().await.unwrap();
    }

    #[tokio::test]
    async fn melt_rejects_a_quote_owned_by_another_wallet() {
        let db = create_test_db().await;
        let mut quote = test_melt_quote();
        quote.mint_url = Some(
            cdk_common::mint_url::MintUrl::from_str("https://other-mint.example.com").unwrap(),
        );
        let quote_id = quote.id.clone();
        db.add_melt_quote(quote).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.reset_default_mint_state();
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;
        let proof = test_proof(crate::wallet::test_utils::test_keyset_id(), 1200);

        let result = wallet
            .prepare_melt_proofs(&quote_id, vec![proof], HashMap::new())
            .await;
        assert!(matches!(result, Err(Error::IncorrectMint)));
        assert_eq!(
            db.get_melt_quote(&quote_id)
                .await
                .unwrap()
                .unwrap()
                .used_by_operation,
            None
        );
    }

    #[tokio::test]
    async fn test_confirm_prepared_melt_prefers_persisted_saga_metadata() {
        let db = create_test_db().await;
        let quote = test_melt_quote();
        let quote_id = quote.id.clone();
        db.add_melt_quote(quote).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.reset_default_mint_state();
        mock_client.set_post_melt_response(Ok(MeltQuoteResponse::Bolt11(bolt11_status(
            &quote_id,
            MeltQuoteState::Paid,
            Some("preimage123".to_string()),
        ))));
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;

        let mut saga_metadata = HashMap::new();
        saga_metadata.insert("memo".to_string(), "persisted saga metadata".to_string());
        let mut stale_metadata = HashMap::new();
        stale_metadata.insert("memo".to_string(), "stale handle metadata".to_string());

        let proof = test_proof(crate::wallet::test_utils::test_keyset_id(), 1200);
        let prepared = wallet
            .prepare_melt_proofs(&quote_id, vec![proof], saga_metadata.clone())
            .await
            .unwrap();

        wallet
            .confirm_prepared_melt_with_options(prepared.operation_id(), MeltConfirmOptions::new())
            .await
            .unwrap();

        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].metadata, saga_metadata);
    }

    #[tokio::test]
    async fn test_confirm_prefer_async_prefers_persisted_saga_metadata() {
        let db = create_test_db().await;
        let quote = test_melt_quote();
        let quote_id = quote.id.clone();
        db.add_melt_quote(quote).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.reset_default_mint_state();
        mock_client.set_post_melt_response(Ok(MeltQuoteResponse::Bolt11(bolt11_status(
            &quote_id,
            MeltQuoteState::Paid,
            Some("preimage123".to_string()),
        ))));
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;

        let mut stale_metadata = HashMap::new();
        stale_metadata.insert("memo".to_string(), "stale handle metadata".to_string());
        let mut saga_metadata = HashMap::new();
        saga_metadata.insert("memo".to_string(), "persisted saga metadata".to_string());

        let proof = test_proof(crate::wallet::test_utils::test_keyset_id(), 1200);
        let prepared = wallet
            .prepare_melt_proofs(&quote_id, vec![proof], stale_metadata)
            .await
            .unwrap();

        let mut stored_saga = db
            .get_saga(&prepared.operation_id())
            .await
            .unwrap()
            .unwrap();
        match &mut stored_saga.data {
            OperationData::PreparedMelt(data) => {
                data.metadata = saga_metadata.clone();
            }
            _ => panic!("expected prepared melt saga"),
        }
        stored_saga.update_state(stored_saga.state);
        assert!(db.update_saga(stored_saga).await.unwrap());

        let outcome = prepared
            .confirm_prefer_async_with_options(MeltConfirmOptions::new())
            .await
            .unwrap();
        assert!(matches!(outcome, MeltOutcome::Paid(_)));

        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].metadata, saga_metadata);
    }

    #[tokio::test]
    async fn test_confirm_prepared_melt_prefer_async_prefers_persisted_saga_metadata() {
        let db = create_test_db().await;
        let quote = test_melt_quote();
        let quote_id = quote.id.clone();
        db.add_melt_quote(quote).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.reset_default_mint_state();
        mock_client.set_post_melt_response(Ok(MeltQuoteResponse::Bolt11(bolt11_status(
            &quote_id,
            MeltQuoteState::Paid,
            Some("preimage123".to_string()),
        ))));
        let wallet = create_test_wallet_with_mock(db.clone(), mock_client).await;

        let mut saga_metadata = HashMap::new();
        saga_metadata.insert("memo".to_string(), "persisted saga metadata".to_string());
        let mut stale_metadata = HashMap::new();
        stale_metadata.insert("memo".to_string(), "stale handle metadata".to_string());

        let proof = test_proof(crate::wallet::test_utils::test_keyset_id(), 1200);
        let prepared = wallet
            .prepare_melt_proofs(&quote_id, vec![proof], saga_metadata.clone())
            .await
            .unwrap();

        wallet
            .confirm_prepared_melt_prefer_async_with_options(
                prepared.operation_id(),
                MeltConfirmOptions::new(),
            )
            .await
            .unwrap();

        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].metadata, saga_metadata);
    }

    /// Build a bolt11 melt-quote status response with the given state/preimage.
    fn bolt11_status(
        quote_id: &str,
        state: MeltQuoteState,
        payment_preimage: Option<String>,
    ) -> MeltQuoteBolt11Response<String> {
        MeltQuoteBolt11Response {
            quote: quote_id.to_string(),
            state,
            expiry: 9999999999,
            fee_reserve: Amount::from(10),
            amount: Amount::from(1000),
            request: Some("lnbc1000...".to_string()),
            payment_preimage,
            change: None,
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
        }
    }

    /// Drive a bolt11 melt to a durable `PendingMelt` using a mock mint that
    /// accepts the async request and reports `Pending`.
    async fn pending_bolt11_melt(
        http_subscription: bool,
    ) -> (
        Arc<dyn cdk_common::database::WalletDatabase<cdk_common::database::Error> + Send + Sync>,
        crate::nuts::PublicKey,
        uuid::Uuid,
        Arc<MockMintConnector>,
        PendingMelt,
    ) {
        pending_bolt11_melt_with_metadata(http_subscription, HashMap::new()).await
    }

    async fn pending_bolt11_melt_with_metadata(
        http_subscription: bool,
        metadata: HashMap<String, String>,
    ) -> (
        Arc<dyn cdk_common::database::WalletDatabase<cdk_common::database::Error> + Send + Sync>,
        crate::nuts::PublicKey,
        uuid::Uuid,
        Arc<MockMintConnector>,
        PendingMelt,
    ) {
        let db = create_test_db().await;
        let mint_url = test_mint_url();
        // Use the keyset the MockMintConnector serves so fee lookups resolve.
        let keyset_id = crate::wallet::test_utils::test_keyset_id();
        let proof_info = crate::wallet::test_utils::test_proof_info(keyset_id, 1200, mint_url);
        let proof_y = proof_info.y;
        let proof = proof_info.proof.clone();
        db.update_proofs(vec![proof_info], vec![]).await.unwrap();

        let quote = test_melt_quote();
        let quote_id = quote.id.clone();
        db.add_melt_quote(quote.clone()).await.unwrap();

        let mock_client = Arc::new(MockMintConnector::new());
        mock_client.reset_default_mint_state();
        // Mint accepts the async melt and reports Pending, so confirm yields a
        // PendingMelt we can drive through reconciliation.
        mock_client.set_post_melt_response(Ok(MeltQuoteResponse::Bolt11(bolt11_status(
            &quote_id,
            MeltQuoteState::Pending,
            None,
        ))));

        let wallet = if http_subscription {
            create_test_wallet_with_mock_http_subscription(db.clone(), mock_client.clone()).await
        } else {
            create_test_wallet_with_mock(db.clone(), mock_client.clone()).await
        };

        let prepared = wallet
            .prepare_melt_proofs(&quote_id, vec![proof], metadata)
            .await
            .unwrap();
        let operation_id = prepared.operation_id();

        let pending = match prepared.confirm_prefer_async().await.unwrap() {
            MeltOutcome::Pending(pending) => pending,
            MeltOutcome::Paid(_) => panic!("expected pending melt outcome"),
        };

        (db, proof_y, operation_id, mock_client, pending)
    }

    #[tokio::test]
    async fn test_resume_pending_melt_paid_finalizes() {
        let (db, proof_y, operation_id, mock_client, pending) = pending_bolt11_melt(false).await;
        let quote_id = pending.quote_id.clone();

        mock_client.set_melt_quote_status_response(Ok(MeltQuoteBolt11Response {
            payment_preimage: Some("preimage123".to_string()),
            ..bolt11_status(&quote_id, MeltQuoteState::Paid, None)
        }));
        mock_client._set_restore_response(Ok(RestoreResponse {
            outputs: Vec::new(),
            signatures: Vec::new(),
        }));

        let saga = db.get_saga(&operation_id).await.unwrap().unwrap();
        let finalized = pending
            .wallet
            .resume_melt_saga(&saga)
            .await
            .expect("resume should succeed")
            .expect("melt should finalize");
        assert_eq!(finalized.state(), MeltQuoteState::Paid);
        assert_eq!(finalized.payment_proof(), Some("preimage123"));

        let stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].state, State::Spent);
        assert!(db.get_saga(&operation_id).await.unwrap().is_none());

        let replayed = pending
            .wait()
            .await
            .expect("a concurrent waiter should recover the completed receipt");
        assert_eq!(replayed.state(), MeltQuoteState::Paid);
        assert_eq!(replayed.payment_proof(), Some("preimage123"));
    }

    #[tokio::test]
    async fn test_wait_pending_melt_polls_saga_and_finalizes() {
        let (db, proof_y, operation_id, mock_client, pending) = pending_bolt11_melt(true).await;
        let wallet = pending.wallet.clone();
        let quote_id = pending.quote_id.clone();

        let paid_status = MeltQuoteBolt11Response {
            payment_preimage: Some("preimage123".to_string()),
            ..bolt11_status(&quote_id, MeltQuoteState::Paid, None)
        };
        mock_client._set_restore_response(Ok(RestoreResponse {
            outputs: vec![],
            signatures: vec![],
        }));
        mock_client.push_melt_quote_status_response(Ok(paid_status.clone()));
        mock_client.push_melt_quote_status_response(Ok(paid_status));

        let finalized = tokio::time::timeout(
            Duration::from_secs(5),
            wallet.wait_pending_melt(operation_id),
        )
        .await
        .expect("wait timed out")
        .expect("pending melt should finalize");

        assert_eq!(finalized.state(), MeltQuoteState::Paid);
        assert_eq!(finalized.payment_proof(), Some("preimage123"));

        let stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].state, State::Spent);
        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].ys, vec![proof_y]);
        assert!(db.get_saga(&operation_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_reconstructed_wait_does_not_race_an_active_melt_request() {
        let (db, proof_y, operation_id, mock_client, pending) = pending_bolt11_melt(true).await;
        let quote_id = pending.quote_id.clone();

        let mut saga = db.get_saga(&operation_id).await.unwrap().unwrap();
        assert_eq!(
            saga.state,
            WalletSagaState::Melt(MeltSagaState::PaymentPending),
            "the mint response must be persisted before returning a pending handle"
        );

        // Recreate the narrow window after MeltRequested is persisted but
        // before the original network call returns. A second process may see
        // an old Unpaid quote during this window, but must not compensate it.
        saga.update_state(WalletSagaState::Melt(MeltSagaState::MeltRequested));
        assert!(db.update_saga(saga).await.unwrap());

        let unpaid = bolt11_status(&quote_id, MeltQuoteState::Unpaid, None);
        for _ in 0..4 {
            mock_client.push_melt_quote_status_response(Ok(unpaid.clone()));
        }

        let reconstructed = pending
            .wallet
            .pending_melt(operation_id)
            .await
            .expect("pending handle should reconstruct");
        let wait_result =
            tokio::time::timeout(Duration::from_millis(250), reconstructed.wait()).await;
        assert!(
            wait_result.is_err(),
            "recent MeltRequested state should remain leased to the active request"
        );

        let stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored[0].state, State::Pending);
        assert_eq!(
            db.get_saga(&operation_id).await.unwrap().unwrap().state,
            WalletSagaState::Melt(MeltSagaState::MeltRequested)
        );
    }

    #[tokio::test]
    async fn test_resume_pending_melt_pending_keeps_saga() {
        let (db, proof_y, operation_id, mock_client, pending) = pending_bolt11_melt(false).await;
        let quote_id = pending.quote_id.clone();

        mock_client.set_melt_quote_status_response(Ok(bolt11_status(
            &quote_id,
            MeltQuoteState::Pending,
            None,
        )));

        let saga = db.get_saga(&operation_id).await.unwrap().unwrap();
        assert!(pending
            .wallet
            .resume_melt_saga(&saga)
            .await
            .unwrap()
            .is_none());

        // Proofs stay pending and the saga is retained for a later check.
        let stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored[0].state, State::Pending);
        assert!(db.get_saga(&operation_id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_resume_pending_melt_unknown_keeps_saga() {
        let (db, _proof_y, operation_id, mock_client, pending) = pending_bolt11_melt(false).await;
        let quote_id = pending.quote_id.clone();

        mock_client.set_melt_quote_status_response(Ok(bolt11_status(
            &quote_id,
            MeltQuoteState::Unknown,
            None,
        )));

        let saga = db.get_saga(&operation_id).await.unwrap().unwrap();
        assert!(pending
            .wallet
            .resume_melt_saga(&saga)
            .await
            .unwrap()
            .is_none());
        assert!(db.get_saga(&operation_id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_resume_pending_melt_stable_unpaid_compensates() {
        let (db, proof_y, operation_id, mock_client, pending) = pending_bolt11_melt(false).await;
        let quote_id = pending.quote_id.clone();

        let unpaid_status = bolt11_status(&quote_id, MeltQuoteState::Unpaid, None);
        mock_client.push_melt_quote_status_response(Ok(unpaid_status.clone()));
        mock_client.push_melt_quote_status_response(Ok(unpaid_status));

        let saga = db.get_saga(&operation_id).await.unwrap().unwrap();
        let finalized = pending
            .wallet
            .resume_melt_saga(&saga)
            .await
            .unwrap()
            .expect("stable unpaid state should compensate");
        assert_eq!(finalized.state(), MeltQuoteState::Unpaid);

        // Proofs released back to Unspent and the saga cleaned up.
        let stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored[0].state, State::Unspent);
        assert!(db.get_saga(&operation_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_resume_stale_failure_preserves_spent_proof() {
        let (db, proof_y, operation_id, mock_client, pending) = pending_bolt11_melt(false).await;
        let quote_id = pending.quote_id.clone();

        db.update_proofs_state(vec![proof_y], State::Spent)
            .await
            .unwrap();
        let failed_status = bolt11_status(&quote_id, MeltQuoteState::Failed, None);
        mock_client.push_melt_quote_status_response(Ok(failed_status.clone()));
        mock_client.push_melt_quote_status_response(Ok(failed_status));

        let saga = db.get_saga(&operation_id).await.unwrap().unwrap();
        let finalized = pending
            .wallet
            .resume_melt_saga(&saga)
            .await
            .unwrap()
            .expect("stable failed state should compensate");
        assert_eq!(finalized.state(), MeltQuoteState::Failed);

        let stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored[0].state, State::Spent);
        assert_eq!(stored[0].used_by_operation, Some(operation_id));
    }

    #[tokio::test]
    async fn test_resume_stale_failure_preserves_newer_proof_owner() {
        let (db, proof_y, _operation_id, mock_client, pending) = pending_bolt11_melt(false).await;
        let quote_id = pending.quote_id.clone();
        let newer_operation_id = uuid::Uuid::new_v4();

        let mut stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        stored[0].state = State::Reserved;
        stored[0].used_by_operation = Some(newer_operation_id);
        db.update_proofs(stored, vec![]).await.unwrap();

        let failed_status = bolt11_status(&quote_id, MeltQuoteState::Failed, None);
        mock_client.push_melt_quote_status_response(Ok(failed_status.clone()));
        mock_client.push_melt_quote_status_response(Ok(failed_status));

        let saga = db.get_saga(&pending.operation_id).await.unwrap().unwrap();
        let finalized = pending
            .wallet
            .resume_melt_saga(&saga)
            .await
            .unwrap()
            .expect("stable failed state should compensate");
        assert_eq!(finalized.state(), MeltQuoteState::Failed);

        let stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored[0].state, State::Reserved);
        assert_eq!(stored[0].used_by_operation, Some(newer_operation_id));
    }

    #[tokio::test]
    async fn test_resume_failed_with_payment_proof_keeps_waiting() {
        let (db, proof_y, operation_id, mock_client, pending) = pending_bolt11_melt(false).await;
        let quote_id = pending.quote_id.clone();

        // HTTP reports Failed but carries a payment proof: never revert proofs.
        mock_client.set_melt_quote_status_response(Ok(bolt11_status(
            &quote_id,
            MeltQuoteState::Failed,
            Some("preimage123".to_string()),
        )));

        let saga = db.get_saga(&operation_id).await.unwrap().unwrap();
        assert!(pending
            .wallet
            .resume_melt_saga(&saga)
            .await
            .unwrap()
            .is_none());

        // Proofs remain pending and the saga is retained.
        let stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored[0].state, State::Pending);
        assert!(db.get_saga(&operation_id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_resume_status_error_keeps_pending() {
        let (db, proof_y, operation_id, mock_client, pending) = pending_bolt11_melt(false).await;
        mock_client.set_melt_quote_status_response(Err(Error::Custom("mint offline".to_string())));

        let saga = db.get_saga(&operation_id).await.unwrap().unwrap();
        assert!(pending
            .wallet
            .resume_melt_saga(&saga)
            .await
            .unwrap()
            .is_none());

        // Recovery kept the melt pending: proofs and saga are preserved.
        let stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored[0].state, State::Pending);
        assert!(db.get_saga(&operation_id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_wait_rechecks_transient_non_paid_state_before_compensating() {
        let (db, proof_y, operation_id, mock_client, pending) = pending_bolt11_melt(true).await;
        let quote_id = pending.quote_id.clone();

        // The first status is a stale non-paid result. Recovery must confirm it
        // before releasing proofs; the second check observes the paid state.
        mock_client.push_melt_quote_status_response(Ok(bolt11_status(
            &quote_id,
            MeltQuoteState::Unpaid,
            None,
        )));
        mock_client.push_melt_quote_status_response(Ok(MeltQuoteBolt11Response {
            payment_preimage: Some("preimage123".to_string()),
            ..bolt11_status(&quote_id, MeltQuoteState::Paid, None)
        }));
        mock_client._set_restore_response(Ok(RestoreResponse {
            outputs: Vec::new(),
            signatures: Vec::new(),
        }));

        let finalized =
            tokio::time::timeout(std::time::Duration::from_secs(5), pending.into_future())
                .await
                .expect("wait timed out")
                .expect("melt should finalize");
        assert_eq!(finalized.state(), MeltQuoteState::Paid);

        let stored = db.get_proofs_by_ys(vec![proof_y]).await.unwrap();
        assert_eq!(stored[0].state, State::Spent);
        assert!(db.get_saga(&operation_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_wait_pending_melt_records_persisted_metadata() {
        let mut metadata = HashMap::new();
        metadata.insert("label".to_string(), "ffi wait".to_string());
        let (db, _proof_y, _operation_id, mock_client, pending) =
            pending_bolt11_melt_with_metadata(true, metadata.clone()).await;
        let quote_id = pending.quote_id.clone();

        mock_client.push_melt_quote_status_response(Ok(MeltQuoteBolt11Response {
            payment_preimage: Some("preimage123".to_string()),
            ..bolt11_status(&quote_id, MeltQuoteState::Paid, None)
        }));
        mock_client.push_melt_quote_status_response(Ok(MeltQuoteBolt11Response {
            payment_preimage: Some("preimage123".to_string()),
            ..bolt11_status(&quote_id, MeltQuoteState::Paid, None)
        }));
        mock_client._set_restore_response(Ok(RestoreResponse {
            outputs: Vec::new(),
            signatures: Vec::new(),
        }));

        tokio::time::timeout(std::time::Duration::from_secs(5), pending.into_future())
            .await
            .expect("wait timed out")
            .expect("melt should finalize");

        let transactions = db.list_transactions(None, None, None).await.unwrap();
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].metadata, metadata);
    }
}
