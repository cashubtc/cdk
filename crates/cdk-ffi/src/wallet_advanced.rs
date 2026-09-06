//! Thin UniFFI exposure of explicit expert wallet capabilities.
//!
//! These objects convert foreign-language records into `cdk::wallet::advanced`
//! requests. Proof selection, reissue, validation, subscription, and recovery
//! behavior remains implemented by the core wallet.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use cdk::wallet::advanced as core;

use crate::error::FfiError;
#[cfg(feature = "npubcash")]
use crate::npubcash::NpubCashUserResponse;
use crate::token::Token;
use crate::types::{
    Amount, AuthProof, CurrencyUnit, KeySet, MintBackupReceipt, MintBackupRequest, MintInfo,
    MintQuote, MintRestoreReceipt, MintRestoreRequest, MintUrl, P2PKSigningKey, Proof, ProofInfo,
    ProofState, ProofStateUpdate, PublicKey, SpendingConditions, SplitTarget, Transaction,
    TransactionId,
};
use crate::wallet::Wallet;
use crate::wallet_api::{
    MintReceipt, MintSession, PaymentConfirmation, PaymentPlan, PaymentReceipt, PaymentSession,
    ReceiveReceipt, ReceiveRequest, SendPlan, SendReceipt, SendRequest, WalletIdentity,
    WalletManager,
};

fn convert_proofs(proofs: Vec<Proof>) -> Result<cdk::nuts::Proofs, FfiError> {
    proofs.into_iter().map(TryInto::try_into).collect()
}

fn convert_signing_keys(keys: Vec<String>) -> Result<Vec<cdk::nuts::SecretKey>, FfiError> {
    keys.into_iter()
        .map(|key| {
            cdk::nuts::SecretKey::from_hex(&key)
                .map_err(|error| FfiError::invalid_input(format!("Invalid signing key: {error}")))
        })
        .collect()
}

/// Explicit denomination and locking controls for mint issuance.
#[derive(Debug, Clone, uniffi::Record)]
pub struct MintClaimOptions {
    /// Explicit output denomination strategy.
    pub amount_split_target: SplitTarget,
    /// Spending conditions applied to newly issued proofs.
    #[uniffi(default = None)]
    pub conditions: Option<SpendingConditions>,
}

impl TryFrom<MintClaimOptions> for core::MintClaimOptions {
    type Error = FfiError;

    fn try_from(value: MintClaimOptions) -> Result<Self, Self::Error> {
        Ok(Self {
            amount_split_target: value.amount_split_target.into(),
            conditions: value.conditions.map(TryInto::try_into).transpose()?,
        })
    }
}

/// How an advanced send treats selected P2PK-locked proofs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LockedProofPolicy {
    /// Reissue locked proofs into fresh bearer proofs before sending.
    Reissue,
    /// Sign locked proofs and include them directly in the token.
    PassThrough,
}

impl From<LockedProofPolicy> for core::LockedProofPolicy {
    fn from(value: LockedProofPolicy) -> Self {
        match value {
            LockedProofPolicy::Reissue => Self::Reissue,
            LockedProofPolicy::PassThrough => Self::PassThrough,
        }
    }
}

/// Proof-shaping and locking controls for an advanced ecash send.
#[derive(Clone, uniffi::Record)]
pub struct SendAdvancedOptions {
    /// Spending conditions applied to newly created proofs.
    #[uniffi(default = None)]
    pub conditions: Option<SpendingConditions>,
    /// Explicit output denomination strategy.
    pub amount_split_target: SplitTarget,
    /// Maximum number of proofs encoded into the token.
    #[uniffi(default = None)]
    pub max_proofs: Option<u32>,
    /// Use NUT-28 P2BK output encryption.
    #[uniffi(default = false)]
    pub use_p2bk: bool,
    /// Hex-encoded keys for selected P2PK-locked inputs.
    #[uniffi(default)]
    pub p2pk_signing_keys: Vec<String>,
    /// How selected P2PK-locked proofs are handled.
    pub locked_proof_policy: LockedProofPolicy,
}

impl fmt::Debug for SendAdvancedOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendAdvancedOptions")
            .field("conditions", &self.conditions)
            .field("amount_split_target", &self.amount_split_target)
            .field("max_proofs", &self.max_proofs)
            .field("use_p2bk", &self.use_p2bk)
            .field("p2pk_signing_keys", &"[REDACTED]")
            .field("locked_proof_policy", &self.locked_proof_policy)
            .finish()
    }
}

impl TryFrom<SendAdvancedOptions> for core::SendAdvancedOptions {
    type Error = FfiError;

    fn try_from(value: SendAdvancedOptions) -> Result<Self, Self::Error> {
        Ok(Self {
            conditions: value.conditions.map(TryInto::try_into).transpose()?,
            amount_split_target: value.amount_split_target.into(),
            max_proofs: value.max_proofs.map(|value| value as usize),
            use_p2bk: value.use_p2bk,
            p2pk_signing_keys: convert_signing_keys(value.p2pk_signing_keys)?,
            locked_proof_policy: value.locked_proof_policy.into(),
        })
    }
}

/// Credentials and denomination controls for receiving locked ecash.
#[derive(Clone, uniffi::Record)]
pub struct ReceiveAdvancedOptions {
    /// Explicit output denomination strategy.
    pub amount_split_target: SplitTarget,
    /// Hex-encoded P2PK signing keys used to unlock proofs.
    #[uniffi(default)]
    pub p2pk_signing_keys: Vec<String>,
    /// HTLC preimages used to unlock proofs.
    #[uniffi(default)]
    pub preimages: Vec<String>,
}

impl fmt::Debug for ReceiveAdvancedOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReceiveAdvancedOptions")
            .field("amount_split_target", &self.amount_split_target)
            .field("p2pk_signing_keys", &"[REDACTED]")
            .field("preimages", &"[REDACTED]")
            .finish()
    }
}

impl TryFrom<ReceiveAdvancedOptions> for core::ReceiveAdvancedOptions {
    type Error = FfiError;

    fn try_from(value: ReceiveAdvancedOptions) -> Result<Self, Self::Error> {
        Ok(Self {
            amount_split_target: value.amount_split_target.into(),
            p2pk_signing_keys: convert_signing_keys(value.p2pk_signing_keys)?,
            preimages: value.preimages,
        })
    }
}

/// Explicit source of funds for an advanced outgoing payment.
#[derive(Clone, uniffi::Enum)]
pub enum PaymentFunding {
    /// Select spendable proofs from the wallet.
    Wallet,
    /// Use the provided raw proof set.
    Proofs { proofs: Vec<Proof> },
    /// Decode and use an encoded Cashu token.
    Token { token: String },
}

impl fmt::Debug for PaymentFunding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wallet => f.write_str("Wallet"),
            Self::Proofs { proofs } => f
                .debug_struct("Proofs")
                .field("proof_count", &proofs.len())
                .finish(),
            Self::Token { .. } => f.debug_tuple("Token").field(&"[REDACTED]").finish(),
        }
    }
}

impl TryFrom<PaymentFunding> for core::PaymentFunding {
    type Error = FfiError;

    fn try_from(value: PaymentFunding) -> Result<Self, Self::Error> {
        match value {
            PaymentFunding::Wallet => Ok(Self::Wallet),
            PaymentFunding::Proofs { proofs } => Ok(Self::Proofs(convert_proofs(proofs)?)),
            PaymentFunding::Token { token } => Ok(Self::Token(token)),
        }
    }
}

/// Explicit funding controls for preparing a payment plan.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PaymentPrepareOptions {
    /// Funds to reserve for the payment.
    pub funding: PaymentFunding,
}

impl TryFrom<PaymentPrepareOptions> for core::PaymentPrepareOptions {
    type Error = FfiError;

    fn try_from(value: PaymentPrepareOptions) -> Result<Self, Self::Error> {
        Ok(Self {
            funding: value.funding.try_into()?,
        })
    }
}

/// Where mint metadata may be loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MetadataSource {
    /// Use cached metadata and never contact the mint.
    CacheOnly,
    /// Prefer cached metadata and refresh when absent or stale.
    CacheOrNetwork,
    /// Contact the mint and replace the cached snapshot.
    Refresh,
}

impl From<MetadataSource> for core::MetadataSource {
    fn from(value: MetadataSource) -> Self {
        match value {
            MetadataSource::CacheOnly => Self::CacheOnly,
            MetadataSource::CacheOrNetwork => Self::CacheOrNetwork,
            MetadataSource::Refresh => Self::Refresh,
        }
    }
}

/// Consistent mint-info and keyset snapshot.
#[derive(Debug, Clone, uniffi::Record)]
pub struct MintMetadataSnapshot {
    /// Mint capabilities and contact information.
    pub info: MintInfo,
    /// Active and historical keysets for this wallet's unit.
    pub keysets: Vec<KeySet>,
}

impl From<core::MintMetadataSnapshot> for MintMetadataSnapshot {
    fn from(value: core::MintMetadataSnapshot) -> Self {
        Self {
            info: value.info.into(),
            keysets: value.keysets.into_iter().map(Into::into).collect(),
        }
    }
}

/// Filter for inspecting locally stored proofs.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct ProofQuery {
    /// States to include. An empty list includes every state.
    #[uniffi(default)]
    pub states: Vec<ProofState>,
    /// Optional spending-condition filters.
    #[uniffi(default = None)]
    pub conditions: Option<Vec<SpendingConditions>>,
}

impl TryFrom<ProofQuery> for core::ProofQuery {
    type Error = FfiError;

    fn try_from(value: ProofQuery) -> Result<Self, Self::Error> {
        Ok(Self {
            states: value.states.into_iter().map(Into::into).collect(),
            conditions: value
                .conditions
                .map(|conditions| {
                    conditions
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?,
        })
    }
}

/// Treatment of future redemption fees during proof reissue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ReissueFeePolicy {
    /// Return the requested face value without adding its future fee.
    Deduct,
    /// Add the future fee so the output redeems to the requested value.
    AddToOutput,
}

impl From<ReissueFeePolicy> for core::ReissueFeePolicy {
    fn from(value: ReissueFeePolicy) -> Self {
        match value {
            ReissueFeePolicy::Deduct => Self::Deduct,
            ReissueFeePolicy::AddToOutput => Self::AddToOutput,
        }
    }
}

/// Output encryption applied during proof reissue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ReissueProtection {
    /// Create ordinary Cashu proofs.
    Plain,
    /// Create NUT-28 P2BK-encrypted proofs.
    P2bk,
}

impl From<ReissueProtection> for core::ReissueProtection {
    fn from(value: ReissueProtection) -> Self {
        match value {
            ReissueProtection::Plain => Self::Plain,
            ReissueProtection::P2bk => Self::P2bk,
        }
    }
}

/// Expert request to reissue an explicit proof set.
#[derive(Clone, uniffi::Record)]
pub struct ReissueRequest {
    /// Input proofs consumed by the swap.
    pub proofs: Vec<Proof>,
    /// Selected output amount, or all value when omitted.
    #[uniffi(default = None)]
    pub amount: Option<Amount>,
    /// Explicit output denomination strategy.
    pub amount_split_target: SplitTarget,
    /// Spending conditions applied to selected outputs.
    #[uniffi(default = None)]
    pub conditions: Option<SpendingConditions>,
    /// Treatment of future redemption fees.
    pub fee_policy: ReissueFeePolicy,
    /// Optional NUT-28 output protection.
    pub protection: ReissueProtection,
}

impl fmt::Debug for ReissueRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReissueRequest")
            .field("proof_count", &self.proofs.len())
            .field("amount", &self.amount)
            .field("amount_split_target", &self.amount_split_target)
            .field("conditions", &self.conditions)
            .field("fee_policy", &self.fee_policy)
            .field("protection", &self.protection)
            .finish()
    }
}

impl TryFrom<ReissueRequest> for core::ReissueRequest {
    type Error = FfiError;

    fn try_from(value: ReissueRequest) -> Result<Self, Self::Error> {
        Ok(Self {
            proofs: convert_proofs(value.proofs)?,
            amount: value.amount.map(Into::into),
            amount_split_target: value.amount_split_target.into(),
            conditions: value.conditions.map(TryInto::try_into).transpose()?,
            fee_policy: value.fee_policy.into(),
            protection: value.protection.into(),
        })
    }
}

/// Proofs selected by an expert reissue operation.
#[derive(Clone, uniffi::Record)]
pub struct ReissueReceipt {
    /// Selected outputs. Omission means all reissued value stayed in the wallet.
    pub proofs: Option<Vec<Proof>>,
}

impl fmt::Debug for ReissueReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReissueReceipt")
            .field(
                "proof_count",
                &self.proofs.as_ref().map(|proofs| proofs.len()),
            )
            .finish()
    }
}

impl From<core::ReissueReceipt> for ReissueReceipt {
    fn from(value: core::ReissueReceipt) -> Self {
        Self {
            proofs: value
                .proofs
                .map(|proofs| proofs.into_iter().map(Into::into).collect()),
        }
    }
}

/// Expert request to import raw proofs rather than an encoded token.
#[derive(Clone, uniffi::Record)]
pub struct ProofImportRequest {
    /// Proofs to redeem.
    pub proofs: Vec<Proof>,
    /// Explicit output denomination strategy.
    pub amount_split_target: SplitTarget,
    /// Hex-encoded P2PK signing keys used to unlock proofs.
    #[uniffi(default)]
    pub p2pk_signing_keys: Vec<String>,
    /// HTLC preimages used to unlock proofs.
    #[uniffi(default)]
    pub preimages: Vec<String>,
    /// Optional transaction memo.
    #[uniffi(default = None)]
    pub memo: Option<String>,
    /// Optional original encoded token retained with history.
    #[uniffi(default = None)]
    pub encoded_token: Option<String>,
    /// Application metadata stored with the transaction.
    #[uniffi(default)]
    pub metadata: HashMap<String, String>,
}

impl fmt::Debug for ProofImportRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProofImportRequest")
            .field("proof_count", &self.proofs.len())
            .field("amount_split_target", &self.amount_split_target)
            .field("p2pk_signing_keys", &"[REDACTED]")
            .field("preimages", &"[REDACTED]")
            .field("memo", &self.memo)
            .field(
                "encoded_token",
                &self.encoded_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("metadata", &self.metadata)
            .finish()
    }
}

impl TryFrom<ProofImportRequest> for core::ProofImportRequest {
    type Error = FfiError;

    fn try_from(value: ProofImportRequest) -> Result<Self, Self::Error> {
        Ok(Self {
            proofs: convert_proofs(value.proofs)?,
            amount_split_target: value.amount_split_target.into(),
            p2pk_signing_keys: value
                .p2pk_signing_keys
                .into_iter()
                .map(|key| {
                    cdk::nuts::SecretKey::from_hex(&key).map_err(|error| {
                        FfiError::invalid_input(format!("Invalid signing key: {error}"))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            preimages: value.preimages,
            memo: value.memo,
            encoded_token: value.encoded_token,
            metadata: value.metadata,
        })
    }
}

/// Expert fee-estimation input.
#[derive(Clone, uniffi::Enum)]
pub enum FeeEstimateRequest {
    /// Fee charged when an explicit proof set is redeemed.
    Proofs { proofs: Vec<Proof> },
    /// Fee charged for a number of inputs from one keyset.
    KeysetInputs { keyset_id: String, count: u64 },
}

impl fmt::Debug for FeeEstimateRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Proofs { proofs } => f
                .debug_struct("Proofs")
                .field("proof_count", &proofs.len())
                .finish(),
            Self::KeysetInputs { keyset_id, count } => f
                .debug_struct("KeysetInputs")
                .field("keyset_id", keyset_id)
                .field("count", count)
                .finish(),
        }
    }
}

impl TryFrom<FeeEstimateRequest> for core::FeeEstimateRequest {
    type Error = FfiError;

    fn try_from(value: FeeEstimateRequest) -> Result<Self, Self::Error> {
        match value {
            FeeEstimateRequest::Proofs { proofs } => Ok(Self::Proofs(convert_proofs(proofs)?)),
            FeeEstimateRequest::KeysetInputs { keyset_id, count } => Ok(Self::KeysetInputs {
                keyset_id: keyset_id.parse().map_err(|error| {
                    FfiError::invalid_input(format!("Invalid keyset ID: {error}"))
                })?,
                count,
            }),
        }
    }
}

/// Fee attributed to one keyset.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct KeysetFeeEntry {
    /// Protocol keyset identifier.
    pub keyset_id: String,
    /// Fee charged by inputs from this keyset.
    pub amount: Amount,
}

/// Breakdown of a proof-input fee estimate.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FeeEstimate {
    /// Total fee.
    pub total: Amount,
    /// Per-keyset contributions.
    pub per_keyset: Vec<KeysetFeeEntry>,
}

impl From<cdk::fees::ProofsFeeBreakdown> for FeeEstimate {
    fn from(value: cdk::fees::ProofsFeeBreakdown) -> Self {
        let mut per_keyset = value
            .per_keyset
            .into_iter()
            .map(|(keyset_id, amount)| KeysetFeeEntry {
                keyset_id: keyset_id.to_string(),
                amount: amount.into(),
            })
            .collect::<Vec<_>>();
        per_keyset.sort_by(|left, right| left.keyset_id.cmp(&right.keyset_id));
        Self {
            total: value.total.into(),
            per_keyset,
        }
    }
}

/// Checks applied to an encoded token without redeeming it.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum TokenCheck {
    /// Verify every proof's DLEQ evidence.
    Dleq,
    /// Require every proof to meet the provided offline conditions.
    ValidateSpendingConditions { conditions: SpendingConditions },
}

impl TryFrom<TokenCheck> for core::TokenCheck {
    type Error = FfiError;

    fn try_from(value: TokenCheck) -> Result<Self, Self::Error> {
        match value {
            TokenCheck::Dleq => Ok(Self::Dleq),
            TokenCheck::ValidateSpendingConditions { conditions } => {
                Ok(Self::ValidateSpendingConditions(conditions.try_into()?))
            }
        }
    }
}

/// Request to validate a token without changing wallet state.
#[derive(Clone, uniffi::Record)]
pub struct TokenValidationRequest {
    /// Token to validate.
    pub token: Arc<Token>,
    /// Checks applied in order.
    pub checks: Vec<TokenCheck>,
}

impl fmt::Debug for TokenValidationRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenValidationRequest")
            .field("token", &"[REDACTED]")
            .field("checks", &self.checks)
            .finish()
    }
}

/// Raw transaction and proofs retained for protocol-level inspection.
#[derive(Clone, uniffi::Record)]
pub struct TransactionDetails {
    /// Stored transaction record.
    pub transaction: Transaction,
    /// Proofs still present locally for the transaction.
    pub proofs: Vec<Proof>,
}

impl fmt::Debug for TransactionDetails {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransactionDetails")
            .field("transaction", &self.transaction)
            .field("proof_count", &self.proofs.len())
            .finish()
    }
}

impl From<core::TransactionDetails> for TransactionDetails {
    fn from(value: core::TransactionDetails) -> Self {
        Self {
            transaction: value.transaction.into(),
            proofs: value.proofs.into_iter().map(Into::into).collect(),
        }
    }
}

/// Result of reconciling an outgoing transaction.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum TransactionRecovery {
    /// A durable send was unclaimed and its value was restored.
    SendReclaimed { amount: Amount },
    /// A legacy transaction's pending proofs were checked without reissuing.
    LegacyReconciled,
}

impl From<core::TransactionRecovery> for TransactionRecovery {
    fn from(value: core::TransactionRecovery) -> Self {
        match value {
            core::TransactionRecovery::SendReclaimed { amount } => Self::SendReclaimed {
                amount: amount.into(),
            },
            core::TransactionRecovery::LegacyReconciled => Self::LegacyReconciled,
        }
    }
}

/// Whether an advanced payment execution may reissue proofs first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PaymentSwapPolicy {
    /// Reissue proofs when needed or fee-efficient.
    Allow,
    /// Submit the selected proofs directly to the melt endpoint.
    Skip,
}

impl From<PaymentSwapPolicy> for core::PaymentSwapPolicy {
    fn from(value: PaymentSwapPolicy) -> Self {
        match value {
            PaymentSwapPolicy::Allow => Self::Allow,
            PaymentSwapPolicy::Skip => Self::Skip,
        }
    }
}

/// Protocol-specific controls for executing an outgoing payment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct PaymentExecutionOptions {
    /// Proof-reissue policy.
    pub swap: PaymentSwapPolicy,
}

impl From<PaymentExecutionOptions> for core::PaymentExecutionOptions {
    fn from(value: PaymentExecutionOptions) -> Self {
        Self {
            swap: value.swap.into(),
        }
    }
}

/// Advanced execution view of a normal durable payment plan.
#[derive(uniffi::Object)]
pub struct AdvancedPaymentPlan {
    inner: cdk::wallet::payment::PaymentPlan,
}

impl fmt::Debug for AdvancedPaymentPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl AdvancedPaymentPlan {
    /// Execute with an explicit proof-reissue policy and wait for settlement.
    pub async fn execute(
        &self,
        options: PaymentExecutionOptions,
    ) -> Result<PaymentReceipt, FfiError> {
        Ok(self.inner.execute_with(options.into()).await?.into())
    }

    /// Submit with an explicit proof-reissue policy, returning on async acceptance.
    pub async fn submit(
        &self,
        options: PaymentExecutionOptions,
    ) -> Result<PaymentConfirmation, FfiError> {
        Ok(self.inner.submit_with(options.into()).await?.into())
    }
}

/// Low-level proof or quote notification filter.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum SubscriptionRequest {
    ProofStates {
        filters: Vec<String>,
    },
    Bolt11MintQuotes {
        quote_ids: Vec<String>,
    },
    Bolt11Payments {
        quote_ids: Vec<String>,
    },
    Bolt12MintQuotes {
        quote_ids: Vec<String>,
    },
    Bolt12Payments {
        quote_ids: Vec<String>,
    },
    OnchainMintQuotes {
        quote_ids: Vec<String>,
    },
    OnchainPayments {
        quote_ids: Vec<String>,
    },
    CustomMintQuotes {
        method: String,
        quote_ids: Vec<String>,
    },
    CustomPayments {
        method: String,
        quote_ids: Vec<String>,
    },
}

impl From<SubscriptionRequest> for core::SubscriptionRequest {
    fn from(value: SubscriptionRequest) -> Self {
        match value {
            SubscriptionRequest::ProofStates { filters } => Self::ProofStates(filters),
            SubscriptionRequest::Bolt11MintQuotes { quote_ids } => {
                Self::Bolt11MintQuotes(quote_ids)
            }
            SubscriptionRequest::Bolt11Payments { quote_ids } => Self::Bolt11Payments(quote_ids),
            SubscriptionRequest::Bolt12MintQuotes { quote_ids } => {
                Self::Bolt12MintQuotes(quote_ids)
            }
            SubscriptionRequest::Bolt12Payments { quote_ids } => Self::Bolt12Payments(quote_ids),
            SubscriptionRequest::OnchainMintQuotes { quote_ids } => {
                Self::OnchainMintQuotes(quote_ids)
            }
            SubscriptionRequest::OnchainPayments { quote_ids } => Self::OnchainPayments(quote_ids),
            SubscriptionRequest::CustomMintQuotes { method, quote_ids } => {
                Self::CustomMintQuotes { method, quote_ids }
            }
            SubscriptionRequest::CustomPayments { method, quote_ids } => {
                Self::CustomPayments { method, quote_ids }
            }
        }
    }
}

/// Protocol notification kind accompanying a JSON payload.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum NotificationKind {
    ProofState,
    Bolt11MintQuote,
    Bolt11Payment,
    Bolt12MintQuote,
    Bolt12Payment,
    OnchainMintQuote,
    OnchainPayment,
    CustomMintQuote { method: String },
    CustomPayment { method: String },
}

/// One low-level NUT-17 notification.
#[derive(Debug, Clone, uniffi::Record)]
pub struct WalletNotification {
    /// Protocol notification kind.
    pub kind: NotificationKind,
    /// JSON encoding of the corresponding Cashu response type.
    pub payload_json: String,
}

fn wallet_notification(
    event: cdk::event::MintEvent<String>,
) -> Result<WalletNotification, FfiError> {
    let kind = match event.inner() {
        cdk::nuts::NotificationPayload::ProofState(_) => NotificationKind::ProofState,
        cdk::nuts::NotificationPayload::MintQuoteBolt11Response(_) => {
            NotificationKind::Bolt11MintQuote
        }
        cdk::nuts::NotificationPayload::MeltQuoteBolt11Response(_) => {
            NotificationKind::Bolt11Payment
        }
        cdk::nuts::NotificationPayload::MintQuoteBolt12Response(_) => {
            NotificationKind::Bolt12MintQuote
        }
        cdk::nuts::NotificationPayload::MeltQuoteBolt12Response(_) => {
            NotificationKind::Bolt12Payment
        }
        cdk::nuts::NotificationPayload::MintQuoteOnchainResponse(_) => {
            NotificationKind::OnchainMintQuote
        }
        cdk::nuts::NotificationPayload::MeltQuoteOnchainResponse(_) => {
            NotificationKind::OnchainPayment
        }
        cdk::nuts::NotificationPayload::CustomMintQuoteResponse(method, _) => {
            NotificationKind::CustomMintQuote {
                method: method.clone(),
            }
        }
        cdk::nuts::NotificationPayload::CustomMeltQuoteResponse(method, _) => {
            NotificationKind::CustomPayment {
                method: method.clone(),
            }
        }
    };
    Ok(WalletNotification {
        kind,
        payload_json: serde_json::to_string(event.inner())?,
    })
}

/// Active low-level wallet subscription.
#[derive(uniffi::Object)]
pub struct ActiveSubscription {
    inner: tokio::sync::Mutex<core::ActiveSubscription>,
    id: String,
}

impl fmt::Debug for ActiveSubscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActiveSubscription")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl ActiveSubscription {
    /// Mint-generated subscription identifier.
    pub fn id(&self) -> String {
        self.id.clone()
    }

    /// Wait for the next notification.
    pub async fn receive(&self) -> Result<WalletNotification, FfiError> {
        let event = self
            .inner
            .lock()
            .await
            .recv()
            .await
            .ok_or_else(|| FfiError::internal("Wallet subscription closed"))?;
        wallet_notification(event)
    }

    /// Return the next queued notification without waiting.
    pub async fn try_receive(&self) -> Result<Option<WalletNotification>, FfiError> {
        self.inner
            .lock()
            .await
            .try_recv()
            .map(wallet_notification)
            .transpose()
    }
}

/// Explicit expert capabilities for one wallet.
#[derive(uniffi::Object)]
pub struct AdvancedWallet {
    wallet: Arc<cdk::Wallet>,
}

impl AdvancedWallet {
    pub(crate) fn new(wallet: Arc<cdk::Wallet>) -> Self {
        Self { wallet }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl AdvancedWallet {
    /// Claim a mint session with explicit denomination or locking controls.
    pub async fn claim_mint(
        &self,
        session: Arc<MintSession>,
        options: MintClaimOptions,
    ) -> Result<MintReceipt, FfiError> {
        if session.core_session().wallet_identity() != self.wallet.identity() {
            return Err(FfiError::invalid_input(
                "Mint session belongs to a different wallet",
            ));
        }
        Ok(session
            .core_session()
            .claim_with(options.try_into()?)
            .await?
            .into())
    }

    /// Reserve an advanced ecash send for review before execution.
    pub async fn plan_send(
        &self,
        request: SendRequest,
        options: SendAdvancedOptions,
    ) -> Result<Arc<SendPlan>, FfiError> {
        let request: cdk::wallet::send::SendRequest = request.into();
        Ok(Arc::new(SendPlan::from_core(
            self.wallet
                .plan_send(request.with_advanced(options.try_into()?))
                .await?,
        )))
    }

    /// Prepare and execute an advanced ecash send.
    pub async fn send(
        &self,
        request: SendRequest,
        options: SendAdvancedOptions,
    ) -> Result<SendReceipt, FfiError> {
        let request: cdk::wallet::send::SendRequest = request.into();
        Ok(self
            .wallet
            .send(request.with_advanced(options.try_into()?))
            .await?
            .into())
    }

    /// Receive locked ecash with explicit credentials and denominations.
    pub async fn receive(
        &self,
        request: ReceiveRequest,
        options: ReceiveAdvancedOptions,
    ) -> Result<ReceiveReceipt, FfiError> {
        let request: cdk::wallet::receive::ReceiveRequest = request.into();
        Ok(self
            .wallet
            .receive(request.with_advanced(options.try_into()?))
            .await?
            .into())
    }

    /// Prepare a payment from an explicit wallet, proof, or token funding source.
    pub async fn prepare_payment(
        &self,
        session: Arc<PaymentSession>,
        options: PaymentPrepareOptions,
    ) -> Result<Arc<PaymentPlan>, FfiError> {
        if session.core_session().wallet_identity() != self.wallet.identity() {
            return Err(FfiError::invalid_input(
                "Payment session belongs to a different wallet",
            ));
        }
        Ok(Arc::new(PaymentPlan::from_core(
            session
                .core_session()
                .prepare_with(options.try_into()?)
                .await?,
        )))
    }

    /// Enter the explicit execution-policy surface for a payment plan.
    pub fn payment_plan(
        &self,
        plan: Arc<PaymentPlan>,
    ) -> Result<Arc<AdvancedPaymentPlan>, FfiError> {
        if plan.core_plan().wallet_identity() != self.wallet.identity() {
            return Err(FfiError::invalid_input(
                "Payment plan belongs to a different wallet",
            ));
        }
        Ok(Arc::new(AdvancedPaymentPlan {
            inner: plan.core_plan().clone(),
        }))
    }
}

impl fmt::Debug for AdvancedWallet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdvancedWallet")
            .field("wallet", &self.wallet.identity())
            .finish_non_exhaustive()
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl AdvancedWallet {
    /// Change how long mint metadata remains fresh; `None` never expires.
    pub fn set_metadata_cache_ttl(&self, seconds: Option<u64>) {
        self.wallet
            .advanced()
            .set_metadata_cache_ttl(seconds.map(Duration::from_secs));
    }

    /// Load one coherent mint-info and keyset snapshot.
    pub async fn mint_metadata(
        &self,
        source: MetadataSource,
    ) -> Result<MintMetadataSnapshot, FfiError> {
        Ok(self
            .wallet
            .advanced()
            .mint_metadata(core::MintMetadataRequest {
                source: source.into(),
            })
            .await?
            .into())
    }

    /// Inspect locally stored proof records.
    pub async fn proofs(&self, query: ProofQuery) -> Result<Vec<ProofInfo>, FfiError> {
        Ok(self
            .wallet
            .advanced()
            .proofs(query.try_into()?)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Check explicit proof states with the mint.
    pub async fn check_proofs(
        &self,
        proofs: Vec<Proof>,
    ) -> Result<Vec<ProofStateUpdate>, FfiError> {
        Ok(self
            .wallet
            .advanced()
            .check_proofs(core::ProofCheckRequest {
                proofs: convert_proofs(proofs)?,
            })
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Synchronize an explicit proof set with the mint.
    pub async fn synchronize_proof_states(&self, proofs: Vec<Proof>) -> Result<(), FfiError> {
        self.wallet
            .advanced()
            .synchronize_proof_states(convert_proofs(proofs)?)
            .await?;
        Ok(())
    }

    /// Release local proof reservations after verifying they are orphaned.
    pub async fn release_proofs(&self, ys: Vec<PublicKey>) -> Result<(), FfiError> {
        self.wallet
            .advanced()
            .release_proofs(core::ProofReleaseRequest {
                ys: ys
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<Vec<_>, _>>()?,
            })
            .await?;
        Ok(())
    }

    /// Reconcile orphaned pending proofs with the mint.
    pub async fn reconcile_proofs(&self) -> Result<Amount, FfiError> {
        Ok(self.wallet.advanced().reconcile_proofs().await?.into())
    }

    /// Reissue explicit proofs using protocol-level output controls.
    pub async fn reissue(&self, request: ReissueRequest) -> Result<ReissueReceipt, FfiError> {
        Ok(self
            .wallet
            .advanced()
            .reissue(request.try_into()?)
            .await?
            .into())
    }

    /// Redeem raw proofs with explicit credentials and metadata.
    pub async fn import_proofs(&self, request: ProofImportRequest) -> Result<Amount, FfiError> {
        Ok(self
            .wallet
            .advanced()
            .import_proofs(request.try_into()?)
            .await?
            .into())
    }

    /// Estimate an expert proof-input fee.
    pub async fn estimate_fee(&self, request: FeeEstimateRequest) -> Result<FeeEstimate, FfiError> {
        Ok(self
            .wallet
            .advanced()
            .estimate_fee(request.try_into()?)
            .await?
            .into())
    }

    /// Validate an encoded token without changing wallet state.
    pub async fn validate_token(&self, request: TokenValidationRequest) -> Result<(), FfiError> {
        let checks = request
            .checks
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()?;
        self.wallet
            .advanced()
            .validate_token(core::TokenValidationRequest {
                token: request.token.inner.clone(),
                checks,
            })
            .await?;
        Ok(())
    }

    /// Inspect one raw transaction and the proofs still stored for it.
    pub async fn transaction_details(
        &self,
        id: TransactionId,
    ) -> Result<Option<TransactionDetails>, FfiError> {
        Ok(self
            .wallet
            .advanced()
            .transaction_details(id.try_into()?)
            .await?
            .map(Into::into))
    }

    /// Reclaim a durable send or reconcile a legacy outgoing transaction.
    pub async fn recover_transaction(
        &self,
        id: TransactionId,
    ) -> Result<TransactionRecovery, FfiError> {
        Ok(self
            .wallet
            .advanced()
            .recover_transaction(id.try_into()?)
            .await?
            .into())
    }

    /// Subscribe to low-level proof or quote notifications.
    pub async fn subscribe(
        &self,
        request: SubscriptionRequest,
    ) -> Result<Arc<ActiveSubscription>, FfiError> {
        let subscription = self.wallet.advanced().subscribe(request.into()).await?;
        let id = subscription.name().clone();
        Ok(Arc::new(ActiveSubscription {
            inner: tokio::sync::Mutex::new(subscription),
            id,
        }))
    }

    /// Derive and persist the next P2PK signing key.
    pub async fn create_signing_key(&self) -> Result<PublicKey, FfiError> {
        Ok(self.wallet.advanced().create_signing_key().await?.into())
    }

    /// List persisted P2PK signing keys.
    pub async fn signing_keys(&self) -> Result<Vec<P2PKSigningKey>, FfiError> {
        Ok(self
            .wallet
            .advanced()
            .signing_keys()
            .await
            .map_err(|error| FfiError::internal(error.to_string()))?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Install a clear-auth token.
    pub async fn set_clear_auth_token(&self, token: String) -> Result<(), FfiError> {
        self.wallet.advanced().set_clear_auth_token(token).await?;
        Ok(())
    }

    /// Install an OIDC refresh token.
    pub async fn set_refresh_token(&self, token: String) -> Result<(), FfiError> {
        self.wallet.advanced().set_refresh_token(token).await?;
        Ok(())
    }

    /// Refresh the current access token.
    pub async fn refresh_access_token(&self) -> Result<(), FfiError> {
        self.wallet.advanced().refresh_access_token().await?;
        Ok(())
    }

    /// Mint blind-auth proofs from a protected mint.
    pub async fn mint_blind_auth(&self, amount: Amount) -> Result<Vec<Proof>, FfiError> {
        Ok(self
            .wallet
            .advanced()
            .mint_blind_auth(amount.into())
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Return locally stored unspent blind-auth proofs.
    pub async fn blind_auth_proofs(&self) -> Result<Vec<AuthProof>, FfiError> {
        Ok(self
            .wallet
            .advanced()
            .blind_auth_proofs()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Enable npub.cash and require newly created quotes to be wallet-locked.
    #[cfg(feature = "npubcash")]
    pub async fn enable_npubcash(&self, service_url: String) -> Result<(), FfiError> {
        self.wallet.advanced().enable_npubcash(service_url).await?;
        Ok(())
    }

    /// Reconcile npub.cash quotes that are missing from local wallet state.
    #[cfg(feature = "npubcash")]
    pub async fn reconcile_npubcash_quotes(&self) -> Result<Vec<MintQuote>, FfiError> {
        Ok(self
            .wallet
            .advanced()
            .reconcile_npubcash_quotes()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Claim every paid quote attributable to this wallet's npub.cash identity.
    #[cfg(feature = "npubcash")]
    pub async fn claim_npubcash_quotes(&self) -> Result<Amount, FfiError> {
        Ok(self.wallet.advanced().claim_npubcash_quotes().await?.into())
    }

    /// Fetch this wallet's current npub.cash account settings.
    #[cfg(feature = "npubcash")]
    pub async fn npubcash_user(&self) -> Result<NpubCashUserResponse, FfiError> {
        Ok(self.wallet.advanced().npubcash_user().await?.into())
    }
}

#[uniffi::export]
impl Wallet {
    /// Access protocol-level controls intentionally outside ordinary workflows.
    pub fn advanced_wallet(&self) -> Arc<AdvancedWallet> {
        Arc::new(AdvancedWallet::new(self.inner().clone()))
    }
}

/// Protocol details extracted from an encoded token.
#[derive(Clone, uniffi::Record)]
pub struct DecodedToken {
    /// Mint URL encoded in the token.
    pub mint_url: MintUrl,
    /// Raw token proofs.
    pub proofs: Vec<Proof>,
    /// Token memo, when present.
    pub memo: Option<String>,
    /// Token value.
    pub value: Amount,
    /// Token currency unit.
    pub unit: CurrencyUnit,
    /// Known redemption fee, or no value when the mint is unknown.
    pub redeem_fee: Option<Amount>,
}

impl fmt::Debug for DecodedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecodedToken")
            .field("mint_url", &self.mint_url)
            .field("proof_count", &self.proofs.len())
            .field("memo", &self.memo)
            .field("value", &self.value)
            .field("unit", &self.unit)
            .field("redeem_fee", &self.redeem_fee)
            .finish()
    }
}

impl From<core::DecodedToken> for DecodedToken {
    fn from(value: core::DecodedToken) -> Self {
        Self {
            mint_url: value.mint_url.into(),
            proofs: value.proofs.into_iter().map(Into::into).collect(),
            memo: value.memo,
            value: value.value.into(),
            unit: value.unit.into(),
            redeem_fee: value.redeem_fee.map(Into::into),
        }
    }
}

/// Proof records belonging to one managed wallet.
#[derive(Debug, Clone, uniffi::Record)]
pub struct WalletProofRecords {
    /// Wallet that owns these records.
    pub wallet: WalletIdentity,
    /// Stored unspent proofs.
    pub proofs: Vec<ProofInfo>,
}

/// Explicit expert capabilities for a multi-mint manager.
#[derive(uniffi::Object)]
pub struct AdvancedWalletManager {
    manager: Arc<cdk::wallet::WalletManager>,
}

impl AdvancedWalletManager {
    pub(crate) fn new(manager: Arc<cdk::wallet::WalletManager>) -> Self {
        Self { manager }
    }
}

impl fmt::Debug for AdvancedWalletManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdvancedWalletManager")
            .finish_non_exhaustive()
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl AdvancedWalletManager {
    /// Decode a token using keysets from its registered mint wallet.
    pub async fn inspect_token(&self, token: Arc<Token>) -> Result<DecodedToken, FfiError> {
        Ok(self
            .manager
            .advanced()
            .inspect_token(&token.inner)
            .await?
            .into())
    }

    /// Inspect stored unspent proofs across every configured wallet.
    pub async fn proof_records(&self) -> Result<Vec<WalletProofRecords>, FfiError> {
        Ok(self
            .manager
            .advanced()
            .proof_records()
            .await?
            .into_iter()
            .map(|(wallet, proofs)| WalletProofRecords {
                wallet: wallet.into(),
                proofs: proofs.into_iter().map(Into::into).collect(),
            })
            .collect())
    }

    /// Publish an encrypted NUT-27 mint backup to Nostr relays.
    pub async fn backup_mints(
        &self,
        request: MintBackupRequest,
    ) -> Result<MintBackupReceipt, FfiError> {
        Ok(self
            .manager
            .advanced()
            .backup_mints(request.into())
            .await?
            .into())
    }

    /// Decrypt a NUT-27 backup and optionally register its mints.
    pub async fn restore_mints(
        &self,
        request: MintRestoreRequest,
    ) -> Result<MintRestoreReceipt, FfiError> {
        Ok(self
            .manager
            .advanced()
            .restore_mints(request.into())
            .await?
            .into())
    }

    /// Return the manager-wide npub.cash mint, when configured.
    #[cfg(feature = "npubcash")]
    pub async fn active_npubcash_mint(&self) -> Result<Option<MintUrl>, FfiError> {
        Ok(self
            .manager
            .advanced()
            .active_npubcash_mint()
            .await?
            .map(Into::into))
    }

    /// Select the manager-wide npub.cash mint.
    #[cfg(feature = "npubcash")]
    pub async fn set_active_npubcash_mint(&self, mint_url: MintUrl) -> Result<(), FfiError> {
        self.manager
            .advanced()
            .set_active_npubcash_mint(mint_url.try_into()?)
            .await?;
        Ok(())
    }

    /// Synchronize npub.cash quotes from the active mint.
    #[cfg(feature = "npubcash")]
    pub async fn synchronize_npubcash_quotes(&self) -> Result<Vec<MintQuote>, FfiError> {
        Ok(self
            .manager
            .advanced()
            .synchronize_npubcash_quotes()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }
}

#[uniffi::export]
impl WalletManager {
    /// Access manager-wide protocol and integration operations.
    pub fn advanced_manager(&self) -> Arc<AdvancedWalletManager> {
        Arc::new(AdvancedWalletManager::new(self.inner()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::use_debug)]
    #[test]
    fn advanced_options_redact_unlocking_material() {
        let send = SendAdvancedOptions {
            conditions: None,
            amount_split_target: SplitTarget::None,
            max_proofs: None,
            use_p2bk: false,
            p2pk_signing_keys: vec!["sensitive-signing-key".to_string()],
            locked_proof_policy: LockedProofPolicy::Reissue,
        };
        let receive = ReceiveAdvancedOptions {
            amount_split_target: SplitTarget::None,
            p2pk_signing_keys: vec!["sensitive-receive-key".to_string()],
            preimages: vec!["sensitive-preimage".to_string()],
        };

        let send_output = format!("{send:?}");
        let receive_output = format!("{receive:?}");
        assert!(!send_output.contains("sensitive-signing-key"));
        assert!(!receive_output.contains("sensitive-receive-key"));
        assert!(!receive_output.contains("sensitive-preimage"));
        assert!(send_output.contains("[REDACTED]"));
        assert!(receive_output.contains("[REDACTED]"));
    }
}
