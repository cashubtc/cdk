//! Explicit expert operations for protocol-level wallet integrations.
//!
//! Ordinary applications should use the request, session, plan, and receipt
//! types in the domain modules under [`crate::wallet`]. This module is the intentional
//! escape hatch for callers that must inspect proofs or keysets, control proof
//! denominations, import externally funded quotes, or exercise batch protocol
//! extensions.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use cdk_common::wallet::{KeysetLoadPolicy, ProofInfo, Transaction, TransactionId};
use cdk_common::AuthProof;

pub use super::auth::{AuthMintConnector, AuthWallet};
pub use super::builder::WalletBuilder;
pub use super::manager::{DecodedToken, MintAdvancedOptions};
use super::mint::{MintQuoteId, MintSession, MintSessionState};
pub use super::mint_connector::http_client::{
    AuthHttpClient as BaseAuthHttpClient, HttpClient as BaseHttpClient,
};
pub use super::mint_connector::transport::Transport as HttpTransport;
pub use super::mint_connector::{
    AuthHttpClient, HttpClient, LnurlPayInvoiceResponse, LnurlPayResponse, MintConnector,
    RateLimitConfig, RateLimiterManager, TokenBucket,
};
#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
pub use super::mint_connector::{
    RateLimitedTorAuthHttpClient, RateLimitedTorHttpClient, TorAuthHttpClient, TorHttpClient,
};
pub use super::mint_metadata_cache::{FreshnessStatus, MintMetadataCache};
#[cfg(feature = "nostr")]
pub use super::nostr_backup::{
    MintBackupReceipt, MintBackupRequest, MintRestorePolicy, MintRestoreReceipt, MintRestoreRequest,
};
use super::payment::PaymentQuote;
#[cfg(all(feature = "npubcash", not(target_arch = "wasm32")))]
pub use super::streams::npubcash::WalletNpubCashProofStream;
#[cfg(not(target_arch = "wasm32"))]
pub use super::streams::payment::PaymentStream;
#[cfg(not(target_arch = "wasm32"))]
pub use super::streams::proof::{MultipleMintQuoteProofStream, SingleMintQuoteProofStream};
pub use super::subscription::ActiveSubscription;
use super::{MeltConfirmOptions, Wallet, WalletIdentity, WalletManager};
use crate::amount::{KeysetFeeAndAmounts, SplitTarget};
use crate::fees::ProofsFeeBreakdown;
use crate::mint_url::MintUrl;
use crate::nuts::nut00::token::Token;
use crate::nuts::{
    Id, KeySet, MintInfo, MintQuoteState, ProofState, Proofs, PublicKey, SecretKey,
    SpendingConditions, State,
};
use crate::{Amount, Error, OidcClient};

/// Protocol-specific controls for claiming an incoming mint quote.
#[derive(Debug, Clone, Default)]
pub struct MintClaimOptions {
    /// Explicit output denomination strategy.
    pub amount_split_target: SplitTarget,
    /// Spending conditions applied to the newly issued proofs.
    pub conditions: Option<SpendingConditions>,
}

/// How a send treats selected P2PK-locked proofs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LockedProofPolicy {
    /// Reissue locked proofs into fresh bearer proofs before sending.
    #[default]
    Reissue,
    /// Sign locked proofs and include them directly in the token.
    PassThrough,
}

impl From<LockedProofPolicy> for crate::wallet::P2PKLockedProofSendMode {
    fn from(value: LockedProofPolicy) -> Self {
        match value {
            LockedProofPolicy::Reissue => Self::Swap,
            LockedProofPolicy::PassThrough => Self::SignAndSend,
        }
    }
}

/// Protocol-specific controls for an ecash send.
#[derive(Clone, Default)]
pub struct SendAdvancedOptions {
    /// Spending conditions applied to newly created proofs.
    pub conditions: Option<SpendingConditions>,
    /// Explicit output denomination strategy.
    pub amount_split_target: SplitTarget,
    /// Maximum number of proofs encoded into the token.
    pub max_proofs: Option<usize>,
    /// Use NUT-28 P2BK output encryption.
    pub use_p2bk: bool,
    /// Signing keys for selected P2PK-locked input proofs.
    pub p2pk_signing_keys: Vec<SecretKey>,
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

/// Protocol-specific controls for receiving locked ecash.
#[derive(Clone, Default)]
pub struct ReceiveAdvancedOptions {
    /// Explicit output denomination strategy.
    pub amount_split_target: SplitTarget,
    /// P2PK signing keys used to unlock proofs.
    pub p2pk_signing_keys: Vec<SecretKey>,
    /// Preimages used to satisfy HTLC conditions.
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

/// Source of funds used to prepare an outgoing payment.
#[derive(Clone, Default)]
pub enum PaymentFunding {
    /// Select spendable proofs from this wallet.
    #[default]
    Wallet,
    /// Use an explicit proof set, including proofs not currently stored by the wallet.
    Proofs(Proofs),
    /// Decode and use an encoded Cashu token.
    Token(String),
}

impl fmt::Debug for PaymentFunding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wallet => f.write_str("Wallet"),
            Self::Proofs(proofs) => f
                .debug_tuple("Proofs")
                .field(&format_args!("{} proofs", proofs.len()))
                .finish(),
            Self::Token(_) => f.debug_tuple("Token").field(&"[REDACTED]").finish(),
        }
    }
}

/// Controls how an outgoing payment plan reserves its funds.
#[derive(Debug, Clone, Default)]
pub struct PaymentPrepareOptions {
    /// Funds to reserve for the payment.
    pub funding: PaymentFunding,
}

/// Whether confirmation may reissue proofs before paying the mint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PaymentSwapPolicy {
    /// Reissue proofs when that reduces fees or produces the required amount.
    #[default]
    Allow,
    /// Send the selected proofs directly to the melt endpoint.
    Skip,
}

/// Protocol-specific controls for confirming an outgoing payment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PaymentExecutionOptions {
    /// Whether confirmation may perform a pre-payment proof reissue.
    pub swap: PaymentSwapPolicy,
}

impl From<PaymentExecutionOptions> for MeltConfirmOptions {
    fn from(value: PaymentExecutionOptions) -> Self {
        match value.swap {
            PaymentSwapPolicy::Allow => Self::default(),
            PaymentSwapPolicy::Skip => Self::skip_swap(),
        }
    }
}

/// Expert proof-shaping options applied while opening a standalone wallet.
///
/// Ordinary applications should rely on the defaults. This type exists for
/// integrations that deliberately control proof denomination distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalletOpenAdvancedOptions {
    target_proof_count: usize,
}

impl Default for WalletOpenAdvancedOptions {
    fn default() -> Self {
        Self {
            target_proof_count: 3,
        }
    }
}

impl WalletOpenAdvancedOptions {
    /// Set the preferred number of proofs retained per denomination.
    pub fn with_target_proof_count(mut self, count: usize) -> Self {
        self.target_proof_count = count;
        self
    }

    pub(crate) const fn target_proof_count(self) -> usize {
        self.target_proof_count
    }
}

/// Explicit access to protocol-level operations for one wallet.
///
/// Obtain this handle with [`Wallet::advanced`]. Keeping expert operations on
/// this type makes the ordinary [`Wallet`] API focused on application
/// workflows while preserving the controls required by protocol integrations.
#[derive(Clone, Copy)]
pub struct AdvancedWallet<'a> {
    wallet: &'a Wallet,
}

impl fmt::Debug for AdvancedWallet<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdvancedWallet")
            .field("wallet", &self.wallet.identity())
            .finish_non_exhaustive()
    }
}

impl<'a> AdvancedWallet<'a> {
    pub(crate) const fn new(wallet: &'a Wallet) -> Self {
        Self { wallet }
    }

    #[cfg(any(not(target_arch = "wasm32"), feature = "npubcash", feature = "nwc"))]
    pub(crate) const fn core_wallet(&self) -> &Wallet {
        self.wallet
    }
}

/// Mutable expert configuration for one wallet instance.
///
/// Obtain this handle with [`Wallet::advanced_mut`]. Changes such as replacing
/// a connector affect this wallet value, not independently cloned values.
pub struct AdvancedWalletMut<'a> {
    wallet: &'a mut Wallet,
}

impl fmt::Debug for AdvancedWalletMut<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdvancedWalletMut")
            .field("wallet", &self.wallet.identity())
            .finish_non_exhaustive()
    }
}

impl<'a> AdvancedWalletMut<'a> {
    pub(crate) const fn new(wallet: &'a mut Wallet) -> Self {
        Self { wallet }
    }
}

/// Explicit access to manager-wide protocol and integration operations.
///
/// Obtain this handle with [`WalletManager::advanced`].
#[derive(Clone, Copy)]
pub struct AdvancedWalletManager<'a> {
    manager: &'a WalletManager,
}

impl fmt::Debug for AdvancedWalletManager<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdvancedWalletManager")
            .finish_non_exhaustive()
    }
}

impl<'a> AdvancedWalletManager<'a> {
    pub(crate) const fn new(manager: &'a WalletManager) -> Self {
        Self { manager }
    }

    #[cfg(feature = "nostr")]
    pub(crate) const fn core_manager(&self) -> &WalletManager {
        self.manager
    }
}

/// Low-level notification filter for protocol integrations.
#[derive(Debug, Clone)]
pub enum SubscriptionRequest {
    /// Proof-state changes for the provided proof Y values.
    ProofStates(Vec<String>),
    /// BOLT11 mint-quote changes.
    Bolt11MintQuotes(Vec<String>),
    /// BOLT11 melt-quote changes.
    Bolt11Payments(Vec<String>),
    /// BOLT12 mint-quote changes.
    Bolt12MintQuotes(Vec<String>),
    /// BOLT12 melt-quote changes.
    Bolt12Payments(Vec<String>),
    /// On-chain mint-quote changes.
    OnchainMintQuotes(Vec<String>),
    /// On-chain melt-quote changes.
    OnchainPayments(Vec<String>),
    /// Custom mint-quote method and identifiers.
    CustomMintQuotes {
        /// Mint-advertised payment method.
        method: String,
        /// Quote identifiers.
        quote_ids: Vec<String>,
    },
    /// Custom melt-quote method and identifiers.
    CustomPayments {
        /// Mint-advertised payment method.
        method: String,
        /// Quote identifiers.
        quote_ids: Vec<String>,
    },
}

impl From<SubscriptionRequest> for super::WalletSubscription {
    fn from(value: SubscriptionRequest) -> Self {
        match value {
            SubscriptionRequest::ProofStates(filters) => Self::ProofState(filters),
            SubscriptionRequest::Bolt11MintQuotes(filters) => Self::Bolt11MintQuoteState(filters),
            SubscriptionRequest::Bolt11Payments(filters) => Self::Bolt11MeltQuoteState(filters),
            SubscriptionRequest::Bolt12MintQuotes(filters) => Self::Bolt12MintQuoteState(filters),
            SubscriptionRequest::Bolt12Payments(filters) => Self::Bolt12MeltQuoteState(filters),
            SubscriptionRequest::OnchainMintQuotes(filters) => Self::MintQuoteOnchainState(filters),
            SubscriptionRequest::OnchainPayments(filters) => Self::MeltQuoteOnchainState(filters),
            SubscriptionRequest::CustomMintQuotes { method, quote_ids } => {
                Self::MintQuoteCustom(method, quote_ids)
            }
            SubscriptionRequest::CustomPayments { method, quote_ids } => {
                Self::MeltQuoteCustom(method, quote_ids)
            }
        }
    }
}

/// Where mint metadata may be loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MetadataSource {
    /// Use in-memory or persisted metadata and never contact the mint.
    CacheOnly,
    /// Prefer cached metadata and contact the mint when it is absent or stale.
    #[default]
    CacheOrNetwork,
    /// Contact the mint and replace the cached snapshot.
    Refresh,
}

impl From<MetadataSource> for KeysetLoadPolicy {
    fn from(value: MetadataSource) -> Self {
        match value {
            MetadataSource::CacheOnly => Self::CacheOnly,
            MetadataSource::CacheOrNetwork => Self::CacheThenNetwork,
            MetadataSource::Refresh => Self::Refresh,
        }
    }
}

/// Request for a consistent mint-info and keyset snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct MintMetadataRequest {
    /// Where the snapshot may be loaded from.
    pub source: MetadataSource,
}

/// Consistent protocol metadata for the wallet's mint and unit.
#[derive(Debug, Clone)]
pub struct MintMetadataSnapshot {
    /// Wallet whose mint supplied this metadata.
    pub wallet: WalletIdentity,
    /// Mint capabilities and contact information.
    pub info: MintInfo,
    /// Active and historical keysets for this wallet's unit.
    pub keysets: Vec<KeySet>,
}

impl MintMetadataSnapshot {
    /// Return the active keyset with the lowest advertised input fee.
    pub fn active_keyset(&self) -> Option<&KeySet> {
        self.keysets
            .iter()
            .filter(|keyset| keyset.active.unwrap_or(false))
            .min_by_key(|keyset| keyset.input_fee_ppk)
    }

    /// Find one keyset by its protocol identifier.
    pub fn keyset(&self, id: Id) -> Option<&KeySet> {
        self.keysets.iter().find(|keyset| keyset.id == id)
    }
}

/// Filter for inspecting the wallet's stored proofs.
#[derive(Debug, Clone)]
pub struct ProofQuery {
    /// Proof states to include. An empty vector includes every state.
    pub states: Vec<State>,
    /// Optional spending-condition filters.
    pub conditions: Option<Vec<SpendingConditions>>,
}

impl Default for ProofQuery {
    fn default() -> Self {
        Self {
            states: vec![State::Unspent],
            conditions: None,
        }
    }
}

impl ProofQuery {
    /// Inspect proofs in all locally stored states.
    pub fn all() -> Self {
        Self {
            states: Vec::new(),
            conditions: None,
        }
    }
}

/// Request a NUT-07 state check for an explicit proof set.
#[derive(Clone)]
pub struct ProofCheckRequest {
    /// Proofs whose mint state should be checked.
    pub proofs: Proofs,
}

impl fmt::Debug for ProofCheckRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProofCheckRequest")
            .field("proof_count", &self.proofs.len())
            .finish()
    }
}

/// Emergency request to return selected local proof reservations to `Unspent`.
///
/// This bypasses durable operation ownership. Callers must first establish
/// that no active operation owns the proofs.
#[derive(Debug, Clone)]
pub struct ProofReleaseRequest {
    /// Proof Y values whose local state should be released.
    pub ys: Vec<PublicKey>,
}

/// Whether proof selection should reserve enough value to cover input fees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ProofSelectionFeePolicy {
    /// Select the requested face value without adding redemption fees.
    #[default]
    Exclude,
    /// Select enough value for the requested amount plus its input fees.
    Include,
}

/// Expert input for deterministic proof-set selection.
#[derive(Clone)]
pub struct ProofSelectionRequest {
    /// Face value to select.
    pub amount: Amount,
    /// Candidate proofs.
    pub proofs: Proofs,
    /// Keysets currently active at the mint.
    pub active_keyset_ids: Vec<Id>,
    /// Known input fees and denominations by keyset.
    pub keyset_fees: KeysetFeeAndAmounts,
    /// Whether the selected value must also cover its input fees.
    pub fee_policy: ProofSelectionFeePolicy,
}

impl fmt::Debug for ProofSelectionRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProofSelectionRequest")
            .field("amount", &self.amount)
            .field("proof_count", &self.proofs.len())
            .field("active_keyset_ids", &self.active_keyset_ids)
            .field("keyset_fees", &self.keyset_fees)
            .field("fee_policy", &self.fee_policy)
            .finish()
    }
}

/// Select a proof set using the wallet's deterministic input-selection policy.
pub fn select_proofs(request: ProofSelectionRequest) -> Result<Proofs, Error> {
    Wallet::select_proofs(
        request.amount,
        request.proofs,
        &request.active_keyset_ids,
        &request.keyset_fees,
        request.fee_policy == ProofSelectionFeePolicy::Include,
    )
}

/// Whether a reissue should add the future redemption fee to its selected output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ReissueFeePolicy {
    /// Return the requested face value without adding its future redemption fee.
    #[default]
    Deduct,
    /// Add the future redemption fee so the selected output redeems to the requested value.
    AddToOutput,
}

/// Output encryption applied during proof reissue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ReissueProtection {
    /// Create ordinary Cashu proofs.
    #[default]
    Plain,
    /// Create NUT-28 P2BK-encrypted proofs.
    P2bk,
}

/// Expert request to reissue an explicit set of proofs.
#[derive(Clone)]
pub struct ReissueRequest {
    /// Input proofs consumed by the swap.
    pub proofs: Proofs,
    /// Selected output amount, or `None` to reissue the entire input value into the wallet.
    pub amount: Option<Amount>,
    /// Explicit output denomination strategy.
    pub amount_split_target: SplitTarget,
    /// Spending conditions applied to the selected outputs.
    pub conditions: Option<SpendingConditions>,
    /// Treatment of the selected outputs' future redemption fee.
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

/// Proofs selected by an expert reissue operation.
#[derive(Clone)]
pub struct ReissueReceipt {
    /// Selected outputs. `None` means the entire reissued value stayed in the wallet.
    pub proofs: Option<Proofs>,
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

/// Expert request to import raw proofs rather than an encoded token.
#[derive(Clone)]
pub struct ProofImportRequest {
    /// Proofs to redeem.
    pub proofs: Proofs,
    /// Explicit output denomination strategy.
    pub amount_split_target: SplitTarget,
    /// P2PK signing keys used to unlock proofs.
    pub p2pk_signing_keys: Vec<SecretKey>,
    /// HTLC preimages used to unlock proofs.
    pub preimages: Vec<String>,
    /// Optional transaction memo.
    pub memo: Option<String>,
    /// Optional original encoded token retained with transaction history.
    pub encoded_token: Option<String>,
    /// Application metadata stored with the transaction.
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

/// Request to attach an existing mint quote to this wallet.
#[derive(Debug, Clone)]
pub struct MintQuoteImportRequest {
    /// Mint-provided quote identifier.
    pub id: MintQuoteId,
    /// Payment method required when the quote is not yet known locally.
    pub method: Option<crate::nuts::PaymentMethod>,
}

/// Request to refresh several locally known mint quotes with NUT-29.
#[derive(Debug, Clone)]
pub struct MintBatchRefreshRequest {
    /// Quotes, all using the same payment method.
    pub quote_ids: Vec<MintQuoteId>,
}

/// Request to claim several mint quotes with one NUT-29 operation.
#[derive(Clone)]
pub struct MintBatchClaimRequest {
    /// Unique quote identifiers sharing one method and unit.
    pub quote_ids: Vec<MintQuoteId>,
    /// Explicit output denomination strategy.
    pub amount_split_target: SplitTarget,
    /// Spending conditions applied to issued proofs.
    pub conditions: Option<SpendingConditions>,
    /// Signing keys for externally created quotes not present in wallet storage.
    pub external_keys: HashMap<MintQuoteId, SecretKey>,
}

impl fmt::Debug for MintBatchClaimRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MintBatchClaimRequest")
            .field("quote_ids", &self.quote_ids)
            .field("amount_split_target", &self.amount_split_target)
            .field("conditions", &self.conditions)
            .field("external_keys", &"[REDACTED]")
            .finish()
    }
}

/// Receipt for a successful batch mint operation.
#[derive(Clone)]
pub struct MintBatchReceipt {
    /// Quotes claimed by the operation.
    pub quote_ids: Vec<MintQuoteId>,
    /// Total issued value.
    pub amount: Amount,
    /// Raw issued proofs for expert integrations.
    pub proofs: Proofs,
}

impl fmt::Debug for MintBatchReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MintBatchReceipt")
            .field("quote_ids", &self.quote_ids)
            .field("amount", &self.amount)
            .field("proof_count", &self.proofs.len())
            .finish()
    }
}

/// Local mint-session selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MintSessionFilter {
    /// Quotes that have not been fully issued, including expired quotes.
    #[default]
    Unissued,
    /// Unexpired quotes that have not been fully issued.
    Active,
    /// Every locally stored quote belonging to this wallet.
    All,
}

/// Local outgoing-payment quote selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PaymentQuoteFilter {
    /// Pending payments only.
    #[default]
    Pending,
    /// Pending payments and unexpired unpaid quotes.
    Active,
    /// Every locally stored quote belonging to this wallet.
    All,
}

/// Expert fee-estimation input.
#[derive(Clone)]
pub enum FeeEstimateRequest {
    /// Fee charged when an explicit proof set is redeemed.
    Proofs(Proofs),
    /// Fee charged for a number of inputs from one keyset.
    KeysetInputs {
        /// Keyset identifier.
        keyset_id: Id,
        /// Number of inputs.
        count: u64,
    },
}

impl fmt::Debug for FeeEstimateRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Proofs(proofs) => f
                .debug_tuple("Proofs")
                .field(&format_args!("{} proofs", proofs.len()))
                .finish(),
            Self::KeysetInputs { keyset_id, count } => f
                .debug_struct("KeysetInputs")
                .field("keyset_id", keyset_id)
                .field("count", count)
                .finish(),
        }
    }
}

/// Checks applied to an encoded token without redeeming it.
#[derive(Debug, Clone)]
pub enum TokenCheck {
    /// Verify every proof's DLEQ evidence against its mint keyset.
    Dleq,
    /// Require every proof to meet these offline spending conditions.
    ValidateSpendingConditions(SpendingConditions),
}

/// Request to validate a token without changing wallet state.
#[derive(Clone)]
pub struct TokenValidationRequest {
    /// Token to validate.
    pub token: Token,
    /// Checks to apply in order.
    pub checks: Vec<TokenCheck>,
}

/// Raw transaction and its proofs for protocol-level inspection.
#[derive(Clone)]
pub struct TransactionDetails {
    /// Stored transaction, including protocol identifiers and metadata.
    pub transaction: Transaction,
    /// Proofs still present locally for the transaction's Y values.
    pub proofs: Proofs,
}

impl fmt::Debug for TransactionDetails {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransactionDetails")
            .field("transaction", &self.transaction)
            .field("proof_count", &self.proofs.len())
            .finish()
    }
}

/// Result of reconciling an outgoing transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionRecovery {
    /// A durable send was still unclaimed and its value was restored.
    SendReclaimed {
        /// Value returned to the wallet.
        amount: Amount,
    },
    /// A legacy transaction's pending proofs were checked without reissuing.
    LegacyReconciled,
}

impl fmt::Debug for TokenValidationRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenValidationRequest")
            .field("token", &"[REDACTED]")
            .field("checks", &self.checks)
            .finish()
    }
}

impl Wallet {
    /// Access protocol-level controls that are intentionally outside the
    /// ordinary application workflow API.
    pub const fn advanced(&self) -> AdvancedWallet<'_> {
        AdvancedWallet::new(self)
    }

    /// Access mutable expert configuration for this wallet value.
    pub fn advanced_mut(&mut self) -> AdvancedWalletMut<'_> {
        AdvancedWalletMut::new(self)
    }
}

impl AdvancedWalletMut<'_> {
    /// Move persisted wallet state to a replacement URL advertised by NUT-06.
    ///
    /// The caller must verify the replacement URL against authenticated mint
    /// metadata before relocating it.
    pub async fn relocate_mint(&mut self, new_url: MintUrl) -> Result<WalletIdentity, Error> {
        self.wallet.update_mint_url(new_url).await?;
        Ok(self.wallet.identity())
    }

    /// Replace the mint connector used by this wallet value.
    pub fn replace_connector(&mut self, connector: Arc<dyn MintConnector + Send + Sync>) {
        self.wallet.set_client(connector);
    }

    /// Change the preferred number of proofs retained per denomination.
    pub fn set_target_proof_count(&mut self, count: usize) {
        self.wallet.set_target_proof_count(count);
    }
}

impl AdvancedWallet<'_> {
    /// Return the configured mint connector.
    pub fn connector(&self) -> Arc<dyn MintConnector + Send + Sync> {
        self.wallet.mint_connector()
    }

    /// Subscribe to low-level proof or quote notifications.
    pub async fn subscribe(
        &self,
        request: SubscriptionRequest,
    ) -> Result<ActiveSubscription, Error> {
        self.wallet
            .subscribe(super::WalletSubscription::from(request))
            .await
    }

    /// Create an OIDC client using this wallet's configured transport.
    pub fn authentication_client(
        &self,
        openid_discovery: String,
        client_id: Option<String>,
    ) -> OidcClient {
        self.wallet.oidc_client(openid_discovery, client_id)
    }

    /// Change how long mint metadata remains fresh.
    ///
    /// `None` keeps cached metadata until an explicit refresh.
    pub fn set_metadata_cache_ttl(&self, ttl: Option<Duration>) {
        self.wallet.set_metadata_cache_ttl(ttl);
    }

    /// Inspect the current metadata cache freshness.
    pub fn metadata_cache_status(&self) -> FreshnessStatus {
        self.wallet.get_metadata_cache_info()
    }

    /// Mint blind-auth proofs from a protected mint.
    pub async fn mint_blind_auth(&self, amount: Amount) -> Result<Proofs, Error> {
        self.wallet.mint_blind_auth(amount).await
    }

    /// Return locally stored unspent blind-auth proofs.
    pub async fn blind_auth_proofs(&self) -> Result<Vec<AuthProof>, Error> {
        self.wallet.get_unspent_auth_proofs().await
    }

    /// Install a clear-auth token.
    pub async fn set_clear_auth_token(&self, token: String) -> Result<(), Error> {
        self.wallet.set_cat(token).await
    }

    /// Install an OIDC refresh token.
    pub async fn set_refresh_token(&self, token: String) -> Result<(), Error> {
        self.wallet.set_refresh_token(token).await
    }

    /// Refresh the wallet's current access token.
    pub async fn refresh_access_token(&self) -> Result<(), Error> {
        self.wallet.refresh_access_token().await
    }

    /// Replace the protected-mint authentication client.
    pub async fn set_authentication_wallet(&self, auth_wallet: Option<AuthWallet>) {
        self.wallet.set_auth_client(auth_wallet).await;
    }

    /// Derive and persist the next P2PK signing key.
    pub async fn create_signing_key(&self) -> Result<PublicKey, Error> {
        self.wallet.generate_public_key().await
    }

    /// Find one persisted P2PK signing key.
    pub async fn signing_key(
        &self,
        public_key: &PublicKey,
    ) -> Result<Option<cdk_common::wallet::P2PKSigningKey>, cdk_common::database::Error> {
        self.wallet.get_public_key(public_key).await
    }

    /// List persisted P2PK signing keys.
    pub async fn signing_keys(
        &self,
    ) -> Result<Vec<cdk_common::wallet::P2PKSigningKey>, cdk_common::database::Error> {
        self.wallet.get_public_keys().await
    }

    /// Return the most recently derived P2PK signing key.
    pub async fn latest_signing_key(
        &self,
    ) -> Result<Option<cdk_common::wallet::P2PKSigningKey>, cdk_common::database::Error> {
        self.wallet.get_latest_public_key().await
    }

    /// Load one coherent mint-info and keyset snapshot.
    pub async fn mint_metadata(
        &self,
        request: MintMetadataRequest,
    ) -> Result<MintMetadataSnapshot, Error> {
        let info = match request.source {
            MetadataSource::CacheOnly => self
                .wallet
                .metadata_cache
                .load_cached(&self.wallet.localstore)
                .await?
                .mint_info
                .clone(),
            MetadataSource::CacheOrNetwork => self.wallet.load_mint_info().await?,
            MetadataSource::Refresh => {
                self.wallet
                    .fetch_mint_info()
                    .await?
                    .ok_or_else(|| Error::UnknownMint {
                        mint_url: self.wallet.mint_url.to_string(),
                    })?
            }
        };
        let keysets = self.wallet.keysets(request.source.into()).await?;

        Ok(MintMetadataSnapshot {
            wallet: self.wallet.identity(),
            info,
            keysets,
        })
    }

    /// Inspect locally stored proof records using an explicit expert filter.
    pub async fn proofs(&self, query: ProofQuery) -> Result<Vec<ProofInfo>, Error> {
        self.wallet
            .localstore
            .get_proofs(
                Some(self.wallet.mint_url.clone()),
                Some(self.wallet.unit.clone()),
                (!query.states.is_empty()).then_some(query.states),
                query.conditions,
            )
            .await
            .map_err(Into::into)
    }

    /// Check explicit proof states with the mint and persist newly spent states.
    pub async fn check_proofs(&self, request: ProofCheckRequest) -> Result<Vec<ProofState>, Error> {
        self.wallet.check_proofs_spent(request.proofs).await
    }

    /// Synchronize the full local state of an explicit proof set with its mint.
    pub async fn synchronize_proof_states(&self, proofs: Proofs) -> Result<(), Error> {
        self.wallet.sync_proofs_state(proofs).await
    }

    /// Release explicit local reservations after the caller verifies they are orphaned.
    pub async fn release_proofs(&self, request: ProofReleaseRequest) -> Result<(), Error> {
        self.wallet.unreserve_proofs(request.ys).await
    }

    /// Reconcile orphaned pending proofs with the mint and return value restored
    /// to the available balance.
    pub async fn reconcile_proofs(&self) -> Result<Amount, Error> {
        self.wallet.check_all_pending_proofs().await
    }

    /// Reissue explicit proofs using protocol-level output controls.
    pub async fn reissue(&self, request: ReissueRequest) -> Result<ReissueReceipt, Error> {
        let proofs = self
            .wallet
            .swap(
                request.amount,
                request.amount_split_target,
                request.proofs,
                request.conditions,
                request.fee_policy == ReissueFeePolicy::AddToOutput,
                request.protection == ReissueProtection::P2bk,
            )
            .await?;
        Ok(ReissueReceipt { proofs })
    }

    /// Redeem raw proofs with explicit credentials and transaction metadata.
    pub async fn import_proofs(&self, request: ProofImportRequest) -> Result<Amount, Error> {
        self.wallet
            .receive_proofs(
                request.proofs,
                super::ReceiveOptions {
                    amount_split_target: request.amount_split_target,
                    p2pk_signing_keys: request.p2pk_signing_keys,
                    preimages: request.preimages,
                    metadata: request.metadata,
                },
                request.memo,
                request.encoded_token,
            )
            .await
    }

    /// Attach an existing mint quote and return its workflow session.
    pub async fn import_mint_quote(
        &self,
        request: MintQuoteImportRequest,
    ) -> Result<MintSession, Error> {
        let quote = self
            .wallet
            .fetch_mint_quote(request.id.as_str(), request.method)
            .await?;
        Ok(MintSession::from_quote(self.wallet.clone(), quote))
    }

    /// Refresh several mint sessions with one NUT-29 request.
    pub async fn refresh_mint_batch(
        &self,
        request: MintBatchRefreshRequest,
    ) -> Result<Vec<MintSessionState>, Error> {
        let ids = request
            .quote_ids
            .iter()
            .map(MintQuoteId::as_str)
            .collect::<Vec<_>>();
        Ok(self
            .wallet
            .batch_check_mint_quote_status(&ids)
            .await?
            .into_iter()
            .map(|quote| {
                MintSession::from_quote(self.wallet.clone(), quote)
                    .initial_state()
                    .clone()
            })
            .collect())
    }

    /// Claim several mint sessions with one durable NUT-29 operation.
    pub async fn claim_mint_batch(
        &self,
        request: MintBatchClaimRequest,
    ) -> Result<MintBatchReceipt, Error> {
        let quote_ids = request.quote_ids;
        let ids = quote_ids
            .iter()
            .map(MintQuoteId::as_str)
            .collect::<Vec<_>>();
        let external_keys = (!request.external_keys.is_empty()).then(|| {
            request
                .external_keys
                .into_iter()
                .map(|(id, key)| (id.to_string(), key))
                .collect()
        });
        let proofs = self
            .wallet
            .batch_mint(
                &ids,
                request.amount_split_target,
                request.conditions,
                external_keys,
            )
            .await?;
        let amount = Amount::try_sum(proofs.iter().map(|proof| proof.amount))?;

        Ok(MintBatchReceipt {
            quote_ids,
            amount,
            proofs,
        })
    }

    /// List locally stored incoming-payment sessions.
    pub async fn mint_sessions(
        &self,
        filter: MintSessionFilter,
    ) -> Result<Vec<MintSession>, Error> {
        let now = crate::util::unix_time();
        Ok(self
            .wallet
            .localstore
            .get_mint_quotes()
            .await?
            .into_iter()
            .filter(|quote| {
                quote.mint_url == self.wallet.mint_url && quote.unit == self.wallet.unit
            })
            .filter(|quote| match filter {
                MintSessionFilter::Unissued => quote.state != MintQuoteState::Issued,
                MintSessionFilter::Active => {
                    quote.state != MintQuoteState::Issued && quote.expiry > now
                }
                MintSessionFilter::All => true,
            })
            .map(|quote| MintSession::from_quote(self.wallet.clone(), quote))
            .collect())
    }

    /// List locally stored outgoing-payment quote previews.
    pub async fn payment_quotes(
        &self,
        filter: PaymentQuoteFilter,
    ) -> Result<Vec<PaymentQuote>, Error> {
        let now = crate::util::unix_time();
        Ok(self
            .wallet
            .localstore
            .get_melt_quotes()
            .await?
            .into_iter()
            .filter(|quote| {
                quote.mint_url.as_ref() == Some(&self.wallet.mint_url)
                    && quote.unit == self.wallet.unit
            })
            .filter(|quote| match filter {
                PaymentQuoteFilter::Pending => quote.state == crate::nuts::MeltQuoteState::Pending,
                PaymentQuoteFilter::Active => {
                    quote.state == crate::nuts::MeltQuoteState::Pending
                        || (quote.state == crate::nuts::MeltQuoteState::Unpaid
                            && quote.expiry > now)
                }
                PaymentQuoteFilter::All => true,
            })
            .map(|quote| super::payment::payment_quote(&quote))
            .collect())
    }

    /// Estimate an expert proof-input fee.
    pub async fn estimate_fee(
        &self,
        request: FeeEstimateRequest,
    ) -> Result<ProofsFeeBreakdown, Error> {
        match request {
            FeeEstimateRequest::Proofs(proofs) => self.wallet.get_proofs_fee(&proofs).await,
            FeeEstimateRequest::KeysetInputs { keyset_id, count } => {
                let total = self.wallet.get_keyset_count_fee(&keyset_id, count).await?;
                Ok(ProofsFeeBreakdown {
                    total,
                    per_keyset: HashMap::from([(keyset_id, total)]),
                })
            }
        }
    }

    /// Validate an encoded token without redeeming or changing wallet state.
    pub async fn validate_token(&self, request: TokenValidationRequest) -> Result<(), Error> {
        for check in request.checks {
            match check {
                TokenCheck::Dleq => self.wallet.verify_token_dleq(&request.token).await?,
                TokenCheck::ValidateSpendingConditions(conditions) => {
                    self.wallet
                        .verify_token_p2pk(&request.token, conditions)
                        .await?
                }
            }
        }
        Ok(())
    }

    /// Inspect one raw transaction together with proofs still stored for it.
    pub async fn transaction_details(
        &self,
        id: TransactionId,
    ) -> Result<Option<TransactionDetails>, Error> {
        let Some(transaction) = self.wallet.get_transaction(id).await? else {
            return Ok(None);
        };
        let proofs = self.wallet.get_proofs_for_transaction(id).await?;
        Ok(Some(TransactionDetails {
            transaction,
            proofs,
        }))
    }

    /// Reclaim a saga-backed send or reconcile a legacy outgoing transaction.
    pub async fn recover_transaction(
        &self,
        id: TransactionId,
    ) -> Result<TransactionRecovery, Error> {
        match self.wallet.recover_outgoing_transaction(id).await? {
            Some(amount) => Ok(TransactionRecovery::SendReclaimed { amount }),
            None => Ok(TransactionRecovery::LegacyReconciled),
        }
    }
}

impl WalletManager {
    /// Access manager-wide protocol and integration operations.
    pub const fn advanced(&self) -> AdvancedWalletManager<'_> {
        AdvancedWalletManager::new(self)
    }
}

impl AdvancedWalletManager<'_> {
    /// Create an OIDC client using a configured mint transport when available.
    pub async fn authentication_client(
        &self,
        mint_url: &MintUrl,
        openid_discovery: String,
        client_id: Option<String>,
    ) -> OidcClient {
        self.manager
            .oidc_client_for_mint(mint_url, openid_discovery, client_id)
            .await
    }

    /// Decode a token using keysets from its registered mint wallet.
    pub async fn inspect_token(&self, token: &Token) -> Result<DecodedToken, Error> {
        self.manager.get_token_data(token).await
    }

    /// Inspect stored unspent proofs across every configured wallet.
    pub async fn proof_records(
        &self,
    ) -> Result<std::collections::BTreeMap<WalletIdentity, Vec<ProofInfo>>, Error> {
        let mut records = std::collections::BTreeMap::new();
        for wallet in self.manager.wallets().await {
            records.insert(
                wallet.identity(),
                wallet.advanced().proofs(ProofQuery::default()).await?,
            );
        }
        Ok(records)
    }

    /// Return the mint selected for manager-wide npub.cash operations.
    #[cfg(feature = "npubcash")]
    pub async fn active_npubcash_mint(&self) -> Result<Option<MintUrl>, Error> {
        self.manager.get_active_npubcash_mint().await
    }

    /// Select the mint used for manager-wide npub.cash operations.
    #[cfg(feature = "npubcash")]
    pub async fn set_active_npubcash_mint(&self, mint_url: MintUrl) -> Result<(), Error> {
        self.manager.set_active_npubcash_mint(mint_url).await
    }

    /// Synchronize quotes from the manager's active npub.cash mint.
    #[cfg(feature = "npubcash")]
    pub async fn synchronize_npubcash_quotes(
        &self,
    ) -> Result<Vec<cdk_common::wallet::MintQuote>, Error> {
        self.manager.synchronize_npubcash_quotes().await
    }
}
