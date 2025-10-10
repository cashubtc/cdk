//! NUT-XX: Mining share functionality

use bitcoin::hashes::{sha256, Hash};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fmt::Display;

use super::nut02::Id;
use super::{CurrencyUnit, PublicKey};
use crate::Amount;
use thiserror::Error;

/// NUT-XX Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid Amount
    #[error("Invalid amount")]
    InvalidAmount,
    /// Invalid hash
    #[error("Invalid hash")]
    InvalidHash,
}

/// Quote state for mining shares
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub enum QuoteState {
    /// Quote is not paid
    #[default]
    Unpaid,
    /// Quote is paid
    Paid,
    /// Quote is paid and cashu tokens have been issued for it
    Issued,
}

impl Display for QuoteState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QuoteState::Unpaid => write!(f, "UNPAID"),
            QuoteState::Paid => write!(f, "PAID"),
            QuoteState::Issued => write!(f, "ISSUED"),
        }
    }
}

impl std::str::FromStr for QuoteState {
    type Err = Error;

    fn from_str(state: &str) -> Result<Self, Self::Err> {
        match state.to_uppercase().as_str() {
            "UNPAID" => Ok(QuoteState::Unpaid),
            "PAID" => Ok(QuoteState::Paid),
            "ISSUED" => Ok(QuoteState::Issued),
            _ => Err(Error::InvalidAmount),
        }
    }
}

/// Melt Mining share request  
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MeltQuoteMiningShareRequest {
    /// Amount to mint
    pub amount: Amount,
    /// Currency unit
    pub unit: CurrencyUnit,
    /// Pubkey for NUT-20 signature validation
    pub pubkey: PublicKey,
}

/// Melt quote mining share response
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MeltQuoteMiningShareResponse<Q> {
    /// Quote ID
    pub quote: Q,
    /// Amount
    pub amount: Option<Amount>,
    /// Currency unit
    pub unit: Option<CurrencyUnit>,
    /// Quote State
    pub state: QuoteState,
    /// Unix timestamp until which the quote is valid
    pub expiry: Option<u64>,
    /// Pubkey for NUT-20
    pub pubkey: PublicKey,
}

/// Mining share mint quote request
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MintQuoteMiningShareRequest {
    /// Amount to mint
    pub amount: Amount,
    /// Currency unit
    pub unit: CurrencyUnit,
    /// Mining share hash (block header hash)
    pub header_hash: sha256::Hash,
    /// Optional description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Pubkey for NUT-20 signature validation
    pub pubkey: PublicKey,
}

impl MintQuoteMiningShareRequest {
    /// Validate the mining share request
    pub fn validate(&self) -> Result<(), Error> {
        // Valid amounts are between 1 and u64::MAX inclusive
        // Amounts use exponential units (2^difficulty)
        if self.amount == Amount::ZERO || self.amount > Amount::from(u64::MAX) {
            return Err(Error::InvalidAmount);
        }

        // Header hash validation - ensure it's not all zeros
        if self.header_hash.to_byte_array().iter().all(|&b| b == 0) {
            return Err(Error::InvalidHash);
        }

        Ok(())
    }
}

/// Mining share mint quote response
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MintQuoteMiningShareResponse<Q> {
    /// Quote ID
    pub quote: Q,
    /// Request identifier (header hash)
    pub request: String,
    /// Amount
    pub amount: Option<Amount>,
    /// Currency unit
    pub unit: Option<CurrencyUnit>,
    /// Quote State
    pub state: QuoteState,
    /// Unix timestamp until which the quote is valid
    pub expiry: Option<u64>,
    /// Pubkey for NUT-20
    pub pubkey: PublicKey,
    /// Keyset ID for this quote
    pub keyset_id: Id,
    /// Amount that has been issued for this quote
    pub amount_issued: Amount,
}

impl<Q: ToString> MintQuoteMiningShareResponse<Q> {
    /// Convert quote ID to string
    pub fn to_string_id(&self) -> MintQuoteMiningShareResponse<String> {
        MintQuoteMiningShareResponse {
            quote: self.quote.to_string(),
            request: self.request.clone(),
            amount: self.amount,
            unit: self.unit.clone(),
            state: self.state,
            expiry: self.expiry,
            pubkey: self.pubkey,
            keyset_id: self.keyset_id,
            amount_issued: self.amount_issued,
        }
    }

    /// Check if quote has expired
    pub fn is_expired(&self) -> bool {
        if let Some(expiry) = self.expiry {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            now > expiry
        } else {
            false
        }
    }

    /// Check if quote is fully issued
    pub fn is_fully_issued(&self) -> bool {
        if let Some(amount) = self.amount {
            // TODO: Consider adding validation/alerting for over-issuance scenarios
            // where amount_issued > amount, as this could indicate a bug or security issue
            self.amount_issued >= amount
        } else {
            false
        }
    }
}

#[cfg(feature = "mint")]
impl From<MintQuoteMiningShareResponse<uuid::Uuid>> for MintQuoteMiningShareResponse<String> {
    fn from(value: MintQuoteMiningShareResponse<uuid::Uuid>) -> Self {
        Self {
            quote: value.quote.to_string(),
            request: value.request,
            amount: value.amount,
            unit: value.unit,
            state: value.state,
            expiry: value.expiry,
            pubkey: value.pubkey,
            keyset_id: value.keyset_id,
            amount_issued: value.amount_issued,
        }
    }
}

#[cfg(feature = "mint")]
impl From<MintQuoteMiningShareResponse<crate::quote_id::QuoteId>>
    for MintQuoteMiningShareResponse<String>
{
    fn from(value: MintQuoteMiningShareResponse<crate::quote_id::QuoteId>) -> Self {
        Self {
            quote: value.quote.to_string(),
            request: value.request,
            amount: value.amount,
            unit: value.unit,
            state: value.state,
            expiry: value.expiry,
            pubkey: value.pubkey,
            keyset_id: value.keyset_id,
            amount_issued: value.amount_issued,
        }
    }
}

impl From<super::nut23::QuoteState> for QuoteState {
    fn from(state: super::nut23::QuoteState) -> Self {
        match state {
            super::nut23::QuoteState::Unpaid => QuoteState::Unpaid,
            super::nut23::QuoteState::Paid => QuoteState::Paid,
            super::nut23::QuoteState::Issued => QuoteState::Issued,
        }
    }
}

impl From<QuoteState> for super::nut23::QuoteState {
    fn from(state: QuoteState) -> Self {
        match state {
            QuoteState::Unpaid => super::nut23::QuoteState::Unpaid,
            QuoteState::Paid => super::nut23::QuoteState::Paid,
            QuoteState::Issued => super::nut23::QuoteState::Issued,
        }
    }
}

/// Quote state for mining shares
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub enum MiningShareQuoteState {
    /// Quote is not paid
    #[default]
    Unpaid,
    /// Quote is paid
    Paid,
    /// Quote is paid and cashu tokens have been issued for it
    Issued,
}

impl Display for MiningShareQuoteState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MiningShareQuoteState::Unpaid => write!(f, "UNPAID"),
            MiningShareQuoteState::Paid => write!(f, "PAID"),
            MiningShareQuoteState::Issued => write!(f, "ISSUED"),
        }
    }
}

impl std::str::FromStr for MiningShareQuoteState {
    type Err = Error;

    fn from_str(state: &str) -> Result<Self, Self::Err> {
        match state.to_uppercase().as_str() {
            "UNPAID" => Ok(MiningShareQuoteState::Unpaid),
            "PAID" => Ok(MiningShareQuoteState::Paid),
            "ISSUED" => Ok(MiningShareQuoteState::Issued),
            _ => Err(Error::InvalidAmount),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_mining_share_quote_response_serialization() {
        let mut id_bytes = vec![0x01]; // v2 version (KeySetVersion::Version01)
        id_bytes.extend_from_slice(&[1u8; 32]); // 32 bytes of data
        let keyset_id = Id::from_bytes(&id_bytes).unwrap();
        let pubkey = PublicKey::from_hex(
            "02e6642fd69bd211f93f7f1f36ca51a26a5290eb2dd1b0d8279a87bb0d480c8443",
        )
        .unwrap();

        let response = MintQuoteMiningShareResponse {
            quote: Uuid::new_v4(),
            request: "test_header_hash".to_string(),
            amount: Some(Amount::from(100)),
            unit: Some(CurrencyUnit::Sat),
            state: QuoteState::Paid,
            expiry: Some(1234567890),
            pubkey,
            keyset_id,
            amount_issued: Amount::from(50),
        };

        // Test serialization/deserialization
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: MintQuoteMiningShareResponse<Uuid> = serde_json::from_str(&json).unwrap();

        assert_eq!(response, deserialized);
        assert_eq!(response.amount_issued, Amount::from(50));
    }

    #[test]
    fn test_mining_share_quote_response_to_string_id() {
        let mut id_bytes = vec![0x01]; // v2 version (KeySetVersion::Version01)
        id_bytes.extend_from_slice(&[1u8; 32]); // 32 bytes of data
        let keyset_id = Id::from_bytes(&id_bytes).unwrap();
        let pubkey = PublicKey::from_hex(
            "02e6642fd69bd211f93f7f1f36ca51a26a5290eb2dd1b0d8279a87bb0d480c8443",
        )
        .unwrap();
        let uuid = Uuid::new_v4();

        let response = MintQuoteMiningShareResponse {
            quote: uuid,
            request: "test_header_hash".to_string(),
            amount: Some(Amount::from(100)),
            unit: Some(CurrencyUnit::Sat),
            state: QuoteState::Paid,
            expiry: Some(1234567890),
            pubkey,
            keyset_id,
            amount_issued: Amount::from(25),
        };

        let string_response = response.to_string_id();

        assert_eq!(string_response.quote, uuid.to_string());
        assert_eq!(string_response.amount_issued, Amount::from(25));
        assert_eq!(string_response.request, "test_header_hash");
        assert_eq!(string_response.state, QuoteState::Paid);
    }

    #[test]
    fn test_mining_share_quote_response_is_fully_issued() {
        let mut id_bytes = vec![0x01]; // v2 version (KeySetVersion::Version01)
        id_bytes.extend_from_slice(&[1u8; 32]); // 32 bytes of data
        let keyset_id = Id::from_bytes(&id_bytes).unwrap();
        let pubkey = PublicKey::from_hex(
            "02e6642fd69bd211f93f7f1f36ca51a26a5290eb2dd1b0d8279a87bb0d480c8443",
        )
        .unwrap();

        // Test partially issued
        let response = MintQuoteMiningShareResponse {
            quote: Uuid::new_v4(),
            request: "test_header_hash".to_string(),
            amount: Some(Amount::from(100)),
            unit: Some(CurrencyUnit::Sat),
            state: QuoteState::Paid,
            expiry: Some(1234567890),
            pubkey,
            keyset_id,
            amount_issued: Amount::from(50),
        };

        assert!(!response.is_fully_issued());

        // Test fully issued
        let response_full = MintQuoteMiningShareResponse {
            amount_issued: Amount::from(100),
            ..response.clone()
        };

        assert!(response_full.is_fully_issued());

        // Test over-issued
        let response_over = MintQuoteMiningShareResponse {
            amount_issued: Amount::from(150),
            ..response
        };

        assert!(response_over.is_fully_issued());
    }

    #[test]
    fn test_mining_share_request_validation() {
        let pubkey = PublicKey::from_hex(
            "02e6642fd69bd211f93f7f1f36ca51a26a5290eb2dd1b0d8279a87bb0d480c8443",
        )
        .unwrap();
        let header_hash = sha256::Hash::from_byte_array([1u8; 32]);

        // Valid request
        let valid_request = MintQuoteMiningShareRequest {
            amount: Amount::from(50),
            unit: CurrencyUnit::Sat,
            header_hash,
            description: None,
            pubkey,
        };

        assert!(valid_request.validate().is_ok());

        // Invalid amount (zero)
        let invalid_zero = MintQuoteMiningShareRequest {
            amount: Amount::ZERO,
            ..valid_request.clone()
        };

        assert!(invalid_zero.validate().is_err());

        // Invalid amount (too large)
        let invalid_large = MintQuoteMiningShareRequest {
            amount: Amount::from(300),
            ..valid_request.clone()
        };

        assert!(invalid_large.validate().is_err());

        // Invalid header hash (all zeros)
        let invalid_hash = MintQuoteMiningShareRequest {
            header_hash: sha256::Hash::from_byte_array([0u8; 32]),
            ..valid_request
        };

        assert!(invalid_hash.validate().is_err());
    }

    #[test]
    fn test_quote_state_string_conversion() {
        assert_eq!(QuoteState::Unpaid.to_string(), "UNPAID");
        assert_eq!(QuoteState::Paid.to_string(), "PAID");
        assert_eq!(QuoteState::Issued.to_string(), "ISSUED");

        assert_eq!("UNPAID".parse::<QuoteState>().unwrap(), QuoteState::Unpaid);
        assert_eq!("PAID".parse::<QuoteState>().unwrap(), QuoteState::Paid);
        assert_eq!("ISSUED".parse::<QuoteState>().unwrap(), QuoteState::Issued);

        // Case insensitive
        assert_eq!("unpaid".parse::<QuoteState>().unwrap(), QuoteState::Unpaid);
        assert_eq!("paid".parse::<QuoteState>().unwrap(), QuoteState::Paid);
        assert_eq!("issued".parse::<QuoteState>().unwrap(), QuoteState::Issued);

        // Invalid state
        assert!("INVALID".parse::<QuoteState>().is_err());
    }
}
