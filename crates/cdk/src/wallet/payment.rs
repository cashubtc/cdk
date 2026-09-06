//! Outgoing payment quotes, plans, and receipts.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::advanced::{PaymentExecutionOptions, PaymentFunding, PaymentPrepareOptions};
use super::operation::{OperationId, OperationKind, OperationReference, OperationState};
use super::{MeltOutcome, Wallet, WalletIdentity};
use crate::nuts::{MeltOptions, MeltQuoteState, PaymentMethod};
use crate::{Amount, Error};

/// Stable identifier for an outgoing payment quote.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PaymentQuoteId(String);

impl PaymentQuoteId {
    /// Create an identifier from the mint-provided value.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Borrow the mint-provided value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PaymentQuoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<String> for PaymentQuoteId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// Typed outgoing payment target.
#[derive(Clone)]
pub enum PaymentTarget {
    /// BOLT11 invoice and the amount behavior to use when requesting a quote.
    Bolt11 {
        /// Invoice string.
        invoice: String,
        /// Whether to use the invoice amount, supply an amount, or make a partial payment.
        amount: Bolt11PaymentAmount,
    },
    /// BOLT12 offer and requested milli-satoshi amount.
    Bolt12 {
        /// Offer string.
        offer: String,
        /// Whether to use the offer amount or supply one for an amountless offer.
        amount: Bolt12PaymentAmount,
    },
    /// Bitcoin address payment.
    Onchain {
        /// Destination address.
        address: String,
        /// Amount sent to the address.
        amount: Amount,
        /// Optional absolute fee ceiling.
        max_fee: Option<Amount>,
    },
    /// Extension payment rail.
    Custom {
        /// Mint-advertised method name.
        method: String,
        /// Method-specific payment request.
        request: String,
        /// Optional amount for variable-amount methods.
        amount: Option<Amount>,
        /// Method-specific JSON understood by the mint.
        extra: Option<String>,
    },
}

impl PaymentTarget {
    /// Pay the full amount encoded in a BOLT11 invoice.
    pub fn bolt11(invoice: impl Into<String>) -> Self {
        Self::Bolt11 {
            invoice: invoice.into(),
            amount: Bolt11PaymentAmount::Invoice,
        }
    }

    /// Pay an amountless BOLT11 invoice using a milli-satoshi amount.
    pub fn bolt11_amountless(invoice: impl Into<String>, amount_msat: Amount) -> Self {
        Self::Bolt11 {
            invoice: invoice.into(),
            amount: Bolt11PaymentAmount::Amountless(amount_msat),
        }
    }

    /// Pay part of a BOLT11 invoice using multi-part payment semantics.
    pub fn bolt11_mpp(invoice: impl Into<String>, amount_msat: Amount) -> Self {
        Self::Bolt11 {
            invoice: invoice.into(),
            amount: Bolt11PaymentAmount::Mpp(amount_msat),
        }
    }

    /// Pay the amount encoded in a BOLT12 offer.
    pub fn bolt12(offer: impl Into<String>) -> Self {
        Self::Bolt12 {
            offer: offer.into(),
            amount: Bolt12PaymentAmount::Offer,
        }
    }

    /// Pay an amountless BOLT12 offer using a milli-satoshi amount.
    pub fn bolt12_amountless(offer: impl Into<String>, amount_msat: Amount) -> Self {
        Self::Bolt12 {
            offer: offer.into(),
            amount: Bolt12PaymentAmount::Amountless(amount_msat),
        }
    }
}

/// Amount behavior for a BOLT11 payment quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Bolt11PaymentAmount {
    /// Use the amount encoded in the invoice.
    #[default]
    Invoice,
    /// Supply a milli-satoshi amount for an amountless invoice.
    Amountless(Amount),
    /// Request a partial multi-part payment in milli-satoshis.
    Mpp(Amount),
}

/// Amount behavior for a BOLT12 payment quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Bolt12PaymentAmount {
    /// Use the amount encoded in the offer.
    #[default]
    Offer,
    /// Supply a milli-satoshi amount for an amountless offer.
    Amountless(Amount),
}

impl fmt::Debug for PaymentTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bolt11 { amount, .. } => f
                .debug_struct("Bolt11")
                .field("invoice", &"[REDACTED]")
                .field("amount", amount)
                .finish(),
            Self::Bolt12 { amount, .. } => f
                .debug_struct("Bolt12")
                .field("offer", &"[REDACTED]")
                .field("amount", amount)
                .finish(),
            Self::Onchain {
                amount, max_fee, ..
            } => f
                .debug_struct("Onchain")
                .field("address", &"[REDACTED]")
                .field("amount", amount)
                .field("max_fee", max_fee)
                .finish(),
            Self::Custom { method, amount, .. } => f
                .debug_struct("Custom")
                .field("method", method)
                .field("request", &"[REDACTED]")
                .field("amount", amount)
                .field("extra", &"[REDACTED]")
                .finish(),
        }
    }
}

/// Request for one or more outgoing payment quotes.
#[derive(Debug, Clone)]
pub struct PaymentQuoteRequest {
    /// Destination and payment rail.
    pub target: PaymentTarget,
    /// Application metadata persisted when a quote is prepared.
    pub metadata: HashMap<String, String>,
}

/// Resolution strategy for an email-like payment address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressPaymentRoute {
    /// Resolve the address with LNURL-pay and quote its BOLT11 invoice.
    LightningAddress,
    /// Resolve the address with BIP-353 and require a BOLT12 offer.
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    Bip353 {
        /// Network used to validate addresses in the resolved Bitcoin URI.
        network: bitcoin::Network,
    },
    /// Prefer BIP-353 when the mint supports BOLT12, falling back to LNURL-pay
    /// only when DNS resolution is unavailable.
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    Automatic {
        /// Network used to validate addresses in the resolved Bitcoin URI.
        network: bitcoin::Network,
    },
}

impl Default for AddressPaymentRoute {
    fn default() -> Self {
        #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
        {
            Self::Automatic {
                network: bitcoin::Network::Bitcoin,
            }
        }
        #[cfg(not(all(feature = "bip353", not(target_arch = "wasm32"))))]
        {
            Self::LightningAddress
        }
    }
}

/// Request to resolve and quote an email-like payment address.
#[derive(Clone)]
pub struct AddressPaymentRequest {
    /// Lightning or BIP-353 address.
    pub address: String,
    /// Payment amount in millisatoshis.
    pub amount_msat: Amount,
    /// Resolution behavior.
    pub route: AddressPaymentRoute,
    /// Application metadata persisted with the payment plan.
    pub metadata: HashMap<String, String>,
}

impl fmt::Debug for AddressPaymentRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AddressPaymentRequest")
            .field("address", &"[REDACTED]")
            .field("amount_msat", &self.amount_msat)
            .field("route", &self.route)
            .field("metadata", &self.metadata)
            .finish()
    }
}

impl AddressPaymentRequest {
    /// Create a request using the platform's preferred address resolution.
    pub fn new(address: impl Into<String>, amount_msat: Amount) -> Self {
        Self {
            address: address.into(),
            amount_msat,
            route: AddressPaymentRoute::default(),
            metadata: HashMap::new(),
        }
    }

    /// Create an LNURL-pay-only request.
    pub fn lightning_address(address: impl Into<String>, amount_msat: Amount) -> Self {
        Self {
            address: address.into(),
            amount_msat,
            route: AddressPaymentRoute::LightningAddress,
            metadata: HashMap::new(),
        }
    }
}

/// Result of quoting an outgoing payment target.
#[derive(Clone)]
pub enum PaymentQuoteResult {
    /// A single quote for Lightning or an extension payment rail.
    Single(PaymentSession),
    /// On-chain fee and confirmation-target alternatives.
    Options(Vec<PaymentSession>),
}

impl fmt::Debug for PaymentQuoteResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Single(session) => f.debug_tuple("Single").field(session).finish(),
            Self::Options(sessions) => f
                .debug_tuple("Options")
                .field(&format_args!("{} sessions", sessions.len()))
                .finish(),
        }
    }
}

impl PaymentQuoteResult {
    /// Return the only session, or fail when the target produced fee alternatives.
    pub fn into_single(self) -> Result<PaymentSession, Error> {
        match self {
            Self::Single(session) => Ok(session),
            Self::Options(_) => Err(Error::InvalidOperationState),
        }
    }

    /// Return every session represented by the result.
    pub fn into_sessions(self) -> Vec<PaymentSession> {
        match self {
            Self::Single(session) => vec![session],
            Self::Options(sessions) => sessions,
        }
    }
}

impl PaymentQuoteRequest {
    /// Create a request with no application metadata.
    pub fn new(target: PaymentTarget) -> Self {
        Self {
            target,
            metadata: HashMap::new(),
        }
    }
}

/// Lifecycle of an outgoing payment quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PaymentState {
    /// The payment has not started.
    Unpaid,
    /// The mint is still processing the payment.
    Pending,
    /// The payment completed successfully.
    Paid,
    /// The mint reported a definitive payment failure.
    Failed,
    /// The mint cannot yet determine the result.
    Unknown,
}

impl fmt::Display for PaymentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unpaid => "unpaid",
            Self::Pending => "pending",
            Self::Paid => "paid",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        })
    }
}

impl From<MeltQuoteState> for PaymentState {
    fn from(value: MeltQuoteState) -> Self {
        match value {
            MeltQuoteState::Unpaid => Self::Unpaid,
            MeltQuoteState::Pending => Self::Pending,
            MeltQuoteState::Paid => Self::Paid,
            MeltQuoteState::Failed => Self::Failed,
            MeltQuoteState::Unknown => Self::Unknown,
        }
    }
}

/// Public preview of an outgoing payment quote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentQuote {
    /// Stable quote identifier.
    pub id: PaymentQuoteId,
    /// Amount delivered by the mint.
    pub amount: Amount,
    /// Maximum mint fee reserved for the payment.
    pub fee_reserve: Amount,
    /// Current quote state.
    pub state: PaymentState,
    /// Expiry as a Unix timestamp.
    pub expires_at: u64,
    /// Estimated confirmation target for on-chain payments.
    pub estimated_blocks: Option<u32>,
    /// Payment rail.
    pub method: PaymentMethod,
}

/// A quote that can be prepared into a fund-reserving payment plan.
#[derive(Clone)]
pub struct PaymentSession {
    inner: Arc<PaymentSessionInner>,
}

struct PaymentSessionInner {
    wallet: Wallet,
    quote: crate::wallet::MeltQuote,
    metadata: HashMap<String, String>,
    select_before_prepare: bool,
}

impl fmt::Debug for PaymentSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaymentSession")
            .field("quote_id", &self.inner.quote.id)
            .field("amount", &self.inner.quote.amount)
            .field("fee_reserve", &self.inner.quote.fee_reserve)
            .finish_non_exhaustive()
    }
}

impl PaymentSession {
    pub(super) fn new(
        wallet: Wallet,
        quote: crate::wallet::MeltQuote,
        metadata: HashMap<String, String>,
        select_before_prepare: bool,
    ) -> Self {
        Self {
            inner: Arc::new(PaymentSessionInner {
                wallet,
                quote,
                metadata,
                select_before_prepare,
            }),
        }
    }

    /// Immutable quote details for user review.
    pub fn quote(&self) -> PaymentQuote {
        payment_quote(&self.inner.quote)
    }

    /// Stable mint-provided quote identifier.
    pub fn id(&self) -> PaymentQuoteId {
        PaymentQuoteId::new(self.inner.quote.id.clone())
    }

    /// Wallet that owns this payment quote.
    pub fn wallet_identity(&self) -> WalletIdentity {
        self.inner.wallet.identity()
    }

    /// Refresh a persisted quote from the mint.
    pub async fn refresh(&self) -> Result<PaymentQuote, Error> {
        let quote = payment_quote(
            &self
                .inner
                .wallet
                .check_melt_quote_status(&self.inner.quote.id)
                .await?,
        );
        self.inner.wallet.publish_operation_event(
            OperationReference::PaymentQuote(quote.id.clone()),
            OperationKind::Payment,
            match quote.state {
                PaymentState::Unpaid => OperationState::Ready,
                PaymentState::Pending | PaymentState::Unknown => OperationState::Pending,
                PaymentState::Paid => OperationState::Completed,
                PaymentState::Failed => OperationState::Failed,
            },
            Some(quote.amount),
        );
        if matches!(quote.state, PaymentState::Paid | PaymentState::Failed) {
            self.inner.wallet.publish_balance_event().await;
            self.inner
                .wallet
                .publish_quote_transactions(quote.id.as_str())
                .await;
        }
        Ok(quote)
    }

    /// Reserve funds and create a confirm-or-cancel payment plan.
    pub async fn prepare(&self) -> Result<PaymentPlan, Error> {
        self.prepare_with(PaymentPrepareOptions::default()).await
    }

    /// Reserve explicit wallet, proof, or token funds for this payment.
    pub async fn prepare_with(&self, options: PaymentPrepareOptions) -> Result<PaymentPlan, Error> {
        let quote = if self.inner.select_before_prepare {
            self.inner
                .wallet
                .select_onchain_melt_quote(self.inner.quote.clone())
                .await?
        } else {
            self.inner.quote.clone()
        };
        let prepared = match options.funding {
            PaymentFunding::Wallet => {
                self.inner
                    .wallet
                    .prepare_melt(&quote.id, self.inner.metadata.clone())
                    .await?
            }
            PaymentFunding::Proofs(proofs) => {
                self.inner
                    .wallet
                    .prepare_melt_proofs(&quote.id, proofs, self.inner.metadata.clone())
                    .await?
            }
            PaymentFunding::Token(token) => {
                self.inner
                    .wallet
                    .prepare_melt_token(&quote.id, &token, self.inner.metadata.clone())
                    .await?
            }
        };
        // Another session may select a different on-chain fee option before
        // this preparation reserves the quote. Never return a plan for an
        // option the caller did not select; no payment has started yet.
        if prepared.quote().fee_index != quote.fee_index
            || prepared.quote().fee_reserve != quote.fee_reserve
            || prepared.quote().amount != quote.amount
        {
            prepared.cancel().await?;
            return Err(Error::ConcurrentUpdate);
        }
        match PaymentPlan::from_prepared(self.inner.wallet.clone(), &prepared) {
            Ok(plan) => {
                self.inner.wallet.publish_operation_event(
                    OperationReference::Workflow(plan.operation_id()),
                    OperationKind::Payment,
                    OperationState::AwaitingExecution,
                    Some(plan.amount()),
                );
                self.inner.wallet.publish_balance_event().await;
                Ok(plan)
            }
            Err(error) => {
                if let Err(cleanup_error) = prepared.cancel().await {
                    tracing::warn!(
                        "Could not cancel payment plan after construction failed: {}",
                        cleanup_error
                    );
                }
                Err(error)
            }
        }
    }

    /// Prepare and execute this payment with the default policy.
    pub async fn execute(&self) -> Result<PaymentReceipt, Error> {
        self.prepare().await?.execute().await
    }

    /// Prepare and submit this payment, returning when asynchronous processing
    /// has been accepted or the payment has completed.
    pub async fn submit(&self) -> Result<PaymentConfirmation, Error> {
        self.prepare().await?.submit().await
    }
}

pub(super) fn payment_quote(quote: &crate::wallet::MeltQuote) -> PaymentQuote {
    PaymentQuote {
        id: PaymentQuoteId::new(quote.id.clone()),
        amount: quote.amount,
        fee_reserve: quote.fee_reserve,
        state: quote.state.into(),
        expires_at: quote.expiry,
        estimated_blocks: quote.estimated_blocks,
        method: quote.payment_method.clone(),
    }
}

/// Receipt for a successfully finalized outgoing payment.
#[derive(Clone, PartialEq, Eq)]
pub struct PaymentReceipt {
    /// Durable operation identifier.
    pub operation_id: OperationId,
    /// Mint quote identifier.
    pub quote_id: PaymentQuoteId,
    /// Method-specific settlement proof, such as a Lightning preimage.
    pub payment_proof: Option<String>,
    /// Value delivered by the payment.
    pub amount: Amount,
    /// Actual fee charged.
    pub fee_paid: Amount,
}

impl fmt::Debug for PaymentReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaymentReceipt")
            .field("operation_id", &self.operation_id)
            .field("quote_id", &self.quote_id)
            .field(
                "payment_proof",
                &self.payment_proof.as_ref().map(|_| "[REDACTED]"),
            )
            .field("amount", &self.amount)
            .field("fee_paid", &self.fee_paid)
            .finish()
    }
}

fn payment_receipt(
    operation_id: OperationId,
    finalized: cdk_common::common::FinalizedMelt,
) -> PaymentReceipt {
    PaymentReceipt {
        operation_id,
        quote_id: PaymentQuoteId::new(finalized.quote_id()),
        payment_proof: finalized.payment_proof().map(str::to_owned),
        amount: finalized.amount(),
        fee_paid: finalized.fee_paid(),
    }
}

/// Outgoing payment accepted for asynchronous processing by the mint.
#[derive(Clone)]
pub struct PendingPayment {
    wallet: Arc<Wallet>,
    quote_id: PaymentQuoteId,
    operation_id: OperationId,
}

impl fmt::Debug for PendingPayment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingPayment")
            .field("operation_id", &self.operation_id)
            .field("quote_id", &self.quote_id)
            .finish_non_exhaustive()
    }
}

impl PendingPayment {
    pub(super) fn from_pending(wallet: Wallet, pending: &crate::wallet::PendingMelt) -> Self {
        Self {
            wallet: Arc::new(wallet),
            quote_id: PaymentQuoteId::new(pending.quote_id()),
            operation_id: pending.operation_id().into(),
        }
    }

    /// Quote identifier for this pending payment.
    pub fn quote_id(&self) -> &PaymentQuoteId {
        &self.quote_id
    }

    /// Durable operation identifier for this pending payment.
    pub const fn operation_id(&self) -> OperationId {
        self.operation_id
    }

    /// Wait for the payment to finalize.
    pub async fn wait(&self) -> Result<PaymentReceipt, Error> {
        let finalized = self
            .wallet
            .wait_pending_melt(self.operation_id.as_uuid())
            .await?;
        let receipt = payment_receipt(self.operation_id, finalized);
        self.wallet.publish_operation_event(
            OperationReference::Workflow(self.operation_id),
            OperationKind::Payment,
            OperationState::Completed,
            Some(receipt.amount),
        );
        self.wallet.publish_balance_event().await;
        self.wallet
            .publish_transaction_events(self.operation_id.as_uuid())
            .await;
        Ok(receipt)
    }
}

/// Result of async-preferred payment confirmation.
#[derive(Debug, Clone)]
pub enum PaymentConfirmation {
    /// Payment finalized during confirmation.
    Completed(PaymentReceipt),
    /// The mint accepted asynchronous processing.
    Pending(PendingPayment),
}

/// Reviewable, durable outgoing payment plan.
#[derive(Clone)]
#[must_use = "execute or cancel the plan to release its reserved funds"]
pub struct PaymentPlan {
    wallet: Wallet,
    operation_id: OperationId,
    quote_id: PaymentQuoteId,
    amount: Amount,
    maximum_fee: Amount,
}

impl fmt::Debug for PaymentPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaymentPlan")
            .field("operation_id", &self.operation_id)
            .field("quote_id", &self.quote_id)
            .field("amount", &self.amount)
            .field("maximum_fee", &self.maximum_fee)
            .finish_non_exhaustive()
    }
}

impl PaymentPlan {
    pub(super) fn from_prepared(
        wallet: Wallet,
        prepared: &crate::wallet::PreparedMelt,
    ) -> Result<Self, Error> {
        let maximum_fee = prepared
            .quote()
            .fee_reserve
            .checked_add(prepared.swap_fee())
            .and_then(|fee| fee.checked_add(prepared.input_fee()))
            .ok_or(Error::AmountOverflow)?;
        Ok(Self {
            wallet,
            operation_id: prepared.operation_id().into(),
            quote_id: PaymentQuoteId::new(prepared.quote().id.clone()),
            amount: prepared.amount(),
            maximum_fee,
        })
    }

    /// Durable operation identifier used to resume this plan.
    pub const fn operation_id(&self) -> OperationId {
        self.operation_id
    }

    /// Mint quote identifier.
    pub fn quote_id(&self) -> &PaymentQuoteId {
        &self.quote_id
    }

    /// Value delivered by the payment.
    pub const fn amount(&self) -> Amount {
        self.amount
    }

    /// Maximum mint, swap, and input fee charged by this plan.
    pub const fn maximum_fee(&self) -> Amount {
        self.maximum_fee
    }

    /// Wallet that owns this payment plan.
    pub fn wallet_identity(&self) -> WalletIdentity {
        self.wallet.identity()
    }

    /// Execute the plan and wait for a successful receipt.
    pub async fn execute(&self) -> Result<PaymentReceipt, Error> {
        self.execute_with(PaymentExecutionOptions::default()).await
    }

    /// Execute with explicit proof-reissue behavior and wait for a receipt.
    pub async fn execute_with(
        &self,
        options: PaymentExecutionOptions,
    ) -> Result<PaymentReceipt, Error> {
        let finalized = self
            .wallet
            .confirm_prepared_melt_with_options(self.operation_id.as_uuid(), options.into())
            .await?;
        let receipt = payment_receipt(self.operation_id, finalized);
        self.publish_completed(&receipt).await;
        Ok(receipt)
    }

    /// Submit the plan, returning early when the mint accepts async processing.
    pub async fn submit(&self) -> Result<PaymentConfirmation, Error> {
        self.submit_with(PaymentExecutionOptions::default()).await
    }

    /// Submit with explicit proof-reissue behavior, returning on async acceptance.
    pub async fn submit_with(
        &self,
        options: PaymentExecutionOptions,
    ) -> Result<PaymentConfirmation, Error> {
        let outcome = self
            .wallet
            .confirm_prepared_melt_prefer_async_with_options(
                self.operation_id.as_uuid(),
                options.into(),
            )
            .await?;
        match outcome {
            MeltOutcome::Paid(finalized) => {
                let receipt = payment_receipt(self.operation_id, finalized);
                self.publish_completed(&receipt).await;
                Ok(PaymentConfirmation::Completed(receipt))
            }
            MeltOutcome::Pending(pending) => {
                self.wallet.publish_operation_event(
                    OperationReference::Workflow(self.operation_id),
                    OperationKind::Payment,
                    OperationState::Pending,
                    Some(self.amount),
                );
                self.wallet.publish_balance_event().await;
                self.wallet
                    .publish_transaction_events(self.operation_id.as_uuid())
                    .await;
                Ok(PaymentConfirmation::Pending(PendingPayment::from_pending(
                    self.wallet.clone(),
                    &pending,
                )))
            }
        }
    }

    /// Cancel the plan and release reserved funds.
    pub async fn cancel(&self) -> Result<(), Error> {
        self.wallet
            .cancel_prepared_melt(self.operation_id.as_uuid())
            .await?;
        self.wallet.publish_operation_event(
            OperationReference::Workflow(self.operation_id),
            OperationKind::Payment,
            OperationState::Canceled,
            Some(self.amount),
        );
        self.wallet.publish_balance_event().await;
        Ok(())
    }

    async fn publish_completed(&self, receipt: &PaymentReceipt) {
        self.wallet.publish_operation_event(
            OperationReference::Workflow(self.operation_id),
            OperationKind::Payment,
            OperationState::Completed,
            Some(receipt.amount),
        );
        self.wallet.publish_balance_event().await;
        self.wallet
            .publish_transaction_events(self.operation_id.as_uuid())
            .await;
    }
}

impl Wallet {
    /// Quote an outgoing payment.
    ///
    /// On-chain requests can return several sessions with different fee and
    /// confirmation targets. Other payment rails return one session.
    pub async fn quote_payment(
        &self,
        request: PaymentQuoteRequest,
    ) -> Result<PaymentQuoteResult, Error> {
        let metadata = request.metadata;
        let result = match request.target {
            PaymentTarget::Bolt11 { invoice, amount } => {
                let options = match amount {
                    Bolt11PaymentAmount::Invoice => None,
                    Bolt11PaymentAmount::Amountless(amount_msat) => {
                        Some(MeltOptions::new_amountless(amount_msat))
                    }
                    Bolt11PaymentAmount::Mpp(amount_msat) => {
                        Some(MeltOptions::new_mpp(amount_msat))
                    }
                };
                let quote = self
                    .melt_quote::<PaymentMethod, _>(PaymentMethod::BOLT11, invoice, options, None)
                    .await?;
                PaymentQuoteResult::Single(PaymentSession::new(
                    self.clone(),
                    quote,
                    metadata,
                    false,
                ))
            }
            PaymentTarget::Bolt12 { offer, amount } => {
                let options = match amount {
                    Bolt12PaymentAmount::Offer => None,
                    Bolt12PaymentAmount::Amountless(amount_msat) => {
                        Some(MeltOptions::new_amountless(amount_msat))
                    }
                };
                let quote = self
                    .melt_quote::<PaymentMethod, _>(PaymentMethod::BOLT12, offer, options, None)
                    .await?;
                PaymentQuoteResult::Single(PaymentSession::new(
                    self.clone(),
                    quote,
                    metadata,
                    false,
                ))
            }
            PaymentTarget::Onchain {
                address,
                amount,
                max_fee,
            } => PaymentQuoteResult::Options(
                self.quote_onchain_melt_options(&address, amount, max_fee)
                    .await?
                    .into_iter()
                    .map(|quote| PaymentSession::new(self.clone(), quote, metadata.clone(), true))
                    .collect(),
            ),
            PaymentTarget::Custom {
                method,
                request,
                amount,
                extra,
            } => {
                let options = amount.map(MeltOptions::new_amountless);
                let quote = self
                    .melt_quote::<PaymentMethod, _>(
                        PaymentMethod::from(method),
                        request,
                        options,
                        extra,
                    )
                    .await?;
                PaymentQuoteResult::Single(PaymentSession::new(
                    self.clone(),
                    quote,
                    metadata,
                    false,
                ))
            }
        };
        for session in result.clone().into_sessions() {
            let quote = session.quote();
            self.publish_operation_event(
                OperationReference::PaymentQuote(quote.id.clone()),
                OperationKind::Payment,
                OperationState::Ready,
                Some(quote.amount),
            );
        }
        Ok(result)
    }

    /// Resolve an email-like address and quote its Lightning payment.
    pub async fn quote_address_payment(
        &self,
        request: AddressPaymentRequest,
    ) -> Result<PaymentSession, Error> {
        let quote = match request.route {
            AddressPaymentRoute::LightningAddress => {
                self.melt_lightning_address_quote(&request.address, request.amount_msat)
                    .await?
            }
            #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
            AddressPaymentRoute::Bip353 { network } => {
                self.melt_bip353_quote(&request.address, request.amount_msat, network)
                    .await?
            }
            #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
            AddressPaymentRoute::Automatic { network } => {
                self.melt_human_readable_quote(&request.address, request.amount_msat, network)
                    .await?
            }
        };
        let session = PaymentSession::new(self.clone(), quote, request.metadata, false);
        let preview = session.quote();
        self.publish_operation_event(
            OperationReference::PaymentQuote(preview.id.clone()),
            OperationKind::Payment,
            OperationState::Ready,
            Some(preview.amount),
        );
        Ok(session)
    }

    /// Resume a prepared outgoing payment after a process restart.
    pub async fn resume_payment(&self, operation_id: OperationId) -> Result<PaymentPlan, Error> {
        let prepared = self.prepared_melt(operation_id.as_uuid()).await?;
        PaymentPlan::from_prepared(self.clone(), &prepared)
    }

    /// Resume a locally persisted outgoing payment quote before preparation.
    pub async fn resume_payment_quote(
        &self,
        quote_id: PaymentQuoteId,
    ) -> Result<PaymentSession, Error> {
        let quote = self
            .localstore
            .get_melt_quote(quote_id.as_str())
            .await?
            .ok_or(Error::UnknownQuote)?;
        if quote.mint_url.as_ref() != Some(&self.mint_url) || quote.unit != self.unit {
            return Err(Error::InvalidOperationState);
        }
        Ok(PaymentSession::new(
            self.clone(),
            quote,
            HashMap::new(),
            false,
        ))
    }

    /// Resume a payment that the mint is still processing.
    pub async fn resume_pending_payment(
        &self,
        operation_id: OperationId,
    ) -> Result<PendingPayment, Error> {
        let pending = self.pending_melt(operation_id.as_uuid()).await?;
        Ok(PendingPayment::from_pending(self.clone(), &pending))
    }
}
