//! Bolt12
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{BlindSignature, CurrencyUnit, MeltOptions, MeltQuoteState, PaymentMethod, PublicKey};
#[cfg(feature = "mint")]
use crate::quote_id::QuoteId;
use crate::Amount;

fn default_bolt12_method() -> PaymentMethod {
    PaymentMethod::BOLT12
}

/// NUT18 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Unknown Quote State
    #[error("Unknown quote state")]
    UnknownState,
    /// Amount overflow
    #[error("Amount Overflow")]
    AmountOverflow,
    /// Publickey not defined
    #[error("Publickey not defined")]
    PublickeyUndefined,
}

/// Mint quote request [NUT-24]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuoteBolt12Request {
    /// Amount
    pub amount: Option<Amount>,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
    /// Memo to create the invoice with
    pub description: Option<String>,
    /// Pubkey
    pub pubkey: PublicKey,
}

/// Mint quote response [NUT-24]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "Q: Serialize + for<'a> Deserialize<'a>")]
pub struct MintQuoteBolt12Response<Q> {
    /// Quote Id
    pub quote: Q,
    /// Payment request to fulfil
    pub request: String,
    /// Amount
    pub amount: Option<Amount>,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
    /// Payment method
    #[serde(default = "default_bolt12_method")]
    pub method: PaymentMethod,
    /// Unix timestamp until the quote is valid
    pub expiry: Option<u64>,
    /// Pubkey
    pub pubkey: PublicKey,
    /// Amount that has been paid
    pub amount_paid: Amount,
    /// Amount that has been issued
    pub amount_issued: Amount,
    /// Unix timestamp indicating when the quote was last updated
    #[serde(default)]
    pub updated_at: u64,
}

#[cfg(feature = "mint")]
impl<Q: ToString> MintQuoteBolt12Response<Q> {
    /// Convert the MintQuote with a quote type Q to a String
    pub fn to_string_id(&self) -> MintQuoteBolt12Response<String> {
        MintQuoteBolt12Response {
            quote: self.quote.to_string(),
            request: self.request.clone(),
            amount: self.amount,
            unit: self.unit.clone(),
            method: self.method.clone(),
            expiry: self.expiry,
            pubkey: self.pubkey,
            amount_paid: self.amount_paid,
            amount_issued: self.amount_issued,
            updated_at: self.updated_at,
        }
    }
}

#[cfg(feature = "mint")]
impl From<MintQuoteBolt12Response<QuoteId>> for MintQuoteBolt12Response<String> {
    fn from(value: MintQuoteBolt12Response<QuoteId>) -> Self {
        Self {
            quote: value.quote.to_string(),
            request: value.request,
            expiry: value.expiry,
            amount_paid: value.amount_paid,
            amount_issued: value.amount_issued,
            pubkey: value.pubkey,
            amount: value.amount,
            unit: value.unit,
            method: value.method,
            updated_at: value.updated_at,
        }
    }
}

/// Melt quote request [NUT-18]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltQuoteBolt12Request {
    /// Bolt12 invoice to be paid
    pub request: String,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
    /// Payment Options
    pub options: Option<MeltOptions>,
}

/// Melt quote response [NUT-25]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MeltQuoteBolt12Response<Q> {
    /// Quote Id
    pub quote: Q,
    /// The amount that needs to be provided
    pub amount: Amount,
    /// The fee reserve that is required
    pub fee_reserve: Amount,
    /// Quote State
    pub state: MeltQuoteState,
    /// Unix timestamp until the quote is valid
    pub expiry: u64,
    /// Payment preimage
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_preimage: Option<String>,
    /// Change
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<Vec<BlindSignature>>,
    /// Payment request to fulfill
    // REVIEW: This is now required in the spec, we should remove the option once all mints update
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    /// Unit
    // REVIEW: This is now required in the spec, we should remove the option once all mints update
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<CurrencyUnit>,
    /// Payment method
    #[serde(default = "default_bolt12_method")]
    pub method: PaymentMethod,
}

impl<Q: ToString> MeltQuoteBolt12Response<Q> {
    /// Convert a `MeltQuoteBolt12Response` with type Q (generic/unknown) to a
    /// `MeltQuoteBolt12Response` with `String`
    pub fn to_string_id(self) -> MeltQuoteBolt12Response<String> {
        MeltQuoteBolt12Response {
            quote: self.quote.to_string(),
            amount: self.amount,
            fee_reserve: self.fee_reserve,
            state: self.state,
            expiry: self.expiry,
            payment_preimage: self.payment_preimage,
            change: self.change,
            request: self.request,
            unit: self.unit,
            method: self.method,
        }
    }
}

#[cfg(feature = "mint")]
impl From<MeltQuoteBolt12Response<QuoteId>> for MeltQuoteBolt12Response<String> {
    fn from(value: MeltQuoteBolt12Response<QuoteId>) -> Self {
        Self {
            quote: value.quote.to_string(),
            amount: value.amount,
            fee_reserve: value.fee_reserve,
            state: value.state,
            expiry: value.expiry,
            payment_preimage: value.payment_preimage,
            change: value.change,
            request: value.request,
            unit: value.unit,
            method: value.method,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{from_value, json, to_value};

    use super::*;
    use crate::nut00::KnownMethod;

    #[test]
    fn mint_quote_bolt12_response_serializes_method() {
        let response = MintQuoteBolt12Response {
            quote: "quote-id".to_string(),
            request: "lno1...".to_string(),
            amount: Some(Amount::from(10)),
            unit: CurrencyUnit::Sat,
            method: PaymentMethod::Known(KnownMethod::Bolt12),
            expiry: Some(1_701_704_757),
            pubkey: PublicKey::from_hex(
                "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
            )
            .expect("valid public key"),
            amount_paid: Amount::ZERO,
            amount_issued: Amount::ZERO,
            updated_at: 0,
        };

        let value = to_value(&response).expect("serialize response");
        assert_eq!(value["method"], json!("bolt12"));

        let decoded: MintQuoteBolt12Response<String> =
            from_value(value).expect("deserialize response");
        assert_eq!(decoded.method, PaymentMethod::Known(KnownMethod::Bolt12));
    }

    #[test]
    fn mint_quote_bolt12_response_defaults_method() {
        let value = json!({
            "quote": "quote-id",
            "request": "lno1...",
            "amount": 10,
            "unit": "sat",
            "expiry": 1_701_704_757,
            "pubkey": "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
            "amount_paid": 0,
            "amount_issued": 0
        });

        let decoded: MintQuoteBolt12Response<String> =
            from_value(value).expect("deserialize response");
        assert_eq!(decoded.method, PaymentMethod::Known(KnownMethod::Bolt12));
    }

    #[test]
    fn mint_quote_bolt12_response_tolerates_unknown_fields() {
        let value = json!({
            "quote": "quote-id",
            "request": "lno1...",
            "amount": 10,
            "unit": "sat",
            "expiry": 1_701_704_757,
            "pubkey": "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
            "amount_paid": 0,
            "amount_issued": 0,
            "future_extension": {
                "payjoin": "https://payjoin.example/pj"
            }
        });

        let decoded: MintQuoteBolt12Response<String> =
            from_value(value).expect("deserialize response");
        assert_eq!(decoded.quote, "quote-id");
        assert_eq!(decoded.method, PaymentMethod::Known(KnownMethod::Bolt12));
    }

    #[test]
    fn melt_quote_bolt12_response_serializes_method() {
        let response = MeltQuoteBolt12Response {
            quote: "quote-id".to_string(),
            amount: Amount::from(10),
            fee_reserve: Amount::from(2),
            state: MeltQuoteState::Unpaid,
            expiry: 1_701_704_757,
            payment_preimage: None,
            change: None,
            request: Some("lno1...".to_string()),
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::Known(KnownMethod::Bolt12),
        };

        let value = to_value(&response).expect("serialize response");
        assert_eq!(value["method"], json!("bolt12"));

        let decoded: MeltQuoteBolt12Response<String> =
            from_value(value).expect("deserialize response");
        assert_eq!(decoded.method, PaymentMethod::Known(KnownMethod::Bolt12));
    }

    #[test]
    fn melt_quote_bolt12_response_defaults_method() {
        let value = json!({
            "quote": "quote-id",
            "amount": 10,
            "fee_reserve": 2,
            "state": "UNPAID",
            "expiry": 1_701_704_757,
            "request": "lno1...",
            "unit": "sat"
        });

        let decoded: MeltQuoteBolt12Response<String> =
            from_value(value).expect("deserialize response");
        assert_eq!(decoded.method, PaymentMethod::Known(KnownMethod::Bolt12));
    }
}
