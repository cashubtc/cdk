//! Bolt11

use std::fmt;
use std::str::FromStr;

use lightning_invoice::Bolt11Invoice;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{BlindSignature, CurrencyUnit, MeltQuoteState, Mpp, PaymentMethod, PublicKey};
#[cfg(feature = "mint")]
use crate::quote_id::QuoteId;
use crate::util::serde_helpers::deserialize_empty_string_as_none;
use crate::Amount;

fn default_bolt11_method() -> PaymentMethod {
    PaymentMethod::BOLT11
}

/// NUT023 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Unknown Quote State
    #[error("Unknown Quote State")]
    UnknownState,
    /// Amount overflow
    #[error("Amount overflow")]
    AmountOverflow,
    /// Invalid Amount
    #[error("Invalid Request")]
    InvalidAmountRequest,
}

/// Mint quote request [NUT-04]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuoteBolt11Request {
    /// Amount
    pub amount: Amount,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
    /// Memo to create the invoice with
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// NUT-19 Pubkey
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pubkey: Option<PublicKey>,
}

/// Possible states of a quote
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum QuoteState {
    /// Quote has not been paid
    #[default]
    Unpaid,
    /// Quote has been paid and wallet can mint
    Paid,
    /// ecash issued for quote
    Issued,
}

impl fmt::Display for QuoteState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Unpaid => write!(f, "UNPAID"),
            Self::Paid => write!(f, "PAID"),
            Self::Issued => write!(f, "ISSUED"),
        }
    }
}

impl FromStr for QuoteState {
    type Err = Error;

    fn from_str(state: &str) -> Result<Self, Self::Err> {
        match state {
            "PAID" => Ok(Self::Paid),
            "UNPAID" => Ok(Self::Unpaid),
            "ISSUED" => Ok(Self::Issued),
            _ => Err(Error::UnknownState),
        }
    }
}

/// Mint quote response [NUT-04]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MintQuoteBolt11Response<Q> {
    /// Quote Id
    pub quote: Q,
    /// Payment request to fulfil
    pub request: String,
    /// Amount
    // REVIEW: This is now required in the spec, we should remove the option once all mints update
    pub amount: Option<Amount>,
    /// Unit
    // REVIEW: This is now required in the spec, we should remove the option once all mints update
    pub unit: Option<CurrencyUnit>,
    /// Payment method
    #[serde(default = "default_bolt11_method")]
    pub method: PaymentMethod,
    /// Amount that has been paid
    #[serde(default)]
    pub amount_paid: Amount,
    /// Amount that has been issued
    #[serde(default)]
    pub amount_issued: Amount,
    /// Unix timestamp indicating when the quote was last updated
    #[serde(default)]
    pub updated_at: u64,
    /// Quote State
    #[serde(default)]
    pub state: QuoteState,
    /// Unix timestamp until the quote is valid
    pub expiry: Option<u64>,
    /// NUT-19 Pubkey
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_empty_string_as_none"
    )]
    pub pubkey: Option<PublicKey>,
}
impl<Q: ToString> MintQuoteBolt11Response<Q> {
    /// Convert the MintQuote with a quote type Q to a String
    pub fn to_string_id(&self) -> MintQuoteBolt11Response<String> {
        MintQuoteBolt11Response {
            quote: self.quote.to_string(),
            request: self.request.clone(),
            state: self.state,
            expiry: self.expiry,
            pubkey: self.pubkey,
            amount: self.amount,
            unit: self.unit.clone(),
            method: self.method.clone(),
            amount_paid: self.amount_paid,
            amount_issued: self.amount_issued,
            updated_at: self.updated_at,
        }
    }
}

#[cfg(feature = "mint")]
impl From<MintQuoteBolt11Response<QuoteId>> for MintQuoteBolt11Response<String> {
    fn from(value: MintQuoteBolt11Response<QuoteId>) -> Self {
        Self {
            quote: value.quote.to_string(),
            request: value.request,
            state: value.state,
            expiry: value.expiry,
            pubkey: value.pubkey,
            amount: value.amount,
            unit: value.unit.clone(),
            method: value.method,
            amount_paid: value.amount_paid,
            amount_issued: value.amount_issued,
            updated_at: value.updated_at,
        }
    }
}

/// BOLT11 melt quote request [NUT-23]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltQuoteBolt11Request {
    /// Bolt11 invoice to be paid
    pub request: Bolt11Invoice,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
    /// Payment Options
    pub options: Option<MeltOptions>,
}

/// Melt Options
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MeltOptions {
    /// Mpp Options
    Mpp {
        /// MPP
        mpp: Mpp,
    },
    /// Amountless options
    Amountless {
        /// Amountless
        amountless: Amountless,
    },
}

impl MeltOptions {
    /// Create new [`MeltOptions::Mpp`]
    pub fn new_mpp<A>(amount: A) -> Self
    where
        A: Into<Amount>,
    {
        Self::Mpp {
            mpp: Mpp {
                amount: amount.into(),
            },
        }
    }

    /// Create new [`MeltOptions::Amountless`]
    pub fn new_amountless<A>(amount_msat: A) -> Self
    where
        A: Into<Amount>,
    {
        Self::Amountless {
            amountless: Amountless {
                amount_msat: amount_msat.into(),
            },
        }
    }

    /// Payment amount
    pub fn amount_msat(&self) -> Amount {
        match self {
            Self::Mpp { mpp } => mpp.amount,
            Self::Amountless { amountless } => amountless.amount_msat,
        }
    }
}

/// Amountless payment
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Amountless {
    /// Amount to pay in msat
    pub amount_msat: Amount,
}

impl MeltQuoteBolt11Request {
    /// Amount from [`MeltQuoteBolt11Request`]
    ///
    /// Amount can either be defined in the bolt11 invoice,
    /// in the request for an amountless bolt11 or in MPP option.
    pub fn amount_msat(&self) -> Result<Amount, Error> {
        let MeltQuoteBolt11Request {
            request, options, ..
        } = self;

        match options {
            None => Ok(request
                .amount_milli_satoshis()
                .ok_or(Error::InvalidAmountRequest)?
                .into()),
            Some(MeltOptions::Mpp { mpp }) => Ok(mpp.amount),
            Some(MeltOptions::Amountless { amountless }) => {
                let amount = amountless.amount_msat;
                if let Some(amount_msat) = request.amount_milli_satoshis() {
                    if amount != amount_msat.into() {
                        return Err(Error::InvalidAmountRequest);
                    }
                }
                Ok(amount)
            }
        }
    }
}

/// Melt quote response [NUT-05]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MeltQuoteBolt11Response<Q> {
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
    #[serde(default = "default_bolt11_method")]
    pub method: PaymentMethod,
}

impl<Q: ToString> MeltQuoteBolt11Response<Q> {
    /// Convert a `MeltQuoteBolt11Response` with type Q (generic/unknown) to a
    /// `MeltQuoteBolt11Response` with `String`
    pub fn to_string_id(self) -> MeltQuoteBolt11Response<String> {
        MeltQuoteBolt11Response {
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
impl From<MeltQuoteBolt11Response<QuoteId>> for MeltQuoteBolt11Response<String> {
    fn from(value: MeltQuoteBolt11Response<QuoteId>) -> Self {
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
    use std::str::FromStr;

    use serde_json::{from_value, json, to_value};

    use super::*;
    use crate::nut00::KnownMethod;

    const INVOICE_10_SATS: &str = "lnbc100n1p5z3a63pp56854ytysg7e5z9fl3w5mgvrlqjfcytnjv8ff5hm5qt6gl6alxesqdqqcqzzsxqyz5vqsp5p0x0dlhn27s63j4emxnk26p7f94u0lyarnfp5yqmac9gzy4ngdss9qxpqysgqne3v0hnzt2lp0hc69xpzckk0cdcar7glvjhq60lsrfe8gejdm8c564prrnsft6ctxxyrewp4jtezrq3gxxqnfjj0f9tw2qs9y0lslmqpfu7et9";

    fn request_with_options(options: Option<MeltOptions>) -> MeltQuoteBolt11Request {
        MeltQuoteBolt11Request {
            request: Bolt11Invoice::from_str(INVOICE_10_SATS).expect("valid bolt11 invoice"),
            unit: CurrencyUnit::Sat,
            options,
        }
    }

    #[test]
    fn quote_state_display_and_parse_cover_wire_values() {
        assert_eq!(QuoteState::Unpaid.to_string(), "UNPAID");
        assert_eq!(QuoteState::Paid.to_string(), "PAID");
        assert_eq!(QuoteState::Issued.to_string(), "ISSUED");

        assert_eq!(QuoteState::from_str("UNPAID").unwrap(), QuoteState::Unpaid);
        assert_eq!(QuoteState::from_str("PAID").unwrap(), QuoteState::Paid);
        assert_eq!(QuoteState::from_str("ISSUED").unwrap(), QuoteState::Issued);
        assert!(matches!(
            QuoteState::from_str("paid"),
            Err(Error::UnknownState)
        ));
    }

    #[test]
    fn melt_options_report_configured_amounts() {
        assert_eq!(
            MeltOptions::new_mpp(Amount::from(21)).amount_msat(),
            21.into()
        );
        assert_eq!(
            MeltOptions::new_amountless(Amount::from(34)).amount_msat(),
            34.into()
        );
    }

    #[test]
    fn melt_quote_amount_uses_invoice_or_options() {
        assert_eq!(
            request_with_options(None).amount_msat().unwrap(),
            10_000.into()
        );

        assert_eq!(
            request_with_options(Some(MeltOptions::new_mpp(Amount::from(7))))
                .amount_msat()
                .unwrap(),
            7.into()
        );

        assert_eq!(
            request_with_options(Some(MeltOptions::new_amountless(Amount::from(10_000))))
                .amount_msat()
                .unwrap(),
            10_000.into()
        );
    }

    #[test]
    fn amountless_quote_rejects_invoice_amount_mismatch() {
        let result = request_with_options(Some(MeltOptions::new_amountless(Amount::from(9_999))))
            .amount_msat();

        assert!(matches!(result, Err(Error::InvalidAmountRequest)));
    }

    #[test]
    fn mint_quote_bolt11_response_serializes_method() {
        let response = MintQuoteBolt11Response {
            quote: "quote-id".to_string(),
            request: "lnbc...".to_string(),
            amount: Some(Amount::from(10)),
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            amount_paid: Amount::ZERO,
            amount_issued: Amount::ZERO,
            updated_at: 0,
            state: QuoteState::Unpaid,
            expiry: Some(1_701_704_757),
            pubkey: None,
        };

        let value = to_value(&response).expect("serialize response");
        assert_eq!(value["method"], json!("bolt11"));

        let decoded: MintQuoteBolt11Response<String> =
            from_value(value).expect("deserialize response");
        assert_eq!(decoded.method, PaymentMethod::Known(KnownMethod::Bolt11));
    }

    #[test]
    fn mint_quote_bolt11_response_defaults_method() {
        let value = json!({
            "quote": "quote-id",
            "request": "lnbc...",
            "amount": 10,
            "unit": "sat",
            "state": "UNPAID",
            "expiry": 1_701_704_757
        });

        let decoded: MintQuoteBolt11Response<String> =
            from_value(value).expect("deserialize response");
        assert_eq!(decoded.method, PaymentMethod::Known(KnownMethod::Bolt11));
    }

    #[test]
    fn melt_quote_bolt11_response_serializes_method() {
        let response = MeltQuoteBolt11Response {
            quote: "quote-id".to_string(),
            amount: Amount::from(10),
            fee_reserve: Amount::from(2),
            state: MeltQuoteState::Unpaid,
            expiry: 1_701_704_757,
            payment_preimage: None,
            change: None,
            request: Some("lnbc...".to_string()),
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::Known(KnownMethod::Bolt11),
        };

        let value = to_value(&response).expect("serialize response");
        assert_eq!(value["method"], json!("bolt11"));

        let decoded: MeltQuoteBolt11Response<String> =
            from_value(value).expect("deserialize response");
        assert_eq!(decoded.method, PaymentMethod::Known(KnownMethod::Bolt11));
    }

    #[test]
    fn melt_quote_bolt11_response_defaults_method() {
        let value = json!({
            "quote": "quote-id",
            "amount": 10,
            "fee_reserve": 2,
            "state": "UNPAID",
            "expiry": 1_701_704_757,
            "request": "lnbc...",
            "unit": "sat"
        });

        let decoded: MeltQuoteBolt11Response<String> =
            from_value(value).expect("deserialize response");
        assert_eq!(decoded.method, PaymentMethod::Known(KnownMethod::Bolt11));
    }
}
