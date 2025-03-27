//! Wallet Types

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use bitcoin::hashes::{sha256, Hash, HashEngine};
use cashu::util::hex;
use cashu::{nut00, Proofs, PublicKey};
use serde::{Deserialize, Serialize};

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
    /// Amount of quote
    pub amount: Amount,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// User-defined metadata
    pub metadata: HashMap<String, String>,
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
