//! CDK Database

use std::collections::HashMap;
use std::fmt::Debug;
use std::str::FromStr;

use async_trait::async_trait;
use bitcoin::hashes::{sha256, Hash, HashEngine};
use cashu::util::hex;
use cashu::{nut00, Amount, Proofs};
use serde::{Deserialize, Serialize};

use super::Error;
use crate::common::ProofInfo;
use crate::mint_url::MintUrl;
use crate::nuts::{
    CurrencyUnit, Id, KeySetInfo, Keys, MintInfo, PublicKey, SpendingConditions, State,
};
use crate::wallet;
use crate::wallet::MintQuote as WalletMintQuote;

pub trait Database: ProofDatabase + TransactionDatabase {}

/// Wallet Proofs Database trait
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait ProofDatabase: Debug {
    /// Add Mint to storage
    async fn add_mint(&self, mint_url: MintUrl, mint_info: Option<MintInfo>) -> Result<(), Error>;
    /// Remove Mint from storage
    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), Error>;
    /// Get mint from storage
    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, Error>;
    /// Get all mints from storage
    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, Error>;
    /// Update mint url
    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), Error>;

    /// Add mint keyset to storage
    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), Error>;
    /// Get mint keysets for mint url
    async fn get_mint_keysets(&self, mint_url: MintUrl) -> Result<Option<Vec<KeySetInfo>>, Error>;
    /// Get mint keyset by id
    async fn get_keyset_by_id(&self, keyset_id: &Id) -> Result<Option<KeySetInfo>, Error>;

    /// Add [`Keys`] to storage
    async fn add_keys(&self, keys: Keys) -> Result<(), Error>;
    /// Get [`Keys`] from storage
    async fn get_keys(&self, id: &Id) -> Result<Option<Keys>, Error>;
    /// Remove [`Keys`] from storage
    async fn remove_keys(&self, id: &Id) -> Result<(), Error>;

    /// Update the proofs in storage by adding new proofs or removing proofs by
    /// their Y value.
    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), Error>;
    /// Set proofs as pending in storage. Proofs are identified by their Y
    /// value.
    async fn set_pending_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Error>;
    /// Reserve proofs in storage. Proofs are identified by their Y value.
    async fn reserve_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Error>;
    /// Set proofs as unspent in storage. Proofs are identified by their Y
    /// value.
    async fn set_unspent_proofs(&self, ys: Vec<PublicKey>) -> Result<(), Error>;
    /// Get proofs from storage
    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<State>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, Error>;

    /// Increment Keyset counter
    async fn increment_keyset_counter(&self, keyset_id: &Id, count: u32) -> Result<(), Error>;
    /// Get current Keyset counter
    async fn get_keyset_counter(&self, keyset_id: &Id) -> Result<Option<u32>, Error>;
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait TransactionDatabase: Debug {
    /// Add mint quote to storage
    async fn add_mint_quote(&self, quote: WalletMintQuote) -> Result<(), Error>;
    /// Get mint quote from storage
    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<WalletMintQuote>, Error>;
    /// Get mint quotes from storage
    async fn get_mint_quotes(&self) -> Result<Vec<WalletMintQuote>, Error>;
    /// Remove mint quote from storage
    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Error>;

    /// Add melt quote to storage
    async fn add_melt_quote(&self, quote: wallet::MeltQuote) -> Result<(), Error>;
    /// Get melt quote from storage
    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<wallet::MeltQuote>, Error>;
    /// Remove melt quote from storage
    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Error>;

    /// Add transaction to storage
    async fn add_transaction(&self, transaction: Transaction) -> Result<(), Error>;
    /// Get transaction from storage
    async fn get_transaction(&self, id: &TransactionId) -> Result<Option<Transaction>, Error>;
    /// Get all transactions from storage that match the given criteria
    async fn get_transactions(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        start_timestamp: Option<u64>,
        end_timestamp: Option<u64>,
    ) -> Result<Vec<Transaction>, Error>;
    // Remove transaction from storage
    async fn remove_transaction(&self, id: &TransactionId) -> Result<(), Error>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub amount: Amount,
    pub direction: TransactionDirection,
    pub mint_url: MintUrl,
    pub timestamp: u64,
    pub unit: CurrencyUnit,
    pub ys: Vec<PublicKey>,
    pub memo: Option<String>,
    pub metadata: HashMap<String, String>,
}

impl Transaction {
    pub fn id(&self) -> TransactionId {
        TransactionId::new(self.ys.clone())
    }

    pub fn matches_criteria(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        start_timestamp: Option<u64>,
        end_timestamp: Option<u64>,
    ) -> bool {
        if let Some(mint_url) = mint_url {
            if self.mint_url != mint_url {
                return false;
            }
        }
        if let Some(unit) = unit {
            if self.unit != unit {
                return false;
            }
        }
        if let Some(start_timestamp) = start_timestamp {
            if self.timestamp < start_timestamp {
                return false;
            }
        }
        if let Some(end_timestamp) = end_timestamp {
            if self.timestamp > end_timestamp {
                return false;
            }
        }
        true
    }
}

impl Ord for Transaction {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .timestamp
            .cmp(&self.timestamp)
            .then_with(|| self.id().cmp(&other.id()))
    }
}

impl PartialOrd for Transaction {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Transaction direction
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionDirection {
    Incoming,
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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

    /// Get inner value
    pub fn as_bytes(&self) -> &[u8; 32] {
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
        let bytes = hex::decode(value)?;
        let mut array = [0u8; 32];
        array.copy_from_slice(&bytes);
        Ok(Self(array))
    }
}

impl TryFrom<Proofs> for TransactionId {
    type Error = nut00::Error;

    fn try_from(proofs: Proofs) -> Result<Self, Self::Error> {
        let ys = proofs
            .iter()
            .map(|proof| proof.y())
            .collect::<Result<Vec<PublicKey>, nut00::Error>>()?;
        Ok(Self::new(ys))
    }
}
