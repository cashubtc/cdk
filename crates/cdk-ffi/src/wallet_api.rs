//! Thin UniFFI exposure of the core `cdk::Wallet` workflow API.
//!
//! This module performs only foreign-language type conversion and object
//! lifetime bridging. Wallet behavior lives in `cdk::wallet`.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use bip39::Mnemonic;

use crate::database::WalletStore;
use crate::error::{FfiError, WalletErrorKind};
use crate::types::{
    Amount, BitcoinNetwork, CurrencyUnit, MintInfo, MintUrl, PaymentMethod, PaymentRequest,
    PublicKey, Restored, SupportedMethod, TransactionDirection, TransactionStatus,
};
use crate::wallet::{RateLimit, Wallet, WalletConfig};

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
    /// Optional operational tuning.
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

/// Configuration for opening a multi-mint wallet manager.
#[derive(Clone, uniffi::Record)]
pub struct WalletManagerOpenRequest {
    /// BIP-39 mnemonic shared by every managed wallet.
    pub mnemonic: String,
    /// Durable wallet storage.
    pub store: WalletStore,
    /// Optional shared proxy URL.
    #[uniffi(default = None)]
    pub proxy_url: Option<String>,
    /// Optional manager-wide request pacing.
    #[uniffi(default = None)]
    pub rate_limit: Option<RateLimit>,
}

impl fmt::Debug for WalletManagerOpenRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WalletManagerOpenRequest")
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

/// Lifetime policy for cached mint metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MetadataCachePolicy {
    /// Use the core wallet's default expiry.
    Default,
    /// Keep metadata until an explicit refresh.
    NeverExpires,
    /// Expire metadata after the stated number of seconds.
    ExpiresAfter { seconds: u64 },
}

/// Explicitly advanced proof and metadata tuning for managed wallets.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct ManagedWalletAdvancedOptions {
    /// Preferred number of proofs retained per denomination.
    #[uniffi(default = None)]
    pub target_proof_count: Option<u32>,
    /// Optional metadata-cache lifetime override.
    #[uniffi(default = None)]
    pub metadata_cache: Option<MetadataCachePolicy>,
}

impl From<ManagedWalletAdvancedOptions> for cdk::wallet::advanced::MintAdvancedOptions {
    fn from(value: ManagedWalletAdvancedOptions) -> Self {
        let mut config = cdk::wallet::advanced::MintAdvancedOptions::new();
        if let Some(target_proof_count) = value.target_proof_count {
            config = config.with_target_proof_count(target_proof_count as usize);
        }
        match value.metadata_cache {
            None | Some(MetadataCachePolicy::Default) => {}
            Some(MetadataCachePolicy::NeverExpires) => {
                config = config.without_metadata_cache_expiry();
            }
            Some(MetadataCachePolicy::ExpiresAfter { seconds }) => {
                config = config.with_metadata_cache_ttl(Duration::from_secs(seconds));
            }
        }
        config
    }
}

/// Request to discover a mint's units and register wallets for them.
#[derive(Debug, Clone, uniffi::Record)]
pub struct MintRegistrationRequest {
    /// Mint whose supported units should be registered.
    pub mint_url: MintUrl,
    /// Explicitly advanced per-mint proof and metadata options.
    #[uniffi(default)]
    pub advanced: ManagedWalletAdvancedOptions,
}

impl TryFrom<MintRegistrationRequest> for cdk::wallet::MintRegistrationRequest {
    type Error = FfiError;

    fn try_from(value: MintRegistrationRequest) -> Result<Self, Self::Error> {
        Ok(Self::new(value.mint_url.try_into()?).with_advanced(value.advanced.into()))
    }
}

/// Request to create or replace one mint-and-unit wallet configuration.
#[derive(Debug, Clone, uniffi::Record)]
pub struct WalletConfigurationRequest {
    /// Wallet identity being configured.
    pub identity: WalletIdentity,
    /// Explicitly advanced per-mint proof and metadata options.
    #[uniffi(default)]
    pub advanced: ManagedWalletAdvancedOptions,
}

impl TryFrom<WalletConfigurationRequest> for cdk::wallet::WalletConfigurationRequest {
    type Error = FfiError;

    fn try_from(value: WalletConfigurationRequest) -> Result<Self, Self::Error> {
        Ok(Self::new(value.identity.try_into()?).with_advanced(value.advanced.into()))
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

impl From<cdk::wallet::WalletIdentity> for WalletIdentity {
    fn from(value: cdk::wallet::WalletIdentity) -> Self {
        Self {
            mint_url: value.mint_url.into(),
            unit: value.unit.into(),
        }
    }
}

impl TryFrom<WalletIdentity> for cdk::wallet::WalletIdentity {
    type Error = FfiError;

    fn try_from(value: WalletIdentity) -> Result<Self, Self::Error> {
        Ok(Self {
            mint_url: value.mint_url.try_into()?,
            unit: value.unit.into(),
        })
    }
}

/// Balances grouped by spendability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Record)]
pub struct WalletBalance {
    /// Funds that can be spent now.
    pub available: Amount,
    /// Funds committed to an in-flight operation.
    pub pending: Amount,
    /// Funds held by a prepared operation.
    pub reserved: Amount,
}

impl From<cdk::wallet::WalletBalance> for WalletBalance {
    fn from(value: cdk::wallet::WalletBalance) -> Self {
        Self {
            available: value.available.into(),
            pending: value.pending.into(),
            reserved: value.reserved.into(),
        }
    }
}

/// Stable reference to a resumable wallet operation.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum OperationReference {
    /// Incoming mint quote.
    MintQuote { quote_id: String },
    /// Outgoing payment quote.
    PaymentQuote { quote_id: String },
    /// Fund-reserving durable workflow.
    Workflow { operation_id: String },
}

impl From<cdk::wallet::operation::OperationReference> for OperationReference {
    fn from(value: cdk::wallet::operation::OperationReference) -> Self {
        match value {
            cdk::wallet::operation::OperationReference::MintQuote(quote_id) => Self::MintQuote {
                quote_id: quote_id.to_string(),
            },
            cdk::wallet::operation::OperationReference::PaymentQuote(quote_id) => {
                Self::PaymentQuote {
                    quote_id: quote_id.to_string(),
                }
            }
            cdk::wallet::operation::OperationReference::Workflow(operation_id) => Self::Workflow {
                operation_id: operation_id.to_string(),
            },
        }
    }
}

/// Application-level kind of a durable operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum OperationKind {
    /// Incoming payment that issues ecash.
    Mint,
    /// Outgoing ecash token.
    Send,
    /// Incoming ecash token.
    Receive,
    /// Outgoing payment through a mint.
    Payment,
    /// Transfer between mint wallets.
    Transfer,
    /// Protocol-level proof reissuance.
    Reissue,
}

impl From<cdk::wallet::operation::OperationKind> for OperationKind {
    fn from(value: cdk::wallet::operation::OperationKind) -> Self {
        match value {
            cdk::wallet::operation::OperationKind::Mint => Self::Mint,
            cdk::wallet::operation::OperationKind::Send => Self::Send,
            cdk::wallet::operation::OperationKind::Receive => Self::Receive,
            cdk::wallet::operation::OperationKind::Payment => Self::Payment,
            cdk::wallet::operation::OperationKind::Transfer => Self::Transfer,
            cdk::wallet::operation::OperationKind::Reissue => Self::Reissue,
        }
    }
}

impl From<OperationKind> for cdk::wallet::operation::OperationKind {
    fn from(value: OperationKind) -> Self {
        match value {
            OperationKind::Mint => Self::Mint,
            OperationKind::Send => Self::Send,
            OperationKind::Receive => Self::Receive,
            OperationKind::Payment => Self::Payment,
            OperationKind::Transfer => Self::Transfer,
            OperationKind::Reissue => Self::Reissue,
        }
    }
}

/// Stable application-level lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum OperationState {
    AwaitingPayment,
    Ready,
    AwaitingExecution,
    Processing,
    Pending,
    NeedsRecovery,
    Completed,
    Canceled,
    Failed,
}

impl From<cdk::wallet::operation::OperationState> for OperationState {
    fn from(value: cdk::wallet::operation::OperationState) -> Self {
        match value {
            cdk::wallet::operation::OperationState::AwaitingPayment => Self::AwaitingPayment,
            cdk::wallet::operation::OperationState::Ready => Self::Ready,
            cdk::wallet::operation::OperationState::AwaitingExecution => Self::AwaitingExecution,
            cdk::wallet::operation::OperationState::Processing => Self::Processing,
            cdk::wallet::operation::OperationState::Pending => Self::Pending,
            cdk::wallet::operation::OperationState::NeedsRecovery => Self::NeedsRecovery,
            cdk::wallet::operation::OperationState::Completed => Self::Completed,
            cdk::wallet::operation::OperationState::Canceled => Self::Canceled,
            cdk::wallet::operation::OperationState::Failed => Self::Failed,
        }
    }
}

impl From<OperationState> for cdk::wallet::operation::OperationState {
    fn from(value: OperationState) -> Self {
        match value {
            OperationState::AwaitingPayment => Self::AwaitingPayment,
            OperationState::Ready => Self::Ready,
            OperationState::AwaitingExecution => Self::AwaitingExecution,
            OperationState::Processing => Self::Processing,
            OperationState::Pending => Self::Pending,
            OperationState::NeedsRecovery => Self::NeedsRecovery,
            OperationState::Completed => Self::Completed,
            OperationState::Canceled => Self::Canceled,
            OperationState::Failed => Self::Failed,
        }
    }
}

/// Typed instruction for continuing a discovered operation.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum OperationResume {
    Mint {
        quote_id: String,
    },
    Send {
        operation_id: String,
    },
    PaymentQuote {
        quote_id: String,
    },
    Payment {
        operation_id: String,
        quote_id: Option<String>,
        pending: bool,
    },
    Transfer {
        operation_id: String,
    },
    Synchronize,
}

impl From<cdk::wallet::operation::OperationResume> for OperationResume {
    fn from(value: cdk::wallet::operation::OperationResume) -> Self {
        match value {
            cdk::wallet::operation::OperationResume::Mint { quote_id } => Self::Mint {
                quote_id: quote_id.to_string(),
            },
            cdk::wallet::operation::OperationResume::Send { operation_id } => Self::Send {
                operation_id: operation_id.to_string(),
            },
            cdk::wallet::operation::OperationResume::PaymentQuote { quote_id } => {
                Self::PaymentQuote {
                    quote_id: quote_id.to_string(),
                }
            }
            cdk::wallet::operation::OperationResume::Payment {
                operation_id,
                quote_id,
                pending,
            } => Self::Payment {
                operation_id: operation_id.to_string(),
                quote_id: quote_id.map(|id| id.to_string()),
                pending,
            },
            cdk::wallet::operation::OperationResume::Transfer { operation_id } => Self::Transfer {
                operation_id: operation_id.to_string(),
            },
            cdk::wallet::operation::OperationResume::Synchronize => Self::Synchronize,
        }
    }
}

/// Filter for active durable operations.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct OperationQuery {
    #[uniffi(default)]
    pub kinds: Vec<OperationKind>,
    #[uniffi(default)]
    pub states: Vec<OperationState>,
    #[uniffi(default = None)]
    pub limit: Option<u32>,
}

impl From<OperationQuery> for cdk::wallet::operation::OperationQuery {
    fn from(value: OperationQuery) -> Self {
        Self {
            kinds: value.kinds.into_iter().map(Into::into).collect(),
            states: value.states.into_iter().map(Into::into).collect(),
            limit: value.limit.map(|limit| limit as usize),
        }
    }
}

/// Application-facing summary of an active durable operation.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OperationSummary {
    pub wallet: WalletIdentity,
    pub reference: OperationReference,
    pub kind: OperationKind,
    pub state: OperationState,
    pub amount: Option<Amount>,
    pub expires_at: Option<u64>,
    pub created_at: Option<u64>,
    pub updated_at: Option<u64>,
    pub resume: OperationResume,
}

impl From<cdk::wallet::operation::OperationSummary> for OperationSummary {
    fn from(value: cdk::wallet::operation::OperationSummary) -> Self {
        Self {
            wallet: value.wallet.into(),
            reference: value.reference.into(),
            kind: value.kind.into(),
            state: value.state.into(),
            amount: value.amount.map(Into::into),
            expires_at: value.expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
            resume: value.resume.into(),
        }
    }
}

/// Result of reconciling one workflow during synchronization.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OperationUpdate {
    pub reference: OperationReference,
    pub kind: OperationKind,
    pub previous_state: OperationState,
    pub state: OperationState,
    pub error_kind: Option<WalletErrorKind>,
    pub error_message: Option<String>,
    pub retryable: bool,
}

impl From<cdk::wallet::operation::OperationUpdate> for OperationUpdate {
    fn from(value: cdk::wallet::operation::OperationUpdate) -> Self {
        Self {
            reference: value.reference.into(),
            kind: value.kind.into(),
            previous_state: value.previous_state.into(),
            state: value.state.into(),
            error_kind: value.error_kind.map(Into::into),
            error_message: value.error_message,
            retryable: value.retryable,
        }
    }
}

/// High-level wallet change without proof or wire-protocol details.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum WalletEvent {
    BalanceChanged {
        wallet: WalletIdentity,
        balance: WalletBalance,
    },
    OperationChanged {
        wallet: WalletIdentity,
        reference: OperationReference,
        kind: OperationKind,
        state: OperationState,
        amount: Option<Amount>,
    },
    TransactionChanged {
        transaction: HistoryEntry,
    },
    MintPaymentReceived {
        wallet: WalletIdentity,
        quote_id: String,
        amount_paid: Amount,
    },
}

impl From<cdk::wallet::events::WalletEvent> for WalletEvent {
    fn from(value: cdk::wallet::events::WalletEvent) -> Self {
        match value {
            cdk::wallet::events::WalletEvent::BalanceChanged { wallet, balance } => {
                Self::BalanceChanged {
                    wallet: wallet.into(),
                    balance: balance.into(),
                }
            }
            cdk::wallet::events::WalletEvent::OperationChanged { wallet, operation } => {
                Self::OperationChanged {
                    wallet: wallet.into(),
                    reference: operation.reference.into(),
                    kind: operation.kind.into(),
                    state: operation.state.into(),
                    amount: operation.amount.map(Into::into),
                }
            }
            cdk::wallet::events::WalletEvent::TransactionChanged { transaction } => {
                Self::TransactionChanged {
                    transaction: transaction.into(),
                }
            }
            cdk::wallet::events::WalletEvent::MintPaymentReceived {
                wallet,
                quote_id,
                amount_paid,
            } => Self::MintPaymentReceived {
                wallet: wallet.into(),
                quote_id: quote_id.to_string(),
                amount_paid: amount_paid.into(),
            },
        }
    }
}

/// Independent asynchronous receiver for one wallet's application events.
#[derive(uniffi::Object)]
pub struct WalletEventStream {
    receiver: tokio::sync::Mutex<cdk::wallet::events::WalletEventReceiver>,
}

impl fmt::Debug for WalletEventStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WalletEventStream").finish_non_exhaustive()
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl WalletEventStream {
    /// Wait for the next application-level wallet event.
    pub async fn next(&self) -> Result<WalletEvent, FfiError> {
        self.receiver
            .lock()
            .await
            .next()
            .await
            .map(Into::into)
            .map_err(FfiError::internal)
    }
}

/// Whether synchronization may contact the mint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SyncPolicy {
    /// Inspect local durable state only.
    LocalOnly,
    /// Reconcile operations and balances with the mint.
    Online,
}

impl From<SyncPolicy> for cdk::wallet::operation::SyncPolicy {
    fn from(value: SyncPolicy) -> Self {
        match value {
            SyncPolicy::LocalOnly => Self::LocalOnly,
            SyncPolicy::Online => Self::Online,
        }
    }
}

/// Result of one explicit wallet synchronization.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SyncReport {
    /// Wallet reconciled by this pass.
    pub wallet: WalletIdentity,
    /// Balance after reconciliation.
    pub balance: WalletBalance,
    /// Interrupted operations completed successfully.
    pub recovered_operations: u64,
    /// Interrupted operations rolled back safely.
    pub compensated_operations: u64,
    /// Fund-reserving workflows that remain active after reconciliation.
    pub pending_operations: u64,
    /// Operations that could not be reconciled.
    pub failed_operations: u64,
    /// Paid mint quotes claimed during synchronization.
    pub claimed_amount: Amount,
    /// Orphaned value still unspent or pending at the mint, not reclaimed funds.
    pub unresolved_amount: Amount,
    /// Pending outgoing payments finalized during synchronization.
    pub finalized_payments: u64,
    /// Per-operation recovery results.
    pub operations: Vec<OperationUpdate>,
}

impl TryFrom<cdk::wallet::operation::SyncReport> for SyncReport {
    type Error = FfiError;

    fn try_from(value: cdk::wallet::operation::SyncReport) -> Result<Self, Self::Error> {
        Ok(Self {
            wallet: value.wallet.into(),
            balance: value.balance.into(),
            recovered_operations: value
                .recovered_operations
                .try_into()
                .map_err(|_| FfiError::internal("recovery count overflow"))?,
            compensated_operations: value
                .compensated_operations
                .try_into()
                .map_err(|_| FfiError::internal("compensation count overflow"))?,
            pending_operations: value
                .pending_operations
                .try_into()
                .map_err(|_| FfiError::internal("pending count overflow"))?,
            failed_operations: value
                .failed_operations
                .try_into()
                .map_err(|_| FfiError::internal("failure count overflow"))?,
            claimed_amount: value.claimed_amount.into(),
            unresolved_amount: value.unresolved_amount.into(),
            finalized_payments: value
                .finalized_payments
                .try_into()
                .map_err(|_| FfiError::internal("payment count overflow"))?,
            operations: value.operations.into_iter().map(Into::into).collect(),
        })
    }
}

/// Lifecycle of an incoming minting session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MintState {
    /// The payer has not completed payment.
    Unpaid,
    /// The mint received value and ecash can be claimed.
    Paid,
    /// All paid value has been issued into this wallet.
    Issued,
}

impl From<cdk::wallet::mint::MintState> for MintState {
    fn from(value: cdk::wallet::mint::MintState) -> Self {
        match value {
            cdk::wallet::mint::MintState::Unpaid => Self::Unpaid,
            cdk::wallet::mint::MintState::Paid => Self::Paid,
            cdk::wallet::mint::MintState::Issued => Self::Issued,
        }
    }
}

/// Request for an incoming payment that will issue ecash.
#[derive(Debug, Clone, uniffi::Record)]
pub struct MintRequest {
    /// Payment rail offered to the payer.
    pub method: PaymentMethod,
    /// Requested amount, when fixed.
    #[uniffi(default = None)]
    pub amount: Option<Amount>,
    /// Human-readable payment description.
    #[uniffi(default = None)]
    pub description: Option<String>,
    /// Payment-method-specific JSON understood by the mint.
    #[uniffi(default = None)]
    pub extra: Option<String>,
}

impl From<MintRequest> for cdk::wallet::mint::MintRequest {
    fn from(value: MintRequest) -> Self {
        Self {
            method: value.method.into(),
            amount: value.amount.map(Into::into),
            description: value.description,
            extra: value.extra,
        }
    }
}

/// Public state of an incoming payment session.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MintSessionState {
    /// Stable quote identifier.
    pub id: String,
    /// Payment request to present to the payer.
    pub payment_request: String,
    /// Current mint-reported state.
    pub state: MintState,
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

impl From<&cdk::wallet::mint::MintSessionState> for MintSessionState {
    fn from(value: &cdk::wallet::mint::MintSessionState) -> Self {
        Self {
            id: value.id.to_string(),
            payment_request: value.payment_request.clone(),
            state: value.state.into(),
            amount: value.amount.map(Into::into),
            amount_paid: value.amount_paid.into(),
            amount_claimed: value.amount_claimed.into(),
            expires_at: value.expires_at,
            method: value.method.clone().into(),
        }
    }
}

/// Receipt for successfully claimed incoming value.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MintReceipt {
    /// Quote that was claimed.
    pub quote_id: String,
    /// Issued value: cumulative for `claim`, or the newly issued batch for a
    /// receipt stream. `wait` returns a batch or an already-issued local total.
    pub amount: Amount,
    /// Wallet that received the value.
    pub wallet: WalletIdentity,
}

impl From<cdk::wallet::mint::MintReceipt> for MintReceipt {
    fn from(value: cdk::wallet::mint::MintReceipt) -> Self {
        Self {
            quote_id: value.quote_id.to_string(),
            amount: value.amount.into(),
            wallet: value.wallet.into(),
        }
    }
}

/// Durable handle for an incoming mint quote.
#[derive(uniffi::Object)]
pub struct MintSession {
    inner: cdk::wallet::mint::MintSession,
}

impl fmt::Debug for MintSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

#[cfg(feature = "advanced-wallet")]
impl MintSession {
    pub(crate) fn core_session(&self) -> &cdk::wallet::mint::MintSession {
        &self.inner
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl MintSession {
    /// Stable quote identifier used to resume this session.
    pub fn id(&self) -> String {
        self.inner.id().to_string()
    }

    /// State captured when this handle was created.
    pub fn initial_state(&self) -> MintSessionState {
        self.inner.initial_state().into()
    }

    /// Refresh this quote from the mint.
    pub async fn refresh(&self) -> Result<MintSessionState, FfiError> {
        Ok((&self.inner.refresh().await?).into())
    }

    /// Claim all paid value for this quote.
    pub async fn claim(&self) -> Result<MintReceipt, FfiError> {
        Ok(self.inner.claim().await?.into())
    }

    /// Wait for payment and claim the quote before the timeout elapses.
    pub async fn wait(&self, timeout_seconds: u64) -> Result<MintReceipt, FfiError> {
        Ok(self
            .inner
            .wait(Duration::from_secs(timeout_seconds))
            .await?
            .into())
    }
}

/// Whether a send may contact the mint to obtain suitable denominations.
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum SendMode {
    /// Contact the mint when an exact local proof selection is unavailable.
    OnlineExact,
    /// Prefer local proofs within the stated tolerance, then contact the mint.
    OnlineTolerant { tolerance: Amount },
    /// Never contact the mint and require an exact local proof selection.
    OfflineExact,
    /// Never contact the mint and allow overpayment within the stated tolerance.
    OfflineTolerant { tolerance: Amount },
}

impl From<SendMode> for cdk::wallet::send::SendMode {
    fn from(value: SendMode) -> Self {
        match value {
            SendMode::OnlineExact => Self::OnlineExact,
            SendMode::OnlineTolerant { tolerance } => Self::OnlineTolerant {
                tolerance: tolerance.into(),
            },
            SendMode::OfflineExact => Self::OfflineExact,
            SendMode::OfflineTolerant { tolerance } => Self::OfflineTolerant {
                tolerance: tolerance.into(),
            },
        }
    }
}

/// Whether the receiver has claimed a confirmed send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SendStatus {
    /// The token remains reclaimable by the sender.
    Unclaimed,
    /// The receiver has spent the token.
    Claimed,
}

impl From<cdk::wallet::send::SendStatus> for SendStatus {
    fn from(value: cdk::wallet::send::SendStatus) -> Self {
        match value {
            cdk::wallet::send::SendStatus::Unclaimed => Self::Unclaimed,
            cdk::wallet::send::SendStatus::Claimed => Self::Claimed,
        }
    }
}

/// Request to send an encoded ecash token.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SendRequest {
    /// Value to transfer.
    pub amount: Amount,
    /// Online/offline selection behavior.
    pub mode: SendMode,
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

impl From<SendRequest> for cdk::wallet::send::SendRequest {
    fn from(value: SendRequest) -> Self {
        let mut request = Self::new(value.amount.into());
        request.mode = value.mode.into();
        request.memo = value.memo;
        request.include_fee = value.include_fee;
        request.metadata = value.metadata;
        request
    }
}

/// Receipt for a confirmed ecash send.
#[derive(Clone, uniffi::Record)]
pub struct SendReceipt {
    /// Durable operation identifier.
    pub operation_id: String,
    /// Encoded token value.
    pub amount: Amount,
    /// Fee reserved for this send.
    pub fee: Amount,
    /// Encoded token to deliver to the receiver.
    pub token: String,
}

impl fmt::Debug for SendReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendReceipt")
            .field("operation_id", &self.operation_id)
            .field("amount", &self.amount)
            .field("fee", &self.fee)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

impl From<cdk::wallet::send::SendReceipt> for SendReceipt {
    fn from(value: cdk::wallet::send::SendReceipt) -> Self {
        Self {
            operation_id: value.operation_id.to_string(),
            amount: value.amount.into(),
            fee: value.fee.into(),
            token: value.token.to_string(),
        }
    }
}

/// Reviewable, durable ecash send plan.
#[derive(uniffi::Object)]
pub struct SendPlan {
    inner: cdk::wallet::send::SendPlan,
}

impl fmt::Debug for SendPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

#[cfg(feature = "advanced-wallet")]
impl SendPlan {
    pub(crate) fn from_core(inner: cdk::wallet::send::SendPlan) -> Self {
        Self { inner }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl SendPlan {
    /// Durable operation identifier used to resume this plan.
    pub fn operation_id(&self) -> String {
        self.inner.operation_id().to_string()
    }

    /// Value encoded into the token.
    pub fn amount(&self) -> Amount {
        self.inner.amount().into()
    }

    /// Maximum fee reserved by this plan.
    pub fn fee(&self) -> Amount {
        self.inner.fee().into()
    }

    /// Execute the plan and create the token.
    pub async fn execute(&self) -> Result<SendReceipt, FfiError> {
        Ok(self.inner.execute().await?.into())
    }

    /// Cancel the plan and release its reserved funds.
    pub async fn cancel(&self) -> Result<(), FfiError> {
        self.inner.cancel().await?;
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

impl From<ReceiveRequest> for cdk::wallet::receive::ReceiveRequest {
    fn from(value: ReceiveRequest) -> Self {
        let mut request = Self::new(value.token);
        request.metadata = value.metadata;
        request
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

impl From<cdk::wallet::receive::ReceiveReceipt> for ReceiveReceipt {
    fn from(value: cdk::wallet::receive::ReceiveReceipt) -> Self {
        Self {
            amount: value.amount.into(),
            wallet: value.wallet.into(),
        }
    }
}

/// Fee and debit limits enforced while planning a NUT-18 request payment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Record)]
pub struct RequestPaymentLimits {
    /// Maximum receiver-selected method fee.
    #[uniffi(default = None)]
    pub maximum_method_fee: Option<Amount>,
    /// Maximum total wallet debit, including mint input fees.
    #[uniffi(default = None)]
    pub maximum_total_amount: Option<Amount>,
}

impl From<RequestPaymentLimits> for cdk::wallet::payment_request::RequestPaymentLimits {
    fn from(value: RequestPaymentLimits) -> Self {
        Self {
            maximum_method_fee: value.maximum_method_fee.map(Into::into),
            maximum_total_amount: value.maximum_total_amount.map(Into::into),
        }
    }
}

/// Request to pay a receiver-provided NUT-18 payment request.
#[derive(Clone, uniffi::Record)]
pub struct RequestPayment {
    /// Receiver-provided protocol request.
    pub payment_request: Arc<PaymentRequest>,
    /// Amount supplied for an open-amount request.
    #[uniffi(default = None)]
    pub amount: Option<Amount>,
    /// Mint required for manager-based planning, or automatic selection.
    #[uniffi(default = None)]
    pub mint: Option<MintUrl>,
    /// Limits checked before returning a fund-reserving plan.
    #[uniffi(default)]
    pub limits: RequestPaymentLimits,
}

impl fmt::Debug for RequestPayment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestPayment")
            .field("payment_request", &"[REDACTED]")
            .field("amount", &self.amount)
            .field("mint", &self.mint)
            .field("limits", &self.limits)
            .finish()
    }
}

impl TryFrom<RequestPayment> for cdk::wallet::payment_request::RequestPayment {
    type Error = FfiError;

    fn try_from(value: RequestPayment) -> Result<Self, Self::Error> {
        Ok(Self {
            payment_request: value.payment_request.inner().clone(),
            amount: value.amount.map(Into::into),
            mint: value.mint.map(TryInto::try_into).transpose()?,
            limits: value.limits.into(),
        })
    }
}

/// Receipt for a delivered NUT-18 request payment.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RequestPaymentReceipt {
    /// Durable send operation identifier.
    pub operation_id: String,
    /// Wallet selected for the payment.
    pub wallet: WalletIdentity,
    /// Receiver-requested value before fees.
    pub requested_amount: Amount,
    /// Total wallet debit.
    pub total_amount: Amount,
}

impl From<cdk::wallet::payment_request::RequestPaymentReceipt> for RequestPaymentReceipt {
    fn from(value: cdk::wallet::payment_request::RequestPaymentReceipt) -> Self {
        Self {
            operation_id: value.operation_id.to_string(),
            wallet: value.wallet.into(),
            requested_amount: value.requested_amount.into(),
            total_amount: value.total_amount.into(),
        }
    }
}

/// Reviewable NUT-18 request payment with funds already reserved.
#[derive(uniffi::Object)]
pub struct RequestPaymentPlan {
    inner: cdk::wallet::payment_request::RequestPaymentPlan,
}

impl fmt::Debug for RequestPaymentPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl RequestPaymentPlan {
    /// Durable send operation identifier.
    pub fn operation_id(&self) -> String {
        self.inner.operation_id().to_string()
    }

    /// Wallet selected for the payment.
    pub fn wallet(&self) -> WalletIdentity {
        self.inner.wallet().clone().into()
    }

    /// Receiver-requested value before fees.
    pub fn requested_amount(&self) -> Amount {
        self.inner.requested_amount().into()
    }

    /// Receiver-selected method, when restricted.
    pub fn method(&self) -> Option<String> {
        self.inner.method().map(str::to_owned)
    }

    /// Receiver-selected method fee.
    pub fn method_fee(&self) -> Amount {
        self.inner.method_fee().into()
    }

    /// Requested value plus receiver-selected method fee.
    pub fn payment_amount(&self) -> Amount {
        self.inner.payment_amount().into()
    }

    /// Mint input fees charged to construct the token.
    pub fn input_fee(&self) -> Amount {
        self.inner.input_fee().into()
    }

    /// Total wallet debit.
    pub fn total_amount(&self) -> Amount {
        self.inner.total_amount().into()
    }

    /// Create and deliver the payment token.
    pub async fn execute(&self) -> Result<RequestPaymentReceipt, FfiError> {
        Ok(self.inner.execute().await?.into())
    }

    /// Release reserved funds without paying.
    pub async fn cancel(&self) -> Result<(), FfiError> {
        self.inner.cancel().await?;
        Ok(())
    }
}

/// Receiver-enforced spending lock for a created payment request.
#[derive(Clone, uniffi::Enum)]
pub enum PaymentRequestLock {
    /// Require P2PK signatures.
    P2pk {
        public_keys: Vec<PublicKey>,
        signatures_required: u64,
    },
    /// Require an HTLC hash and optional P2PK signatures.
    HtlcHash {
        hash: String,
        public_keys: Vec<PublicKey>,
        signatures_required: u64,
    },
    /// Derive an HTLC from a secret preimage and optional P2PK signatures.
    HtlcPreimage {
        preimage: String,
        public_keys: Vec<PublicKey>,
        signatures_required: u64,
    },
}

impl fmt::Debug for PaymentRequestLock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::P2pk {
                public_keys,
                signatures_required,
            } => f
                .debug_struct("P2pk")
                .field("public_keys", public_keys)
                .field("signatures_required", signatures_required)
                .finish(),
            Self::HtlcHash {
                hash,
                public_keys,
                signatures_required,
            } => f
                .debug_struct("HtlcHash")
                .field("hash", hash)
                .field("public_keys", public_keys)
                .field("signatures_required", signatures_required)
                .finish(),
            Self::HtlcPreimage {
                public_keys,
                signatures_required,
                ..
            } => f
                .debug_struct("HtlcPreimage")
                .field("preimage", &"[REDACTED]")
                .field("public_keys", public_keys)
                .field("signatures_required", signatures_required)
                .finish(),
        }
    }
}

fn convert_public_keys(keys: Vec<PublicKey>) -> Result<Vec<cdk::nuts::PublicKey>, FfiError> {
    keys.into_iter().map(TryInto::try_into).collect()
}

impl TryFrom<PaymentRequestLock> for cdk::wallet::payment_request::PaymentRequestLock {
    type Error = FfiError;

    fn try_from(value: PaymentRequestLock) -> Result<Self, Self::Error> {
        Ok(match value {
            PaymentRequestLock::P2pk {
                public_keys,
                signatures_required,
            } => Self::P2pk {
                public_keys: convert_public_keys(public_keys)?,
                signatures_required,
            },
            PaymentRequestLock::HtlcHash {
                hash,
                public_keys,
                signatures_required,
            } => Self::HtlcHash {
                hash,
                public_keys: convert_public_keys(public_keys)?,
                signatures_required,
            },
            PaymentRequestLock::HtlcPreimage {
                preimage,
                public_keys,
                signatures_required,
            } => Self::HtlcPreimage {
                preimage,
                public_keys: convert_public_keys(public_keys)?,
                signatures_required,
            },
        })
    }
}

/// Delivery transport advertised by a created payment request.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum PaymentRequestTransport {
    /// Deliver the encoded request out of band.
    OutOfBand,
    /// Accept payment payloads through HTTP POST.
    Http { url: String },
    /// Accept gift-wrapped payment payloads on Nostr relays.
    Nostr { relays: Vec<String> },
}

impl TryFrom<PaymentRequestTransport> for cdk::wallet::payment_request::PaymentRequestTransport {
    type Error = FfiError;

    fn try_from(value: PaymentRequestTransport) -> Result<Self, Self::Error> {
        Ok(match value {
            PaymentRequestTransport::OutOfBand => Self::OutOfBand,
            PaymentRequestTransport::Http { url } => Self::Http(
                url.parse()
                    .map_err(|error| FfiError::invalid_input(format!("Invalid URL: {error}")))?,
            ),
            PaymentRequestTransport::Nostr { relays } => Self::Nostr(relays),
        })
    }
}

/// How a receiver's mint list constrains payer selection.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum PaymentRequestMintPolicy {
    /// Accept any mint.
    Any,
    /// Accept only the listed mints.
    Strict { mints: Vec<MintUrl> },
    /// Prefer the listed mints but accept another compatible mint.
    Preferred { mints: Vec<MintUrl> },
}

fn convert_mint_urls(mints: Vec<MintUrl>) -> Result<Vec<cdk::mint_url::MintUrl>, FfiError> {
    mints.into_iter().map(TryInto::try_into).collect()
}

impl TryFrom<PaymentRequestMintPolicy> for cdk::wallet::payment_request::PaymentRequestMintPolicy {
    type Error = FfiError;

    fn try_from(value: PaymentRequestMintPolicy) -> Result<Self, Self::Error> {
        Ok(match value {
            PaymentRequestMintPolicy::Any => Self::Any,
            PaymentRequestMintPolicy::Strict { mints } => Self::Strict(convert_mint_urls(mints)?),
            PaymentRequestMintPolicy::Preferred { mints } => {
                Self::Preferred(convert_mint_urls(mints)?)
            }
        })
    }
}

/// Request to create a receiver-side NUT-18 payment request.
#[derive(Clone, uniffi::Record)]
pub struct CreatePaymentRequest {
    /// Requested value, or no value for an open-amount request.
    #[uniffi(default = None)]
    pub amount: Option<Amount>,
    /// Requested currency unit.
    pub unit: CurrencyUnit,
    /// Human-readable description.
    #[uniffi(default = None)]
    pub description: Option<String>,
    /// Receiver-enforced spending lock.
    #[uniffi(default = None)]
    pub lock: Option<PaymentRequestLock>,
    /// Delivery transport.
    pub transport: PaymentRequestTransport,
    /// Accepted or preferred mints.
    pub mint_policy: PaymentRequestMintPolicy,
    /// Payment rails the payer's mint must support.
    #[uniffi(default)]
    pub supported_methods: Vec<SupportedMethod>,
}

impl fmt::Debug for CreatePaymentRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreatePaymentRequest")
            .field("amount", &self.amount)
            .field("unit", &self.unit)
            .field("description", &self.description)
            .field("lock", &self.lock)
            .field("transport", &self.transport)
            .field("mint_policy", &self.mint_policy)
            .field("supported_methods", &self.supported_methods)
            .finish()
    }
}

impl TryFrom<CreatePaymentRequest> for cdk::wallet::payment_request::CreatePaymentRequest {
    type Error = FfiError;

    fn try_from(value: CreatePaymentRequest) -> Result<Self, Self::Error> {
        Ok(Self {
            amount: value.amount.map(Into::into),
            unit: value.unit.into(),
            description: value.description,
            lock: value.lock.map(TryInto::try_into).transpose()?,
            transport: value.transport.try_into()?,
            mint_policy: value.mint_policy.try_into()?,
            supported_methods: value
                .supported_methods
                .into_iter()
                .map(Into::into)
                .collect(),
        })
    }
}

/// Persistable state for a Nostr payment-request receiver.
#[derive(Clone, uniffi::Record)]
pub struct PaymentRequestReceiverState {
    /// Secret key used to unwrap gift-wrapped payments.
    pub secret_key_hex: String,
    /// Relays carrying request payments.
    pub relays: Vec<String>,
    /// Public key used by the listener filter.
    pub public_key_hex: String,
    /// Mints accepted or preferred by the request.
    pub mints: Vec<MintUrl>,
    /// Whether unlisted mints remain acceptable.
    pub mint_preferred: Option<bool>,
}

impl fmt::Debug for PaymentRequestReceiverState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaymentRequestReceiverState")
            .field("secret_key_hex", &"[REDACTED]")
            .field("relays", &self.relays)
            .field("public_key_hex", &self.public_key_hex)
            .field("mints", &self.mints)
            .field("mint_preferred", &self.mint_preferred)
            .finish()
    }
}

impl From<cdk::wallet::payment_request::PaymentRequestReceiverState>
    for PaymentRequestReceiverState
{
    fn from(value: cdk::wallet::payment_request::PaymentRequestReceiverState) -> Self {
        Self {
            secret_key_hex: value.secret_key_hex,
            relays: value.relays,
            public_key_hex: value.public_key_hex,
            mints: value.mints.into_iter().map(Into::into).collect(),
            mint_preferred: value.mint_preferred,
        }
    }
}

impl TryFrom<PaymentRequestReceiverState>
    for cdk::wallet::payment_request::PaymentRequestReceiverState
{
    type Error = FfiError;

    fn try_from(value: PaymentRequestReceiverState) -> Result<Self, Self::Error> {
        Ok(Self {
            secret_key_hex: value.secret_key_hex,
            relays: value.relays,
            public_key_hex: value.public_key_hex,
            mints: convert_mint_urls(value.mints)?,
            mint_preferred: value.mint_preferred,
        })
    }
}

/// Handle that waits for and redeems a Nostr-delivered request payment.
#[derive(uniffi::Object)]
pub struct PaymentRequestReceiver {
    inner: cdk::wallet::payment_request::PaymentRequestReceiver,
}

impl fmt::Debug for PaymentRequestReceiver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl PaymentRequestReceiver {
    /// Export credentials that can resume this receiver after a restart.
    pub fn state(&self) -> PaymentRequestReceiverState {
        self.inner.state().into()
    }

    /// Wait for the first matching payment and redeem it.
    pub async fn receive(&self) -> Result<Amount, FfiError> {
        Ok(self.inner.receive().await?.into())
    }

    /// Wait up to the requested number of seconds for a matching payment.
    pub async fn receive_with_timeout(
        &self,
        timeout_seconds: u64,
    ) -> Result<Option<Amount>, FfiError> {
        Ok(self
            .inner
            .receive_with_timeout(Duration::from_secs(timeout_seconds))
            .await?
            .map(Into::into))
    }
}

/// Receiver-created NUT-18 request and optional Nostr listener.
#[derive(uniffi::Record)]
pub struct CreatedPaymentRequest {
    /// Protocol request to present to the payer.
    pub payment_request: Arc<PaymentRequest>,
    /// Listener used for Nostr delivery.
    pub receiver: Option<Arc<PaymentRequestReceiver>>,
}

impl From<cdk::wallet::payment_request::CreatedPaymentRequest> for CreatedPaymentRequest {
    fn from(value: cdk::wallet::payment_request::CreatedPaymentRequest) -> Self {
        Self {
            payment_request: Arc::new(PaymentRequest::from_inner(value.payment_request)),
            receiver: value
                .receiver
                .map(|inner| Arc::new(PaymentRequestReceiver { inner })),
        }
    }
}

/// Typed outgoing payment target.
#[derive(Clone, uniffi::Enum)]
pub enum PaymentTarget {
    /// BOLT11 invoice and amount behavior.
    Bolt11 {
        invoice: String,
        amount: Bolt11PaymentAmount,
    },
    /// BOLT12 offer and amount behavior.
    Bolt12 {
        offer: String,
        amount: Bolt12PaymentAmount,
    },
    /// Bitcoin address payment.
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

/// Amount behavior for a BOLT11 payment quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Bolt11PaymentAmount {
    /// Use the amount encoded in the invoice.
    Invoice,
    /// Supply an amount for an amountless invoice.
    Amountless { amount_msat: Amount },
    /// Request a partial multi-part payment.
    Mpp { amount_msat: Amount },
}

/// Amount behavior for a BOLT12 payment quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Bolt12PaymentAmount {
    /// Use the amount encoded in the offer.
    Offer,
    /// Supply an amount for an amountless offer.
    Amountless { amount_msat: Amount },
}

impl From<Bolt12PaymentAmount> for cdk::wallet::payment::Bolt12PaymentAmount {
    fn from(value: Bolt12PaymentAmount) -> Self {
        match value {
            Bolt12PaymentAmount::Offer => Self::Offer,
            Bolt12PaymentAmount::Amountless { amount_msat } => Self::Amountless(amount_msat.into()),
        }
    }
}

impl From<Bolt11PaymentAmount> for cdk::wallet::payment::Bolt11PaymentAmount {
    fn from(value: Bolt11PaymentAmount) -> Self {
        match value {
            Bolt11PaymentAmount::Invoice => Self::Invoice,
            Bolt11PaymentAmount::Amountless { amount_msat } => Self::Amountless(amount_msat.into()),
            Bolt11PaymentAmount::Mpp { amount_msat } => Self::Mpp(amount_msat.into()),
        }
    }
}

impl From<PaymentTarget> for cdk::wallet::payment::PaymentTarget {
    fn from(value: PaymentTarget) -> Self {
        match value {
            PaymentTarget::Bolt11 { invoice, amount } => Self::Bolt11 {
                invoice,
                amount: amount.into(),
            },
            PaymentTarget::Bolt12 { offer, amount } => Self::Bolt12 {
                offer,
                amount: amount.into(),
            },
            PaymentTarget::Onchain {
                address,
                amount,
                max_fee,
            } => Self::Onchain {
                address,
                amount: amount.into(),
                max_fee: max_fee.map(Into::into),
            },
            PaymentTarget::Custom {
                method,
                request,
                amount,
                extra,
            } => Self::Custom {
                method,
                request,
                amount: amount.map(Into::into),
                extra,
            },
        }
    }
}

/// Request for one or more outgoing payment quotes.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PaymentQuoteRequest {
    /// Destination and payment rail.
    pub target: PaymentTarget,
    /// Application metadata persisted with the payment.
    #[uniffi(default)]
    pub metadata: HashMap<String, String>,
}

impl From<PaymentQuoteRequest> for cdk::wallet::payment::PaymentQuoteRequest {
    fn from(value: PaymentQuoteRequest) -> Self {
        Self {
            target: value.target.into(),
            metadata: value.metadata,
        }
    }
}

/// Resolution strategy for an email-like payment address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AddressPaymentRoute {
    /// Resolve with LNURL-pay and quote its BOLT11 invoice.
    LightningAddress,
    /// Resolve with BIP-353 and require a BOLT12 offer.
    Bip353 { network: BitcoinNetwork },
    /// Prefer BIP-353 and fall back to LNURL-pay when DNS is unavailable.
    Automatic { network: BitcoinNetwork },
}

impl TryFrom<AddressPaymentRoute> for cdk::wallet::payment::AddressPaymentRoute {
    type Error = FfiError;

    fn try_from(value: AddressPaymentRoute) -> Result<Self, Self::Error> {
        match value {
            AddressPaymentRoute::LightningAddress => Ok(Self::LightningAddress),
            #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
            AddressPaymentRoute::Bip353 { network } => Ok(Self::Bip353 {
                network: network.into(),
            }),
            #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
            AddressPaymentRoute::Automatic { network } => Ok(Self::Automatic {
                network: network.into(),
            }),
            #[cfg(not(all(feature = "bip353", not(target_arch = "wasm32"))))]
            AddressPaymentRoute::Bip353 { .. } | AddressPaymentRoute::Automatic { .. } => Err(
                FfiError::invalid_input("BIP-353 address resolution is not enabled in this build"),
            ),
        }
    }
}

/// Request to resolve and quote an email-like payment address.
#[derive(Clone, uniffi::Record)]
pub struct AddressPaymentRequest {
    /// Lightning or BIP-353 address.
    pub address: String,
    /// Payment amount in millisatoshis.
    pub amount_msat: Amount,
    /// Resolution behavior.
    pub route: AddressPaymentRoute,
    /// Application metadata persisted with the payment.
    #[uniffi(default)]
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

impl TryFrom<AddressPaymentRequest> for cdk::wallet::payment::AddressPaymentRequest {
    type Error = FfiError;

    fn try_from(value: AddressPaymentRequest) -> Result<Self, Self::Error> {
        Ok(Self {
            address: value.address,
            amount_msat: value.amount_msat.into(),
            route: value.route.try_into()?,
            metadata: value.metadata,
        })
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
    /// The mint cannot yet determine the result.
    Unknown,
}

impl From<cdk::wallet::payment::PaymentState> for PaymentState {
    fn from(value: cdk::wallet::payment::PaymentState) -> Self {
        match value {
            cdk::wallet::payment::PaymentState::Unpaid => Self::Unpaid,
            cdk::wallet::payment::PaymentState::Pending => Self::Pending,
            cdk::wallet::payment::PaymentState::Paid => Self::Paid,
            cdk::wallet::payment::PaymentState::Failed => Self::Failed,
            cdk::wallet::payment::PaymentState::Unknown => Self::Unknown,
        }
    }
}

/// Public outgoing payment quote preview.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PaymentQuote {
    /// Stable quote identifier.
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

impl From<cdk::wallet::payment::PaymentQuote> for PaymentQuote {
    fn from(value: cdk::wallet::payment::PaymentQuote) -> Self {
        Self {
            id: value.id.to_string(),
            amount: value.amount.into(),
            fee_reserve: value.fee_reserve.into(),
            state: value.state.into(),
            expires_at: value.expires_at,
            estimated_blocks: value.estimated_blocks,
            method: value.method.into(),
        }
    }
}

/// Quote that can be prepared into a fund-reserving payment plan.
#[derive(uniffi::Object)]
pub struct PaymentSession {
    inner: cdk::wallet::payment::PaymentSession,
}

/// Result of quoting an outgoing payment target.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum PaymentQuoteResult {
    /// A single Lightning or extension-payment quote.
    Single { session: Arc<PaymentSession> },
    /// On-chain fee and confirmation-target alternatives.
    Options { sessions: Vec<Arc<PaymentSession>> },
}

impl From<cdk::wallet::payment::PaymentQuoteResult> for PaymentQuoteResult {
    fn from(value: cdk::wallet::payment::PaymentQuoteResult) -> Self {
        match value {
            cdk::wallet::payment::PaymentQuoteResult::Single(inner) => Self::Single {
                session: Arc::new(PaymentSession { inner }),
            },
            cdk::wallet::payment::PaymentQuoteResult::Options(sessions) => Self::Options {
                sessions: sessions
                    .into_iter()
                    .map(|inner| Arc::new(PaymentSession { inner }))
                    .collect(),
            },
        }
    }
}

impl fmt::Debug for PaymentSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

#[cfg(feature = "advanced-wallet")]
impl PaymentSession {
    pub(crate) fn core_session(&self) -> &cdk::wallet::payment::PaymentSession {
        &self.inner
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl PaymentSession {
    /// Stable mint-provided quote identifier.
    pub fn id(&self) -> String {
        self.inner.id().to_string()
    }

    /// Immutable quote details for user review.
    pub fn quote(&self) -> PaymentQuote {
        self.inner.quote().into()
    }

    /// Refresh this quote from the mint.
    pub async fn refresh(&self) -> Result<PaymentQuote, FfiError> {
        Ok(self.inner.refresh().await?.into())
    }

    /// Reserve funds and create a confirm-or-cancel plan.
    pub async fn prepare(&self) -> Result<Arc<PaymentPlan>, FfiError> {
        Ok(Arc::new(PaymentPlan {
            inner: self.inner.prepare().await?,
        }))
    }

    /// Prepare and execute this payment with the default policy.
    pub async fn execute(&self) -> Result<PaymentReceipt, FfiError> {
        Ok(self.inner.execute().await?.into())
    }

    /// Prepare and submit this payment, returning on asynchronous acceptance.
    pub async fn submit(&self) -> Result<PaymentConfirmation, FfiError> {
        Ok(self.inner.submit().await?.into())
    }
}

/// Receipt for a successfully finalized outgoing payment.
#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct PaymentReceipt {
    /// Durable operation identifier.
    pub operation_id: String,
    /// Mint quote identifier.
    pub quote_id: String,
    /// Method-specific settlement proof.
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

impl From<cdk::wallet::payment::PaymentReceipt> for PaymentReceipt {
    fn from(value: cdk::wallet::payment::PaymentReceipt) -> Self {
        Self {
            operation_id: value.operation_id.to_string(),
            quote_id: value.quote_id.to_string(),
            payment_proof: value.payment_proof,
            amount: value.amount.into(),
            fee_paid: value.fee_paid.into(),
        }
    }
}

/// Payment accepted for asynchronous processing by the mint.
#[derive(uniffi::Object)]
pub struct PendingPayment {
    inner: cdk::wallet::payment::PendingPayment,
}

impl fmt::Debug for PendingPayment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl PendingPayment {
    /// Quote identifier for this pending payment.
    pub fn quote_id(&self) -> String {
        self.inner.quote_id().to_string()
    }

    /// Durable operation identifier.
    pub fn operation_id(&self) -> String {
        self.inner.operation_id().to_string()
    }

    /// Wait for this payment to finalize.
    pub async fn wait(&self) -> Result<PaymentReceipt, FfiError> {
        Ok(self.inner.wait().await?.into())
    }
}

/// Result of async-preferred outgoing payment confirmation.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum PaymentConfirmation {
    /// Payment finalized during confirmation.
    Completed { receipt: PaymentReceipt },
    /// The mint accepted asynchronous processing.
    Pending { payment: Arc<PendingPayment> },
}

impl From<cdk::wallet::payment::PaymentConfirmation> for PaymentConfirmation {
    fn from(value: cdk::wallet::payment::PaymentConfirmation) -> Self {
        match value {
            cdk::wallet::payment::PaymentConfirmation::Completed(receipt) => Self::Completed {
                receipt: receipt.into(),
            },
            cdk::wallet::payment::PaymentConfirmation::Pending(payment) => Self::Pending {
                payment: Arc::new(PendingPayment { inner: payment }),
            },
        }
    }
}

/// Reviewable, durable outgoing payment plan.
#[derive(uniffi::Object)]
pub struct PaymentPlan {
    inner: cdk::wallet::payment::PaymentPlan,
}

impl fmt::Debug for PaymentPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

#[cfg(feature = "advanced-wallet")]
impl PaymentPlan {
    pub(crate) fn from_core(inner: cdk::wallet::payment::PaymentPlan) -> Self {
        Self { inner }
    }

    pub(crate) fn core_plan(&self) -> &cdk::wallet::payment::PaymentPlan {
        &self.inner
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl PaymentPlan {
    /// Durable operation identifier used to resume this plan.
    pub fn operation_id(&self) -> String {
        self.inner.operation_id().to_string()
    }

    /// Mint quote identifier.
    pub fn quote_id(&self) -> String {
        self.inner.quote_id().to_string()
    }

    /// Value delivered by the payment.
    pub fn amount(&self) -> Amount {
        self.inner.amount().into()
    }

    /// Maximum mint, swap, and input fee charged by this plan.
    pub fn maximum_fee(&self) -> Amount {
        self.inner.maximum_fee().into()
    }

    /// Execute the plan and wait for a receipt.
    pub async fn execute(&self) -> Result<PaymentReceipt, FfiError> {
        Ok(self.inner.execute().await?.into())
    }

    /// Submit, returning early if the mint accepts async processing.
    pub async fn submit(&self) -> Result<PaymentConfirmation, FfiError> {
        Ok(self.inner.submit().await?.into())
    }

    /// Cancel the plan and release reserved funds.
    pub async fn cancel(&self) -> Result<(), FfiError> {
        self.inner.cancel().await?;
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

impl From<HistoryQuery> for cdk::wallet::history::HistoryQuery {
    fn from(value: HistoryQuery) -> Self {
        Self {
            direction: value.direction.map(Into::into),
            limit: value.limit.map(|limit| limit as usize),
        }
    }
}

/// Application-facing wallet history entry.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HistoryEntry {
    /// Stable transaction identifier.
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
    /// Related mint or melt quote identifier.
    pub quote_id: Option<String>,
    /// Durable operation identifier.
    pub operation_id: Option<String>,
    /// Payment rail, when applicable.
    pub payment_method: Option<PaymentMethod>,
    /// Durable transaction status.
    pub status: TransactionStatus,
}

impl From<cdk::wallet::history::HistoryEntry> for HistoryEntry {
    fn from(value: cdk::wallet::history::HistoryEntry) -> Self {
        Self {
            id: value.id.to_string(),
            wallet: value.wallet.into(),
            direction: value.direction.into(),
            amount: value.amount.into(),
            fee: value.fee.into(),
            timestamp: value.timestamp,
            memo: value.memo,
            metadata: value.metadata,
            quote_id: value.quote_id,
            operation_id: value.operation_id.map(|id| id.to_string()),
            payment_method: value.payment_method.map(Into::into),
            status: value.status.into(),
        }
    }
}

/// Seed-restore scan configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct RestoreRequest {
    /// Number of deterministic outputs requested per batch.
    #[uniffi(default = 100)]
    pub batch_size: u32,
    /// Consecutive empty batches that terminate the scan.
    #[uniffi(default = 3)]
    pub max_gap: u32,
}

impl From<RestoreRequest> for cdk::wallet::RestoreRequest {
    fn from(value: RestoreRequest) -> Self {
        Self {
            batch_size: value.batch_size,
            max_gap: value.max_gap,
        }
    }
}

/// Destination amount behavior for a cross-mint transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CrossMintTransferAmount {
    /// Move the maximum value supported by the source balance and mint limits.
    Maximum,
    /// Deliver an exact amount at the destination.
    Exact { amount: Amount },
}

impl From<CrossMintTransferAmount> for cdk::wallet::transfer::CrossMintTransferAmount {
    fn from(value: CrossMintTransferAmount) -> Self {
        match value {
            CrossMintTransferAmount::Maximum => Self::Maximum,
            CrossMintTransferAmount::Exact { amount } => Self::Exact(amount.into()),
        }
    }
}

/// Request to move value between two mint wallets.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CrossMintTransferRequest {
    /// Wallet that pays the Lightning invoice.
    pub source: WalletIdentity,
    /// Wallet that receives newly issued ecash.
    pub destination: WalletIdentity,
    /// Amount delivered at the destination.
    pub amount: CrossMintTransferAmount,
    /// Application metadata stored with the source transaction.
    #[uniffi(default)]
    pub metadata: HashMap<String, String>,
}

impl TryFrom<CrossMintTransferRequest> for cdk::wallet::transfer::CrossMintTransferRequest {
    type Error = FfiError;

    fn try_from(value: CrossMintTransferRequest) -> Result<Self, Self::Error> {
        Ok(Self {
            source: value.source.try_into()?,
            destination: value.destination.try_into()?,
            amount: value.amount.into(),
            metadata: value.metadata,
        })
    }
}

/// Successful cross-mint transfer result.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CrossMintTransferReceipt {
    /// Durable source operation identifier.
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

impl From<cdk::wallet::transfer::CrossMintTransferReceipt> for CrossMintTransferReceipt {
    fn from(value: cdk::wallet::transfer::CrossMintTransferReceipt) -> Self {
        Self {
            operation_id: value.operation_id.to_string(),
            destination: value.destination.into(),
            destination_quote_id: value.destination_quote_id.to_string(),
            amount: value.amount.into(),
            source_fee: value.source_fee.into(),
        }
    }
}

/// Source payment succeeded, but destination issuance remains claimable.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CrossMintClaimPending {
    /// Durable source operation identifier.
    pub operation_id: String,
    /// Destination wallet.
    pub destination: WalletIdentity,
    /// Paid destination quote that remains claimable.
    pub destination_quote_id: String,
    /// Amount expected at the destination.
    pub amount: Amount,
    /// Actual source-side fee.
    pub source_fee: Amount,
    /// Why immediate issuance failed.
    pub error_message: String,
    /// Whether retrying this same operation can be useful.
    pub retryable: bool,
}

impl From<cdk::wallet::transfer::CrossMintClaimPending> for CrossMintClaimPending {
    fn from(value: cdk::wallet::transfer::CrossMintClaimPending) -> Self {
        Self {
            operation_id: value.operation_id.to_string(),
            destination: value.destination.into(),
            destination_quote_id: value.destination_quote_id.to_string(),
            amount: value.amount.into(),
            source_fee: value.source_fee.into(),
            error_message: value.error_message,
            retryable: value.retryable,
        }
    }
}

/// Outcome of confirming a cross-mint transfer.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CrossMintTransferOutcome {
    /// Source payment and destination issuance both completed.
    Completed { receipt: CrossMintTransferReceipt },
    /// Source payment completed; destination issuance is durably recoverable.
    ClaimPending { pending: CrossMintClaimPending },
}

impl From<cdk::wallet::transfer::CrossMintTransferOutcome> for CrossMintTransferOutcome {
    fn from(value: cdk::wallet::transfer::CrossMintTransferOutcome) -> Self {
        match value {
            cdk::wallet::transfer::CrossMintTransferOutcome::Completed(receipt) => {
                Self::Completed {
                    receipt: receipt.into(),
                }
            }
            cdk::wallet::transfer::CrossMintTransferOutcome::ClaimPending(pending) => {
                Self::ClaimPending {
                    pending: pending.into(),
                }
            }
        }
    }
}

/// Durable, reviewable cross-mint transfer plan.
#[derive(uniffi::Object)]
pub struct CrossMintTransferPlan {
    inner: cdk::wallet::transfer::CrossMintTransferPlan,
}

impl fmt::Debug for CrossMintTransferPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl CrossMintTransferPlan {
    /// Durable operation identifier used to resume this plan.
    pub fn operation_id(&self) -> String {
        self.inner.operation_id().to_string()
    }

    /// Amount expected at the destination.
    pub fn amount(&self) -> Amount {
        self.inner.amount().into()
    }

    /// Maximum combined mint and input fee.
    pub fn maximum_fee(&self) -> Amount {
        self.inner.maximum_fee().into()
    }

    /// Destination wallet.
    pub fn destination(&self) -> WalletIdentity {
        self.inner.destination().clone().into()
    }

    /// Quote that will issue ecash at the destination.
    pub fn destination_quote_id(&self) -> String {
        self.inner.destination_quote_id().to_string()
    }

    /// Pay the destination quote and claim its ecash.
    pub async fn execute(&self) -> Result<CrossMintTransferOutcome, FfiError> {
        Ok(self.inner.execute().await?.into())
    }

    /// Cancel the local plan and release its source funds.
    pub async fn cancel(&self) -> Result<(), FfiError> {
        self.inner.cancel().await?;
        Ok(())
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Wallet {
    /// Open a single mint-and-unit wallet without contacting the mint.
    #[uniffi::constructor]
    pub fn open(request: WalletOpenRequest) -> Result<Self, FfiError> {
        Self::from_configuration(
            request.mint_url,
            request.unit,
            request.mnemonic,
            request.store,
            request.config.unwrap_or_default(),
        )
    }

    /// Mint and unit managed by this wallet.
    pub fn identity(&self) -> WalletIdentity {
        self.inner().identity().into()
    }

    /// Replace or disable client-side request pacing.
    pub fn set_rate_limit(&self, rate_limit: RateLimit) -> Result<(), FfiError> {
        match rate_limit.to_config()? {
            Some(config) => self.inner().set_rate_limiting_config(config),
            None => self.inner().disable_rate_limiting(),
        }
        Ok(())
    }

    /// Whether this wallet is currently pacing requests.
    pub fn is_rate_limited(&self) -> bool {
        self.inner().is_rate_limited()
    }

    /// Persist outstanding rate-limit budgets before shutdown.
    pub async fn flush_rate_limits(&self) {
        self.inner().flush_rate_limits().await;
    }

    /// Read available, pending, and reserved balances in one call.
    pub async fn balance(&self) -> Result<WalletBalance, FfiError> {
        Ok(self.inner().balance().await?.into())
    }

    /// Explicitly reconcile wallet state.
    pub async fn synchronize(&self, policy: SyncPolicy) -> Result<SyncReport, FfiError> {
        self.inner().synchronize(policy.into()).await?.try_into()
    }

    /// Discover every locally durable operation that still needs attention.
    pub async fn operations(
        &self,
        query: OperationQuery,
    ) -> Result<Vec<OperationSummary>, FfiError> {
        Ok(self
            .inner()
            .operations(query.into())
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Subscribe to high-level application events from this wallet.
    pub fn events(&self) -> Arc<WalletEventStream> {
        Arc::new(WalletEventStream {
            receiver: tokio::sync::Mutex::new(self.inner().events()),
        })
    }

    /// Create an incoming-payment session.
    pub async fn request_mint(&self, request: MintRequest) -> Result<Arc<MintSession>, FfiError> {
        Ok(Arc::new(MintSession {
            inner: self.inner().request_mint(request.into()).await?,
        }))
    }

    /// Resume a locally known incoming-payment session.
    pub async fn resume_mint(&self, quote_id: String) -> Result<Arc<MintSession>, FfiError> {
        Ok(Arc::new(MintSession {
            inner: self
                .inner()
                .resume_mint(cdk::wallet::mint::MintQuoteId::new(quote_id))
                .await?,
        }))
    }

    /// Select and reserve funds for an ecash transfer.
    pub async fn plan_send(&self, request: SendRequest) -> Result<Arc<SendPlan>, FfiError> {
        Ok(Arc::new(SendPlan {
            inner: self.inner().plan_send(request.into()).await?,
        }))
    }

    /// Prepare and execute an ecash send.
    pub async fn send(&self, request: SendRequest) -> Result<SendReceipt, FfiError> {
        Ok(self.inner().send(request.into()).await?.into())
    }

    /// Resume a prepared send after a process restart.
    pub async fn resume_send(&self, operation_id: String) -> Result<Arc<SendPlan>, FfiError> {
        Ok(Arc::new(SendPlan {
            inner: self
                .inner()
                .resume_send(parse_operation_id(&operation_id)?)
                .await?,
        }))
    }

    /// List confirmed sends whose tokens can still be checked or reclaimed.
    pub async fn pending_send_ids(&self) -> Result<Vec<String>, FfiError> {
        Ok(self
            .inner()
            .pending_send_ids()
            .await?
            .into_iter()
            .map(|operation_id| operation_id.to_string())
            .collect())
    }

    /// Check whether a confirmed send has been claimed by its receiver.
    pub async fn send_status(&self, operation_id: String) -> Result<SendStatus, FfiError> {
        Ok(self
            .inner()
            .send_status(parse_operation_id(&operation_id)?)
            .await?
            .into())
    }

    /// Reclaim an unclaimed send and return the restored value.
    pub async fn reclaim_send(&self, operation_id: String) -> Result<Amount, FfiError> {
        Ok(self
            .inner()
            .reclaim_send(parse_operation_id(&operation_id)?)
            .await?
            .into())
    }

    /// Validate and redeem an encoded token into this wallet.
    pub async fn receive(&self, request: ReceiveRequest) -> Result<ReceiveReceipt, FfiError> {
        Ok(self.inner().receive(request.into()).await?.into())
    }

    /// Reserve funds in this wallet for a NUT-18 request payment.
    pub async fn plan_request_payment(
        &self,
        request: RequestPayment,
    ) -> Result<Arc<RequestPaymentPlan>, FfiError> {
        Ok(Arc::new(RequestPaymentPlan {
            inner: self
                .inner()
                .plan_request_payment(request.try_into()?)
                .await?,
        }))
    }

    /// Prepare and execute a NUT-18 request payment.
    pub async fn pay_request(
        &self,
        request: RequestPayment,
    ) -> Result<RequestPaymentReceipt, FfiError> {
        Ok(self.inner().pay_request(request.try_into()?).await?.into())
    }

    /// Quote an outgoing payment.
    pub async fn quote_payment(
        &self,
        request: PaymentQuoteRequest,
    ) -> Result<PaymentQuoteResult, FfiError> {
        Ok(self.inner().quote_payment(request.into()).await?.into())
    }

    /// Resolve and quote a Lightning or BIP-353 address.
    pub async fn quote_address_payment(
        &self,
        request: AddressPaymentRequest,
    ) -> Result<Arc<PaymentSession>, FfiError> {
        Ok(Arc::new(PaymentSession {
            inner: self
                .inner()
                .quote_address_payment(request.try_into()?)
                .await?,
        }))
    }

    /// Resume a locally persisted outgoing payment quote before preparation.
    pub async fn resume_payment_quote(
        &self,
        quote_id: String,
    ) -> Result<Arc<PaymentSession>, FfiError> {
        Ok(Arc::new(PaymentSession {
            inner: self
                .inner()
                .resume_payment_quote(cdk::wallet::payment::PaymentQuoteId::new(quote_id))
                .await?,
        }))
    }

    /// Resume a prepared outgoing payment after a process restart.
    pub async fn resume_payment(&self, operation_id: String) -> Result<Arc<PaymentPlan>, FfiError> {
        Ok(Arc::new(PaymentPlan {
            inner: self
                .inner()
                .resume_payment(parse_operation_id(&operation_id)?)
                .await?,
        }))
    }

    /// Resume a payment that the mint is still processing.
    pub async fn resume_pending_payment(
        &self,
        operation_id: String,
    ) -> Result<Arc<PendingPayment>, FfiError> {
        Ok(Arc::new(PendingPayment {
            inner: self
                .inner()
                .resume_pending_payment(parse_operation_id(&operation_id)?)
                .await?,
        }))
    }

    /// Read application-facing transaction history.
    pub async fn history(&self, query: HistoryQuery) -> Result<Vec<HistoryEntry>, FfiError> {
        Ok(self
            .inner()
            .history(query.into())
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Re-scan deterministic wallet history from the seed.
    pub async fn restore_from_seed(&self, request: RestoreRequest) -> Result<Restored, FfiError> {
        Ok(self.inner().restore_from_seed(request.into()).await?.into())
    }
}

fn parse_operation_id(value: &str) -> Result<cdk::wallet::operation::OperationId, FfiError> {
    value
        .parse()
        .map_err(|error| FfiError::invalid_input(format!("Invalid operation ID: {error}")))
}

/// Multi-mint root object.
#[derive(uniffi::Object)]
pub struct WalletManager {
    manager: Arc<cdk::wallet::WalletManager>,
}

impl fmt::Debug for WalletManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WalletManager").finish_non_exhaustive()
    }
}

impl WalletManager {
    #[cfg(feature = "advanced-wallet")]
    pub(crate) fn inner(&self) -> Arc<cdk::wallet::WalletManager> {
        Arc::clone(&self.manager)
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl WalletManager {
    /// Open all locally configured mint wallets without network access.
    #[uniffi::constructor]
    pub fn open(request: WalletManagerOpenRequest) -> Result<Self, FfiError> {
        let db = crate::database::resolve_wallet_store(request.store)?;
        let localstore = crate::database::create_cdk_database_from_ffi(db);
        let mnemonic = Mnemonic::parse(&request.mnemonic)
            .map_err(|error| FfiError::invalid_input(format!("Invalid mnemonic: {error}")))?;
        let seed = mnemonic.to_seed_normalized("");
        let proxy_url = request
            .proxy_url
            .as_deref()
            .map(url::Url::parse)
            .transpose()
            .map_err(|error| FfiError::invalid_input(format!("Invalid URL: {error}")))?;
        let rate_limit = request
            .rate_limit
            .as_ref()
            .map(RateLimit::to_config)
            .transpose()?;
        let runtime = crate::runtime::RuntimeGuard::new().map_err(FfiError::internal)?;
        let manager = runtime.block_on(async move {
            let mut builder = cdk::wallet::WalletManagerBuilder::new()
                .with_store(localstore)
                .with_seed(seed);
            if let Some(proxy_url) = proxy_url {
                builder = builder.with_proxy(proxy_url);
            }
            builder = match rate_limit {
                Some(Some(rate_limit)) => builder.with_rate_limiting_config(rate_limit),
                Some(None) => builder.with_rate_limiting_disabled(),
                None => builder,
            };
            builder.build().await
        })?;
        Ok(Self {
            manager: Arc::new(manager),
        })
    }

    /// Replace or disable manager-wide request pacing.
    pub fn set_rate_limit(&self, rate_limit: RateLimit) -> Result<(), FfiError> {
        self.manager
            .set_rate_limiting_config(rate_limit.to_config()?);
        Ok(())
    }

    /// Whether the manager is currently pacing requests.
    pub fn is_rate_limited(&self) -> bool {
        self.manager.is_rate_limited()
    }

    /// Persist outstanding rate-limit budgets before shutdown.
    pub async fn flush_rate_limits(&self) {
        self.manager.flush_rate_limits().await;
    }

    /// Discover a mint's supported units and register wallets for them.
    pub async fn register_mint(
        &self,
        request: MintRegistrationRequest,
    ) -> Result<Vec<Arc<Wallet>>, FfiError> {
        Ok(self
            .manager
            .register_mint(cdk::wallet::MintRegistrationRequest::try_from(request)?)
            .await?
            .into_iter()
            .map(|wallet| Arc::new(Wallet::from_inner(Arc::new(wallet))))
            .collect())
    }

    /// Create or replace one mint-and-unit wallet configuration.
    pub async fn configure_wallet(
        &self,
        request: WalletConfigurationRequest,
    ) -> Result<Arc<Wallet>, FfiError> {
        let wallet = self
            .manager
            .configure_wallet(cdk::wallet::WalletConfigurationRequest::try_from(request)?)
            .await?;
        Ok(Arc::new(Wallet::from_inner(Arc::new(wallet))))
    }

    /// Return an already configured mint wallet.
    pub async fn wallet(&self, identity: WalletIdentity) -> Result<Arc<Wallet>, FfiError> {
        let wallet = self.manager.wallet(identity.try_into()?).await?;
        Ok(Arc::new(Wallet::from_inner(Arc::new(wallet))))
    }

    /// Return a mint wallet, creating its local configuration when absent.
    pub async fn open_wallet(&self, identity: WalletIdentity) -> Result<Arc<Wallet>, FfiError> {
        let wallet = self.manager.open_wallet(identity.try_into()?).await?;
        Ok(Arc::new(Wallet::from_inner(Arc::new(wallet))))
    }

    /// List all configured mint wallets.
    pub async fn wallets(&self) -> Vec<Arc<Wallet>> {
        self.manager
            .wallets()
            .await
            .into_iter()
            .map(|wallet| Arc::new(Wallet::from_inner(Arc::new(wallet))))
            .collect()
    }

    /// List configured unit wallets for one mint.
    pub async fn wallets_for_mint(&self, mint_url: MintUrl) -> Result<Vec<Arc<Wallet>>, FfiError> {
        let mint_url = cdk::mint_url::MintUrl::try_from(mint_url)?;
        Ok(self
            .manager
            .wallets_for_mint(&mint_url)
            .await
            .into_iter()
            .map(|wallet| Arc::new(Wallet::from_inner(Arc::new(wallet))))
            .collect())
    }

    /// Whether a wallet is configured for this mint and unit.
    pub async fn contains_wallet(&self, identity: WalletIdentity) -> Result<bool, FfiError> {
        Ok(self.manager.contains_wallet(&identity.try_into()?).await)
    }

    /// Whether any wallet is configured for a mint.
    pub async fn contains_mint(&self, mint_url: MintUrl) -> Result<bool, FfiError> {
        let mint_url = cdk::mint_url::MintUrl::try_from(mint_url)?;
        Ok(self.manager.contains_mint(&mint_url).await)
    }

    /// Remove one wallet from this manager without deleting persisted mint data.
    pub async fn forget_wallet(&self, identity: WalletIdentity) -> Result<(), FfiError> {
        self.manager.forget_wallet(identity.try_into()?).await?;
        Ok(())
    }

    /// Read balances for every configured mint wallet.
    pub async fn balances(&self) -> Result<Vec<WalletBalanceEntry>, FfiError> {
        Ok(self
            .manager
            .balances()
            .await?
            .into_iter()
            .map(|(wallet, balance)| WalletBalanceEntry {
                wallet: wallet.into(),
                balance: balance.into(),
            })
            .collect())
    }

    /// Sum available balances across mints, grouped by currency unit.
    pub async fn balance_totals(&self) -> Result<Vec<UnitBalanceEntry>, FfiError> {
        Ok(self
            .manager
            .balance_totals()
            .await?
            .into_iter()
            .map(|(unit, amount)| UnitBalanceEntry {
                unit: unit.into(),
                amount: amount.into(),
            })
            .collect())
    }

    /// Fetch a mint's current public capabilities.
    pub async fn mint_info(&self, mint_url: MintUrl) -> Result<MintInfo, FfiError> {
        let mint_url = cdk::mint_url::MintUrl::try_from(mint_url)?;
        Ok(self.manager.mint_info(&mint_url).await?.into())
    }

    /// Claim paid but unissued mint quotes, optionally for one mint only.
    pub async fn claim_pending_mints(&self, mint_url: Option<MintUrl>) -> Result<Amount, FfiError> {
        let mint_url = mint_url.map(TryInto::try_into).transpose()?;
        Ok(self.manager.claim_pending_mints(mint_url).await?.into())
    }

    /// Select a compatible wallet and reserve funds for a NUT-18 request payment.
    pub async fn plan_request_payment(
        &self,
        request: RequestPayment,
    ) -> Result<Arc<RequestPaymentPlan>, FfiError> {
        Ok(Arc::new(RequestPaymentPlan {
            inner: self
                .manager
                .plan_request_payment(request.try_into()?)
                .await?,
        }))
    }

    /// Select a wallet, prepare, and execute a NUT-18 request payment.
    pub async fn pay_request(
        &self,
        request: RequestPayment,
    ) -> Result<RequestPaymentReceipt, FfiError> {
        Ok(self.manager.pay_request(request.try_into()?).await?.into())
    }

    /// Create a receiver-side NUT-18 request and optional Nostr listener.
    pub async fn create_payment_request(
        &self,
        request: CreatePaymentRequest,
    ) -> Result<CreatedPaymentRequest, FfiError> {
        Ok(self
            .manager
            .create_payment_request(request.try_into()?)
            .await?
            .into())
    }

    /// Restore a Nostr request-payment receiver from persisted credentials.
    pub fn resume_payment_request_receiver(
        &self,
        state: PaymentRequestReceiverState,
    ) -> Result<Arc<PaymentRequestReceiver>, FfiError> {
        Ok(Arc::new(PaymentRequestReceiver {
            inner: self
                .manager
                .resume_payment_request_receiver(state.try_into()?)?,
        }))
    }

    /// Create a durable maximum-balance cross-mint transfer plan.
    pub async fn plan_cross_mint_transfer(
        &self,
        request: CrossMintTransferRequest,
    ) -> Result<Arc<CrossMintTransferPlan>, FfiError> {
        Ok(Arc::new(CrossMintTransferPlan {
            inner: self
                .manager
                .plan_cross_mint_transfer(request.try_into()?)
                .await?,
        }))
    }

    /// Prepare and execute a transfer between two mint wallets.
    pub async fn transfer(
        &self,
        request: CrossMintTransferRequest,
    ) -> Result<CrossMintTransferOutcome, FfiError> {
        Ok(self.manager.transfer(request.try_into()?).await?.into())
    }

    /// Resume a cross-mint transfer by its source operation identifier.
    pub async fn resume_transfer(
        &self,
        operation_id: String,
    ) -> Result<Arc<CrossMintTransferPlan>, FfiError> {
        Ok(Arc::new(CrossMintTransferPlan {
            inner: self
                .manager
                .resume_transfer(parse_operation_id(&operation_id)?)
                .await?,
        }))
    }

    /// Synchronize every configured mint wallet.
    pub async fn synchronize(&self, policy: SyncPolicy) -> Result<Vec<SyncReport>, FfiError> {
        self.manager
            .synchronize_all(policy.into())
            .await?
            .into_iter()
            .map(TryInto::try_into)
            .collect()
    }

    /// Discover active durable operations across every configured wallet.
    pub async fn operations(
        &self,
        query: OperationQuery,
    ) -> Result<Vec<OperationSummary>, FfiError> {
        Ok(self
            .manager
            .operations(query.into())
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Read application-facing history across every configured wallet.
    pub async fn history(&self, query: HistoryQuery) -> Result<Vec<HistoryEntry>, FfiError> {
        Ok(self
            .manager
            .history_all(query.into())
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }
}

/// One wallet and its current balances.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WalletBalanceEntry {
    /// Wallet whose balance was read.
    pub wallet: WalletIdentity,
    /// Spendability breakdown.
    pub balance: WalletBalance,
}

/// Available balance summed across mints for one currency unit.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct UnitBalanceEntry {
    /// Currency unit.
    pub unit: CurrencyUnit,
    /// Immediately spendable total.
    pub amount: Amount,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::use_debug)]
    #[test]
    fn open_requests_redact_credentials() {
        let request = WalletOpenRequest {
            mint_url: "https://mint.example.com".to_string(),
            unit: CurrencyUnit::Sat,
            mnemonic: "secret mnemonic".to_string(),
            store: crate::database::custom_wallet_store(
                crate::sqlite::WalletSqliteDatabase::new_in_memory()
                    .expect("in-memory wallet database should open"),
            ),
            config: None,
        };
        let output = format!("{request:?}");
        assert!(!output.contains("secret mnemonic"));
        assert!(output.contains("[REDACTED]"));
    }

    #[allow(clippy::use_debug)]
    #[test]
    fn payment_requests_redact_destinations() {
        let target = PaymentTarget::Bolt11 {
            invoice: "lnbc-sensitive-invoice".to_string(),
            amount: Bolt11PaymentAmount::Invoice,
        };
        let address = AddressPaymentRequest {
            address: "alice@example.com".to_string(),
            amount_msat: Amount::new(1_000),
            route: AddressPaymentRoute::LightningAddress,
            metadata: HashMap::new(),
        };

        let target_output = format!("{target:?}");
        let address_output = format!("{address:?}");
        assert!(!target_output.contains("lnbc-sensitive-invoice"));
        assert!(!address_output.contains("alice@example.com"));
        assert!(target_output.contains("[REDACTED]"));
        assert!(address_output.contains("[REDACTED]"));
    }
}
