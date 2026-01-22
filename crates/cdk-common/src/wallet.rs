//! Wallet Types

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use bitcoin::hashes::{sha256, Hash, HashEngine};
use cashu::util::hex;
use cashu::{nut00, BlindedMessage, PaymentMethod, Proofs, PublicKey};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::mint_url::MintUrl;
use crate::nuts::{CurrencyUnit, MeltQuoteState, MintQuoteState, SecretKey};
use crate::{Amount, Error};

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
        let difference = self.amount_paid.saturating_sub(self.amount_issued);

        if difference != Amount::ZERO {
            return difference;
        }

        // When fully caught up, BOLT11 quotes can still mint their full amount
        let is_unminted_bolt11 =
            self.state != MintQuoteState::Issued && self.payment_method == PaymentMethod::BOLT11;

        if is_unminted_bolt11 {
            self.amount.unwrap_or(Amount::ZERO)
        } else {
            Amount::ZERO
        }
    }
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

/// States specific to send saga
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendSagaState {
    /// Proofs selected and reserved
    ProofsReserved,
    /// Token created, proofs marked pending spent
    TokenCreated,
    /// Send is being rolled back (transient state)
    RollingBack,
}

impl fmt::Display for SendSagaState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SendSagaState::ProofsReserved => write!(f, "proofs_reserved"),
            SendSagaState::TokenCreated => write!(f, "token_created"),
            SendSagaState::RollingBack => write!(f, "rolling_back"),
        }
    }
}

impl FromStr for SendSagaState {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "proofs_reserved" => Ok(SendSagaState::ProofsReserved),
            "token_created" => Ok(SendSagaState::TokenCreated),
            "rolling_back" => Ok(SendSagaState::RollingBack),
            _ => Err(Error::InvalidOperationState),
        }
    }
}

/// States specific to receive saga
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiveSagaState {
    /// Input proofs validated and stored as pending
    ProofsPending,
    /// Swap request sent to mint
    SwapRequested,
}

impl fmt::Display for ReceiveSagaState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReceiveSagaState::ProofsPending => write!(f, "proofs_pending"),
            ReceiveSagaState::SwapRequested => write!(f, "swap_requested"),
        }
    }
}

impl FromStr for ReceiveSagaState {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "proofs_pending" => Ok(ReceiveSagaState::ProofsPending),
            "swap_requested" => Ok(ReceiveSagaState::SwapRequested),
            _ => Err(Error::InvalidOperationState),
        }
    }
}

/// States specific to swap saga (wallet-side)
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwapSagaState {
    /// Input proofs reserved
    ProofsReserved,
    /// Swap request sent to mint
    SwapRequested,
}

impl fmt::Display for SwapSagaState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SwapSagaState::ProofsReserved => write!(f, "proofs_reserved"),
            SwapSagaState::SwapRequested => write!(f, "swap_requested"),
        }
    }
}

impl FromStr for SwapSagaState {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "proofs_reserved" => Ok(SwapSagaState::ProofsReserved),
            "swap_requested" => Ok(SwapSagaState::SwapRequested),
            _ => Err(Error::InvalidOperationState),
        }
    }
}

/// States specific to mint (issue) saga
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueSagaState {
    /// Pre-mint secrets created, counter incremented
    SecretsPrepared,
    /// Mint request sent
    MintRequested,
}

impl fmt::Display for IssueSagaState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IssueSagaState::SecretsPrepared => write!(f, "secrets_prepared"),
            IssueSagaState::MintRequested => write!(f, "mint_requested"),
        }
    }
}

impl FromStr for IssueSagaState {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "secrets_prepared" => Ok(IssueSagaState::SecretsPrepared),
            "mint_requested" => Ok(IssueSagaState::MintRequested),
            _ => Err(Error::InvalidOperationState),
        }
    }
}

/// States specific to melt saga
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeltSagaState {
    /// Proofs reserved, quote verified
    ProofsReserved,
    /// Melt request sent to mint
    MeltRequested,
    /// Payment pending (waiting for LN confirmation)
    PaymentPending,
}

impl fmt::Display for MeltSagaState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MeltSagaState::ProofsReserved => write!(f, "proofs_reserved"),
            MeltSagaState::MeltRequested => write!(f, "melt_requested"),
            MeltSagaState::PaymentPending => write!(f, "payment_pending"),
        }
    }
}

impl FromStr for MeltSagaState {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "proofs_reserved" => Ok(MeltSagaState::ProofsReserved),
            "melt_requested" => Ok(MeltSagaState::MeltRequested),
            "payment_pending" => Ok(MeltSagaState::PaymentPending),
            _ => Err(Error::InvalidOperationState),
        }
    }
}

/// Wallet saga state for different operation types
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "state", rename_all = "snake_case")]
pub enum WalletSagaState {
    /// Send saga states
    Send(SendSagaState),
    /// Receive saga states
    Receive(ReceiveSagaState),
    /// Swap saga states
    Swap(SwapSagaState),
    /// Mint (issue) saga states
    Issue(IssueSagaState),
    /// Melt saga states
    Melt(MeltSagaState),
}

impl WalletSagaState {
    /// Get the operation kind
    pub fn kind(&self) -> OperationKind {
        match self {
            WalletSagaState::Send(_) => OperationKind::Send,
            WalletSagaState::Receive(_) => OperationKind::Receive,
            WalletSagaState::Swap(_) => OperationKind::Swap,
            WalletSagaState::Issue(_) => OperationKind::Mint,
            WalletSagaState::Melt(_) => OperationKind::Melt,
        }
    }

    /// Get string representation of the inner state
    pub fn state_str(&self) -> &'static str {
        match self {
            WalletSagaState::Send(s) => match s {
                SendSagaState::ProofsReserved => "proofs_reserved",
                SendSagaState::TokenCreated => "token_created",
                SendSagaState::RollingBack => "rolling_back",
            },
            WalletSagaState::Receive(s) => match s {
                ReceiveSagaState::ProofsPending => "proofs_pending",
                ReceiveSagaState::SwapRequested => "swap_requested",
            },
            WalletSagaState::Swap(s) => match s {
                SwapSagaState::ProofsReserved => "proofs_reserved",
                SwapSagaState::SwapRequested => "swap_requested",
            },
            WalletSagaState::Issue(s) => match s {
                IssueSagaState::SecretsPrepared => "secrets_prepared",
                IssueSagaState::MintRequested => "mint_requested",
            },
            WalletSagaState::Melt(s) => match s {
                MeltSagaState::ProofsReserved => "proofs_reserved",
                MeltSagaState::MeltRequested => "melt_requested",
                MeltSagaState::PaymentPending => "payment_pending",
            },
        }
    }
}

/// Operation-specific data for Send operations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendOperationData {
    /// Target amount to send
    pub amount: Amount,
    /// Memo for the send
    pub memo: Option<String>,
    /// Derivation counter start
    pub counter_start: Option<u32>,
    /// Derivation counter end
    pub counter_end: Option<u32>,
    /// Token data (when in Pending/Finalized state)
    pub token: Option<String>,
    /// Proofs being sent
    pub proofs: Option<Proofs>,
}

/// Operation-specific data for Receive operations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiveOperationData {
    /// Token to receive
    pub token: String,
    /// Derivation counter start
    pub counter_start: Option<u32>,
    /// Derivation counter end
    pub counter_end: Option<u32>,
    /// Amount received
    pub amount: Option<Amount>,
    /// Blinded messages for recovery
    ///
    /// Stored so that if a crash occurs after the mint accepts the swap,
    /// we can use these to query the mint for signatures and reconstruct proofs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blinded_messages: Option<Vec<BlindedMessage>>,
}

/// Operation-specific data for Swap operations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapOperationData {
    /// Input amount
    pub input_amount: Amount,
    /// Output amount
    pub output_amount: Amount,
    /// Derivation counter start
    pub counter_start: Option<u32>,
    /// Derivation counter end
    pub counter_end: Option<u32>,
    /// Blinded messages for recovery
    ///
    /// Stored so that if a crash occurs after the mint accepts the swap,
    /// we can use these to query the mint for signatures and reconstruct proofs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blinded_messages: Option<Vec<BlindedMessage>>,
}

/// Operation-specific data for Mint operations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintOperationData {
    /// Quote ID
    pub quote_id: String,
    /// Amount to mint
    pub amount: Amount,
    /// Derivation counter start
    pub counter_start: Option<u32>,
    /// Derivation counter end
    pub counter_end: Option<u32>,
    /// Blinded messages for recovery
    ///
    /// Stored so that if a crash occurs after the mint accepts the request,
    /// we can use these to query the mint for signatures and reconstruct proofs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blinded_messages: Option<Vec<BlindedMessage>>,
}

/// Operation-specific data for Melt operations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltOperationData {
    /// Quote ID
    pub quote_id: String,
    /// Amount to melt
    pub amount: Amount,
    /// Fee reserve
    pub fee_reserve: Amount,
    /// Derivation counter start
    pub counter_start: Option<u32>,
    /// Derivation counter end
    pub counter_end: Option<u32>,
    /// Change amount (if any)
    pub change_amount: Option<Amount>,
    /// Blinded messages for change recovery
    ///
    /// Stored so that if a crash occurs after the mint accepts the melt,
    /// we can use these to query the mint for change signatures and reconstruct proofs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_blinded_messages: Option<Vec<BlindedMessage>>,
}

/// Operation data enum
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum OperationData {
    /// Send operation data
    Send(SendOperationData),
    /// Receive operation data
    Receive(ReceiveOperationData),
    /// Swap operation data
    Swap(SwapOperationData),
    /// Mint operation data
    Mint(MintOperationData),
    /// Melt operation data
    Melt(MeltOperationData),
}

impl OperationData {
    /// Get the operation kind
    pub fn kind(&self) -> OperationKind {
        match self {
            OperationData::Send(_) => OperationKind::Send,
            OperationData::Receive(_) => OperationKind::Receive,
            OperationData::Swap(_) => OperationKind::Swap,
            OperationData::Mint(_) => OperationKind::Mint,
            OperationData::Melt(_) => OperationKind::Melt,
        }
    }
}

/// Wallet saga for crash-tolerant recovery.
///
/// Sagas represent in-progress wallet operations that need to survive crashes.
/// They use **optimistic locking** via the `version` field to handle concurrent
/// access from multiple wallet instances safely.
///
/// # Optimistic Locking
///
/// When multiple wallet instances share the same database (e.g., mobile app
/// backgrounded while desktop app runs), they might both try to recover the
/// same incomplete saga. Optimistic locking prevents conflicts:
///
/// 1. Each saga has a `version` number starting at 0
/// 2. When updating, the database checks: `WHERE id = ? AND version = ?`
/// 3. If the version matches, the update succeeds and `version` increments
/// 4. If the version doesn't match, another instance modified it first
///
/// This is preferable to pessimistic locking (mutexes) because:
/// - Works across process boundaries (multiple wallet instances)
/// - No deadlock risk
/// - No lock expiration/cleanup needed
/// - Conflicts are rare in practice (sagas are short-lived)
///
/// Instance A reads saga with version=1
/// Instance B reads saga with version=1
/// Instance A updates successfully, version becomes 2
/// Instance B's update fails (version mismatch) - it knows to skip
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletSaga {
    /// Unique operation ID
    pub id: uuid::Uuid,
    /// Operation kind (derived from state)
    pub kind: OperationKind,
    /// Saga state (operation-specific)
    pub state: WalletSagaState,
    /// Amount involved in the operation
    pub amount: Amount,
    /// Mint URL
    pub mint_url: MintUrl,
    /// Currency unit
    pub unit: CurrencyUnit,
    /// Quote ID (for mint/melt operations)
    pub quote_id: Option<String>,
    /// Creation timestamp (unix seconds)
    pub created_at: u64,
    /// Last update timestamp (unix seconds)
    pub updated_at: u64,
    /// Operation-specific data
    pub data: OperationData,
    /// Version number for optimistic locking.
    ///
    /// Incremented on each update. Used to detect concurrent modifications:
    /// - If update succeeds: this instance "won" the race
    /// - If update fails (version mismatch): another instance modified it
    ///
    /// Recovery code should treat version conflicts as "someone else handled it"
    /// and skip to the next saga rather than retrying.
    pub version: u32,
}

impl WalletSaga {
    /// Create a new wallet saga.
    ///
    /// The saga is created with `version = 0`. Each successful update
    /// will increment the version for optimistic locking.
    pub fn new(
        id: uuid::Uuid,
        state: WalletSagaState,
        amount: Amount,
        mint_url: MintUrl,
        unit: CurrencyUnit,
        data: OperationData,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let quote_id = match &data {
            OperationData::Mint(d) => Some(d.quote_id.clone()),
            OperationData::Melt(d) => Some(d.quote_id.clone()),
            _ => None,
        };

        Self {
            id,
            kind: state.kind(),
            state,
            amount,
            mint_url,
            unit,
            quote_id,
            created_at: now,
            updated_at: now,
            data,
            version: 0,
        }
    }

    /// Update the saga state and increment the version.
    ///
    /// This prepares the saga for an optimistic locking update.
    /// The database layer will verify the previous version matches
    /// before applying the update.
    pub fn update_state(&mut self, state: WalletSagaState) {
        self.state = state;
        self.kind = state.kind();
        self.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.version += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
