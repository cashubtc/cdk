//! Portable application-facing wallet API.
//!
//! Rust applications and generated UniFFI bindings use these same objects.
//! Protocol-level proof, keyset, swap, authentication, and database APIs remain
//! available from `cdk` for applications that deliberately need the engine.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use cdk::nuts::nut00::ProofsMethods;

use crate::database::WalletStore;
use crate::error::FfiError;
use crate::types::{
    Amount, CurrencyUnit, MintUrl, PaymentMethod, Restored, SendKind, TransactionDirection,
    TransactionStatus,
};
use crate::wallet::{RateLimit, Wallet, WalletConfig};
use crate::wallet_repository::{WalletRepository, WalletRepositoryConfig};

/// Configuration for opening one mint-and-unit wallet.
#[derive(Clone, uniffi::Record)]
pub struct WalletOpenRequest {
    /// Mint URL.
    pub mint_url: String,
    /// Currency unit managed by this wallet.
    pub unit: CurrencyUnit,
    /// BIP-39 mnemonic used for deterministic wallet keys.
    pub mnemonic: String,
    /// Durable wallet storage.
    pub store: WalletStore,
    /// Optional operational tuning. Defaults are suitable for most apps.
    #[uniffi(default = None)]
    pub config: Option<WalletConfig>,
}

impl fmt::Debug for WalletOpenRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WalletOpenRequest")
            .field("mint_url", &self.mint_url)
            .field("unit", &self.unit)
            .field("mnemonic", &"[REDACTED]")
            .field("store", &"[REDACTED]")
            .field("config", &self.config)
            .finish()
    }
}

/// Configuration for opening a multi-mint wallet.
#[derive(Clone, uniffi::Record)]
pub struct CashuWalletOpenRequest {
    /// BIP-39 mnemonic used by every mint wallet in the portfolio.
    pub mnemonic: String,
    /// Durable wallet storage.
    pub store: WalletStore,
    /// Optional shared proxy URL.
    #[uniffi(default = None)]
    pub proxy_url: Option<String>,
    /// Optional repository-wide request pacing.
    #[uniffi(default = None)]
    pub rate_limit: Option<RateLimit>,
}

impl fmt::Debug for CashuWalletOpenRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CashuWalletOpenRequest")
            .field("mnemonic", &"[REDACTED]")
            .field("store", &"[REDACTED]")
            .field(
                "proxy_url",
                &self.proxy_url.as_ref().map(|_| "[CONFIGURED]"),
            )
            .field("rate_limit", &self.rate_limit)
            .finish()
    }
}

/// Identity and unit of a mint wallet.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WalletIdentity {
    /// Mint URL.
    pub mint_url: MintUrl,
    /// Currency unit.
    pub unit: CurrencyUnit,
}

/// Balances split by spendability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Record)]
pub struct WalletBalance {
    /// Funds that can be spent now.
    pub available: Amount,
    /// Funds committed to an in-flight payment or an unclaimed send.
    pub pending: Amount,
    /// Funds reserved by a prepared local operation.
    pub reserved: Amount,
}

/// Whether synchronization may contact the mint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SyncPolicy {
    /// Read local state only.
    LocalOnly,
    /// Reconcile sagas, quotes, pending proofs, and payments with the mint.
    Online,
}

/// Lifecycle of an incoming minting session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MintingState {
    /// The payer has not completed the payment.
    Unpaid,
    /// The mint received value and ecash can be claimed.
    Paid,
    /// All paid value has been claimed into the wallet.
    Issued,
}

impl From<cdk::nuts::MintQuoteState> for MintingState {
    fn from(state: cdk::nuts::MintQuoteState) -> Self {
        match state {
            cdk::nuts::MintQuoteState::Unpaid => Self::Unpaid,
            cdk::nuts::MintQuoteState::Paid => Self::Paid,
            cdk::nuts::MintQuoteState::Issued => Self::Issued,
        }
    }
}

/// Lifecycle of an outgoing payment quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PaymentState {
    /// The payment has not started.
    Unpaid,
    /// The mint is still processing the payment.
    Pending,
    /// The payment completed successfully.
    Paid,
    /// The mint reported a definitive payment failure.
    Failed,
    /// The mint cannot yet determine the payment result.
    Unknown,
}

impl From<cdk::nuts::MeltQuoteState> for PaymentState {
    fn from(state: cdk::nuts::MeltQuoteState) -> Self {
        match state {
            cdk::nuts::MeltQuoteState::Unpaid => Self::Unpaid,
            cdk::nuts::MeltQuoteState::Pending => Self::Pending,
            cdk::nuts::MeltQuoteState::Paid => Self::Paid,
            cdk::nuts::MeltQuoteState::Failed => Self::Failed,
            cdk::nuts::MeltQuoteState::Unknown => Self::Unknown,
        }
    }
}

/// Result of one explicit wallet synchronization.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SyncReport {
    /// Wallet reconciled by this synchronization pass.
    pub wallet: WalletIdentity,
    /// Balance after synchronization.
    pub balance: WalletBalance,
    /// Interrupted operations completed successfully.
    pub recovered_operations: u64,
    /// Interrupted operations rolled back safely.
    pub compensated_operations: u64,
    /// Operations that remain pending.
    pub pending_operations: u64,
    /// Operations that could not be reconciled.
    pub failed_operations: u64,
    /// Paid mint quotes claimed during synchronization.
    pub claimed_amount: Amount,
    /// Pending proofs restored to the available balance.
    pub recovered_amount: Amount,
    /// Pending payments finalized during synchronization.
    pub finalized_payments: u64,
}

/// One wallet and its current balances.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WalletBalanceEntry {
    /// Wallet whose balance was read.
    pub wallet: WalletIdentity,
    /// Spendability breakdown.
    pub balance: WalletBalance,
}

/// A request to create ecash for an incoming payment.
#[derive(Debug, Clone, uniffi::Record)]
pub struct MintRequest {
    /// Payment rail offered to the payer.
    pub method: PaymentMethod,
    /// Requested amount. Some payment rails support an unspecified amount.
    #[uniffi(default = None)]
    pub amount: Option<Amount>,
    /// Human-readable payment description.
    #[uniffi(default = None)]
    pub description: Option<String>,
    /// Payment-method-specific JSON understood by the mint.
    #[uniffi(default = None)]
    pub extra: Option<String>,
}

/// Public state of a minting session. Internal quote locks and signing keys are
/// deliberately not part of the application API.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MintSessionState {
    /// Stable quote ID.
    pub id: String,
    /// Payment request to present to the payer.
    pub payment_request: String,
    /// Current mint-reported state.
    pub state: MintingState,
    /// Requested amount, when fixed.
    pub amount: Option<Amount>,
    /// Amount received by the mint.
    pub amount_paid: Amount,
    /// Amount already issued into this wallet.
    pub amount_claimed: Amount,
    /// Quote expiry as a Unix timestamp.
    pub expires_at: u64,
    /// Payment rail used by this session.
    pub method: PaymentMethod,
}

/// Durable handle for an incoming mint quote.
#[derive(uniffi::Object)]
pub struct MintSession {
    wallet: Arc<cdk::Wallet>,
    quote_id: String,
    initial_state: MintSessionState,
}

impl fmt::Debug for MintSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MintSession")
            .field("quote_id", &self.quote_id)
            .finish_non_exhaustive()
    }
}

impl MintSession {
    fn from_quote(wallet: Arc<cdk::Wallet>, quote: cdk::wallet::MintQuote) -> Self {
        let initial_state = mint_session_state(&quote);
        Self {
            wallet,
            quote_id: quote.id,
            initial_state,
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl MintSession {
    /// Stable quote ID used to resume this session.
    pub fn id(&self) -> String {
        self.quote_id.clone()
    }

    /// State captured when this handle was created, available without a network request.
    pub fn initial_state(&self) -> MintSessionState {
        self.initial_state.clone()
    }

    /// Refresh the quote from the mint.
    pub async fn refresh(&self) -> Result<MintSessionState, FfiError> {
        let quote = self.wallet.check_mint_quote_status(&self.quote_id).await?;
        Ok(mint_session_state(&quote))
    }

    /// Ensure all paid value is claimed and return the quote's total claimed amount.
    ///
    /// This is idempotent: calling it again after a successful claim returns
    /// the amount already issued for the quote.
    pub async fn claim(&self) -> Result<Amount, FfiError> {
        if let Some(amount) = claimed_mint_quote_amount(&self.wallet, &self.quote_id).await? {
            return Ok(amount);
        }

        let proofs = self
            .wallet
            .mint(&self.quote_id, cdk::amount::SplitTarget::None, None)
            .await;

        match proofs {
            Ok(proofs) => match claimed_mint_quote_amount(&self.wallet, &self.quote_id).await? {
                Some(amount) => Ok(amount),
                None => Ok(proofs.total_amount()?.into()),
            },
            Err(error) => match claimed_mint_quote_amount(&self.wallet, &self.quote_id).await? {
                Some(amount) => Ok(amount),
                None => Err(error.into()),
            },
        }
    }
}

async fn claimed_mint_quote_amount(
    wallet: &cdk::Wallet,
    quote_id: &str,
) -> Result<Option<Amount>, FfiError> {
    let Some(quote) = wallet
        .localstore
        .get_mint_quote(quote_id)
        .await
        .map_err(cdk::Error::from)?
    else {
        return Ok(None);
    };
    ensure_mint_quote_belongs_to_wallet(wallet, &quote)?;
    Ok((quote.state == cdk::nuts::MintQuoteState::Issued).then_some(quote.amount_issued.into()))
}

fn ensure_mint_quote_belongs_to_wallet(
    wallet: &cdk::Wallet,
    quote: &cdk::wallet::MintQuote,
) -> Result<(), FfiError> {
    if quote.mint_url != wallet.mint_url {
        return Err(cdk::Error::IncorrectMint.into());
    }
    if quote.unit != wallet.unit {
        return Err(cdk::Error::UnsupportedUnit.into());
    }
    Ok(())
}

fn mint_session_state(quote: &cdk::wallet::MintQuote) -> MintSessionState {
    MintSessionState {
        id: quote.id.clone(),
        payment_request: quote.request.clone(),
        state: quote.state.into(),
        amount: quote.amount.map(Into::into),
        amount_paid: quote.amount_paid.into(),
        amount_claimed: quote.amount_issued.into(),
        expires_at: quote.expiry,
        method: quote.payment_method.clone().into(),
    }
}

/// High-level send request. Protocol-specific proof and condition controls are
/// intentionally part of the advanced engine API instead.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SendRequest {
    /// Value to transfer.
    pub amount: Amount,
    /// Online/offline selection behavior.
    pub mode: SendKind,
    /// Memo embedded in the token.
    #[uniffi(default = None)]
    pub memo: Option<String>,
    /// Add input fees so the receiver obtains the exact requested value.
    #[uniffi(default = true)]
    pub include_fee: bool,
    /// Application metadata stored with the transaction.
    #[uniffi(default)]
    pub metadata: HashMap<String, String>,
}

/// Reviewable, durable ecash send plan.
#[derive(uniffi::Object)]
pub struct SendPlan {
    wallet: Arc<cdk::Wallet>,
    operation_id: uuid::Uuid,
    amount: Amount,
    fee: Amount,
}

impl fmt::Debug for SendPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendPlan")
            .field("operation_id", &self.operation_id)
            .field("amount", &self.amount)
            .field("fee", &self.fee)
            .finish_non_exhaustive()
    }
}

impl SendPlan {
    fn new(
        wallet: Arc<cdk::Wallet>,
        prepared: &cdk::wallet::PreparedSend,
    ) -> Result<Self, FfiError> {
        let fee = prepared
            .swap_fee()
            .checked_add(prepared.send_fee())
            .ok_or(cdk::Error::AmountOverflow)?;
        Ok(Self {
            wallet,
            operation_id: prepared.operation_id(),
            amount: prepared.amount().into(),
            fee: fee.into(),
        })
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl SendPlan {
    /// Durable operation ID used to resume this plan.
    pub fn operation_id(&self) -> String {
        self.operation_id.to_string()
    }

    /// Value encoded into the token.
    pub fn amount(&self) -> Amount {
        self.amount
    }

    /// Maximum fee charged by this plan.
    pub fn fee(&self) -> Amount {
        self.fee
    }

    /// Confirm the plan and return the encoded Cashu token.
    pub async fn confirm(&self) -> Result<String, FfiError> {
        Ok(self
            .wallet
            .confirm_send(self.operation_id, None)
            .await?
            .to_string())
    }

    /// Cancel the plan and release its reserved funds.
    pub async fn cancel(&self) -> Result<(), FfiError> {
        self.wallet.cancel_send(self.operation_id).await?;
        Ok(())
    }
}

/// High-level token receipt request.
#[derive(Clone, uniffi::Record)]
pub struct ReceiveRequest {
    /// Encoded Cashu token to redeem.
    pub token: String,
    /// Application metadata stored with the transaction.
    #[uniffi(default)]
    pub metadata: HashMap<String, String>,
}

impl fmt::Debug for ReceiveRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReceiveRequest")
            .field("token", &"[REDACTED]")
            .field("metadata", &self.metadata)
            .finish()
    }
}

/// Receipt for a successfully redeemed token.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ReceiveReceipt {
    /// Value credited to the wallet.
    pub amount: Amount,
    /// Wallet that accepted the token.
    pub wallet: WalletIdentity,
}

/// A typed outgoing payment target.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum PaymentTarget {
    /// BOLT11 invoice, optionally amountless.
    Bolt11 {
        invoice: String,
        amount_msat: Option<Amount>,
    },
    /// BOLT12 offer and requested milli-satoshi amount.
    Bolt12 { offer: String, amount_msat: Amount },
    /// Bitcoin address with an amount and optional absolute fee ceiling.
    Onchain {
        address: String,
        amount: Amount,
        max_fee: Option<Amount>,
    },
    /// Extension payment rail.
    Custom {
        method: String,
        request: String,
        amount: Option<Amount>,
        extra: Option<String>,
    },
}

/// Request for one or more payment quotes.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PaymentQuoteRequest {
    /// Destination and payment rail.
    pub target: PaymentTarget,
    /// Application metadata persisted when a quote is prepared.
    #[uniffi(default)]
    pub metadata: HashMap<String, String>,
}

/// Public payment quote preview.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PaymentQuote {
    /// Stable quote ID.
    pub id: String,
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

/// A quote that can be prepared into a proof-reserving payment plan.
#[derive(uniffi::Object)]
pub struct PaymentSession {
    wallet: Arc<cdk::Wallet>,
    quote: cdk::wallet::MeltQuote,
    metadata: HashMap<String, String>,
    select_before_prepare: bool,
}

impl fmt::Debug for PaymentSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaymentSession")
            .field("quote_id", &self.quote.id)
            .field("amount", &self.quote.amount)
            .field("fee_reserve", &self.quote.fee_reserve)
            .finish_non_exhaustive()
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl PaymentSession {
    /// Immutable quote details for UI review.
    pub fn quote(&self) -> PaymentQuote {
        payment_quote(&self.quote)
    }

    /// Reserve proofs and produce a confirm-or-cancel payment plan.
    pub async fn prepare(&self) -> Result<Arc<PaymentPlan>, FfiError> {
        let quote = if self.select_before_prepare {
            self.wallet
                .select_onchain_melt_quote(self.quote.clone())
                .await?
        } else {
            self.quote.clone()
        };
        let prepared = self
            .wallet
            .prepare_melt(&quote.id, self.metadata.clone())
            .await?;
        match PaymentPlan::new(Arc::clone(&self.wallet), &prepared) {
            Ok(plan) => Ok(Arc::new(plan)),
            Err(error) => {
                if let Err(cleanup_error) = prepared.cancel().await {
                    tracing::warn!(
                        "Could not cancel payment plan after facade construction failed: {}",
                        cleanup_error
                    );
                }
                Err(error)
            }
        }
    }
}

fn payment_quote(quote: &cdk::wallet::MeltQuote) -> PaymentQuote {
    PaymentQuote {
        id: quote.id.clone(),
        amount: quote.amount.into(),
        fee_reserve: quote.fee_reserve.into(),
        state: quote.state.into(),
        expires_at: quote.expiry,
        estimated_blocks: quote.estimated_blocks,
        method: quote.payment_method.clone().into(),
    }
}

/// Receipt for a successfully finalized outgoing payment.
///
/// A receipt exists only for a paid operation, so it does not carry a
/// redundant state field. Cashu change proofs remain an engine detail and are
/// not exposed through the application facade.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct PaymentReceipt {
    /// Durable operation ID.
    pub operation_id: String,
    /// Mint quote ID.
    pub quote_id: String,
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
    operation_id: uuid::Uuid,
    finalized: cdk_common::common::FinalizedMelt,
) -> PaymentReceipt {
    PaymentReceipt {
        operation_id: operation_id.to_string(),
        quote_id: finalized.quote_id().to_string(),
        payment_proof: finalized.payment_proof().map(str::to_owned),
        amount: finalized.amount().into(),
        fee_paid: finalized.fee_paid().into(),
    }
}

/// An outgoing payment accepted for asynchronous processing by the mint.
#[derive(uniffi::Object)]
pub struct PendingPayment {
    wallet: Arc<cdk::Wallet>,
    quote_id: String,
    operation_id: uuid::Uuid,
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
    fn new(wallet: Arc<cdk::Wallet>, pending: &cdk::wallet::PendingMelt) -> Self {
        Self {
            wallet,
            quote_id: pending.quote_id().to_string(),
            operation_id: pending.operation_id(),
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl PendingPayment {
    /// Quote ID for this pending payment.
    pub fn quote_id(&self) -> String {
        self.quote_id.clone()
    }

    /// Durable operation ID for this pending payment.
    pub fn operation_id(&self) -> String {
        self.operation_id.to_string()
    }

    /// Wait for this payment to finalize.
    ///
    /// The wallet uses mint notifications when available and bounded polling
    /// as a fallback. Mobile apps should still call
    /// `Wallet::synchronize(SyncPolicy::Online)` after startup or resume.
    pub async fn wait(&self) -> Result<PaymentReceipt, FfiError> {
        let finalized = self.wallet.wait_pending_melt(self.operation_id).await?;
        Ok(payment_receipt(self.operation_id, finalized))
    }
}

/// Result of async-preferred outgoing payment confirmation.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum PaymentConfirmation {
    /// Payment finalized during confirmation.
    Completed { receipt: PaymentReceipt },
    /// Mint accepted async processing and the payment remains pending.
    Pending { payment: Arc<PendingPayment> },
}

/// Reviewable, durable outgoing payment plan.
#[derive(uniffi::Object)]
pub struct PaymentPlan {
    wallet: Arc<cdk::Wallet>,
    operation_id: uuid::Uuid,
    quote_id: String,
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
    fn new(
        wallet: Arc<cdk::Wallet>,
        prepared: &cdk::wallet::PreparedMelt,
    ) -> Result<Self, FfiError> {
        let maximum_fee = prepared
            .quote()
            .fee_reserve
            .checked_add(prepared.swap_fee())
            .and_then(|fee| fee.checked_add(prepared.input_fee()))
            .ok_or(cdk::Error::AmountOverflow)?;
        Ok(Self {
            wallet,
            operation_id: prepared.operation_id(),
            quote_id: prepared.quote().id.clone(),
            amount: prepared.amount().into(),
            maximum_fee: maximum_fee.into(),
        })
    }

    async fn confirm_prefer_async_with_options(
        &self,
        options: cdk::wallet::MeltConfirmOptions,
    ) -> Result<PaymentConfirmation, FfiError> {
        let outcome = self
            .wallet
            .confirm_prepared_melt_prefer_async_with_options(self.operation_id, options)
            .await?;

        match outcome {
            cdk::wallet::MeltOutcome::Paid(finalized) => Ok(PaymentConfirmation::Completed {
                receipt: payment_receipt(self.operation_id, finalized),
            }),
            cdk::wallet::MeltOutcome::Pending(pending) => Ok(PaymentConfirmation::Pending {
                payment: Arc::new(PendingPayment::new(Arc::clone(&self.wallet), &pending)),
            }),
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl PaymentPlan {
    /// Durable operation ID used to resume this plan.
    pub fn operation_id(&self) -> String {
        self.operation_id.to_string()
    }

    /// Mint quote ID.
    pub fn quote_id(&self) -> String {
        self.quote_id.clone()
    }

    /// Value delivered by the payment.
    pub fn amount(&self) -> Amount {
        self.amount
    }

    /// Maximum mint, swap, and input fee charged by this plan.
    pub fn maximum_fee(&self) -> Amount {
        self.maximum_fee
    }

    /// Confirm the plan and wait for a receipt.
    pub async fn confirm(&self) -> Result<PaymentReceipt, FfiError> {
        let finalized = self
            .wallet
            .confirm_prepared_melt_with_options(
                self.operation_id,
                cdk::wallet::MeltConfirmOptions::default(),
            )
            .await?;
        Ok(payment_receipt(self.operation_id, finalized))
    }

    /// Confirm the plan, returning early when the mint accepts async processing.
    pub async fn confirm_prefer_async(&self) -> Result<PaymentConfirmation, FfiError> {
        self.confirm_prefer_async_with_options(cdk::wallet::MeltConfirmOptions::default())
            .await
    }

    /// Cancel the plan and release reserved funds.
    pub async fn cancel(&self) -> Result<(), FfiError> {
        self.wallet.cancel_prepared_melt(self.operation_id).await?;
        Ok(())
    }
}

/// Transaction-history filter.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct HistoryQuery {
    /// Restrict to incoming or outgoing transactions.
    #[uniffi(default = None)]
    pub direction: Option<TransactionDirection>,
    /// Maximum number of newest entries to return.
    #[uniffi(default = None)]
    pub limit: Option<u32>,
}

/// Application-facing wallet history entry.
///
/// Proof identifiers and settlement secrets are intentionally omitted.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HistoryEntry {
    /// Stable transaction ID.
    pub id: String,
    /// Wallet that owns the transaction.
    pub wallet: WalletIdentity,
    /// Incoming or outgoing flow.
    pub direction: TransactionDirection,
    /// Principal value.
    pub amount: Amount,
    /// Fee charged to this wallet.
    pub fee: Amount,
    /// Unix timestamp.
    pub timestamp: u64,
    /// User-visible memo.
    pub memo: Option<String>,
    /// Application metadata.
    pub metadata: HashMap<String, String>,
    /// Related mint or melt quote ID.
    pub quote_id: Option<String>,
    /// Durable operation ID, when the transaction belongs to a saga.
    pub operation_id: Option<String>,
    /// Payment rail, when applicable.
    pub payment_method: Option<PaymentMethod>,
    /// Durable transaction status.
    pub status: TransactionStatus,
}

impl From<cdk_common::wallet::Transaction> for HistoryEntry {
    fn from(transaction: cdk_common::wallet::Transaction) -> Self {
        Self {
            id: transaction.id().to_string(),
            wallet: WalletIdentity {
                mint_url: transaction.mint_url.into(),
                unit: transaction.unit.into(),
            },
            direction: transaction.direction.into(),
            amount: transaction.amount.into(),
            fee: transaction.fee.into(),
            timestamp: transaction.timestamp,
            memo: transaction.memo,
            metadata: transaction.metadata,
            quote_id: transaction.quote_id,
            operation_id: transaction.saga_id.map(|id| id.to_string()),
            payment_method: transaction.payment_method.map(Into::into),
            status: transaction.status.into(),
        }
    }
}

/// Request to move the maximum currently spendable balance between two mint
/// wallets over Lightning.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CrossMintTransferRequest {
    /// Wallet that pays the Lightning invoice.
    pub source: WalletIdentity,
    /// Wallet that receives newly issued ecash.
    pub destination: WalletIdentity,
    /// Application metadata stored with the source transaction.
    #[uniffi(default)]
    pub metadata: HashMap<String, String>,
}

/// Successful cross-mint transfer result.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CrossMintTransferReceipt {
    /// Durable source operation ID.
    pub operation_id: String,
    /// Destination wallet.
    pub destination: WalletIdentity,
    /// Quote claimed at the destination.
    pub destination_quote_id: String,
    /// Amount issued at the destination.
    pub amount: Amount,
    /// Actual source-side fee.
    pub source_fee: Amount,
}

/// Source payment succeeded, but destination issuance still needs to be
/// claimed. Calling `synchronize(Online)` will retry all such paid quotes.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CrossMintClaimPending {
    /// Durable source operation ID.
    pub operation_id: String,
    /// Destination wallet.
    pub destination: WalletIdentity,
    /// Paid destination quote that remains claimable.
    pub destination_quote_id: String,
    /// Amount expected at the destination.
    pub amount: Amount,
    /// Actual source-side fee.
    pub source_fee: Amount,
    /// Why the immediate issuance attempt failed.
    pub error_message: String,
    /// Whether retrying can be useful.
    pub retryable: bool,
}

/// Outcome of confirming a cross-mint transfer.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CrossMintTransferOutcome {
    /// Source payment and destination issuance both completed.
    Completed { receipt: CrossMintTransferReceipt },
    /// Source payment completed; destination issuance is durably recoverable.
    ClaimPending { pending: CrossMintClaimPending },
}

/// Durable, reviewable cross-mint transfer plan.
#[derive(uniffi::Object)]
pub struct CrossMintTransferPlan {
    source: Arc<cdk::Wallet>,
    destination: Arc<cdk::Wallet>,
    operation_id: uuid::Uuid,
    destination_identity: WalletIdentity,
    destination_quote_id: String,
    amount: Amount,
    maximum_fee: Amount,
}

impl fmt::Debug for CrossMintTransferPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CrossMintTransferPlan")
            .field("operation_id", &self.operation_id)
            .field("destination", &self.destination_identity)
            .field("destination_quote_id", &self.destination_quote_id)
            .field("amount", &self.amount)
            .field("maximum_fee", &self.maximum_fee)
            .finish_non_exhaustive()
    }
}

impl CrossMintTransferPlan {
    fn from_operation(
        source: Arc<cdk::Wallet>,
        destination: Arc<cdk::Wallet>,
        operation: &cdk_common::wallet::CrossMintTransferOperation,
    ) -> Result<Self, FfiError> {
        if source.mint_url != operation.source_mint_url
            || source.unit != operation.source_unit
            || destination.mint_url != operation.destination_mint_url
            || destination.unit != operation.destination_unit
        {
            return Err(cdk::Error::InvalidOperationState.into());
        }

        Ok(Self {
            source,
            destination,
            operation_id: operation.operation_id,
            destination_identity: WalletIdentity {
                mint_url: operation.destination_mint_url.clone().into(),
                unit: operation.destination_unit.clone().into(),
            },
            destination_quote_id: operation.destination_quote_id.clone(),
            amount: operation.amount.into(),
            maximum_fee: operation.maximum_fee.into(),
        })
    }

    fn new(
        source: Arc<cdk::Wallet>,
        destination: Arc<cdk::Wallet>,
        prepared: &cdk::wallet::PreparedMelt,
    ) -> Result<Self, FfiError> {
        let cdk::wallet::PreparedMeltPurpose::CrossMintTransfer {
            destination_mint_url,
            destination_unit,
            destination_quote_id,
        } = prepared.purpose()
        else {
            return Err(cdk::Error::InvalidOperationState.into());
        };
        if destination.mint_url != *destination_mint_url || destination.unit != *destination_unit {
            return Err(cdk::Error::InvalidOperationState.into());
        }
        let maximum_fee = prepared
            .quote()
            .fee_reserve
            .checked_add(prepared.input_fee_without_swap())
            .ok_or(cdk::Error::AmountOverflow)?;
        let operation = cdk_common::wallet::CrossMintTransferOperation {
            operation_id: prepared.operation_id(),
            source_mint_url: source.mint_url.clone(),
            source_unit: source.unit.clone(),
            destination_mint_url: destination_mint_url.clone(),
            destination_unit: destination_unit.clone(),
            destination_quote_id: destination_quote_id.clone(),
            amount: prepared.amount(),
            maximum_fee,
        };

        Self::from_operation(source, destination, &operation)
    }

    fn completed(&self, amount: Amount, source_fee: Amount) -> CrossMintTransferOutcome {
        CrossMintTransferOutcome::Completed {
            receipt: CrossMintTransferReceipt {
                operation_id: self.operation_id.to_string(),
                destination: self.destination_identity.clone(),
                destination_quote_id: self.destination_quote_id.clone(),
                amount,
                source_fee,
            },
        }
    }

    async fn destination_issued_amount(&self) -> Result<Option<Amount>, FfiError> {
        let quote = self
            .destination
            .localstore
            .get_mint_quote(&self.destination_quote_id)
            .await
            .map_err(cdk::Error::from)?;
        let Some(quote) = quote else {
            return Ok(None);
        };
        if quote.mint_url != self.destination.mint_url || quote.unit != self.destination.unit {
            return Err(cdk::Error::InvalidOperationState.into());
        }
        Ok(
            (quote.state == cdk::nuts::MintQuoteState::Issued)
                .then_some(quote.amount_issued.into()),
        )
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl CrossMintTransferPlan {
    /// Durable operation ID used to resume this plan.
    pub fn operation_id(&self) -> String {
        self.operation_id.to_string()
    }

    /// Amount expected at the destination.
    pub fn amount(&self) -> Amount {
        self.amount
    }

    /// Maximum combined mint and input fee.
    pub fn maximum_fee(&self) -> Amount {
        self.maximum_fee
    }

    /// Destination wallet.
    pub fn destination(&self) -> WalletIdentity {
        self.destination_identity.clone()
    }

    /// Quote that will issue ecash at the destination.
    pub fn destination_quote_id(&self) -> String {
        self.destination_quote_id.clone()
    }

    /// Pay the destination quote and claim its ecash.
    pub async fn confirm(&self) -> Result<CrossMintTransferOutcome, FfiError> {
        let finalized = match self
            .source
            .confirm_prepared_melt_with_options(
                self.operation_id,
                cdk::wallet::MeltConfirmOptions::skip_swap(),
            )
            .await
        {
            Ok(finalized) => finalized,
            Err(cdk::Error::InvalidOperationState) => {
                self.source
                    .pending_melt(self.operation_id)
                    .await?
                    .wait()
                    .await?
            }
            Err(error) => return Err(error.into()),
        };
        let source_fee: Amount = finalized.fee_paid().into();

        if let Some(amount) = self.destination_issued_amount().await? {
            return Ok(self.completed(amount, source_fee));
        }

        match self
            .destination
            .mint(
                &self.destination_quote_id,
                cdk::amount::SplitTarget::None,
                None,
            )
            .await
        {
            Ok(proofs) => Ok(self.completed(proofs.total_amount()?.into(), source_fee)),
            Err(error) => {
                if let Some(amount) = self.destination_issued_amount().await? {
                    return Ok(self.completed(amount, source_fee));
                }
                let retryable = !error.is_definitive_failure();
                Ok(CrossMintTransferOutcome::ClaimPending {
                    pending: CrossMintClaimPending {
                        operation_id: self.operation_id.to_string(),
                        destination: self.destination_identity.clone(),
                        destination_quote_id: self.destination_quote_id.clone(),
                        amount: self.amount,
                        source_fee,
                        error_message: error.to_string(),
                        retryable,
                    },
                })
            }
        }
    }

    /// Cancel the local plan and release its source proofs.
    pub async fn cancel(&self) -> Result<(), FfiError> {
        self.source.cancel_prepared_melt(self.operation_id).await?;
        Ok(())
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Wallet {
    /// Open a single mint-and-unit wallet without contacting the mint.
    #[uniffi::constructor]
    pub fn open(request: WalletOpenRequest) -> Result<Self, FfiError> {
        Self::new_advanced(
            request.mint_url,
            request.unit,
            request.mnemonic,
            request.store,
            request.config.unwrap_or_default(),
        )
    }

    /// Mint and unit managed by this wallet.
    pub fn identity(&self) -> WalletIdentity {
        WalletIdentity {
            mint_url: self.inner().mint_url.clone().into(),
            unit: self.inner().unit.clone().into(),
        }
    }

    /// Read the available, pending, and reserved balances in one call.
    pub async fn balance(&self) -> Result<WalletBalance, FfiError> {
        wallet_balance(self.inner()).await
    }

    /// Explicitly reconcile wallet state according to the requested network policy.
    pub async fn synchronize(&self, policy: SyncPolicy) -> Result<SyncReport, FfiError> {
        synchronize_wallet(self.inner(), policy).await
    }

    /// Create an incoming-payment session.
    pub async fn request_minting(
        &self,
        request: MintRequest,
    ) -> Result<Arc<MintSession>, FfiError> {
        let quote = self
            .inner()
            .mint_quote(
                request.method.into(),
                request.amount.map(Into::into),
                request.description,
                request.extra,
            )
            .await?;
        let session = MintSession::from_quote(Arc::clone(self.inner()), quote);
        Ok(Arc::new(session))
    }

    /// Resume a locally known incoming-payment session by quote ID.
    pub async fn minting_session(&self, quote_id: String) -> Result<Arc<MintSession>, FfiError> {
        let quote = self
            .inner()
            .localstore
            .get_mint_quote(&quote_id)
            .await
            .map_err(cdk::Error::from)?
            .ok_or(cdk::Error::UnknownQuote)?;
        ensure_mint_quote_belongs_to_wallet(self.inner(), &quote)?;
        let session = MintSession::from_quote(Arc::clone(self.inner()), quote);
        Ok(Arc::new(session))
    }

    /// Select and reserve proofs for an ecash transfer.
    pub async fn plan_send(&self, request: SendRequest) -> Result<Arc<SendPlan>, FfiError> {
        let options = cdk::wallet::SendOptions {
            memo: request
                .memo
                .map(|memo| cdk::wallet::SendMemo::for_token(&memo)),
            send_kind: request.mode.into(),
            include_fee: request.include_fee,
            metadata: request.metadata,
            ..Default::default()
        };

        let prepared = self
            .inner()
            .prepare_send(request.amount.into(), options)
            .await?;
        match SendPlan::new(Arc::clone(self.inner()), &prepared) {
            Ok(plan) => Ok(Arc::new(plan)),
            Err(error) => {
                if let Err(cleanup_error) = prepared.cancel().await {
                    tracing::warn!(
                        "Could not cancel send plan after facade construction failed: {}",
                        cleanup_error
                    );
                }
                Err(error)
            }
        }
    }

    /// Resume a prepared send after a process restart.
    pub async fn send_plan(&self, operation_id: String) -> Result<Arc<SendPlan>, FfiError> {
        let operation_id = parse_operation_id(&operation_id)?;
        let prepared = self.inner().prepared_send(operation_id).await?;
        Ok(Arc::new(SendPlan::new(
            Arc::clone(self.inner()),
            &prepared,
        )?))
    }

    /// Validate and redeem a token into this wallet.
    pub async fn accept(&self, request: ReceiveRequest) -> Result<ReceiveReceipt, FfiError> {
        let options = cdk::wallet::ReceiveOptions {
            metadata: request.metadata,
            ..Default::default()
        };
        let amount = self.inner().receive(&request.token, options).await?;
        Ok(ReceiveReceipt {
            amount: amount.into(),
            wallet: self.identity(),
        })
    }

    /// Quote an outgoing payment. On-chain requests return one session per fee option.
    pub async fn quote_payment(
        &self,
        request: PaymentQuoteRequest,
    ) -> Result<Vec<Arc<PaymentSession>>, FfiError> {
        let metadata = request.metadata;
        let sessions = match request.target {
            PaymentTarget::Bolt11 {
                invoice,
                amount_msat,
            } => {
                let options = amount_msat.map(cdk::nuts::MeltOptions::new_amountless);
                let quote = self
                    .inner()
                    .melt_quote::<cdk::nuts::PaymentMethod, _>(
                        cdk::nuts::PaymentMethod::from("bolt11"),
                        invoice,
                        options,
                        None,
                    )
                    .await?;
                vec![payment_session(self.inner(), quote, metadata, false)]
            }
            PaymentTarget::Bolt12 { offer, amount_msat } => {
                let quote = self
                    .inner()
                    .melt_quote::<cdk::nuts::PaymentMethod, _>(
                        cdk::nuts::PaymentMethod::from("bolt12"),
                        offer,
                        Some(cdk::nuts::MeltOptions::new_amountless(amount_msat)),
                        None,
                    )
                    .await?;
                vec![payment_session(self.inner(), quote, metadata, false)]
            }
            PaymentTarget::Onchain {
                address,
                amount,
                max_fee,
            } => self
                .inner()
                .quote_onchain_melt_options(&address, amount.into(), max_fee.map(Into::into))
                .await?
                .into_iter()
                .map(|quote| payment_session(self.inner(), quote, metadata.clone(), true))
                .collect(),
            PaymentTarget::Custom {
                method,
                request,
                amount,
                extra,
            } => {
                let options = amount.map(cdk::nuts::MeltOptions::new_amountless);
                let quote = self
                    .inner()
                    .melt_quote::<cdk::nuts::PaymentMethod, _>(
                        cdk::nuts::PaymentMethod::from(method),
                        request,
                        options,
                        extra,
                    )
                    .await?;
                vec![payment_session(self.inner(), quote, metadata, false)]
            }
        };
        Ok(sessions)
    }

    /// Resume a prepared outgoing payment after a process restart.
    pub async fn payment_plan(&self, operation_id: String) -> Result<Arc<PaymentPlan>, FfiError> {
        let operation_id = parse_operation_id(&operation_id)?;
        let prepared = self.inner().prepared_melt(operation_id).await?;
        Ok(Arc::new(PaymentPlan::new(
            Arc::clone(self.inner()),
            &prepared,
        )?))
    }

    /// Resume a payment that the mint is still processing.
    pub async fn pending_payment(
        &self,
        operation_id: String,
    ) -> Result<Arc<PendingPayment>, FfiError> {
        let operation_id = parse_operation_id(&operation_id)?;
        let pending = self.inner().pending_melt(operation_id).await?;
        Ok(Arc::new(PendingPayment::new(
            Arc::clone(self.inner()),
            &pending,
        )))
    }

    /// Read wallet transaction history.
    pub async fn history(&self, query: HistoryQuery) -> Result<Vec<HistoryEntry>, FfiError> {
        let mut transactions = self
            .inner()
            .list_transactions(query.direction.map(Into::into))
            .await?;
        if let Some(limit) = query.limit {
            transactions.truncate(limit as usize);
        }
        Ok(transactions.into_iter().map(Into::into).collect())
    }

    /// Re-scan deterministic wallet history from the seed.
    pub async fn restore_from_seed(&self) -> Result<Restored, FfiError> {
        Ok(self.inner().restore().await?.into())
    }
}

fn payment_session(
    wallet: &Arc<cdk::Wallet>,
    quote: cdk::wallet::MeltQuote,
    metadata: HashMap<String, String>,
    select_before_prepare: bool,
) -> Arc<PaymentSession> {
    Arc::new(PaymentSession {
        wallet: Arc::clone(wallet),
        quote,
        metadata,
        select_before_prepare,
    })
}

fn parse_operation_id(operation_id: &str) -> Result<uuid::Uuid, FfiError> {
    uuid::Uuid::parse_str(operation_id)
        .map_err(|error| FfiError::invalid_input(format!("Invalid operation ID: {error}")))
}

async fn wallet_balance(wallet: &cdk::Wallet) -> Result<WalletBalance, FfiError> {
    use cdk::nuts::State;

    // Read every non-terminal proof state in one database query. Three
    // sequential aggregate calls can observe different moments while an
    // operation moves Unspent -> Reserved -> PendingSpent/Pending.
    let proofs = wallet
        .localstore
        .get_proofs(
            Some(wallet.mint_url.clone()),
            Some(wallet.unit.clone()),
            Some(vec![
                State::Unspent,
                State::Reserved,
                State::Pending,
                State::PendingSpent,
            ]),
            None,
        )
        .await
        .map_err(cdk::Error::from)?;

    let mut balance = WalletBalance::default();
    for proof in proofs {
        let bucket = match proof.state {
            State::Unspent => &mut balance.available,
            State::Reserved => &mut balance.reserved,
            State::Pending | State::PendingSpent => &mut balance.pending,
            State::Spent => continue,
        };
        bucket.value = bucket
            .value
            .checked_add(u64::from(proof.proof.amount))
            .ok_or(cdk::Error::AmountOverflow)?;
    }
    Ok(balance)
}

async fn synchronize_wallet(
    wallet: &cdk::Wallet,
    policy: SyncPolicy,
) -> Result<SyncReport, FfiError> {
    let (recovery, claimed_amount, recovered_amount, finalized_payments) = match policy {
        SyncPolicy::LocalOnly => (
            cdk::wallet::RecoveryReport::default(),
            cdk::Amount::ZERO,
            cdk::Amount::ZERO,
            0,
        ),
        SyncPolicy::Online => {
            let recovery = wallet.recover_incomplete_sagas().await?;
            let claimed_amount = wallet.mint_unissued_quotes().await?;
            let recovered_amount = wallet.check_all_pending_proofs().await?;
            let finalized_payments = wallet.finalize_pending_melts().await?.len() as u64;
            (
                recovery,
                claimed_amount,
                recovered_amount,
                finalized_payments,
            )
        }
    };
    let pending_operations = wallet
        .localstore
        .get_incomplete_sagas()
        .await
        .map_err(cdk::Error::Database)?
        .into_iter()
        .filter(|saga| saga.mint_url == wallet.mint_url && saga.unit == wallet.unit)
        .count();
    let pending_operations =
        u64::try_from(pending_operations).map_err(|_| cdk::Error::AmountOverflow)?;

    Ok(SyncReport {
        wallet: WalletIdentity {
            mint_url: wallet.mint_url.clone().into(),
            unit: wallet.unit.clone().into(),
        },
        balance: wallet_balance(wallet).await?,
        recovered_operations: recovery.recovered as u64,
        compensated_operations: recovery.compensated as u64,
        pending_operations,
        failed_operations: recovery.failed as u64,
        claimed_amount: claimed_amount.into(),
        recovered_amount: recovered_amount.into(),
        finalized_payments,
    })
}

/// Multi-mint root object. It owns shared storage, seed, transport settings,
/// and the set of per-mint wallets.
#[derive(uniffi::Object)]
pub struct CashuWallet {
    repository: Arc<cdk::wallet::WalletRepository>,
}

impl fmt::Debug for CashuWallet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CashuWallet").finish_non_exhaustive()
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl CashuWallet {
    /// Open all locally configured mint wallets without network access.
    #[uniffi::constructor]
    pub fn open(request: CashuWalletOpenRequest) -> Result<Self, FfiError> {
        let repository = WalletRepository::new_with_config(
            request.mnemonic,
            request.store,
            WalletRepositoryConfig {
                proxy_url: request.proxy_url,
                rate_limit: request.rate_limit,
            },
        )?;
        Ok(Self {
            repository: repository.inner(),
        })
    }

    /// Return a mint wallet, creating its local configuration when absent.
    pub async fn wallet(
        &self,
        mint_url: MintUrl,
        unit: CurrencyUnit,
    ) -> Result<Arc<Wallet>, FfiError> {
        let mint_url = mint_url.try_into()?;
        let wallet = self
            .repository
            .get_or_create_wallet(mint_url, unit.into(), None)
            .await?;
        Ok(Arc::new(Wallet::from_inner(Arc::new(wallet))))
    }

    /// List all configured mint wallets.
    pub async fn wallets(&self) -> Vec<Arc<Wallet>> {
        self.repository
            .get_wallets()
            .await
            .into_iter()
            .map(|wallet| Arc::new(Wallet::from_inner(Arc::new(wallet))))
            .collect()
    }

    /// Read balances for every configured mint wallet.
    pub async fn balances(&self) -> Result<Vec<WalletBalanceEntry>, FfiError> {
        let wallets = self.repository.get_wallets().await;
        let mut entries = Vec::with_capacity(wallets.len());
        for wallet in wallets {
            entries.push(WalletBalanceEntry {
                wallet: WalletIdentity {
                    mint_url: wallet.mint_url.clone().into(),
                    unit: wallet.unit.clone().into(),
                },
                balance: wallet_balance(&wallet).await?,
            });
        }
        Ok(entries)
    }

    /// Create a durable maximum-balance transfer plan between two configured
    /// mint wallets.
    pub async fn plan_cross_mint_transfer(
        &self,
        request: CrossMintTransferRequest,
    ) -> Result<Arc<CrossMintTransferPlan>, FfiError> {
        let source = repository_wallet(&self.repository, &request.source).await?;
        let destination = repository_wallet(&self.repository, &request.destination).await?;
        let prepared = source
            .prepare_cross_mint_transfer(&destination, request.metadata)
            .await?;
        match CrossMintTransferPlan::new(Arc::new(source), Arc::new(destination), &prepared) {
            Ok(plan) => Ok(Arc::new(plan)),
            Err(error) => {
                if let Err(cleanup_error) = prepared.cancel().await {
                    tracing::warn!(
                        "Could not cancel cross-mint plan after facade construction failed: {}",
                        cleanup_error
                    );
                }
                Err(error)
            }
        }
    }

    /// Resume a prepared cross-mint transfer from its source operation ID.
    pub async fn cross_mint_transfer_plan(
        &self,
        operation_id: String,
    ) -> Result<Arc<CrossMintTransferPlan>, FfiError> {
        let operation_id = parse_operation_id(&operation_id)?;
        for source in self.repository.get_wallets().await {
            let Some(operation) = source.cross_mint_transfer_operation(operation_id).await? else {
                continue;
            };
            let destination = self
                .repository
                .get_or_create_wallet(
                    operation.destination_mint_url.clone(),
                    operation.destination_unit.clone(),
                    None,
                )
                .await?;
            return Ok(Arc::new(CrossMintTransferPlan::from_operation(
                Arc::new(source),
                Arc::new(destination),
                &operation,
            )?));
        }

        Err(cdk::Error::OperationNotFound.into())
    }

    /// Synchronize every configured mint wallet.
    pub async fn synchronize(&self, policy: SyncPolicy) -> Result<Vec<SyncReport>, FfiError> {
        let wallets = self.repository.get_wallets().await;
        let mut reports = Vec::with_capacity(wallets.len());
        for wallet in wallets {
            reports.push(synchronize_wallet(&wallet, policy).await?);
        }
        Ok(reports)
    }

    /// Read history across all configured mint wallets.
    pub async fn history(&self, query: HistoryQuery) -> Result<Vec<HistoryEntry>, FfiError> {
        let mut transactions = self
            .repository
            .list_transactions(query.direction.map(Into::into))
            .await?;
        if let Some(limit) = query.limit {
            transactions.truncate(limit as usize);
        }
        Ok(transactions.into_iter().map(Into::into).collect())
    }
}

async fn repository_wallet(
    repository: &cdk::wallet::WalletRepository,
    identity: &WalletIdentity,
) -> Result<cdk::Wallet, FfiError> {
    repository
        .get_or_create_wallet(
            identity.mint_url.clone().try_into()?,
            identity.unit.clone().into(),
            None,
        )
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::database::custom_wallet_store;
    use crate::sqlite::WalletSqliteDatabase;

    const MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    const TOKEN: &str = "cashuBpGF0gaJhaUgArSaMTR9YJmFwgaNhYQFhc3hAOWE2ZGJiODQ3YmQyMzJiYTc2ZGIwZGYxOTcyMTZiMjlkM2I4Y2MxNDU1M2NkMjc4MjdmYzFjYzk0MmZlZGI0ZWFjWCEDhhhUP_trhpXfStS6vN6So0qWvc2X3O4NfM-Y1HISZ5JhZGlUaGFuayB5b3VhbXVodHRwOi8vbG9jYWxob3N0OjMzMzhhdWNzYXQ=";

    fn memory_store() -> WalletStore {
        custom_wallet_store(
            WalletSqliteDatabase::new_in_memory().expect("in-memory wallet database should open"),
        )
    }

    fn open_test_wallet() -> Wallet {
        Wallet::open(WalletOpenRequest {
            mint_url: "https://mint.example.com".to_string(),
            unit: CurrencyUnit::Sat,
            mnemonic: MNEMONIC.to_string(),
            store: memory_store(),
            config: None,
        })
        .expect("wallet should open")
    }

    fn test_melt_quote(
        wallet: &Wallet,
        state: cdk::nuts::MeltQuoteState,
    ) -> cdk::wallet::MeltQuote {
        cdk::wallet::MeltQuote {
            id: "melt-quote".to_string(),
            mint_url: Some(wallet.inner().mint_url.clone()),
            unit: wallet.inner().unit.clone(),
            amount: cdk::Amount::from(100),
            request: "lnbc-payment-request".to_string(),
            fee_reserve: cdk::Amount::from(3),
            state,
            expiry: 4_000_000_000,
            payment_proof: None,
            estimated_blocks: Some(6),
            fee_index: None,
            payment_method: cdk::nuts::PaymentMethod::BOLT11,
            used_by_operation: None,
            version: 0,
        }
    }

    #[allow(clippy::use_debug)]
    #[test]
    fn open_request_debug_redacts_seed_and_store() {
        let request = WalletOpenRequest {
            mint_url: "https://mint.example.com".to_string(),
            unit: CurrencyUnit::Sat,
            mnemonic: MNEMONIC.to_string(),
            store: memory_store(),
            config: None,
        };

        let output = format!("{request:?}");
        assert!(!output.contains(MNEMONIC));
        assert!(output.contains("[REDACTED]"));
    }

    #[allow(clippy::use_debug)]
    #[test]
    fn multi_mint_open_request_debug_redacts_credentials() {
        let proxy = "http://wallet-user:wallet-password@proxy.example.com";
        let request = CashuWalletOpenRequest {
            mnemonic: MNEMONIC.to_string(),
            store: memory_store(),
            proxy_url: Some(proxy.to_string()),
            rate_limit: None,
        };

        let output = format!("{request:?}");
        assert!(!output.contains(MNEMONIC));
        assert!(!output.contains(proxy));
        assert!(!output.contains("wallet-password"));
        assert!(output.contains("[CONFIGURED]"));
    }

    #[allow(clippy::use_debug)]
    #[test]
    fn receive_request_debug_redacts_bearer_token() {
        let token = "cashuAeyJiZWFyZXIiOiJzZWNyZXQifQ";
        let request = ReceiveRequest {
            token: token.to_string(),
            metadata: HashMap::new(),
        };

        let output = format!("{request:?}");
        assert!(!output.contains(token));
        assert!(output.contains("[REDACTED]"));
    }

    #[test]
    fn payment_receipt_debug_redacts_payment_proof() {
        let preimage = "payment-preimage";
        let receipt = PaymentReceipt {
            operation_id: uuid::Uuid::new_v4().to_string(),
            quote_id: "public-melt-quote-id".to_string(),
            payment_proof: Some(preimage.to_string()),
            amount: Amount::new(100),
            fee_paid: Amount::new(1),
        };

        let output = format!("{receipt:?}");
        assert!(output.contains("public-melt-quote-id"));
        assert!(!output.contains(preimage));
    }

    #[allow(clippy::use_debug)]
    #[test]
    fn portable_state_and_payment_adapters_preserve_public_values() {
        assert_eq!(
            MintingState::from(cdk::nuts::MintQuoteState::Unpaid),
            MintingState::Unpaid
        );
        assert_eq!(
            MintingState::from(cdk::nuts::MintQuoteState::Paid),
            MintingState::Paid
        );
        assert_eq!(
            MintingState::from(cdk::nuts::MintQuoteState::Issued),
            MintingState::Issued
        );
        for (source, expected) in [
            (cdk::nuts::MeltQuoteState::Unpaid, PaymentState::Unpaid),
            (cdk::nuts::MeltQuoteState::Pending, PaymentState::Pending),
            (cdk::nuts::MeltQuoteState::Paid, PaymentState::Paid),
            (cdk::nuts::MeltQuoteState::Failed, PaymentState::Failed),
            (cdk::nuts::MeltQuoteState::Unknown, PaymentState::Unknown),
        ] {
            assert_eq!(PaymentState::from(source), expected);
        }

        let wallet = open_test_wallet();
        let session = payment_session(
            wallet.inner(),
            test_melt_quote(&wallet, cdk::nuts::MeltQuoteState::Pending),
            HashMap::from([("order".to_string(), "123".to_string())]),
            true,
        );
        let quote = session.quote();
        assert_eq!(quote.id, "melt-quote");
        assert_eq!(quote.amount, Amount::new(100));
        assert_eq!(quote.fee_reserve, Amount::new(3));
        assert_eq!(quote.state, PaymentState::Pending);
        assert_eq!(quote.expires_at, 4_000_000_000);
        assert_eq!(quote.estimated_blocks, Some(6));
        assert_eq!(quote.method, PaymentMethod::Bolt11);
        assert!(format!("{session:?}").contains("melt-quote"));

        let operation_id = uuid::Uuid::now_v7();
        let receipt = payment_receipt(
            operation_id,
            cdk_common::common::FinalizedMelt::new(
                "melt-quote".to_string(),
                cdk::nuts::MeltQuoteState::Paid,
                Some("payment-preimage".to_string()),
                cdk::Amount::from(100),
                cdk::Amount::from(2),
                None,
            ),
        );
        assert_eq!(receipt.operation_id, operation_id.to_string());
        assert_eq!(receipt.quote_id, "melt-quote");
        assert_eq!(receipt.payment_proof.as_deref(), Some("payment-preimage"));
        assert_eq!(receipt.amount, Amount::new(100));
        assert_eq!(receipt.fee_paid, Amount::new(2));
    }

    #[allow(clippy::use_debug)]
    #[tokio::test(flavor = "multi_thread")]
    async fn portable_local_handles_preserve_durable_identity() {
        let wallet = open_test_wallet();
        let mut mint_quote = cdk::wallet::MintQuote::new(
            "mint-quote".to_string(),
            wallet.inner().mint_url.clone(),
            cdk::nuts::PaymentMethod::BOLT11,
            Some(cdk::Amount::from(21)),
            wallet.inner().unit.clone(),
            "lnbc-mint".to_string(),
            4_000_000_000,
            None,
        );
        mint_quote.state = cdk::nuts::MintQuoteState::Paid;
        mint_quote.amount_paid = cdk::Amount::from(21);
        let session = MintSession::from_quote(Arc::clone(wallet.inner()), mint_quote.clone());
        assert_eq!(session.id(), "mint-quote");
        assert_eq!(
            session.initial_state(),
            MintSessionState {
                id: "mint-quote".to_string(),
                payment_request: "lnbc-mint".to_string(),
                state: MintingState::Paid,
                amount: Some(Amount::new(21)),
                amount_paid: Amount::new(21),
                amount_claimed: Amount::zero(),
                expires_at: 4_000_000_000,
                method: PaymentMethod::Bolt11,
            }
        );
        assert!(format!("{session:?}").contains("mint-quote"));
        ensure_mint_quote_belongs_to_wallet(wallet.inner(), &mint_quote)
            .expect("matching quote should be accepted");
        assert_eq!(
            claimed_mint_quote_amount(wallet.inner(), "unknown-quote")
                .await
                .unwrap(),
            None
        );

        let mut wrong_mint = mint_quote.clone();
        wrong_mint.mint_url = "https://other.example.com".parse().unwrap();
        assert!(ensure_mint_quote_belongs_to_wallet(wallet.inner(), &wrong_mint).is_err());
        let mut wrong_unit = mint_quote;
        wrong_unit.unit = cdk::nuts::CurrencyUnit::Msat;
        assert!(ensure_mint_quote_belongs_to_wallet(wallet.inner(), &wrong_unit).is_err());

        let operation_id = uuid::Uuid::now_v7();
        let pending = PendingPayment {
            wallet: Arc::clone(wallet.inner()),
            quote_id: "melt-quote".to_string(),
            operation_id,
        };
        assert_eq!(pending.quote_id(), "melt-quote");
        assert_eq!(pending.operation_id(), operation_id.to_string());
        assert!(format!("{pending:?}").contains("melt-quote"));

        let plan = PaymentPlan {
            wallet: Arc::clone(wallet.inner()),
            operation_id,
            quote_id: "melt-quote".to_string(),
            amount: Amount::new(100),
            maximum_fee: Amount::new(5),
        };
        assert_eq!(plan.operation_id(), operation_id.to_string());
        assert_eq!(plan.quote_id(), "melt-quote");
        assert_eq!(plan.amount(), Amount::new(100));
        assert_eq!(plan.maximum_fee(), Amount::new(5));
        assert!(format!("{plan:?}").contains("melt-quote"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn single_wallet_local_queries_do_not_require_network() {
        let wallet = Wallet::open(WalletOpenRequest {
            mint_url: "https://mint.example.com".to_string(),
            unit: CurrencyUnit::Sat,
            mnemonic: MNEMONIC.to_string(),
            store: memory_store(),
            config: None,
        })
        .expect("wallet should open");

        let identity = wallet.identity();
        assert_eq!(identity.mint_url.url, "https://mint.example.com");
        assert_eq!(identity.unit, CurrencyUnit::Sat);
        assert_eq!(wallet.balance().await.unwrap(), WalletBalance::default());

        let report = wallet.synchronize(SyncPolicy::LocalOnly).await.unwrap();
        assert_eq!(report.wallet, identity);
        assert_eq!(report.balance, WalletBalance::default());
        assert_eq!(report.recovered_operations, 0);
        assert_eq!(report.compensated_operations, 0);
        assert_eq!(report.pending_operations, 0);

        let operation_id = uuid::Uuid::now_v7();
        let saga = cdk_common::wallet::WalletSaga::new(
            operation_id,
            cdk_common::wallet::WalletSagaState::Send(cdk_common::wallet::SendSagaState::Preparing),
            cdk::Amount::from(1),
            wallet.inner().mint_url.clone(),
            wallet.inner().unit.clone(),
            cdk_common::wallet::OperationData::PreparedSend(
                cdk_common::wallet::PreparedSendOperationData {
                    amount: cdk::Amount::from(1),
                    options: cdk::wallet::SendOptions::default(),
                    proofs_to_swap: Vec::new(),
                    proofs_to_send: Vec::new(),
                    swap_fee: cdk::Amount::ZERO,
                    send_fee: cdk::Amount::ZERO,
                },
            ),
        );
        wallet.inner().localstore.add_saga(saga).await.unwrap();
        let report = wallet.synchronize(SyncPolicy::LocalOnly).await.unwrap();
        assert_eq!(report.pending_operations, 1);

        let quote = cdk::wallet::MintQuote::new(
            "local-quote".to_string(),
            wallet.inner().mint_url.clone(),
            cdk::nuts::PaymentMethod::BOLT11,
            Some(cdk::Amount::from(21)),
            wallet.inner().unit.clone(),
            "lnbc-local".to_string(),
            4_000_000_000,
            None,
        );
        wallet
            .inner()
            .localstore
            .add_mint_quote(quote)
            .await
            .unwrap();
        let session = wallet
            .minting_session("local-quote".to_string())
            .await
            .expect("a persisted mint session should resume offline");
        assert_eq!(session.id(), "local-quote");
        assert_eq!(session.initial_state().payment_request, "lnbc-local");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn balance_classifies_unclaimed_sends_as_pending() {
        let wallet = open_test_wallet();
        let token = cdk::nuts::Token::from_str(TOKEN).expect("public test vector should parse");
        let proof = token
            .proofs(&[])
            .expect("test token should expose its proof")
            .into_iter()
            .next()
            .expect("test token should contain a proof");
        let y = proof.y().expect("test proof should have a Y");
        let proof_info = cdk_common::wallet::ProofInfo::new(
            proof,
            wallet.inner().mint_url.clone(),
            cdk::nuts::State::Unspent,
            wallet.inner().unit.clone(),
        )
        .expect("test proof should be valid");
        wallet
            .inner()
            .localstore
            .update_proofs(vec![proof_info], vec![])
            .await
            .unwrap();

        let available = wallet.balance().await.unwrap();
        assert_eq!(available.available, Amount::new(1));
        assert_eq!(available.pending, Amount::zero());

        wallet
            .inner()
            .localstore
            .update_proofs_state(vec![y], cdk::nuts::State::Reserved)
            .await
            .unwrap();
        let reserved = wallet.balance().await.unwrap();
        assert_eq!(reserved.available, Amount::zero());
        assert_eq!(reserved.pending, Amount::zero());
        assert_eq!(reserved.reserved, Amount::new(1));

        wallet
            .inner()
            .localstore
            .update_proofs_state(vec![y], cdk::nuts::State::Pending)
            .await
            .unwrap();
        let pending = wallet.balance().await.unwrap();
        assert_eq!(pending.available, Amount::zero());
        assert_eq!(pending.pending, Amount::new(1));
        assert_eq!(pending.reserved, Amount::zero());

        wallet
            .inner()
            .localstore
            .update_proofs_state(vec![y], cdk::nuts::State::PendingSpent)
            .await
            .unwrap();
        let pending = wallet.balance().await.unwrap();
        assert_eq!(pending.available, Amount::zero());
        assert_eq!(pending.pending, Amount::new(1));
        assert_eq!(pending.reserved, Amount::zero());

        wallet
            .inner()
            .localstore
            .update_proofs_state(vec![y], cdk::nuts::State::Spent)
            .await
            .unwrap();
        assert_eq!(wallet.balance().await.unwrap(), WalletBalance::default());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn history_filters_limits_and_maps_engine_transactions() {
        let wallet = open_test_wallet();
        let incoming_id = uuid::Uuid::now_v7();
        let outgoing_id = uuid::Uuid::now_v7();
        let newest_id = uuid::Uuid::now_v7();
        for (direction, timestamp, saga_id, status) in [
            (
                cdk_common::wallet::TransactionDirection::Incoming,
                1,
                incoming_id,
                cdk_common::wallet::TransactionStatus::Completed,
            ),
            (
                cdk_common::wallet::TransactionDirection::Outgoing,
                2,
                outgoing_id,
                cdk_common::wallet::TransactionStatus::Pending,
            ),
            (
                cdk_common::wallet::TransactionDirection::Outgoing,
                3,
                newest_id,
                cdk_common::wallet::TransactionStatus::Failed,
            ),
        ] {
            wallet
                .inner()
                .localstore
                .add_transaction(cdk_common::wallet::Transaction {
                    mint_url: wallet.inner().mint_url.clone(),
                    direction,
                    amount: cdk::Amount::from(timestamp * 10),
                    fee: cdk::Amount::from(timestamp),
                    unit: wallet.inner().unit.clone(),
                    ys: Vec::new(),
                    timestamp,
                    memo: Some(format!("transaction-{timestamp}")),
                    metadata: HashMap::from([("index".to_string(), timestamp.to_string())]),
                    quote_id: Some(format!("quote-{timestamp}")),
                    payment_request: Some("not-exposed".to_string()),
                    payment_proof: Some("not-exposed".to_string()),
                    payment_method: Some(cdk::nuts::PaymentMethod::BOLT11),
                    saga_id: Some(saga_id),
                    status,
                })
                .await
                .unwrap();
        }

        let history = wallet
            .history(HistoryQuery {
                direction: Some(TransactionDirection::Outgoing),
                limit: Some(1),
            })
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0],
            HistoryEntry {
                id: cdk_common::wallet::TransactionId::from_saga_id(newest_id).to_string(),
                wallet: wallet.identity(),
                direction: TransactionDirection::Outgoing,
                amount: Amount::new(30),
                fee: Amount::new(3),
                timestamp: 3,
                memo: Some("transaction-3".to_string()),
                metadata: HashMap::from([("index".to_string(), "3".to_string())]),
                quote_id: Some("quote-3".to_string()),
                operation_id: Some(newest_id.to_string()),
                payment_method: Some(PaymentMethod::Bolt11),
                status: TransactionStatus::Failed,
            }
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn invalid_operation_ids_fail_before_loading_local_plans() {
        let wallet = open_test_wallet();
        assert!(parse_operation_id("not-an-operation-id").is_err());
        assert!(wallet
            .send_plan("not-an-operation-id".to_string())
            .await
            .is_err());
        assert!(wallet
            .payment_plan("not-an-operation-id".to_string())
            .await
            .is_err());
        assert!(wallet
            .pending_payment("not-an-operation-id".to_string())
            .await
            .is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mint_session_claim_is_idempotent_after_issuance() {
        let wallet = Wallet::open(WalletOpenRequest {
            mint_url: "https://mint.example.com".to_string(),
            unit: CurrencyUnit::Sat,
            mnemonic: MNEMONIC.to_string(),
            store: memory_store(),
            config: None,
        })
        .expect("wallet should open");
        let mut quote = cdk::wallet::MintQuote::new(
            "issued-quote".to_string(),
            wallet.inner().mint_url.clone(),
            cdk::nuts::PaymentMethod::BOLT11,
            Some(cdk::Amount::from(21)),
            wallet.inner().unit.clone(),
            "lnbc-issued".to_string(),
            4_000_000_000,
            None,
        );
        quote.state = cdk::nuts::MintQuoteState::Issued;
        quote.amount_paid = cdk::Amount::from(21);
        quote.amount_issued = cdk::Amount::from(21);
        wallet
            .inner()
            .localstore
            .add_mint_quote(quote)
            .await
            .unwrap();

        let session = wallet
            .minting_session("issued-quote".to_string())
            .await
            .unwrap();
        assert_eq!(session.claim().await.unwrap(), Amount::new(21));
        assert_eq!(session.claim().await.unwrap(), Amount::new(21));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multi_mint_root_identifies_each_balance() {
        let root = CashuWallet::open(CashuWalletOpenRequest {
            mnemonic: MNEMONIC.to_string(),
            store: memory_store(),
            proxy_url: None,
            rate_limit: None,
        })
        .expect("portfolio should open");
        root.wallet(
            MintUrl {
                url: "https://mint.example.com".to_string(),
            },
            CurrencyUnit::Sat,
        )
        .await
        .expect("wallet should be configured");

        let balances = root.balances().await.unwrap();
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].wallet.mint_url.url, "https://mint.example.com");
        assert_eq!(balances[0].balance, WalletBalance::default());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cross_mint_plan_reconstructs_from_its_durable_identity_without_a_source_saga() {
        let root = CashuWallet::open(CashuWalletOpenRequest {
            mnemonic: MNEMONIC.to_string(),
            store: memory_store(),
            proxy_url: None,
            rate_limit: None,
        })
        .expect("portfolio should open");
        let source = root
            .wallet(
                MintUrl {
                    url: "https://source.example.com".to_string(),
                },
                CurrencyUnit::Sat,
            )
            .await
            .unwrap();
        let destination = root
            .wallet(
                MintUrl {
                    url: "https://destination.example.com".to_string(),
                },
                CurrencyUnit::Sat,
            )
            .await
            .unwrap();
        let operation_id = uuid::Uuid::now_v7();
        let operation = cdk_common::wallet::CrossMintTransferOperation {
            operation_id,
            source_mint_url: source.inner().mint_url.clone(),
            source_unit: source.inner().unit.clone(),
            destination_mint_url: destination.inner().mint_url.clone(),
            destination_unit: destination.inner().unit.clone(),
            destination_quote_id: "destination-quote".to_string(),
            amount: cdk::Amount::from(100),
            maximum_fee: cdk::Amount::from(3),
        };
        source
            .inner()
            .localstore
            .kv_write(
                "cdk_wallet",
                "cross_mint_transfers",
                &operation_id.to_string(),
                &serde_json::to_vec(&operation).unwrap(),
            )
            .await
            .unwrap();

        let plan = root
            .cross_mint_transfer_plan(operation_id.to_string())
            .await
            .expect("durable transfer identity should reconstruct the plan");

        assert_eq!(plan.operation_id(), operation_id.to_string());
        assert_eq!(plan.amount(), Amount::new(100));
        assert_eq!(plan.maximum_fee(), Amount::new(3));
        assert_eq!(
            plan.destination().mint_url.url,
            "https://destination.example.com"
        );
        assert_eq!(plan.destination_quote_id(), "destination-quote");
        assert_eq!(plan.destination_issued_amount().await.unwrap(), None);

        let mut quote = cdk::wallet::MintQuote::new(
            "destination-quote".to_string(),
            destination.inner().mint_url.clone(),
            cdk::nuts::PaymentMethod::BOLT11,
            Some(cdk::Amount::from(100)),
            destination.inner().unit.clone(),
            "lnbc-destination".to_string(),
            4_000_000_000,
            None,
        );
        quote.state = cdk::nuts::MintQuoteState::Issued;
        quote.amount_issued = cdk::Amount::from(100);
        destination
            .inner()
            .localstore
            .add_mint_quote(quote)
            .await
            .unwrap();
        assert_eq!(
            plan.destination_issued_amount().await.unwrap(),
            Some(Amount::new(100))
        );
        assert_eq!(
            plan.completed(Amount::new(100), Amount::new(2)),
            CrossMintTransferOutcome::Completed {
                receipt: CrossMintTransferReceipt {
                    operation_id: operation_id.to_string(),
                    destination: plan.destination(),
                    destination_quote_id: "destination-quote".to_string(),
                    amount: Amount::new(100),
                    source_fee: Amount::new(2),
                }
            }
        );
    }
}
