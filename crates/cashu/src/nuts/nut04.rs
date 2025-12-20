//! NUT-04: Mint Tokens via Bolt11
//!
//! <https://github.com/cashubtc/nuts/blob/main/04.md>

use std::fmt;
#[cfg(feature = "mint")]
use std::str::FromStr;

use serde::de::{self, DeserializeOwned, Deserializer, MapAccess, Visitor};
use serde::ser::{SerializeStruct, Serializer};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::nut00::{BlindSignature, BlindedMessage, CurrencyUnit, PaymentMethod};
use crate::nut23::QuoteState;
#[cfg(feature = "mint")]
use crate::quote_id::QuoteId;
#[cfg(feature = "mint")]
use crate::quote_id::QuoteIdError;
use crate::{Amount, PublicKey};

/// NUT04 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Unknown Quote State
    #[error("Unknown Quote State")]
    UnknownState,
    /// Amount overflow
    #[error("Amount overflow")]
    AmountOverflow,
}

/// Mint request [NUT-04]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MintRequest<Q> {
    /// Quote id
    #[cfg_attr(feature = "swagger", schema(max_length = 1_000))]
    pub quote: Q,
    /// Outputs
    #[cfg_attr(feature = "swagger", schema(max_items = 1_000))]
    pub outputs: Vec<BlindedMessage>,
    /// Signature
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

#[cfg(feature = "mint")]
impl TryFrom<MintRequest<String>> for MintRequest<QuoteId> {
    type Error = QuoteIdError;

    fn try_from(value: MintRequest<String>) -> Result<Self, Self::Error> {
        Ok(Self {
            quote: QuoteId::from_str(&value.quote)?,
            outputs: value.outputs,
            signature: value.signature,
        })
    }
}

impl<Q> MintRequest<Q> {
    /// Total [`Amount`] of outputs
    pub fn total_amount(&self) -> Result<Amount, Error> {
        Amount::try_sum(
            self.outputs
                .iter()
                .map(|BlindedMessage { amount, .. }| *amount),
        )
        .map_err(|_| Error::AmountOverflow)
    }
}

/// Mint response [NUT-04]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MintResponse {
    /// Blinded Signatures
    pub signatures: Vec<BlindSignature>,
}

/// Mint Method Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MintMethodSettings {
    /// Payment Method e.g. bolt11
    pub method: PaymentMethod,
    /// Currency Unit e.g. sat
    pub unit: CurrencyUnit,
    /// Min Amount
    pub min_amount: Option<Amount>,
    /// Max Amount
    pub max_amount: Option<Amount>,
    /// Options
    pub options: Option<MintMethodOptions>,
}

impl Serialize for MintMethodSettings {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut num_fields = 3; // method and unit are always present
        if self.min_amount.is_some() {
            num_fields += 1;
        }
        if self.max_amount.is_some() {
            num_fields += 1;
        }

        let mut description_in_top_level = false;
        if let Some(MintMethodOptions::Bolt11 { description }) = &self.options {
            if *description {
                num_fields += 1;
                description_in_top_level = true;
            }
        }

        let mut state = serializer.serialize_struct("MintMethodSettings", num_fields)?;

        state.serialize_field("method", &self.method)?;
        state.serialize_field("unit", &self.unit)?;

        if let Some(min_amount) = &self.min_amount {
            state.serialize_field("min_amount", min_amount)?;
        }

        if let Some(max_amount) = &self.max_amount {
            state.serialize_field("max_amount", max_amount)?;
        }

        // If there's a description flag in Bolt11 options, add it at the top level
        if description_in_top_level {
            state.serialize_field("description", &true)?;
        }

        state.end()
    }
}

struct MintMethodSettingsVisitor;

impl<'de> Visitor<'de> for MintMethodSettingsVisitor {
    type Value = MintMethodSettings;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a MintMethodSettings structure")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut method: Option<PaymentMethod> = None;
        let mut unit: Option<CurrencyUnit> = None;
        let mut min_amount: Option<Amount> = None;
        let mut max_amount: Option<Amount> = None;
        let mut description: Option<bool> = None;

        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "method" => {
                    if method.is_some() {
                        return Err(de::Error::duplicate_field("method"));
                    }
                    method = Some(map.next_value()?);
                }
                "unit" => {
                    if unit.is_some() {
                        return Err(de::Error::duplicate_field("unit"));
                    }
                    unit = Some(map.next_value()?);
                }
                "min_amount" => {
                    if min_amount.is_some() {
                        return Err(de::Error::duplicate_field("min_amount"));
                    }
                    min_amount = Some(map.next_value()?);
                }
                "max_amount" => {
                    if max_amount.is_some() {
                        return Err(de::Error::duplicate_field("max_amount"));
                    }
                    max_amount = Some(map.next_value()?);
                }
                "description" => {
                    if description.is_some() {
                        return Err(de::Error::duplicate_field("description"));
                    }
                    description = Some(map.next_value()?);
                }
                "options" => {
                    // If there are explicit options, they take precedence, except the description
                    // field which we will handle specially
                    let options: Option<MintMethodOptions> = map.next_value()?;

                    if let Some(MintMethodOptions::Bolt11 {
                        description: desc_from_options,
                    }) = options
                    {
                        // If we already found a top-level description, use that instead
                        if description.is_none() {
                            description = Some(desc_from_options);
                        }
                    }
                }
                _ => {
                    // Skip unknown fields
                    let _: serde::de::IgnoredAny = map.next_value()?;
                }
            }
        }

        let method = method.ok_or_else(|| de::Error::missing_field("method"))?;
        let unit = unit.ok_or_else(|| de::Error::missing_field("unit"))?;

        // Create options based on the method and the description flag
        let options = if method == "bolt11" {
            description.map(|description| MintMethodOptions::Bolt11 { description })
        } else {
            None
        };

        Ok(MintMethodSettings {
            method,
            unit,
            min_amount,
            max_amount,
            options,
        })
    }
}

impl<'de> Deserialize<'de> for MintMethodSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(MintMethodSettingsVisitor)
    }
}

/// Mint Method settings options
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(untagged)]
pub enum MintMethodOptions {
    /// Bolt11 Options
    Bolt11 {
        /// Mint supports setting bolt11 description
        description: bool,
    },
    /// Custom Options
    Custom {},
}

/// Mint Settings
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema), schema(as = nut04::Settings))]
pub struct Settings {
    /// Methods to mint
    pub methods: Vec<MintMethodSettings>,
    /// Minting disabled
    pub disabled: bool,
}

impl Settings {
    /// Create new [`Settings`]
    pub fn new(methods: Vec<MintMethodSettings>, disabled: bool) -> Self {
        Self { methods, disabled }
    }

    /// Get [`MintMethodSettings`] for unit method pair
    pub fn get_settings(
        &self,
        unit: &CurrencyUnit,
        method: &PaymentMethod,
    ) -> Option<MintMethodSettings> {
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
    ) -> Option<MintMethodSettings> {
        self.methods
            .iter()
            .position(|settings| &settings.method == method && &settings.unit == unit)
            .map(|index| self.methods.remove(index))
    }

    /// Supported nut04 methods
    pub fn supported_methods(&self) -> Vec<&PaymentMethod> {
        self.methods.iter().map(|a| &a.method).collect()
    }

    /// Supported nut04 units
    pub fn supported_units(&self) -> Vec<&CurrencyUnit> {
        self.methods.iter().map(|s| &s.unit).collect()
    }
}

/// Custom payment method mint quote request
///
/// This is a generic request type that works for any custom payment method.
/// The method name is provided in the URL path, not in the request body.
///
/// The `extra` field allows payment-method-specific fields to be included
/// without being nested. When serialized, extra fields merge into the parent JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MintQuoteCustomRequest {
    /// Amount to mint
    pub amount: Amount,
    /// Currency unit
    pub unit: CurrencyUnit,
    /// Optional description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// NUT-19 Pubkey
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pubkey: Option<PublicKey>,
    /// Extra payment-method-specific fields
    ///
    /// These fields are flattened into the JSON representation, allowing
    /// custom payment methods to include additional data (e.g., ehash share).
    /// This enables proper validation layering: the mint verifies well-defined
    /// fields while passing extra through to the payment processor.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Value::is_null")]
    #[cfg_attr(feature = "swagger", schema(value_type = Object, additional_properties = true))]
    pub extra: serde_json::Value,
}

/// Custom payment method mint quote response
///
/// This is a generic response type for custom payment methods.
///
/// The `extra` field allows payment-method-specific fields to be included
/// without being nested. When serialized, extra fields merge into the parent JSON:
/// ```json
/// {
///   "quote": "abc123",
///   "state": "UNPAID",
///   "amount": 1000,
///   "paypal_link": "https://paypal.me/merchant",
///   "paypal_email": "merchant@example.com"
/// }
/// ```
///
/// This separation enables proper validation layering: the mint verifies
/// well-defined fields (amount, unit, state, etc.) while passing extra through
/// to the gRPC payment processor for method-specific validation.
///
/// It also provides a clean upgrade path: when a payment method becomes speced,
/// its fields can be promoted from `extra` to well-defined struct fields without
/// breaking existing clients (e.g., bolt12's `amount_paid` and `amount_issued`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(bound = "Q: Serialize + for<'a> Deserialize<'a>")]
pub struct MintQuoteCustomResponse<Q> {
    /// Quote ID
    pub quote: Q,
    /// Payment request string (method-specific format)
    pub request: String,
    /// Amount
    pub amount: Option<Amount>,
    /// Currency unit
    pub unit: Option<CurrencyUnit>,
    /// Quote State
    pub state: QuoteState,
    /// Unix timestamp until the quote is valid
    pub expiry: Option<u64>,
    /// NUT-19 Pubkey
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pubkey: Option<PublicKey>,
    /// Extra payment-method-specific fields
    ///
    /// These fields are flattened into the JSON representation, allowing
    /// custom payment methods to include additional data without nesting.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Value::is_null")]
    #[cfg_attr(feature = "swagger", schema(value_type = Object, additional_properties = true))]
    pub extra: serde_json::Value,
}

#[cfg(feature = "mint")]
impl<Q: ToString> MintQuoteCustomResponse<Q> {
    /// Convert the MintQuoteCustomResponse with a quote type Q to a String
    pub fn to_string_id(&self) -> MintQuoteCustomResponse<String> {
        MintQuoteCustomResponse {
            quote: self.quote.to_string(),
            request: self.request.clone(),
            amount: self.amount,
            state: self.state,
            unit: self.unit.clone(),
            expiry: self.expiry,
            pubkey: self.pubkey,
            extra: self.extra.clone(),
        }
    }
}

#[cfg(feature = "mint")]
impl From<MintQuoteCustomResponse<QuoteId>> for MintQuoteCustomResponse<String> {
    fn from(value: MintQuoteCustomResponse<QuoteId>) -> Self {
        Self {
            quote: value.quote.to_string(),
            request: value.request,
            amount: value.amount,
            unit: value.unit,
            expiry: value.expiry,
            state: value.state,
            pubkey: value.pubkey,
            extra: value.extra,
        }
    }
}
#[cfg(test)]
mod tests {
    use serde_json::{from_str, json, to_string};

    use super::*;
    use crate::nut00::KnownMethod;

    #[test]
    fn test_mint_method_settings_top_level_description() {
        // Create JSON with top-level description
        let json_str = r#"{
            "method": "bolt11",
            "unit": "sat",
            "min_amount": 0,
            "max_amount": 10000,
            "description": true
        }"#;

        // Deserialize it
        let settings: MintMethodSettings = from_str(json_str).unwrap();

        // Check that description was correctly moved to options
        assert_eq!(settings.method, PaymentMethod::Known(KnownMethod::Bolt11));
        assert_eq!(settings.unit, CurrencyUnit::Sat);
        assert_eq!(settings.min_amount, Some(Amount::from(0)));
        assert_eq!(settings.max_amount, Some(Amount::from(10000)));

        match settings.options {
            Some(MintMethodOptions::Bolt11 { description }) => {
                assert!(description);
            }
            _ => panic!("Expected Bolt11 options with description = true"),
        }

        // Serialize it back
        let serialized = to_string(&settings).unwrap();
        let parsed: serde_json::Value = from_str(&serialized).unwrap();

        // Verify the description is at the top level
        assert_eq!(parsed["description"], json!(true));
    }

    #[test]
    fn test_both_description_locations() {
        // Create JSON with description in both places (top level and in options)
        let json_str = r#"{
            "method": "bolt11",
            "unit": "sat",
            "min_amount": 0,
            "max_amount": 10000,
            "description": true,
            "options": {
                "description": false
            }
        }"#;

        // Deserialize it - top level should take precedence
        let settings: MintMethodSettings = from_str(json_str).unwrap();

        match settings.options {
            Some(MintMethodOptions::Bolt11 { description }) => {
                assert!(description, "Top-level description should take precedence");
            }
            _ => panic!("Expected Bolt11 options with description = true"),
        }
    }
}
