//! Bolt12
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{CurrencyUnit, MeltOptions, PaymentMethod, PublicKey};
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
pub type MeltQuoteBolt12Response<Q> = crate::nuts::nut23::MeltQuoteBolt11Response<Q>;

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
}
