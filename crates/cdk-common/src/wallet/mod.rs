//! Wallet Types

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use bitcoin::hashes::{sha256, Hash, HashEngine};
use cashu::util::hex;
use cashu::{nut00, PaymentMethod, Proof, Proofs, PublicKey};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::mint_url::MintUrl;
use crate::nuts::{
    CurrencyUnit, MeltQuoteState, MintQuoteState, SecretKey, SpendingConditions, State,
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
            self.amount_paid.saturating_sub(self.amount_issued)
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

/// Abstract wallet interface for Cashu protocol operations.
///
/// This trait defines the complete set of operations a Cashu wallet must support,
/// using associated types to remain implementation-agnostic. It enables:
///
/// - **Polymorphism**: program against the interface rather than a concrete wallet
/// - **FFI support**: wrap the trait with foreign-function-friendly types
/// - **Testability**: mock wallet behavior in tests without a real mint
///
/// A wallet is bound to a single mint URL and currency unit. For multi-mint
/// scenarios, see `MultiMintWallet` which manages a collection of per-mint wallets.
///
/// # Lifecycle
///
/// A typical usage flow:
/// 1. **Mint** — request a quote, pay the invoice, mint proofs
/// 2. **Send** — select proofs and produce a token for the recipient
/// 3. **Receive** — swap incoming proofs into the local wallet
/// 4. **Melt** — redeem proofs by paying a Lightning invoice
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait Wallet: Send + Sync {
    // --- Associated types ---

    /// Numeric amount (e.g. `cdk_common::Amount`)
    type Amount: Clone + Send + Sync;
    /// Ordered collection of ecash proofs
    type Proofs: Clone + Send + Sync;
    /// Single ecash proof
    type Proof: Clone + Send + Sync;
    /// Mint-issued quote describing how to fund the wallet (NUT-04 / NUT-23 / NUT-25)
    type MintQuote: Clone + Send + Sync;
    /// Mint-issued quote describing a Lightning payment to execute (NUT-05 / NUT-24)
    type MeltQuote: Clone + Send + Sync;
    /// Outcome of a confirmed melt, including preimage and fee details
    type MeltResult: Clone + Send + Sync;
    /// Serialisable ecash token (V3 / V4)
    type Token: Clone + Send + Sync;
    /// Currency unit for this wallet's keyset (e.g. `sat`, `usd`)
    type CurrencyUnit: Clone + Send + Sync;
    /// Parsed and validated mint URL
    type MintUrl: Clone + Send + Sync;
    /// Mint self-description returned by `GET /v1/info`
    type MintInfo: Clone + Send + Sync;
    /// Keyset metadata (id, unit, fee rate, active flag, …)
    type KeySetInfo: Clone + Send + Sync;
    /// Error type returned by all fallible operations
    type Error: Send + Sync + 'static;
    /// Configuration for [`send`](Self::send) (memo, amount split target, P2PK, …)
    type SendOptions: Clone + Send + Sync;
    /// Configuration for [`receive`](Self::receive) (P2PK pre-image, …)
    type ReceiveOptions: Clone + Send + Sync;
    /// P2PK / HTLC spending conditions attached to outputs
    type SpendingConditions: Clone + Send + Sync;
    /// Strategy for splitting proof amounts (e.g. powers-of-two, custom targets)
    type SplitTarget: Clone + Send + Sync + Default;
    /// Payment protocol selector (Bolt11, Bolt12, or a custom method name)
    type PaymentMethod: Clone + Send + Sync;
    /// Per-method melt options (MPP, amountless invoices, …)
    type MeltOptions: Clone + Send + Sync;
    /// Summary returned by [`restore`](Self::restore) (proofs recovered, amount, …)
    type Restored: Clone + Send + Sync;
    /// Persistent record of a wallet operation (mint, melt, send, receive, …)
    type Transaction: Clone + Send + Sync;
    /// Unique identifier for a [`Transaction`](Self::Transaction)
    type TransactionId: Clone + Send + Sync;
    /// Incoming vs outgoing filter for [`list_transactions`](Self::list_transactions)
    type TransactionDirection: Clone + Send + Sync;
    /// NUT-18 payment request
    type PaymentRequest: Clone + Send + Sync;
    /// Handle to an active WebSocket subscription; dropping it unsubscribes
    type Subscription: Send + Sync;
    /// Parameters passed to [`subscribe`](Self::subscribe) to filter events
    type SubscribeParams: Clone + Send + Sync;

    // --- Identity ---

    /// Return the mint URL this wallet is bound to.
    fn mint_url(&self) -> Self::MintUrl;

    /// Return the currency unit this wallet operates in.
    fn unit(&self) -> Self::CurrencyUnit;

    // --- Balance ---

    /// Return the sum of all `Unspent` proof amounts for this mint and unit.
    async fn total_balance(&self) -> Result<Self::Amount, Self::Error>;

    /// Return the sum of all `Pending` proof amounts (proofs involved in
    /// in-flight operations that have not yet settled).
    async fn total_pending_balance(&self) -> Result<Self::Amount, Self::Error>;

    /// Return the sum of all `Reserved` proof amounts (proofs locked to
    /// a send that has not been claimed or revoked).
    async fn total_reserved_balance(&self) -> Result<Self::Amount, Self::Error>;

    // --- Mint info ---

    /// Fetch mint info by calling `GET /v1/info` on the mint.
    ///
    /// Always makes a network request and updates the local cache.
    /// Returns `None` if the mint does not expose info.
    async fn fetch_mint_info(&self) -> Result<Option<Self::MintInfo>, Self::Error>;

    /// Return cached mint info, re-fetching from the mint only when the
    /// cache TTL has expired.
    async fn load_mint_info(&self) -> Result<Self::MintInfo, Self::Error>;

    /// Return the active keyset that has the lowest input fee per proof (`input_fee_ppk`).
    async fn get_active_keyset(&self) -> Result<Self::KeySetInfo, Self::Error>;

    /// Fetch the latest keysets from the mint, store them locally, and return
    /// the full list of keysets for this wallet's unit.
    async fn refresh_keysets(&self) -> Result<Vec<Self::KeySetInfo>, Self::Error>;

    // --- Minting ---

    /// Request a mint quote for the given payment method.
    ///
    /// The mint returns an invoice (or equivalent payment request) that, once
    /// paid, allows the caller to mint ecash proofs of the quoted amount.
    ///
    /// # Arguments
    /// * `method` — payment protocol to use (Bolt11, Bolt12, or custom)
    /// * `amount` — requested amount; **required** for Bolt11 and Custom,
    ///   optional for Bolt12 (the payer chooses the amount)
    /// * `description` — optional memo embedded in the invoice; only honoured
    ///   when the mint advertises description support for the method
    /// * `extra` — optional JSON string with method-specific fields (used by
    ///   custom payment methods)
    async fn mint_quote(
        &self,
        method: Self::PaymentMethod,
        amount: Option<Self::Amount>,
        description: Option<String>,
        extra: Option<String>,
    ) -> Result<Self::MintQuote, Self::Error>;

    /// Re-fetch the current state of a mint quote from the mint.
    ///
    /// Use this to poll whether the underlying invoice has been paid.
    /// The returned quote reflects the latest `state` and `amount_paid`.
    async fn refresh_mint_quote(&self, quote_id: &str) -> Result<Self::MintQuote, Self::Error>;

    // --- Melting ---

    /// Request a melt quote to pay an external invoice with ecash.
    ///
    /// The mint estimates the amount of ecash (including fees) needed to
    /// settle the given payment request.
    ///
    /// # Arguments
    /// * `method` — payment protocol to use (Bolt11, Bolt12, or custom)
    /// * `request` — the payment request string (e.g. a BOLT-11 invoice or
    ///   BOLT-12 offer)
    /// * `options` — method-specific options (MPP, amountless invoice amount, …)
    /// * `extra` — optional JSON string with custom-method-specific fields
    async fn melt_quote(
        &self,
        method: Self::PaymentMethod,
        request: String,
        options: Option<Self::MeltOptions>,
        extra: Option<String>,
    ) -> Result<Self::MeltQuote, Self::Error>;

    // --- Sending ---

    /// Select proofs for the given `amount`, optionally swap for exact
    /// change, and produce an ecash token to hand to the recipient.
    async fn send(
        &self,
        amount: Self::Amount,
        options: Self::SendOptions,
    ) -> Result<Self::Token, Self::Error>;

    /// Return the IDs of all in-flight send operations whose proofs are
    /// currently in the `Reserved` state.
    async fn get_pending_sends(&self) -> Result<Vec<String>, Self::Error>;

    /// Cancel a pending send and return the reserved proofs to `Unspent`.
    ///
    /// Returns the total amount of proofs reclaimed.
    async fn revoke_send(&self, operation_id: &str) -> Result<Self::Amount, Self::Error>;

    /// Check whether the recipient has already swapped the proofs from
    /// a pending send. Returns `true` if the proofs have been spent.
    async fn check_send_status(&self, operation_id: &str) -> Result<bool, Self::Error>;

    // --- Receiving ---

    /// Decode an ecash token string, swap the proofs into this wallet,
    /// and return the received amount.
    async fn receive(
        &self,
        encoded_token: &str,
        options: Self::ReceiveOptions,
    ) -> Result<Self::Amount, Self::Error>;

    /// Swap raw proofs into this wallet (e.g. proofs obtained out-of-band).
    ///
    /// `memo` and `token` are optional metadata stored alongside the
    /// resulting transaction record.
    async fn receive_proofs(
        &self,
        proofs: Self::Proofs,
        options: Self::ReceiveOptions,
        memo: Option<String>,
        token: Option<String>,
    ) -> Result<Self::Amount, Self::Error>;

    // --- Swapping ---

    /// Swap `input_proofs` at the mint, optionally changing denominations
    /// or attaching spending conditions to the new outputs.
    ///
    /// Returns `None` when no change proofs are produced (all value went
    /// to conditioned outputs).
    async fn swap(
        &self,
        amount: Option<Self::Amount>,
        amount_split_target: Self::SplitTarget,
        input_proofs: Self::Proofs,
        spending_conditions: Option<Self::SpendingConditions>,
        include_fees: bool,
    ) -> Result<Option<Self::Proofs>, Self::Error>;

    // --- Proofs ---

    /// Return all proofs in the `Unspent` state.
    async fn get_unspent_proofs(&self) -> Result<Self::Proofs, Self::Error>;

    /// Return all proofs in the `Pending` state (involved in an in-flight
    /// mint, melt, or swap).
    async fn get_pending_proofs(&self) -> Result<Self::Proofs, Self::Error>;

    /// Return all proofs in the `Reserved` state (locked to an unclaimed send).
    async fn get_reserved_proofs(&self) -> Result<Self::Proofs, Self::Error>;

    /// Return all proofs in the `PendingSpent` state (sent to the mint
    /// but not yet confirmed spent).
    async fn get_pending_spent_proofs(&self) -> Result<Self::Proofs, Self::Error>;

    /// Query the mint for the current state of every pending proof,
    /// reclaim any that are still unspent, and return the total
    /// amount reclaimed.
    async fn check_all_pending_proofs(&self) -> Result<Self::Amount, Self::Error>;

    /// Ask the mint which of the given `proofs` have been spent.
    ///
    /// Returns a `Vec<bool>` aligned with the input: `true` = spent.
    async fn check_proofs_spent(&self, proofs: Self::Proofs) -> Result<Vec<bool>, Self::Error>;

    /// Swap the given proofs back into the wallet, discarding any that
    /// the mint reports as already spent.
    async fn reclaim_unspent(&self, proofs: Self::Proofs) -> Result<(), Self::Error>;

    // --- Transactions ---

    /// List recorded transactions, optionally filtered by direction.
    ///
    /// Pass `None` to return both incoming and outgoing transactions.
    async fn list_transactions(
        &self,
        direction: Option<Self::TransactionDirection>,
    ) -> Result<Vec<Self::Transaction>, Self::Error>;

    /// Look up a single transaction by its ID.
    ///
    /// Returns `None` if no transaction with that ID exists.
    async fn get_transaction(
        &self,
        id: Self::TransactionId,
    ) -> Result<Option<Self::Transaction>, Self::Error>;

    /// Return the proofs that were involved in the given transaction.
    async fn get_proofs_for_transaction(
        &self,
        id: Self::TransactionId,
    ) -> Result<Self::Proofs, Self::Error>;

    /// Revert a transaction by returning its proofs to the `Unspent` state.
    ///
    /// This is only valid for transactions whose proofs have **not** been
    /// spent at the mint.
    async fn revert_transaction(&self, id: Self::TransactionId) -> Result<(), Self::Error>;

    // --- Token verification ---

    /// Verify the DLEQ (Discrete-Log Equality) proofs on every proof
    /// inside the given token, ensuring they were signed by the mint.
    async fn verify_token_dleq(&self, token: &Self::Token) -> Result<(), Self::Error>;

    // --- Wallet recovery ---

    /// Deterministically re-derive all secrets from the wallet seed and
    /// recover any proofs that the mint still considers unspent.
    async fn restore(&self) -> Result<Self::Restored, Self::Error>;

    // --- Keysets & fees ---

    /// Return the `input_fee_ppk` (parts per thousand) for the given keyset.
    async fn get_keyset_fees(&self, keyset_id: &str) -> Result<u64, Self::Error>;

    /// Calculate the total fee for spending `proof_count` proofs from the
    /// given keyset.
    async fn calculate_fee(
        &self,
        proof_count: u64,
        keyset_id: &str,
    ) -> Result<Self::Amount, Self::Error>;

    // --- Subscriptions ---

    /// Open a WebSocket subscription to the mint for real-time state
    /// updates (e.g. quote state changes).
    ///
    /// The returned handle stays subscribed until it is dropped.
    async fn subscribe(
        &self,
        params: Self::SubscribeParams,
    ) -> Result<Self::Subscription, Self::Error>;

    // --- Payment requests ---

    /// Fulfil a NUT-18 payment request by sending ecash to the payee
    /// via the transport specified in the request.
    ///
    /// `custom_amount` overrides the amount in the request when set.
    async fn pay_request(
        &self,
        request: Self::PaymentRequest,
        custom_amount: Option<Self::Amount>,
    ) -> Result<(), Self::Error>;

    // --- BIP-353 / Lightning Address ---

    /// Resolve a BIP-353 human-readable address to a BOLT-12 offer and
    /// return a melt quote for the given `amount` (in millisatoshis).
    ///
    /// Not available on `wasm32` targets (requires DNS resolution).
    #[cfg(not(target_arch = "wasm32"))]
    async fn melt_bip353_quote(
        &self,
        address: &str,
        amount: Self::Amount,
    ) -> Result<Self::MeltQuote, Self::Error>;

    /// Resolve a Lightning Address (LNURL-pay) and return a melt quote
    /// for the given `amount` (in millisatoshis).
    ///
    /// Not available on `wasm32` targets (requires HTTPS callback).
    #[cfg(not(target_arch = "wasm32"))]
    async fn melt_lightning_address_quote(
        &self,
        address: &str,
        amount: Self::Amount,
    ) -> Result<Self::MeltQuote, Self::Error>;

    /// Resolve a human-readable address (tries BIP-353 first, then
    /// Lightning Address) and return a melt quote for the given `amount`
    /// (in millisatoshis).
    ///
    /// Not available on `wasm32` targets.
    #[cfg(not(target_arch = "wasm32"))]
    async fn melt_human_readable_quote(
        &self,
        address: &str,
        amount: Self::Amount,
    ) -> Result<Self::MeltQuote, Self::Error>;

    // --- Auth ---

    /// Store a Clear Auth Token (CAT) for authenticated mint access.
    async fn set_cat(&self, cat: String) -> Result<(), Self::Error>;

    /// Store an OAuth2 refresh token for authenticated mint access.
    async fn set_refresh_token(&self, refresh_token: String) -> Result<(), Self::Error>;

    /// Use the stored refresh token to obtain a new access token from the
    /// mint's OIDC provider.
    async fn refresh_access_token(&self) -> Result<(), Self::Error>;

    /// Mint blind-auth proofs that can be presented to the mint to
    /// authenticate future requests.
    async fn mint_blind_auth(&self, amount: Self::Amount) -> Result<Self::Proofs, Self::Error>;

    /// Return all unspent blind-auth proofs.
    async fn get_unspent_auth_proofs(&self) -> Result<Self::Proofs, Self::Error>;
}
