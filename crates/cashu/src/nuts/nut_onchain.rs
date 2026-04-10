//! NUT-26: Onchain
//!
//! <https://github.com/cashubtc/nuts/blob/main/26.md>

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::nut00::CurrencyUnit;
use super::nut01::PublicKey;
use super::MeltQuoteState;
#[cfg(feature = "mint")]
use crate::quote_id::QuoteId;
use crate::util::serde_helpers::deserialize_empty_string_as_none;
use crate::{Amount, BlindSignature};

/// Mint quote onchain request [NUT-26]
///
/// Request for an onchain mint quote. Requires a pubkey (NUT-20).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MintQuoteOnchainRequest {
    /// Unit wallet would like to mint
    pub unit: CurrencyUnit,
    /// NUT-20 Pubkey (required)
    pub pubkey: PublicKey,
}

/// Mint quote onchain response [NUT-26]
///
/// Response containing the onchain quote details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MintQuoteOnchainResponse<Q> {
    /// Quote Id
    pub quote: Q,
    /// Bitcoin address to send funds to
    pub request: String,
    /// Unit
    pub unit: CurrencyUnit,
    /// Unix timestamp until the quote is valid
    pub expiry: Option<u64>,
    /// NUT-20 Pubkey from the request
    pub pubkey: PublicKey,
    /// Total confirmed amount paid to the request
    #[serde(default)]
    pub amount_paid: Amount,
    /// Amount of ecash that has been issued for the given mint quote
    #[serde(default)]
    pub amount_issued: Amount,
}

impl<Q: ToString> MintQuoteOnchainResponse<Q> {
    /// Convert the MintQuoteOnchainResponse with a quote type Q to a String
    pub fn to_string_id(&self) -> MintQuoteOnchainResponse<String> {
        MintQuoteOnchainResponse {
            quote: self.quote.to_string(),
            request: self.request.clone(),
            unit: self.unit.clone(),
            expiry: self.expiry,
            pubkey: self.pubkey,
            amount_paid: self.amount_paid,
            amount_issued: self.amount_issued,
        }
    }
}

#[cfg(feature = "mint")]
impl From<MintQuoteOnchainResponse<QuoteId>> for MintQuoteOnchainResponse<String> {
    fn from(value: MintQuoteOnchainResponse<QuoteId>) -> Self {
        Self {
            quote: value.quote.to_string(),
            request: value.request,
            unit: value.unit,
            expiry: value.expiry,
            pubkey: value.pubkey,
            amount_paid: value.amount_paid,
            amount_issued: value.amount_issued,
        }
    }
}

/// Mint method onchain options
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MintMethodOnchainOptions {
    /// Minimum number of confirmations required
    pub confirmations: u32,
}

/// Melt quote onchain request [NUT-26]
///
/// Request for an onchain melt quote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MeltQuoteOnchainRequest {
    /// Bitcoin address to send to
    pub request: String,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
    /// Amount to send in the specified unit
    pub amount: Amount,
    /// Maximum fee the wallet is willing to pay
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_fee_amount: Option<Amount>,
    /// Batching tier hint (e.g. "immediate", "standard", "economy")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// Opaque metadata as a JSON string for future extensions
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<String>,
}

/// Melt quote onchain response [NUT-26]
///
/// Response containing the onchain melt quote details.
/// The `POST /v1/melt/quote/onchain` endpoint returns an **array** of these responses,
/// each with different `fee` amounts and `estimated_blocks`. The wallet chooses which
/// quote to use for melting; the other quotes will expire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MeltQuoteOnchainResponse<Q> {
    /// Quote Id
    pub quote: Q,
    /// Bitcoin address to send to
    pub request: String,
    /// Amount to be melted
    pub amount: Amount,
    /// Unit
    pub unit: CurrencyUnit,
    /// Fee required for the transaction
    pub fee: Amount,
    /// Estimated number of blocks until confirmation
    pub estimated_blocks: u32,
    /// Quote state
    pub state: MeltQuoteState,
    /// Unix timestamp until the quote is valid
    pub expiry: u64,
    /// Change
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<Vec<BlindSignature>>,
    /// Transaction outpoint (txid:vout) once broadcast
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_empty_string_as_none"
    )]
    pub outpoint: Option<String>,
}

impl<Q: ToString> MeltQuoteOnchainResponse<Q> {
    /// Convert the MeltQuoteOnchainResponse with a quote type Q to a String
    pub fn to_string_id(&self) -> MeltQuoteOnchainResponse<String> {
        MeltQuoteOnchainResponse {
            quote: self.quote.to_string(),
            request: self.request.clone(),
            amount: self.amount,
            unit: self.unit.clone(),
            fee: self.fee,
            estimated_blocks: self.estimated_blocks,
            state: self.state,
            expiry: self.expiry,
            change: self.change.clone(),
            outpoint: self.outpoint.clone(),
        }
    }
}

#[cfg(feature = "mint")]
impl From<MeltQuoteOnchainResponse<QuoteId>> for MeltQuoteOnchainResponse<String> {
    fn from(value: MeltQuoteOnchainResponse<QuoteId>) -> Self {
        Self {
            quote: value.quote.to_string(),
            request: value.request,
            amount: value.amount,
            unit: value.unit,
            fee: value.fee,
            estimated_blocks: value.estimated_blocks,
            state: value.state,
            expiry: value.expiry,
            change: value.change,
            outpoint: value.outpoint,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mint_quote_onchain_request_serialization() {
        let request = MintQuoteOnchainRequest {
            unit: CurrencyUnit::Sat,
            pubkey: PublicKey::from_hex(
                "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
            )
            .unwrap(),
        };

        let serialized = serde_json::to_string(&request).unwrap();
        let deserialized: MintQuoteOnchainRequest = serde_json::from_str(&serialized).unwrap();

        assert_eq!(request.unit, deserialized.unit);
        assert_eq!(request.pubkey, deserialized.pubkey);
    }

    #[test]
    fn test_melt_quote_onchain_request_serialization() {
        let request = MeltQuoteOnchainRequest {
            request: "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".to_string(),
            unit: CurrencyUnit::Sat,
            amount: Amount::from(1000),
            max_fee_amount: None,
            tier: None,
            metadata: None,
        };

        let serialized = serde_json::to_string(&request).unwrap();
        let deserialized: MeltQuoteOnchainRequest = serde_json::from_str(&serialized).unwrap();

        assert_eq!(request.request, deserialized.request);
        assert_eq!(request.unit, deserialized.unit);
        assert_eq!(request.amount, deserialized.amount);
    }

    #[test]
    fn test_melt_quote_onchain_response_serialization() {
        let response: MeltQuoteOnchainResponse<String> = MeltQuoteOnchainResponse {
            quote: "TRmjduhIsPxd...".to_string(),
            request: "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".to_string(),
            amount: Amount::from(100000),
            unit: CurrencyUnit::Sat,
            fee: Amount::from(5000),
            estimated_blocks: 1,
            state: MeltQuoteState::Pending,
            expiry: 1701704757,
            change: None,
            outpoint: Some(
                "3b7f3b85c5f1a3c4d2b8e9f6a7c5d8e9f1a2b3c4d5e6f7a8b9c1d2e3f4a5b6c7:2".to_string(),
            ),
        };

        let serialized = serde_json::to_string(&response).unwrap();
        let deserialized: MeltQuoteOnchainResponse<String> =
            serde_json::from_str(&serialized).unwrap();

        assert_eq!(response.quote, deserialized.quote);
        assert_eq!(response.request, deserialized.request);
        assert_eq!(response.amount, deserialized.amount);
        assert_eq!(response.fee, deserialized.fee);
        assert_eq!(response.state, deserialized.state);
        assert_eq!(response.outpoint, deserialized.outpoint);
    }

    #[test]
    fn test_mint_quote_onchain_response_to_string_id() {
        use crate::nuts::nut00::CurrencyUnit;
        use crate::nuts::nut01::PublicKey;
        use crate::Amount;

        let response: MintQuoteOnchainResponse<String> = MintQuoteOnchainResponse {
            quote: "DSGLX9kevM...".to_string(),
            request: "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".to_string(),
            unit: CurrencyUnit::Sat,
            expiry: Some(1701704757),
            pubkey: PublicKey::from_hex(
                "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
            )
            .unwrap(),
            amount_paid: Amount::from(100000),
            amount_issued: Amount::from(0),
        };

        let string_id_response = response.to_string_id();
        assert_eq!(string_id_response.quote, "DSGLX9kevM...");
    }

    #[test]
    fn test_melt_quote_onchain_response_to_string_id() {
        use crate::nuts::nut00::CurrencyUnit;
        use crate::Amount;

        let response: MeltQuoteOnchainResponse<String> = MeltQuoteOnchainResponse {
            quote: "TRmjduhIsPxd...".to_string(),
            request: "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".to_string(),
            amount: Amount::from(100000),
            unit: CurrencyUnit::Sat,
            fee: Amount::from(5000),
            estimated_blocks: 1,
            state: MeltQuoteState::Pending,
            expiry: 1701704757,
            change: None,
            outpoint: Some(
                "3b7f3b85c5f1a3c4d2b8e9f6a7c5d8e9f1a2b3c4d5e6f7a8b9c1d2e3f4a5b6c7:2".to_string(),
            ),
        };

        let string_id_response = response.to_string_id();
        assert_eq!(string_id_response.quote, "TRmjduhIsPxd...");
    }
}
