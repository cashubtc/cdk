//! Mint Types

use lightning::offers::offer::Offer;
use lightning_invoice::Bolt11Invoice;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{CurrencyUnit, PaymentMethod, PublicKey};
use crate::mint_url::MintUrl;
use crate::nuts::{MeltQuoteState, MintQuoteState};
use crate::Amount;

/// Mint Quote Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintQuote {
    /// Quote id
    pub id: String,
    /// Mint Url
    pub mint_url: MintUrl,
    /// Amount of quote
    pub amount: Option<Amount>,
    /// Payment Method
    #[serde(default)]
    pub payment_method: PaymentMethod,
    /// Unit of quote
    pub unit: CurrencyUnit,
    /// Quote payment request e.g. bolt11
    pub request: String,
    /// Quote state
    pub state: MintQuoteState,
    /// Expiration time of quote
    pub expiry: u64,
    /// Value used by ln backend to look up state of request
    pub request_lookup_id: String,
    /// Amount paid
    #[serde(default)]
    pub amount_paid: Amount,
    /// Amount issued
    #[serde(default)]
    pub amount_issued: Amount,
    /// Single use
    #[serde(default)]
    pub single_use: bool,
    /// Payment of payment(s) that filled quote
    #[serde(default)]
    pub payment_ids: Vec<String>,
    /// Pubkey
    pub pubkey: Option<PublicKey>,
}

impl MintQuote {
    /// Create new [`MintQuote`]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mint_url: MintUrl,
        request: String,
        payment_method: PaymentMethod,
        unit: CurrencyUnit,
        amount: Option<Amount>,
        expiry: u64,
        request_lookup_id: String,
        amount_paid: Amount,
        amount_issued: Amount,
        single_use: bool,
        payment_ids: Vec<String>,
        pubkey: Option<PublicKey>,
    ) -> Self {
        let id = Uuid::new_v4();

        Self {
            mint_url,
            id: id.to_string(),
            amount,
            payment_method,
            unit,
            request,
            state: MintQuoteState::Unpaid,
            expiry,
            request_lookup_id,
            amount_paid,
            amount_issued,
            single_use,
            payment_ids,
            pubkey,
        }
    }
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
    pub request: PaymentRequest,
    /// Quote fee reserve
    pub fee_reserve: Amount,
    /// Quote state
    pub state: MeltQuoteState,
    /// Expiration time of quote
    pub expiry: u64,
    /// Payment preimage
    pub payment_preimage: Option<String>,
    /// Value used by ln backend to look up state of request
    pub request_lookup_id: String,
}

impl MeltQuote {
    /// Create new [`MeltQuote`]
    pub fn new(
        request: PaymentRequest,
        unit: CurrencyUnit,
        amount: Amount,
        fee_reserve: Amount,
        expiry: u64,
        request_lookup_id: String,
    ) -> Self {
        let id = Uuid::new_v4();

        Self {
            id: id.to_string(),
            amount,
            unit,
            request,
            fee_reserve,
            state: MeltQuoteState::Unpaid,
            expiry,
            payment_preimage: None,
            request_lookup_id,
        }
    }
}

/// Payment request
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaymentRequest {
    /// Bolt11 Payment
    Bolt11 {
        /// Bolt11 invoice
        bolt11: Bolt11Invoice,
    },
    /// Bolt12 Payment
    Bolt12 {
        /// Offer
        #[serde(with = "offer_serde")]
        offer: Box<Offer>,
        /// Invoice
        invoice: Option<String>,
    },
}

mod offer_serde {
    use std::str::FromStr;

    use serde::{self, Deserialize, Deserializer, Serializer};

    use super::Offer;

    pub fn serialize<S>(offer: &Offer, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = offer.to_string();
        serializer.serialize_str(&s)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Box<Offer>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Box::new(Offer::from_str(&s).map_err(|_| {
            serde::de::Error::custom("Invalid Bolt12 Offer")
        })?))
    }
}
