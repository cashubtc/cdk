//! Wallet Types

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use async_trait::async_trait;
use bitcoin::bip32::DerivationPath;
use bitcoin::hashes::{sha256, Hash, HashEngine};
use cashu::amount::SplitTarget;
use cashu::nuts::nut07::ProofState;
use cashu::nuts::nut18::PaymentRequest;
use cashu::nuts::AuthProof;
use cashu::util::hex;
use cashu::{nut00, PaymentMethod, Proof, Proofs, PublicKey};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::mint_url::MintUrl;
use crate::nuts::{
    CurrencyUnit, Id, MeltQuoteState, MintQuoteState, SecretKey, SpendingConditions, State,
};
use crate::{Amount, Error};

pub mod saga;

pub use saga::{
    IssueSagaState, MeltOperationData, MeltSagaState, MintOperationData, OperationData,
    ReceiveOperationData, ReceiveSagaState, SendOperationData, SendSagaState, SwapOperationData,
    SwapSagaState, WalletSaga, WalletSagaState,
};

/// Wallet Key
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WalletKey {
    /// Mint Url
    pub mint_url: MintUrl,
    /// Currency Unit
    pub unit: CurrencyUnit,
}

impl fmt::Display for WalletKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mint_url: {}, unit: {}", self.mint_url, self.unit,)
    }
}

impl WalletKey {
    /// Create new [`WalletKey`]
    pub fn new(mint_url: MintUrl, unit: CurrencyUnit) -> Self {
        Self { mint_url, unit }
    }
}

/// Proof info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofInfo {
    /// Proof
    pub proof: Proof,
    /// y
    pub y: PublicKey,
    /// Mint Url
    pub mint_url: MintUrl,
    /// Proof State
    pub state: State,
    /// Proof Spending Conditions
    pub spending_condition: Option<SpendingConditions>,
    /// Unit
    pub unit: CurrencyUnit,
    /// Operation ID that is using/spending this proof
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_by_operation: Option<Uuid>,
    /// Operation ID that created this proof
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by_operation: Option<Uuid>,
}

impl ProofInfo {
    /// Create new [`ProofInfo`]
    pub fn new(
        proof: Proof,
        mint_url: MintUrl,
        state: State,
        unit: CurrencyUnit,
    ) -> Result<Self, Error> {
        let y = proof.y()?;

        let spending_condition: Option<SpendingConditions> = (&proof.secret).try_into().ok();

        Ok(Self {
            proof,
            y,
            mint_url,
            state,
            spending_condition,
            unit,
            used_by_operation: None,
            created_by_operation: None,
        })
    }

    /// Create new [`ProofInfo`] with operation tracking
    pub fn new_with_operations(
        proof: Proof,
        mint_url: MintUrl,
        state: State,
        unit: CurrencyUnit,
        used_by_operation: Option<Uuid>,
        created_by_operation: Option<Uuid>,
    ) -> Result<Self, Error> {
        let y = proof.y()?;

        let spending_condition: Option<SpendingConditions> = (&proof.secret).try_into().ok();

        Ok(Self {
            proof,
            y,
            mint_url,
            state,
            spending_condition,
            unit,
            used_by_operation,
            created_by_operation,
        })
    }

    /// Check if [`Proof`] matches conditions
    pub fn matches_conditions(
        &self,
        mint_url: &Option<MintUrl>,
        unit: &Option<CurrencyUnit>,
        state: &Option<Vec<State>>,
        spending_conditions: &Option<Vec<SpendingConditions>>,
    ) -> bool {
        if let Some(mint_url) = mint_url {
            if mint_url.ne(&self.mint_url) {
                return false;
            }
        }

        if let Some(unit) = unit {
            if unit.ne(&self.unit) {
                return false;
            }
        }

        if let Some(state) = state {
            if !state.contains(&self.state) {
                return false;
            }
        }

        if let Some(spending_conditions) = spending_conditions {
            match &self.spending_condition {
                None => {
                    if !spending_conditions.is_empty() {
                        return false;
                    }
                }
                Some(s) => {
                    if !spending_conditions.contains(s) {
                        return false;
                    }
                }
            }
        }

        true
    }
}

/// Mint Quote Info
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuote {
    /// Quote id
    pub id: String,
    /// Mint Url
    pub mint_url: MintUrl,
    /// Payment method
    pub payment_method: PaymentMethod,
    /// Amount of quote
    pub amount: Option<Amount>,
    /// Unit of quote
    pub unit: CurrencyUnit,
    /// Quote payment request e.g. bolt11
    pub request: String,
    /// Quote state
    pub state: MintQuoteState,
    /// Expiration time of quote
    pub expiry: u64,
    /// Secretkey for signing mint quotes [NUT-20]
    pub secret_key: Option<SecretKey>,
    /// Amount minted
    #[serde(default)]
    pub amount_issued: Amount,
    /// Amount paid to the mint for the quote
    #[serde(default)]
    pub amount_paid: Amount,
    /// Operation ID that has reserved this quote (for saga pattern)
    #[serde(default)]
    pub used_by_operation: Option<String>,
    /// Version for optimistic locking
    #[serde(default)]
    pub version: u32,
}

/// Melt Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltQuote {
    /// Quote id
    pub id: String,
    /// Mint Url
    pub mint_url: Option<MintUrl>,
    /// Quote unit
    pub unit: CurrencyUnit,
    /// Quote amount
    pub amount: Amount,
    /// Quote Payment request e.g. bolt11
    pub request: String,
    /// Quote fee reserve
    pub fee_reserve: Amount,
    /// Quote state
    pub state: MeltQuoteState,
    /// Expiration time of quote
    pub expiry: u64,
    /// Payment preimage
    pub payment_preimage: Option<String>,
    /// Payment method
    pub payment_method: PaymentMethod,
    /// Operation ID that has reserved this quote (for saga pattern)
    #[serde(default)]
    pub used_by_operation: Option<String>,
    /// Version for optimistic locking
    #[serde(default)]
    pub version: u32,
}

impl MintQuote {
    /// Create a new MintQuote
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        mint_url: MintUrl,
        payment_method: PaymentMethod,
        amount: Option<Amount>,
        unit: CurrencyUnit,
        request: String,
        expiry: u64,
        secret_key: Option<SecretKey>,
    ) -> Self {
        Self {
            id,
            mint_url,
            payment_method,
            amount,
            unit,
            request,
            state: MintQuoteState::Unpaid,
            expiry,
            secret_key,
            amount_issued: Amount::ZERO,
            amount_paid: Amount::ZERO,
            used_by_operation: None,
            version: 0,
        }
    }

    /// Calculate the total amount including any fees
    pub fn total_amount(&self) -> Amount {
        self.amount_paid
    }

    /// Check if the quote has expired
    pub fn is_expired(&self, current_time: u64) -> bool {
        current_time > self.expiry
    }

    /// Amount that can be minted
    pub fn amount_mintable(&self) -> Amount {
        if self.payment_method == PaymentMethod::BOLT11 {
            // BOLT11 is all-or-nothing: mint full amount when state is Paid
            if self.state == MintQuoteState::Paid {
                self.amount.unwrap_or(Amount::ZERO)
            } else {
                Amount::ZERO
            }
        } else {
            // Other payment methods track incremental payments
            self.amount_paid
                .checked_sub(self.amount_issued)
                .unwrap_or(Amount::ZERO)
        }
    }
}

/// Amounts recovered during a restore operation
#[derive(Debug, Clone, Hash, PartialEq, Eq, Default)]
pub struct Restored {
    /// Amount in the restore that has already been spent
    pub spent: Amount,
    /// Amount restored that is unspent
    pub unspent: Amount,
    /// Amount restored that is pending
    pub pending: Amount,
}

/// Send options
#[derive(Debug, Clone, Default)]
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
    /// Maximum number of proofs to include in the token
    pub max_proofs: Option<usize>,
    /// Metadata
    pub metadata: HashMap<String, String>,
    /// Use P2BK (NUT-28)
    pub use_p2bk: bool,
}

/// Send memo
#[derive(Debug, Clone)]
pub struct SendMemo {
    /// Memo
    pub memo: String,
    /// Include memo in token
    pub include_memo: bool,
}

impl SendMemo {
    /// Create a new send memo
    pub fn for_token(memo: &str) -> Self {
        Self {
            memo: memo.to_string(),
            include_memo: true,
        }
    }
}

/// Receive options
#[derive(Debug, Clone, Default)]
pub struct ReceiveOptions {
    /// Amount split target
    pub amount_split_target: SplitTarget,
    /// P2PK signing keys
    pub p2pk_signing_keys: Vec<SecretKey>,
    /// Preimages
    pub preimages: Vec<String>,
    /// Metadata
    pub metadata: HashMap<String, String>,
}

/// Send Kind
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SendKind {
    #[default]
    /// Allow online swap before send if wallet does not have exact amount
    OnlineExact,
    /// Prefer offline send if difference is less then tolerance
    OnlineTolerance(Amount),
    /// Wallet cannot do an online swap and selected proof must be exactly send amount
    OfflineExact,
    /// Wallet must remain offline but can over pay if below tolerance
    OfflineTolerance(Amount),
}

impl SendKind {
    /// Check if send kind is online
    pub fn is_online(&self) -> bool {
        matches!(self, Self::OnlineExact | Self::OnlineTolerance(_))
    }

    /// Check if send kind is offline
    pub fn is_offline(&self) -> bool {
        matches!(self, Self::OfflineExact | Self::OfflineTolerance(_))
    }

    /// Check if send kind is exact
    pub fn is_exact(&self) -> bool {
        matches!(self, Self::OnlineExact | Self::OfflineExact)
    }

    /// Check if send kind has tolerance
    pub fn has_tolerance(&self) -> bool {
        matches!(self, Self::OnlineTolerance(_) | Self::OfflineTolerance(_))
    }
}

/// Wallet Transaction
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Transaction {
    /// Mint Url
    pub mint_url: MintUrl,
    /// Transaction direction
    pub direction: TransactionDirection,
    /// Amount
    pub amount: Amount,
    /// Fee
    pub fee: Amount,
    /// Currency Unit
    pub unit: CurrencyUnit,
    /// Proof Ys
    pub ys: Vec<PublicKey>,
    /// Unix timestamp
    pub timestamp: u64,
    /// Memo
    pub memo: Option<String>,
    /// User-defined metadata
    pub metadata: HashMap<String, String>,
    /// Quote ID if this is a mint or melt transaction
    pub quote_id: Option<String>,
    /// Payment request (e.g., BOLT11 invoice, BOLT12 offer)
    pub payment_request: Option<String>,
    /// Payment proof (e.g., preimage for Lightning melt transactions)
    pub payment_proof: Option<String>,
    /// Payment method (e.g., Bolt11, Bolt12) for mint/melt transactions
    #[serde(default)]
    pub payment_method: Option<PaymentMethod>,
    /// Saga ID if this transaction was part of a saga
    #[serde(default)]
    pub saga_id: Option<Uuid>,
}

impl Transaction {
    /// Transaction ID
    pub fn id(&self) -> TransactionId {
        TransactionId::new(self.ys.clone())
    }

    /// Check if transaction matches conditions
    pub fn matches_conditions(
        &self,
        mint_url: &Option<MintUrl>,
        direction: &Option<TransactionDirection>,
        unit: &Option<CurrencyUnit>,
    ) -> bool {
        if let Some(mint_url) = mint_url {
            if &self.mint_url != mint_url {
                return false;
            }
        }
        if let Some(direction) = direction {
            if &self.direction != direction {
                return false;
            }
        }
        if let Some(unit) = unit {
            if &self.unit != unit {
                return false;
            }
        }
        true
    }
}

impl PartialOrd for Transaction {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Transaction {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.timestamp
            .cmp(&other.timestamp)
            .reverse()
            .then_with(|| self.id().cmp(&other.id()))
    }
}

/// Transaction Direction
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionDirection {
    /// Incoming transaction (i.e., receive or mint)
    Incoming,
    /// Outgoing transaction (i.e., send or melt)
    Outgoing,
}

impl std::fmt::Display for TransactionDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransactionDirection::Incoming => write!(f, "Incoming"),
            TransactionDirection::Outgoing => write!(f, "Outgoing"),
        }
    }
}

impl FromStr for TransactionDirection {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "Incoming" => Ok(Self::Incoming),
            "Outgoing" => Ok(Self::Outgoing),
            _ => Err(Error::InvalidTransactionDirection),
        }
    }
}

/// Transaction ID
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransactionId([u8; 32]);

impl TransactionId {
    /// Create new [`TransactionId`]
    pub fn new(ys: Vec<PublicKey>) -> Self {
        let mut ys = ys;
        ys.sort();
        let mut hasher = sha256::Hash::engine();
        for y in ys {
            hasher.input(&y.to_bytes());
        }
        let hash = sha256::Hash::from_engine(hasher);
        Self(hash.to_byte_array())
    }

    /// From proofs
    pub fn from_proofs(proofs: Proofs) -> Result<Self, nut00::Error> {
        let ys = proofs
            .iter()
            .map(|proof| proof.y())
            .collect::<Result<Vec<PublicKey>, nut00::Error>>()?;
        Ok(Self::new(ys))
    }

    /// From bytes
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// From hex string
    pub fn from_hex(value: &str) -> Result<Self, Error> {
        let bytes = hex::decode(value)?;
        if bytes.len() != 32 {
            return Err(Error::InvalidTransactionId);
        }
        let mut array = [0u8; 32];
        array.copy_from_slice(&bytes);
        Ok(Self(array))
    }

    /// From slice
    pub fn from_slice(slice: &[u8]) -> Result<Self, Error> {
        if slice.len() != 32 {
            return Err(Error::InvalidTransactionId);
        }
        let mut array = [0u8; 32];
        array.copy_from_slice(slice);
        Ok(Self(array))
    }

    /// Get inner value
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Get inner value as slice
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Display for TransactionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

impl FromStr for TransactionId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_hex(value)
    }
}

impl TryFrom<Proofs> for TransactionId {
    type Error = nut00::Error;

    fn try_from(proofs: Proofs) -> Result<Self, Self::Error> {
        Self::from_proofs(proofs)
    }
}

/// Wallet operation kind
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    /// Send operation
    Send,
    /// Receive operation
    Receive,
    /// Swap operation
    Swap,
    /// Mint operation
    Mint,
    /// Melt operation
    Melt,
}

impl fmt::Display for OperationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OperationKind::Send => write!(f, "send"),
            OperationKind::Receive => write!(f, "receive"),
            OperationKind::Swap => write!(f, "swap"),
            OperationKind::Mint => write!(f, "mint"),
            OperationKind::Melt => write!(f, "melt"),
        }
    }
}

impl FromStr for OperationKind {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "send" => Ok(OperationKind::Send),
            "receive" => Ok(OperationKind::Receive),
            "swap" => Ok(OperationKind::Swap),
            "mint" => Ok(OperationKind::Mint),
            "melt" => Ok(OperationKind::Melt),
            _ => Err(Error::InvalidOperationKind),
        }
    }
}

/// Unified wallet trait providing a common interface for wallet operations.
///
/// This trait abstracts over different wallet implementations (CDK wallet, FFI
/// wrappers, etc.) and provides a consistent interface for balance queries,
/// minting, melting, keyset management, and other core wallet operations.
///
/// All domain types are associated types so each implementation can use its own
/// type system (e.g. FFI-friendly records vs native Rust types).
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait Wallet: Send + Sync {
    /// Error type
    type Error: std::error::Error + Send + Sync + 'static;
    /// Amount type (e.g. `cdk_common::Amount` or FFI `Amount`)
    type Amount: Clone + Send + Sync;
    /// Mint URL type
    type MintUrl: Clone + Send + Sync;
    /// Currency unit type
    type CurrencyUnit: Clone + Send + Sync;
    /// Mint info type
    type MintInfo: Clone + Send + Sync;
    /// Keyset info type
    type KeySetInfo: Clone + Send + Sync;
    /// Mint quote type
    type MintQuote: Clone + Send + Sync;
    /// Melt quote type
    type MeltQuote: Clone + Send + Sync;
    /// Payment method type
    type PaymentMethod: Clone + Send + Sync;
    /// Melt options type
    type MeltOptions: Clone + Send + Sync;
    /// Operation ID type (CDK uses `Uuid`, FFI uses `String`)
    type OperationId: Clone + Send + Sync;
    /// Prepared send type
    type PreparedSend<'a>: Send + Sync
    where
        Self: 'a;
    /// Prepared melt type
    type PreparedMelt<'a>: Send + Sync
    where
        Self: 'a;
    /// Active subscription handle for receiving notifications
    type Subscription: Send + Sync;
    /// Subscribe params type
    type SubscribeParams: Clone + Send + Sync;

    /// Get the mint URL this wallet is connected to
    fn mint_url(&self) -> Self::MintUrl;

    /// Get the currency unit of this wallet
    fn unit(&self) -> Self::CurrencyUnit;

    /// Total unspent balance of the wallet
    async fn total_balance(&self) -> Result<Self::Amount, Self::Error>;

    /// Total pending balance of the wallet
    async fn total_pending_balance(&self) -> Result<Self::Amount, Self::Error>;

    /// Total reserved balance of the wallet
    async fn total_reserved_balance(&self) -> Result<Self::Amount, Self::Error>;

    /// Fetch mint info from the mint (always makes a network call)
    async fn fetch_mint_info(&self) -> Result<Option<Self::MintInfo>, Self::Error>;

    /// Load mint info (from cache if fresh, otherwise fetches)
    async fn load_mint_info(&self) -> Result<Self::MintInfo, Self::Error>;

    /// Refresh keysets from the mint (always fetches fresh data)
    async fn refresh_keysets(&self) -> Result<Vec<Self::KeySetInfo>, Self::Error>;

    /// Get the active keyset with lowest fees
    async fn get_active_keyset(&self) -> Result<Self::KeySetInfo, Self::Error>;

    /// Create a mint quote for the given payment method
    async fn mint_quote(
        &self,
        method: Self::PaymentMethod,
        amount: Option<Self::Amount>,
        description: Option<String>,
        extra: Option<String>,
    ) -> Result<Self::MintQuote, Self::Error>;

    /// Create a melt quote for the given payment method
    async fn melt_quote(
        &self,
        method: Self::PaymentMethod,
        request: String,
        options: Option<Self::MeltOptions>,
        extra: Option<String>,
    ) -> Result<Self::MeltQuote, Self::Error>;

    /// List transactions, optionally filtered by direction
    async fn list_transactions(
        &self,
        direction: Option<TransactionDirection>,
    ) -> Result<Vec<Transaction>, Self::Error>;

    /// Get a transaction by ID
    async fn get_transaction(&self, id: TransactionId) -> Result<Option<Transaction>, Self::Error>;

    /// Get proofs for a transaction by transaction ID
    async fn get_proofs_for_transaction(&self, id: TransactionId) -> Result<Proofs, Self::Error>;

    /// Revert a transaction by reclaiming unspent proofs
    async fn revert_transaction(&self, id: TransactionId) -> Result<(), Self::Error>;

    /// Check all pending proofs and return total amount still pending
    async fn check_all_pending_proofs(&self) -> Result<Self::Amount, Self::Error>;

    /// Check if proofs are spent
    async fn check_proofs_spent(&self, proofs: Proofs) -> Result<Vec<ProofState>, Self::Error>;

    /// Get fees for a specific keyset ID
    async fn get_keyset_fees_by_id(&self, keyset_id: Id) -> Result<u64, Self::Error>;

    /// Calculate fee for a given number of proofs with the specified keyset
    async fn calculate_fee(
        &self,
        proof_count: u64,
        keyset_id: Id,
    ) -> Result<Self::Amount, Self::Error>;

    /// Receive an encoded token
    async fn receive(
        &self,
        encoded_token: &str,
        options: ReceiveOptions,
    ) -> Result<Self::Amount, Self::Error>;

    /// Receive proofs directly
    async fn receive_proofs(
        &self,
        proofs: Proofs,
        options: ReceiveOptions,
        memo: Option<String>,
        token: Option<String>,
    ) -> Result<Self::Amount, Self::Error>;

    /// Prepare a send transaction
    async fn prepare_send(
        &self,
        amount: Self::Amount,
        options: SendOptions,
    ) -> Result<Self::PreparedSend<'_>, Self::Error>;

    /// Get pending send operation IDs
    async fn get_pending_sends(&self) -> Result<Vec<Self::OperationId>, Self::Error>;

    /// Revoke a pending send operation
    async fn revoke_send(
        &self,
        operation_id: Self::OperationId,
    ) -> Result<Self::Amount, Self::Error>;

    /// Check if a pending send has been claimed
    async fn check_send_status(&self, operation_id: Self::OperationId)
        -> Result<bool, Self::Error>;

    /// Mint tokens for a quote
    async fn mint(
        &self,
        quote_id: &str,
        split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, Self::Error>;

    /// Check mint quote status
    async fn check_mint_quote_status(&self, quote_id: &str)
        -> Result<Self::MintQuote, Self::Error>;

    /// Fetch a mint quote from the mint and store it locally
    async fn fetch_mint_quote(
        &self,
        quote_id: &str,
        payment_method: Option<Self::PaymentMethod>,
    ) -> Result<Self::MintQuote, Self::Error>;

    /// Prepare a melt operation
    async fn prepare_melt(
        &self,
        quote_id: &str,
        metadata: HashMap<String, String>,
    ) -> Result<Self::PreparedMelt<'_>, Self::Error>;

    /// Prepare a melt operation with specific proofs
    async fn prepare_melt_proofs(
        &self,
        quote_id: &str,
        proofs: Proofs,
        metadata: HashMap<String, String>,
    ) -> Result<Self::PreparedMelt<'_>, Self::Error>;

    /// Swap proofs
    async fn swap(
        &self,
        amount: Option<Self::Amount>,
        split_target: SplitTarget,
        input_proofs: Proofs,
        spending_conditions: Option<SpendingConditions>,
        include_fees: bool,
        use_p2bk: bool,
    ) -> Result<Option<Proofs>, Self::Error>;

    /// Set Clear Auth Token (CAT)
    async fn set_cat(&self, cat: String) -> Result<(), Self::Error>;

    /// Set refresh token
    async fn set_refresh_token(&self, refresh_token: String) -> Result<(), Self::Error>;

    /// Refresh access token using stored refresh token
    async fn refresh_access_token(&self) -> Result<(), Self::Error>;

    /// Mint blind auth tokens
    async fn mint_blind_auth(&self, amount: Self::Amount) -> Result<Proofs, Self::Error>;

    /// Get unspent auth proofs
    async fn get_unspent_auth_proofs(&self) -> Result<Vec<AuthProof>, Self::Error>;

    /// Restore wallet from seed
    async fn restore(&self) -> Result<Restored, Self::Error>;

    /// Verify DLEQ proofs in a token
    async fn verify_token_dleq(&self, token_str: &str) -> Result<(), Self::Error>;

    /// Pay a NUT-18 payment request
    async fn pay_request(
        &self,
        request: PaymentRequest,
        custom_amount: Option<Self::Amount>,
    ) -> Result<(), Self::Error>;

    /// Subscribe to mint quote state updates
    ///
    /// Returns a subscription handle that receives notifications when
    /// any of the given mint quotes change state (e.g., Unpaid → Paid → Issued).
    async fn subscribe_mint_quote_state(
        &self,
        quote_ids: Vec<String>,
        method: Self::PaymentMethod,
    ) -> Result<Self::Subscription, Self::Error>;

    /// Set metadata cache TTL (time-to-live) in seconds
    ///
    /// Controls how long cached mint metadata (keysets, keys, mint info) is considered fresh
    /// before requiring a refresh from the mint server.
    /// If `None`, cache never expires and is always used.
    fn set_metadata_cache_ttl(&self, ttl_secs: Option<u64>);

    /// Subscribe to wallet events
    async fn subscribe(
        &self,
        params: Self::SubscribeParams,
    ) -> Result<Self::Subscription, Self::Error>;

    /// Get a melt quote for a BIP353 address
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn melt_bip353_quote(
        &self,
        bip353_address: &str,
        amount_msat: Self::Amount,
        network: bitcoin::Network,
    ) -> Result<Self::MeltQuote, Self::Error>;

    /// Get a melt quote for a Lightning address
    #[cfg(not(target_arch = "wasm32"))]
    async fn melt_lightning_address_quote(
        &self,
        lightning_address: &str,
        amount_msat: Self::Amount,
    ) -> Result<Self::MeltQuote, Self::Error>;

    /// Get a melt quote for a human-readable address
    ///
    /// Accepts a human-readable address that could be either a BIP353 address
    /// or a Lightning address. Tries BIP353 first if mint supports Bolt12,
    /// falls back to Lightning address.
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn melt_human_readable_quote(
        &self,
        address: &str,
        amount_msat: Self::Amount,
        network: bitcoin::Network,
    ) -> Result<Self::MeltQuote, Self::Error>;

    /// Get a melt quote for a human-readable address (alias for `melt_human_readable_quote`)
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn melt_human_readable(
        &self,
        address: &str,
        amount_msat: Self::Amount,
        network: bitcoin::Network,
    ) -> Result<Self::MeltQuote, Self::Error> {
        self.melt_human_readable_quote(address, amount_msat, network)
            .await
    }

    /// Check a mint quote status (alias for `check_mint_quote_status`)
    async fn check_mint_quote(&self, quote_id: &str) -> Result<Self::MintQuote, Self::Error> {
        self.check_mint_quote_status(quote_id).await
    }

    /// Mint tokens for a quote (alias for `mint`)
    async fn mint_unified(
        &self,
        quote_id: &str,
        split_target: SplitTarget,
        spending_conditions: Option<SpendingConditions>,
    ) -> Result<Proofs, Self::Error> {
        self.mint(quote_id, split_target, spending_conditions).await
    }

    /// Get proofs filtered by states
    ///
    /// Returns all proofs whose state matches any of the given states.
    /// The `Spent` state is typically excluded since spent proofs are removed
    /// from the database.
    async fn get_proofs_by_states(&self, states: Vec<State>) -> Result<Proofs, Self::Error>;

    // P2PK proofs
    /// generates and stores public key in database
    async fn generate_public_key(&self) -> Result<PublicKey, Self::Error>;

    /// gets public key by it's hex value
    async fn get_public_key(
        &self,
        pubkey: &PublicKey,
    ) -> Result<Option<P2PKSigningKey>, Self::Error>;

    /// gets list of stored public keys in database
    async fn get_public_keys(&self) -> Result<Vec<P2PKSigningKey>, Self::Error>;

    /// Gets the latest generated P2PK signing key (most recently created)
    async fn get_latest_public_key(&self) -> Result<Option<P2PKSigningKey>, Self::Error>;

    /// try to get secret key from p2pk signing key in localstore
    async fn get_signing_key(&self, pubkey: &PublicKey) -> Result<Option<SecretKey>, Self::Error>;
}

/// Public key generated for proof signing
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct P2PKSigningKey {
    /// Public key
    pub pubkey: PublicKey,
    /// Derivation path
    pub derivation_path: DerivationPath,
    /// Derivation index
    pub derivation_index: u32,
    /// Created time
    pub created_time: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nuts::Id;
    use crate::secret::Secret;

    #[test]
    fn test_transaction_id_from_hex() {
        let hex_str = "a1b2c3d4e5f60718293a0b1c2d3e4f506172839a0b1c2d3e4f506172839a0b1c";
        let transaction_id = TransactionId::from_hex(hex_str).unwrap();
        assert_eq!(transaction_id.to_string(), hex_str);
    }

    #[test]
    fn test_transaction_id_from_hex_empty_string() {
        let hex_str = "";
        let res = TransactionId::from_hex(hex_str);
        assert!(matches!(res, Err(Error::InvalidTransactionId)));
    }

    #[test]
    fn test_transaction_id_from_hex_longer_string() {
        let hex_str = "a1b2c3d4e5f60718293a0b1c2d3e4f506172839a0b1c2d3e4f506172839a0b1ca1b2";
        let res = TransactionId::from_hex(hex_str);
        assert!(matches!(res, Err(Error::InvalidTransactionId)));
    }

    #[test]
    fn test_matches_conditions() {
        let keyset_id = Id::from_str("00deadbeef123456").unwrap();
        let proof = Proof::new(
            Amount::from(64),
            keyset_id,
            Secret::new("test_secret"),
            PublicKey::from_hex(
                "02deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            )
            .unwrap(),
        );

        let mint_url = MintUrl::from_str("https://example.com").unwrap();
        let proof_info =
            ProofInfo::new(proof, mint_url.clone(), State::Unspent, CurrencyUnit::Sat).unwrap();

        // Test matching mint_url
        assert!(proof_info.matches_conditions(&Some(mint_url.clone()), &None, &None, &None));
        assert!(!proof_info.matches_conditions(
            &Some(MintUrl::from_str("https://different.com").unwrap()),
            &None,
            &None,
            &None
        ));

        // Test matching unit
        assert!(proof_info.matches_conditions(&None, &Some(CurrencyUnit::Sat), &None, &None));
        assert!(!proof_info.matches_conditions(&None, &Some(CurrencyUnit::Msat), &None, &None));

        // Test matching state
        assert!(proof_info.matches_conditions(&None, &None, &Some(vec![State::Unspent]), &None));
        assert!(proof_info.matches_conditions(
            &None,
            &None,
            &Some(vec![State::Unspent, State::Spent]),
            &None
        ));
        assert!(!proof_info.matches_conditions(&None, &None, &Some(vec![State::Spent]), &None));

        // Test with no conditions (should match)
        assert!(proof_info.matches_conditions(&None, &None, &None, &None));

        // Test with multiple conditions
        assert!(proof_info.matches_conditions(
            &Some(mint_url),
            &Some(CurrencyUnit::Sat),
            &Some(vec![State::Unspent]),
            &None
        ));
    }

    #[test]
    fn test_matches_conditions_with_spending_conditions() {
        // This test would need to be expanded with actual SpendingConditions
        // implementation, but we can test the basic case where no spending
        // conditions are present

        let keyset_id = Id::from_str("00deadbeef123456").unwrap();
        let proof = Proof::new(
            Amount::from(64),
            keyset_id,
            Secret::new("test_secret"),
            PublicKey::from_hex(
                "02deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            )
            .unwrap(),
        );

        let mint_url = MintUrl::from_str("https://example.com").unwrap();
        let proof_info =
            ProofInfo::new(proof, mint_url, State::Unspent, CurrencyUnit::Sat).unwrap();

        // Test with empty spending conditions (should match when proof has none)
        assert!(proof_info.matches_conditions(&None, &None, &None, &Some(vec![])));

        // Test with non-empty spending conditions (should not match when proof has none)
        let dummy_condition = SpendingConditions::P2PKConditions {
            data: SecretKey::generate().public_key(),
            conditions: None,
        };
        assert!(!proof_info.matches_conditions(&None, &None, &None, &Some(vec![dummy_condition])));
    }
}
