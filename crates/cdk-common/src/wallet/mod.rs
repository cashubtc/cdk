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
            self.amount_paid
                .checked_sub(self.amount_issued)
                .unwrap_or(Amount::ZERO)
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
