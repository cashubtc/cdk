//! NUT-23: Mint Tokens via Bolt12
//!
//! <https://github.com/cashubtc/nuts/blob/main/23.md>

use serde::{Deserialize, Serialize};
use thiserror::Error;
#[cfg(feature = "mint")]
use uuid::Uuid;

use super::nut00::CurrencyUnit;
use super::{PaymentMethod, PublicKey};
use crate::Amount;

/// NUT23 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Unknown Quote State
    #[error("Unknown Quote State")]
    UnknownState,
    /// Amount overflow
    #[error("Amount overflow")]
    AmountOverflow,
    /// Publickey not defined
    #[error("Publickey not defined")]
    PublickeyUndefined,
}

/// Mint quote request [NUT-23]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MintQuoteBolt12Request {
    /// Amount
    pub amount: Option<Amount>,
    /// Unit wallet would like to pay with
    pub unit: CurrencyUnit,
    /// Memo to create the invoice with
    pub description: Option<String>,
    /// Single use
    pub single_use: bool,
    /// Expiry
    pub expiry: Option<u64>,
    /// Pubkey
    pub pubkey: PublicKey,
}

/// Mint quote response [NUT-23]
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
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
    /// Unix timestamp until the quote is valid
    pub expiry: Option<u64>,
    /// Pubkey
    pub pubkey: PublicKey,
    /// Single use
    pub single_use: bool,
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
            expiry: self.expiry,
            pubkey: self.pubkey,
            single_use: self.single_use,
            amount_paid: self.amount_paid,
            amount_issued: self.amount_issued,
        }
    }
}

#[cfg(feature = "mint")]
impl From<MintQuoteBolt12Response<Uuid>> for MintQuoteBolt12Response<String> {
    fn from(value: MintQuoteBolt12Response<Uuid>) -> Self {
        Self {
            quote: value.quote.to_string(),
            request: value.request,
            single_use: value.single_use,
            expiry: value.expiry,
            amount_paid: value.amount_paid,
            amount_issued: value.amount_issued,
            pubkey: value.pubkey,
            amount: value.amount,
            unit: value.unit,
        }
    }
}

/// Mint Method Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MintBolt12MethodSettings {
    /// Payment Method e.g. bolt11
    pub method: PaymentMethod,
    /// Currency Unit e.g. sat
    pub unit: CurrencyUnit,
    /// Min Amount
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_amount: Option<Amount>,
    /// Max Amount
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_amount: Option<Amount>,
    /// Quote Description
    #[serde(default)]
    pub description: bool,
    /// Max Expiry
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_expiry: Option<u64>,
}

/// Mint Settings
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema), schema(as = nut04::Settings))]
pub struct Settings {
    /// Methods to mint
    pub methods: Vec<MintBolt12MethodSettings>,
    /// Minting disabled
    pub disabled: bool,
}

impl Settings {
    /// Create new [`Settings`]
    pub fn new(methods: Vec<MintBolt12MethodSettings>, disabled: bool) -> Self {
        Self { methods, disabled }
    }

    /// Get [`MintMethodSettings`] for unit method pair
    pub fn get_settings(
        &self,
        unit: &CurrencyUnit,
        method: &PaymentMethod,
    ) -> Option<MintBolt12MethodSettings> {
        for method_settings in self.methods.iter() {
            if method_settings.method.eq(method) && method_settings.unit.eq(unit) {
                return Some(method_settings.clone());
            }
        }

        None
    }

    /// Remove [`MintMethodSettings`] for unit method pair
    pub fn remove_settings(
        &mut self,
        unit: &CurrencyUnit,
        method: &PaymentMethod,
    ) -> Option<MintBolt12MethodSettings> {
        self.methods
            .iter()
            .position(|settings| &settings.method == method && &settings.unit == unit)
            .map(|index| self.methods.remove(index))
    }
}
