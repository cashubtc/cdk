//! Wallet-related FFI types

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use cdk_common::bitcoin;
use serde::{Deserialize, Serialize};

use super::amount::{Amount, SplitTarget};
use super::proof::{Proofs, SpendingConditions};
use crate::error::FfiError;
use crate::token::Token;
use crate::{CurrencyUnit, MintUrl, PublicKey};

/// FFI-compatible SendMemo
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct SendMemo {
    /// Memo text
    pub memo: String,
    /// Include memo in token
    pub include_memo: bool,
}

impl From<SendMemo> for cdk::wallet::SendMemo {
    fn from(memo: SendMemo) -> Self {
        cdk::wallet::SendMemo {
            memo: memo.memo,
            include_memo: memo.include_memo,
        }
    }
}

impl From<cdk::wallet::SendMemo> for SendMemo {
    fn from(memo: cdk::wallet::SendMemo) -> Self {
        Self {
            memo: memo.memo,
            include_memo: memo.include_memo,
        }
    }
}

impl SendMemo {
    /// Convert SendMemo to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode SendMemo from JSON string
#[uniffi::export]
pub fn decode_send_memo(json: String) -> Result<SendMemo, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode SendMemo to JSON string
#[uniffi::export]
pub fn encode_send_memo(memo: SendMemo) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&memo)?)
}

/// FFI-compatible SendKind
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Enum)]
pub enum SendKind {
    /// Allow online swap before send if wallet does not have exact amount
    OnlineExact,
    /// Prefer offline send if difference is less than tolerance
    OnlineTolerance { tolerance: Amount },
    /// Wallet cannot do an online swap and selected proof must be exactly send amount
    OfflineExact,
    /// Wallet must remain offline but can over pay if below tolerance
    OfflineTolerance { tolerance: Amount },
}

impl From<SendKind> for cdk::wallet::SendKind {
    fn from(kind: SendKind) -> Self {
        match kind {
            SendKind::OnlineExact => cdk::wallet::SendKind::OnlineExact,
            SendKind::OnlineTolerance { tolerance } => {
                cdk::wallet::SendKind::OnlineTolerance(tolerance.into())
            }
            SendKind::OfflineExact => cdk::wallet::SendKind::OfflineExact,
            SendKind::OfflineTolerance { tolerance } => {
                cdk::wallet::SendKind::OfflineTolerance(tolerance.into())
            }
        }
    }
}

/// FFI-compatible P2PKSigningKey
#[derive(Debug, Clone, uniffi::Record)]
pub struct P2PKSigningKey {
    /// Public key
    pub pubkey: PublicKey,
    /// Derivation path as string
    pub derivation_path: String,
    /// Derivation index
    pub derivation_index: u32,
    /// Created time
    pub created_time: u64,
}

impl TryFrom<P2PKSigningKey> for cdk_common::wallet::P2PKSigningKey {
    type Error = crate::error::FfiError;

    fn try_from(key: P2PKSigningKey) -> Result<Self, FfiError> {
        Ok(Self {
            pubkey: key.pubkey.try_into()?,
            derivation_path: key
                .derivation_path
                .parse()
                .map_err(|e: bitcoin::bip32::Error| FfiError::Internal {
                    error_message: e.to_string(),
                })?,
            derivation_index: key.derivation_index,
            created_time: key.created_time,
        })
    }
}

impl From<cdk_common::wallet::P2PKSigningKey> for P2PKSigningKey {
    fn from(key: cdk_common::wallet::P2PKSigningKey) -> Self {
        Self {
            pubkey: key.pubkey.into(),
            derivation_path: key.derivation_path.to_string(),
            derivation_index: key.derivation_index,
            created_time: key.created_time,
        }
    }
}

impl From<cdk::wallet::SendKind> for SendKind {
    fn from(kind: cdk::wallet::SendKind) -> Self {
        match kind {
            cdk::wallet::SendKind::OnlineExact => SendKind::OnlineExact,
            cdk::wallet::SendKind::OnlineTolerance(tolerance) => SendKind::OnlineTolerance {
                tolerance: tolerance.into(),
            },
            cdk::wallet::SendKind::OfflineExact => SendKind::OfflineExact,
            cdk::wallet::SendKind::OfflineTolerance(tolerance) => SendKind::OfflineTolerance {
                tolerance: tolerance.into(),
            },
        }
    }
}

/// Policy controlling how keysets are loaded
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, uniffi::Enum, Default,
)]
pub enum KeysetLoadPolicy {
    /// Use in-memory cache and local database only. Never contacts the network.
    CacheOnly,
    /// Check cache first (respects TTL). Falls back to database, then network.
    #[default]
    CacheThenNetwork,
    /// Always fetch fresh data from the mint over the network.
    Refresh,
}

impl From<KeysetLoadPolicy> for cdk_common::wallet::KeysetLoadPolicy {
    fn from(policy: KeysetLoadPolicy) -> Self {
        match policy {
            KeysetLoadPolicy::CacheOnly => cdk_common::wallet::KeysetLoadPolicy::CacheOnly,
            KeysetLoadPolicy::CacheThenNetwork => {
                cdk_common::wallet::KeysetLoadPolicy::CacheThenNetwork
            }
            KeysetLoadPolicy::Refresh => cdk_common::wallet::KeysetLoadPolicy::Refresh,
        }
    }
}

impl From<cdk_common::wallet::KeysetLoadPolicy> for KeysetLoadPolicy {
    fn from(policy: cdk_common::wallet::KeysetLoadPolicy) -> Self {
        match policy {
            cdk_common::wallet::KeysetLoadPolicy::CacheOnly => KeysetLoadPolicy::CacheOnly,
            cdk_common::wallet::KeysetLoadPolicy::CacheThenNetwork => {
                KeysetLoadPolicy::CacheThenNetwork
            }
            cdk_common::wallet::KeysetLoadPolicy::Refresh => KeysetLoadPolicy::Refresh,
        }
    }
}

/// FFI-compatible P2PK locked proof send mode
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, uniffi::Enum, Default,
)]
pub enum P2PKLockedProofSendMode {
    /// Swap locked proofs into fresh proofs before creating the token
    #[default]
    Swap,
    /// Sign locked proofs and include them directly in the token
    SignAndSend,
}

impl From<P2PKLockedProofSendMode> for cdk::wallet::P2PKLockedProofSendMode {
    fn from(mode: P2PKLockedProofSendMode) -> Self {
        match mode {
            P2PKLockedProofSendMode::Swap => cdk::wallet::P2PKLockedProofSendMode::Swap,
            P2PKLockedProofSendMode::SignAndSend => {
                cdk::wallet::P2PKLockedProofSendMode::SignAndSend
            }
        }
    }
}

impl From<cdk::wallet::P2PKLockedProofSendMode> for P2PKLockedProofSendMode {
    fn from(mode: cdk::wallet::P2PKLockedProofSendMode) -> Self {
        match mode {
            cdk::wallet::P2PKLockedProofSendMode::Swap => P2PKLockedProofSendMode::Swap,
            cdk::wallet::P2PKLockedProofSendMode::SignAndSend => {
                P2PKLockedProofSendMode::SignAndSend
            }
        }
    }
}

/// FFI-compatible Send options
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct SendOptions {
    /// Memo
    pub memo: Option<SendMemo>,
    /// Spending conditions
    pub conditions: Option<SpendingConditions>,
    /// Amount split target
    pub amount_split_target: SplitTarget,
    /// Send kind
    pub send_kind: SendKind,
    /// Include fee
    pub include_fee: bool,
    pub use_p2bk: bool,
    /// Maximum number of proofs to include in the token
    pub max_proofs: Option<u32>,
    /// Metadata
    pub metadata: HashMap<String, String>,
    /// Signing keys for P2PK-locked input proofs
    #[serde(default)]
    pub p2pk_signing_keys: Vec<SecretKey>,
    /// How P2PK-locked input proofs should be handled during send
    #[serde(default)]
    pub p2pk_locked_proof_send_mode: P2PKLockedProofSendMode,
}

impl Default for SendOptions {
    fn default() -> Self {
        Self {
            memo: None,
            conditions: None,
            amount_split_target: SplitTarget::None,
            send_kind: SendKind::OnlineExact,
            include_fee: false,
            max_proofs: None,
            metadata: HashMap::new(),
            use_p2bk: false,
            p2pk_signing_keys: Vec::new(),
            p2pk_locked_proof_send_mode: P2PKLockedProofSendMode::Swap,
        }
    }
}

impl TryFrom<SendOptions> for cdk::wallet::SendOptions {
    type Error = FfiError;

    fn try_from(opts: SendOptions) -> Result<Self, Self::Error> {
        let p2pk_signing_keys = opts
            .p2pk_signing_keys
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(cdk::wallet::SendOptions {
            memo: opts.memo.map(Into::into),
            conditions: opts.conditions.map(TryInto::try_into).transpose()?,
            amount_split_target: opts.amount_split_target.into(),
            send_kind: opts.send_kind.into(),
            include_fee: opts.include_fee,
            max_proofs: opts.max_proofs.map(|p| p as usize),
            metadata: opts.metadata,
            use_p2bk: opts.use_p2bk,
            p2pk_signing_keys,
            p2pk_locked_proof_send_mode: opts.p2pk_locked_proof_send_mode.into(),
        })
    }
}

impl From<cdk::wallet::SendOptions> for SendOptions {
    fn from(opts: cdk::wallet::SendOptions) -> Self {
        Self {
            memo: opts.memo.map(Into::into),
            conditions: opts.conditions.map(Into::into),
            amount_split_target: opts.amount_split_target.into(),
            send_kind: opts.send_kind.into(),
            include_fee: opts.include_fee,
            max_proofs: opts.max_proofs.map(|p| p as u32),
            metadata: opts.metadata,
            use_p2bk: opts.use_p2bk,
            p2pk_signing_keys: opts.p2pk_signing_keys.into_iter().map(Into::into).collect(),
            p2pk_locked_proof_send_mode: opts.p2pk_locked_proof_send_mode.into(),
        }
    }
}

impl SendOptions {
    /// Convert SendOptions to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode SendOptions from JSON string
#[uniffi::export]
pub fn decode_send_options(json: String) -> Result<SendOptions, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode SendOptions to JSON string
#[uniffi::export]
pub fn encode_send_options(options: SendOptions) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&options)?)
}

/// FFI-compatible SecretKey
#[derive(Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(transparent)]
pub struct SecretKey {
    /// Hex-encoded secret key (64 characters)
    pub hex: String,
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretKey")
            .field("hex", &"[REDACTED]")
            .finish()
    }
}

impl SecretKey {
    /// Create a new SecretKey from hex string
    pub fn from_hex(hex: String) -> Result<Self, FfiError> {
        // Validate hex string length (should be 64 characters for 32 bytes)
        if hex.len() != 64 {
            return Err(FfiError::internal(
                "Secret key hex must be exactly 64 characters (32 bytes)",
            ));
        }

        // Validate hex format
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(FfiError::internal(
                "Secret key hex contains invalid characters",
            ));
        }

        Ok(Self { hex })
    }

    /// Generate a random secret key
    pub fn random() -> Self {
        use cdk::nuts::SecretKey as CdkSecretKey;
        let secret_key = CdkSecretKey::generate();
        Self {
            hex: secret_key.to_secret_hex(),
        }
    }
}

impl TryFrom<SecretKey> for cdk::nuts::SecretKey {
    type Error = FfiError;

    fn try_from(key: SecretKey) -> Result<Self, Self::Error> {
        cdk::nuts::SecretKey::from_hex(&key.hex)
            .map_err(|e| FfiError::internal(format!("Invalid secret key: {}", e)))
    }
}

impl From<cdk::nuts::SecretKey> for SecretKey {
    fn from(key: cdk::nuts::SecretKey) -> Self {
        Self {
            hex: key.to_secret_hex(),
        }
    }
}

/// FFI-compatible Receive options
#[derive(Clone, Serialize, Deserialize, uniffi::Record)]
pub struct ReceiveOptions {
    /// Amount split target
    pub amount_split_target: SplitTarget,
    /// P2PK signing keys
    #[serde(default)]
    pub p2pk_signing_keys: Vec<SecretKey>,
    /// Preimages for HTLC conditions
    pub preimages: Vec<String>,
    /// Metadata
    pub metadata: HashMap<String, String>,
}

impl fmt::Debug for ReceiveOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReceiveOptions")
            .field("amount_split_target", &self.amount_split_target)
            .field("p2pk_signing_keys", &"[REDACTED]")
            .field("preimages", &"[REDACTED]")
            .field("metadata", &self.metadata)
            .finish()
    }
}

impl Default for ReceiveOptions {
    fn default() -> Self {
        Self {
            amount_split_target: SplitTarget::None,
            p2pk_signing_keys: Vec::new(),
            preimages: Vec::new(),
            metadata: HashMap::new(),
        }
    }
}

impl TryFrom<ReceiveOptions> for cdk::wallet::ReceiveOptions {
    type Error = FfiError;

    fn try_from(opts: ReceiveOptions) -> Result<Self, Self::Error> {
        let p2pk_signing_keys = opts
            .p2pk_signing_keys
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(cdk::wallet::ReceiveOptions {
            amount_split_target: opts.amount_split_target.into(),
            p2pk_signing_keys,
            preimages: opts.preimages,
            metadata: opts.metadata,
        })
    }
}

impl From<cdk::wallet::ReceiveOptions> for ReceiveOptions {
    fn from(opts: cdk::wallet::ReceiveOptions) -> Self {
        Self {
            amount_split_target: opts.amount_split_target.into(),
            p2pk_signing_keys: opts.p2pk_signing_keys.into_iter().map(Into::into).collect(),
            preimages: opts.preimages,
            metadata: opts.metadata,
        }
    }
}

impl ReceiveOptions {
    /// Convert ReceiveOptions to JSON string
    pub fn to_json(&self) -> Result<String, FfiError> {
        Ok(serde_json::to_string(self)?)
    }
}

/// Decode ReceiveOptions from JSON string
#[uniffi::export]
pub fn decode_receive_options(json: String) -> Result<ReceiveOptions, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Encode ReceiveOptions to JSON string
#[uniffi::export]
pub fn encode_receive_options(options: ReceiveOptions) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&options)?)
}

/// FFI-compatible NUT-13 restore options
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct NUT13Options {
    /// Number of blinded messages to request per batch
    pub batch_size: u32,
    /// Number of consecutive empty batches that terminate the scan
    pub max_gap: u32,
}

impl Default for NUT13Options {
    fn default() -> Self {
        cdk::wallet::NUT13Options::default().into()
    }
}

impl TryFrom<NUT13Options> for cdk::wallet::NUT13Options {
    type Error = FfiError;

    fn try_from(opts: NUT13Options) -> Result<Self, Self::Error> {
        Ok(cdk::wallet::NUT13Options::new(
            opts.batch_size,
            opts.max_gap,
        )?)
    }
}

impl From<cdk::wallet::NUT13Options> for NUT13Options {
    fn from(opts: cdk::wallet::NUT13Options) -> Self {
        NUT13Options {
            batch_size: opts.batch_size,
            max_gap: opts.max_gap,
        }
    }
}

/// Reviewable ecash send plan.
///
/// The operation ID addresses the authoritative plan persisted by the wallet.
/// Only immutable preview values are cached on this handle.
#[derive(uniffi::Object)]
pub struct SendPlan {
    wallet: std::sync::Arc<cdk::Wallet>,
    operation_id: uuid::Uuid,
    amount: Amount,
    swap_fee: Amount,
    send_fee: Amount,
}

impl std::fmt::Debug for SendPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SendPlan")
            .field("operation_id", &self.operation_id)
            .field("amount", &self.amount)
            .finish()
    }
}

impl SendPlan {
    /// Create an application send plan from the protocol engine.
    pub(crate) fn new(
        wallet: std::sync::Arc<cdk::Wallet>,
        prepared: &cdk::wallet::PreparedSend,
    ) -> Self {
        Self {
            wallet,
            operation_id: prepared.operation_id(),
            amount: prepared.amount().into(),
            swap_fee: prepared.swap_fee().into(),
            send_fee: prepared.send_fee().into(),
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl SendPlan {
    /// Get the operation ID for this prepared send
    pub fn operation_id(&self) -> String {
        self.operation_id.to_string()
    }

    /// Get the amount to send
    pub fn amount(&self) -> Amount {
        self.amount
    }

    /// Get the total fee for this send operation
    pub fn fee(&self) -> Amount {
        Amount::new(self.swap_fee.value.saturating_add(self.send_fee.value))
    }

    /// Confirm the prepared send and create a token
    pub async fn confirm(self: std::sync::Arc<Self>) -> Result<Token, FfiError> {
        let token = self.wallet.confirm_send(self.operation_id, None).await?;

        Ok(token.into())
    }

    /// Cancel the prepared send operation
    pub async fn cancel(self: std::sync::Arc<Self>) -> Result<(), FfiError> {
        self.wallet.cancel_send(self.operation_id).await?;
        Ok(())
    }
}

/// Receipt for a finalized outgoing payment.
#[derive(Clone, uniffi::Record)]
pub struct PaymentReceipt {
    pub quote_id: String,
    pub state: super::quote::QuoteState,
    pub payment_proof: Option<String>,
    pub change: Option<Proofs>,
    pub amount: Amount,
    pub fee_paid: Amount,
}

impl fmt::Debug for PaymentReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaymentReceipt")
            .field("quote_id", &self.quote_id)
            .field("state", &self.state)
            .field(
                "payment_proof",
                &self.payment_proof.as_ref().map(|_| "[REDACTED]"),
            )
            .field("change_proof_count", &self.change.as_ref().map(Vec::len))
            .field("amount", &self.amount)
            .field("fee_paid", &self.fee_paid)
            .finish()
    }
}

impl From<cdk_common::common::FinalizedMelt> for PaymentReceipt {
    fn from(finalized: cdk_common::common::FinalizedMelt) -> Self {
        Self {
            quote_id: finalized.quote_id().to_string(),
            state: finalized.state().into(),
            payment_proof: finalized.payment_proof().map(|s: &str| s.to_string()),
            change: finalized
                .change()
                .map(|proofs| proofs.iter().cloned().map(|p| p.into()).collect()),
            amount: finalized.amount().into(),
            fee_paid: finalized.fee_paid().into(),
        }
    }
}

/// An outgoing payment accepted for asynchronous processing by the mint.
///
/// Applications receive this handle when the mint accepts a payment for
/// background processing. Call [`PendingPayment::wait`] from a background
/// task/coroutine to poll durable wallet recovery until it settles.
///
/// Mobile apps should also call [`crate::Wallet::synchronize`] with
/// [`crate::SyncPolicy::Online`] on startup/resume, because operating systems
/// may suspend or cancel long-running background waits.
#[derive(uniffi::Object)]
pub struct PendingPayment {
    wallet: Arc<cdk::Wallet>,
    quote_id: String,
    operation_id: uuid::Uuid,
}

impl std::fmt::Debug for PendingPayment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingPayment")
            .field("operation_id", &self.operation_id)
            .field("quote_id", &self.quote_id)
            .finish()
    }
}

impl PendingPayment {
    pub(crate) fn new(wallet: Arc<cdk::Wallet>, pending: &cdk::wallet::PendingMelt) -> Self {
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

    /// Wait for this pending payment to complete.
    ///
    /// This method polls the wallet's durable recovery path until the payment
    /// finalizes or fails.
    ///
    /// This can wait for an extended period. Swift/Kotlin callers should run it
    /// in a cancellable background task or coroutine, not directly in UI
    /// control flow. If the app is suspended or killed before this returns,
    /// call `Wallet::synchronize(SyncPolicy::Online)` after restart/resume.
    pub async fn wait(&self) -> Result<PaymentReceipt, FfiError> {
        let finalized = self.wallet.wait_pending_melt(self.operation_id).await?;

        Ok(finalized.into())
    }
}

/// Result of async-preferred outgoing payment confirmation.
///
/// `Completed` means the payment finalized during confirmation. `Pending`
/// means the mint accepted it for asynchronous processing; call
/// [`PendingPayment::wait`] to complete the normal app flow.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum PaymentConfirmation {
    /// Payment finalized during confirmation.
    Completed { receipt: PaymentReceipt },
    /// Mint accepted async processing and the payment is still pending.
    Pending { payment: Arc<PendingPayment> },
}

/// Reviewable outgoing payment plan.
///
/// The operation ID addresses the authoritative plan persisted by the wallet.
/// Only immutable preview values are cached on this handle.
#[derive(uniffi::Object)]
pub struct PaymentPlan {
    wallet: Arc<cdk::Wallet>,
    operation_id: uuid::Uuid,
    quote_id: String,
    amount: Amount,
    fee_reserve: Amount,
    swap_fee: Amount,
    input_fee: Amount,
}

impl std::fmt::Debug for PaymentPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaymentPlan")
            .field("operation_id", &self.operation_id)
            .field("quote_id", &self.quote_id)
            .field("amount", &self.amount)
            .finish()
    }
}

impl PaymentPlan {
    /// Create an application payment plan from the protocol engine.
    pub(crate) fn new(wallet: Arc<cdk::Wallet>, prepared: &cdk::wallet::PreparedMelt) -> Self {
        Self {
            wallet,
            operation_id: prepared.operation_id(),
            quote_id: prepared.quote().id.clone(),
            amount: prepared.amount().into(),
            fee_reserve: prepared.quote().fee_reserve.into(),
            swap_fee: prepared.swap_fee().into(),
            input_fee: prepared.input_fee().into(),
        }
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
                receipt: finalized.into(),
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

    /// Get the quote ID
    pub fn quote_id(&self) -> String {
        self.quote_id.clone()
    }

    /// Amount delivered by the payment.
    pub fn amount(&self) -> Amount {
        self.amount
    }

    /// Maximum mint, swap, and input fee charged by this plan.
    pub fn maximum_fee(&self) -> Amount {
        Amount::new(
            self.fee_reserve
                .value
                .saturating_add(self.swap_fee.value)
                .saturating_add(self.input_fee.value),
        )
    }

    /// Confirm the plan and execute the payment.
    pub async fn confirm(&self) -> Result<PaymentReceipt, FfiError> {
        let finalized = self
            .wallet
            .confirm_prepared_melt_with_options(
                self.operation_id,
                cdk::wallet::MeltConfirmOptions::default(),
            )
            .await?;

        Ok(finalized.into())
    }

    /// Confirm the plan, allowing asynchronous processing when supported.
    ///
    /// If the melt completes immediately, this returns
    /// `PaymentConfirmation::Completed`. If the mint accepts the payment for
    /// background processing, this returns `PaymentConfirmation::Pending` with a
    /// `PendingPayment` handle.
    ///
    /// Call `PendingPayment::wait()` from a background task/coroutine to poll
    /// for completion. Mobile apps should also call
    /// `Wallet::synchronize(SyncPolicy::Online)` on startup/resume.
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

/// FFI-compatible MeltOptions
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Enum)]
pub enum MeltOptions {
    /// MPP (Multi-Part Payments) options
    Mpp { amount: Amount },
    /// Amountless options
    Amountless { amount_msat: Amount },
}

impl From<MeltOptions> for cdk::nuts::MeltOptions {
    fn from(opts: MeltOptions) -> Self {
        match opts {
            MeltOptions::Mpp { amount } => {
                let cdk_amount: cdk::Amount = amount.into();
                cdk::nuts::MeltOptions::new_mpp(cdk_amount)
            }
            MeltOptions::Amountless { amount_msat } => {
                let cdk_amount: cdk::Amount = amount_msat.into();
                cdk::nuts::MeltOptions::new_amountless(cdk_amount)
            }
        }
    }
}

impl From<cdk::nuts::MeltOptions> for MeltOptions {
    fn from(opts: cdk::nuts::MeltOptions) -> Self {
        match opts {
            cdk::nuts::MeltOptions::Mpp { mpp } => MeltOptions::Mpp {
                amount: mpp.amount.into(),
            },
            cdk::nuts::MeltOptions::Amountless { amountless } => MeltOptions::Amountless {
                amount_msat: amountless.amount_msat.into(),
            },
        }
    }
}

/// Restored Data
#[derive(Debug, Clone, uniffi::Record)]
pub struct Restored {
    pub spent: Amount,
    pub unspent: Amount,
    pub pending: Amount,
}

impl From<cdk_common::wallet::Restored> for Restored {
    fn from(restored: cdk_common::wallet::Restored) -> Self {
        Self {
            spent: restored.spent.into(),
            unspent: restored.unspent.into(),
            pending: restored.pending.into(),
        }
    }
}

/// Report of wallet saga recovery operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Record)]
pub struct RecoveryReport {
    /// Operations successfully completed after crash.
    pub recovered: u64,
    /// Operations rolled back and resources released.
    pub compensated: u64,
    /// Operations still pending and left for a later retry.
    pub skipped: u64,
    /// Operations that could not be recovered.
    pub failed: u64,
}

impl From<cdk::wallet::RecoveryReport> for RecoveryReport {
    fn from(report: cdk::wallet::RecoveryReport) -> Self {
        Self {
            recovered: report.recovered as u64,
            compensated: report.compensated as u64,
            skipped: report.skipped as u64,
            failed: report.failed as u64,
        }
    }
}

/// FFI-compatible WalletKey
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct WalletKey {
    /// Mint Url
    pub mint_url: MintUrl,
    /// Currency Unit
    pub unit: CurrencyUnit,
}

impl TryFrom<WalletKey> for cdk::WalletKey {
    type Error = FfiError;

    fn try_from(value: WalletKey) -> Result<Self, Self::Error> {
        Ok(Self {
            mint_url: value.mint_url.try_into()?,
            unit: value.unit.into(),
        })
    }
}

impl From<cdk::WalletKey> for WalletKey {
    fn from(value: cdk::WalletKey) -> Self {
        Self {
            mint_url: value.mint_url.into(),
            unit: value.unit.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::proof::Proof;

    #[test]
    fn receive_options_debug_redacts_preimages() {
        let preimage = "ffi-receive-preimage-secret";
        let options = ReceiveOptions {
            preimages: vec![preimage.to_string()],
            ..Default::default()
        };

        let debug = format!("{options:?}");

        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(preimage));
    }

    #[test]
    fn payment_receipt_debug_redacts_payment_proof_and_change_proofs() {
        let preimage = "payment-preimage";
        let proof_secret = "change-proof-secret";
        let finalized = PaymentReceipt {
            quote_id: "public-melt-quote-id".to_string(),
            state: super::super::quote::QuoteState::Paid,
            payment_proof: Some(preimage.to_string()),
            change: Some(vec![Proof {
                amount: Amount::new(1),
                secret: proof_secret.to_string(),
                c: "public-signature".to_string(),
                keyset_id: "public-keyset-id".to_string(),
                witness: None,
                dleq: None,
                p2pk_e: None,
            }]),
            amount: Amount::new(100),
            fee_paid: Amount::new(1),
        };

        let debug = format!("{finalized:?}");

        assert!(debug.contains("public-melt-quote-id"));
        assert!(debug.contains("change_proof_count: Some(1)"));
        assert!(!debug.contains(preimage));
        assert!(!debug.contains(proof_secret));
    }
}
