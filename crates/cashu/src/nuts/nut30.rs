//! NUT-30 onchain payment method

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::nut00::{BlindSignature, BlindedMessage, CurrencyUnit, KnownMethod, PaymentMethod};
use super::nut01::PublicKey;
use super::nut05::MeltRequest;
use super::MeltQuoteState;
#[cfg(feature = "mint")]
use crate::quote_id::QuoteId;
use crate::util::serde_helpers::deserialize_empty_string_as_none;
use crate::{Amount, Proofs};

fn default_onchain_method() -> PaymentMethod {
    PaymentMethod::Known(KnownMethod::Onchain)
}

/// Mint quote onchain request
///
/// Request for an onchain mint quote. Requires a pubkey (NUT-20).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuoteOnchainRequest {
    /// Unit wallet would like to mint
    pub unit: CurrencyUnit,
    /// NUT-20 Pubkey (required)
    pub pubkey: PublicKey,
}

/// Mint quote onchain response
///
/// Response containing the onchain quote details.
///
/// `deny_unknown_fields` is intentional: the `NotificationPayload` enum is
/// `#[serde(untagged)]` and several quote-response variants share a large
/// overlap of field names. Rejecting unknown fields ensures an onchain payload
/// cannot silently deserialize as another method (for example `MintQuoteBolt12Response`
/// which carries an `amount` field Onchain does not have).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
#[serde(deny_unknown_fields)]
pub struct MintQuoteOnchainResponse<Q> {
    /// Quote Id
    pub quote: Q,
    /// Bitcoin address to send funds to
    pub request: String,
    /// Unit
    pub unit: CurrencyUnit,
    /// Payment method
    #[serde(default = "default_onchain_method")]
    pub method: PaymentMethod,
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
            method: self.method.clone(),
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
            method: value.method,
            expiry: value.expiry,
            pubkey: value.pubkey,
            amount_paid: value.amount_paid,
            amount_issued: value.amount_issued,
        }
    }
}

/// Melt quote onchain request
///
/// Request for an onchain melt quote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltQuoteOnchainRequest {
    /// Bitcoin address to send to
    pub request: String,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
    /// Amount to send in the specified unit
    pub amount: Amount,
}

/// Melt onchain request
///
/// Request to execute an onchain melt quote. The wallet selects one of the
/// quote's fee options by including that option's `fee_index` value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MeltOnchainRequest<Q> {
    /// Quote ID
    pub quote: Q,
    /// Selected fee option index from the quote's `fee_options`
    pub fee_index: u32,
    /// Proofs
    pub inputs: Proofs,
    /// Blinded messages that can be used to return overpaid onchain fee reserve
    pub outputs: Option<Vec<BlindedMessage>>,
}

impl<Q> From<MeltOnchainRequest<Q>> for MeltRequest<Q>
where
    Q: Serialize + DeserializeOwned,
{
    fn from(request: MeltOnchainRequest<Q>) -> Self {
        MeltRequest::new(request.quote, request.inputs, request.outputs)
            .fee_index(request.fee_index)
    }
}

/// Fee option for an onchain melt quote.
///
/// Each item in an onchain melt quote's `fee_options` represents one
/// available fee reserve and confirmation estimate for the same payment. The wallet
/// selects one option when executing the quote by echoing its
/// `fee_index` value in the melt request.
///
/// The mint enforces these NUT rules on the `fee_options` list as a whole:
///
/// - MUST return at least one item.
/// - MUST NOT contain two items with the same `fee_index`.
/// - The list is fixed for the lifetime of the quote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MeltQuoteOnchainFeeOption {
    /// Server-assigned identifier the wallet echoes back to select this option
    pub fee_index: u32,
    /// Maximum onchain transaction fee the mint may charge for this option
    pub fee_reserve: Amount,
    /// Estimated number of blocks until confirmation
    pub estimated_blocks: u32,
}

/// Melt quote onchain response
///
/// Response containing the onchain melt quote details.
/// The `POST /v1/melt/quote/onchain` endpoint returns one quote with one or
/// more `fee_options`. The wallet chooses one option when executing the quote.
///
/// `deny_unknown_fields` is intentional: the `NotificationPayload` enum is
/// `#[serde(untagged)]` and melt-quote responses for different methods share
/// many field names. Rejecting unknown fields ensures an onchain payload cannot
/// silently deserialize as `MeltQuoteBolt11Response` (which carries `fee_reserve`
/// at the top level, while onchain carries it inside `fee_options`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
#[serde(deny_unknown_fields)]
pub struct MeltQuoteOnchainResponse<Q> {
    /// Quote Id
    pub quote: Q,
    /// Amount to be melted
    pub amount: Amount,
    /// Unit
    pub unit: CurrencyUnit,
    /// Payment method
    #[serde(default = "default_onchain_method")]
    pub method: PaymentMethod,
    /// Quote state
    pub state: MeltQuoteState,
    /// Unix timestamp until the quote is valid
    pub expiry: u64,
    /// Bitcoin address to send to
    pub request: String,
    /// Fee options for the transaction.
    ///
    /// Each entry represents one fee-reserve/confirmation-target pair the mint is
    /// willing to honor for this quote. Per NUT the mint MUST return at
    /// least one entry; MUST NOT return multiple entries with the same
    /// `fee_index`; and the list is fixed for the lifetime of the quote.
    pub fee_options: Vec<MeltQuoteOnchainFeeOption>,
    /// Selected fee option index once the quote is executed
    pub selected_fee_index: Option<u32>,
    /// Transaction outpoint (txid:vout) once broadcast
    #[serde(default, deserialize_with = "deserialize_empty_string_as_none")]
    pub outpoint: Option<String>,
    /// Blind signatures for overpaid onchain fee reserve
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<Vec<BlindSignature>>,
}

impl<Q: ToString> MeltQuoteOnchainResponse<Q> {
    /// Convert the MeltQuoteOnchainResponse with a quote type Q to a String
    pub fn to_string_id(&self) -> MeltQuoteOnchainResponse<String> {
        MeltQuoteOnchainResponse {
            quote: self.quote.to_string(),
            amount: self.amount,
            unit: self.unit.clone(),
            method: self.method.clone(),
            state: self.state,
            expiry: self.expiry,
            request: self.request.clone(),
            fee_options: self.fee_options.clone(),
            selected_fee_index: self.selected_fee_index,
            outpoint: self.outpoint.clone(),
            change: self.change.clone(),
        }
    }
}

#[cfg(feature = "mint")]
impl From<MeltQuoteOnchainResponse<QuoteId>> for MeltQuoteOnchainResponse<String> {
    fn from(value: MeltQuoteOnchainResponse<QuoteId>) -> Self {
        Self {
            quote: value.quote.to_string(),
            amount: value.amount,
            unit: value.unit,
            method: value.method,
            state: value.state,
            expiry: value.expiry,
            request: value.request,
            fee_options: value.fee_options,
            selected_fee_index: value.selected_fee_index,
            outpoint: value.outpoint,
            change: value.change,
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
            amount: Amount::from(100000),
            unit: CurrencyUnit::Sat,
            method: PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Onchain),
            state: MeltQuoteState::Pending,
            expiry: 1701704757,
            request: "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".to_string(),
            fee_options: vec![MeltQuoteOnchainFeeOption {
                fee_index: 0,
                fee_reserve: Amount::from(5000),
                estimated_blocks: 1,
            }],
            selected_fee_index: Some(0),
            outpoint: Some(
                "3b7f3b85c5f1a3c4d2b8e9f6a7c5d8e9f1a2b3c4d5e6f7a8b9c1d2e3f4a5b6c7:2".to_string(),
            ),
            change: None,
        };

        let serialized = serde_json::to_string(&response).unwrap();
        assert!(serialized.contains("\"fee_reserve\""));
        assert!(serialized.contains("\"fee_index\""));
        assert!(!serialized.contains("\"fee\":"));

        let deserialized: MeltQuoteOnchainResponse<String> =
            serde_json::from_str(&serialized).unwrap();

        assert_eq!(response.quote, deserialized.quote);
        assert_eq!(response.request, deserialized.request);
        assert_eq!(response.amount, deserialized.amount);
        assert_eq!(response.method, deserialized.method);
        assert_eq!(response.fee_options, deserialized.fee_options);
        assert_eq!(response.selected_fee_index, deserialized.selected_fee_index);
        assert_eq!(response.state, deserialized.state);
        assert_eq!(response.outpoint, deserialized.outpoint);
        assert_eq!(response.change, deserialized.change);
    }

    #[test]
    fn test_melt_quote_onchain_response_defaults_method() {
        let value = serde_json::json!({
            "quote": "TRmjduhIsPxd...",
            "amount": 100000,
            "unit": "sat",
            "state": "PENDING",
            "expiry": 1701704757,
            "request": "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
            "fee_options": [
                {
                    "fee_index": 0,
                    "fee_reserve": 5000,
                    "estimated_blocks": 1
                }
            ],
            "selected_fee_index": 0
        });

        let decoded: MeltQuoteOnchainResponse<String> =
            serde_json::from_value(value).expect("deserialize response");
        assert_eq!(decoded.method, PaymentMethod::Known(KnownMethod::Onchain));
    }

    #[test]
    fn test_melt_quote_onchain_response_serializes_null_outpoint() {
        let response: MeltQuoteOnchainResponse<String> = MeltQuoteOnchainResponse {
            quote: "TRmjduhIsPxd...".to_string(),
            amount: Amount::from(100000),
            unit: CurrencyUnit::Sat,
            method: PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Onchain),
            state: MeltQuoteState::Pending,
            expiry: 1701704757,
            request: "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".to_string(),
            fee_options: vec![MeltQuoteOnchainFeeOption {
                fee_index: 0,
                fee_reserve: Amount::from(5000),
                estimated_blocks: 1,
            }],
            selected_fee_index: None,
            outpoint: None,
            change: None,
        };

        let serialized = serde_json::to_string(&response).unwrap();
        assert!(serialized.contains("\"outpoint\":null"));

        let deserialized: MeltQuoteOnchainResponse<String> =
            serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.outpoint, None);
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
            method: PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Onchain),
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
    fn test_mint_quote_onchain_response_defaults_method() {
        let value = serde_json::json!({
            "quote": "DSGLX9kevM...",
            "request": "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
            "unit": "sat",
            "expiry": 1701704757,
            "pubkey": "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
            "amount_paid": 100000,
            "amount_issued": 0
        });

        let decoded: MintQuoteOnchainResponse<String> =
            serde_json::from_value(value).expect("deserialize response");
        assert_eq!(decoded.method, PaymentMethod::Known(KnownMethod::Onchain));
    }

    #[test]
    fn test_melt_quote_onchain_response_to_string_id() {
        use crate::nuts::nut00::CurrencyUnit;
        use crate::Amount;

        let response: MeltQuoteOnchainResponse<String> = MeltQuoteOnchainResponse {
            quote: "TRmjduhIsPxd...".to_string(),
            amount: Amount::from(100000),
            unit: CurrencyUnit::Sat,
            method: PaymentMethod::Known(crate::nuts::nut00::KnownMethod::Onchain),
            state: MeltQuoteState::Pending,
            expiry: 1701704757,
            request: "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".to_string(),
            fee_options: vec![MeltQuoteOnchainFeeOption {
                fee_index: 0,
                fee_reserve: Amount::from(5000),
                estimated_blocks: 1,
            }],
            selected_fee_index: Some(0),
            outpoint: Some(
                "3b7f3b85c5f1a3c4d2b8e9f6a7c5d8e9f1a2b3c4d5e6f7a8b9c1d2e3f4a5b6c7:2".to_string(),
            ),
            change: None,
        };

        let string_id_response = response.to_string_id();
        assert_eq!(string_id_response.quote, "TRmjduhIsPxd...");
    }
}
