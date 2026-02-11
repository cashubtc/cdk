//! NUT-05: Melting Tokens
//!
//! <https://github.com/cashubtc/nuts/blob/main/05.md>

use std::fmt;
use std::str::FromStr;

use serde::de::{self, DeserializeOwned, Deserializer, MapAccess, Visitor};
use serde::ser::{SerializeStruct, Serializer};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::nut00::{BlindSignature, BlindedMessage, CurrencyUnit, PaymentMethod, Proofs};
use super::ProofsMethods;
use crate::nut00::KnownMethod;
#[cfg(feature = "mint")]
use crate::quote_id::QuoteId;
use crate::Amount;

/// NUT05 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Unknown Quote State
    #[error("Unknown quote state")]
    UnknownState,
    /// Amount overflow
    #[error("Amount Overflow")]
    AmountOverflow,
    /// Unsupported unit
    #[error("Unsupported unit")]
    UnsupportedUnit,
    /// Invalid quote id
    #[error("Invalid quote id")]
    InvalidQuote,
}

/// Possible states of a quote
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema), schema(as = MeltQuoteState))]
pub enum QuoteState {
    /// Quote has not been paid
    #[default]
    Unpaid,
    /// Quote has been paid
    Paid,
    /// Paying quote is in progress
    Pending,
    /// Unknown state
    Unknown,
    /// Failed
    Failed,
}

impl fmt::Display for QuoteState {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Unpaid => write!(f, "UNPAID"),
            Self::Paid => write!(f, "PAID"),
            Self::Pending => write!(f, "PENDING"),
            Self::Unknown => write!(f, "UNKNOWN"),
            Self::Failed => write!(f, "FAILED"),
        }
    }
}

impl FromStr for QuoteState {
    type Err = Error;

    fn from_str(state: &str) -> Result<Self, Self::Err> {
        match state {
            "PENDING" => Ok(Self::Pending),
            "PAID" => Ok(Self::Paid),
            "UNPAID" => Ok(Self::Unpaid),
            "UNKNOWN" => Ok(Self::Unknown),
            "FAILED" => Ok(Self::Failed),
            _ => Err(Error::UnknownState),
        }
    }
}

/// Melt Bolt11 Request [NUT-05]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MeltRequest<Q> {
    /// Quote ID
    quote: Q,
    /// Proofs
    #[cfg_attr(feature = "swagger", schema(value_type = Vec<crate::Proof>))]
    inputs: Proofs,
    /// Blinded Message that can be used to return change [NUT-08]
    /// Amount field of BlindedMessages `SHOULD` be set to zero
    outputs: Option<Vec<BlindedMessage>>,
    /// Whether the client prefers asynchronous processing
    #[serde(default)]
    #[cfg_attr(feature = "swagger", schema(value_type = bool))]
    prefer_async: bool,
}

#[cfg(feature = "mint")]
impl TryFrom<MeltRequest<String>> for MeltRequest<QuoteId> {
    type Error = Error;

    fn try_from(value: MeltRequest<String>) -> Result<Self, Self::Error> {
        Ok(Self {
            quote: QuoteId::from_str(&value.quote).map_err(|_e| Error::InvalidQuote)?,
            inputs: value.inputs,
            outputs: value.outputs,
            prefer_async: value.prefer_async,
        })
    }
}

// Basic implementation without trait bounds
impl<Q> MeltRequest<Q> {
    /// Quote Id
    pub fn quote_id(&self) -> &Q {
        &self.quote
    }

    /// Get inputs (proofs)
    pub fn inputs(&self) -> &Proofs {
        &self.inputs
    }

    /// Get mutable inputs (proofs)
    pub fn inputs_mut(&mut self) -> &mut Proofs {
        &mut self.inputs
    }

    /// Get outputs (blinded messages for change)
    pub fn outputs(&self) -> &Option<Vec<BlindedMessage>> {
        &self.outputs
    }
}

impl<Q> MeltRequest<Q>
where
    Q: Serialize + DeserializeOwned,
{
    /// Create new [`MeltRequest`]
    pub fn new(quote: Q, inputs: Proofs, outputs: Option<Vec<BlindedMessage>>) -> Self {
        Self {
            quote,
            inputs: inputs.without_dleqs(),
            outputs,
            prefer_async: false,
        }
    }

    /// Set the prefer_async flag for asynchronous processing
    pub fn prefer_async(mut self, prefer_async: bool) -> Self {
        self.prefer_async = prefer_async;
        self
    }

    /// Get the prefer_async flag
    pub fn is_prefer_async(&self) -> bool {
        self.prefer_async
    }

    /// Get quote
    pub fn quote(&self) -> &Q {
        &self.quote
    }

    /// Total [`Amount`] of [`Proofs`]
    pub fn inputs_amount(&self) -> Result<Amount, Error> {
        Amount::try_sum(self.inputs.iter().map(|proof| proof.amount))
            .map_err(|_| Error::AmountOverflow)
    }
}

impl<Q> super::nut10::SpendingConditionVerification for MeltRequest<Q>
where
    Q: std::fmt::Display,
{
    fn inputs(&self) -> &Proofs {
        &self.inputs
    }

    fn sig_all_msg_to_sign(&self) -> String {
        let mut msg = String::new();

        // Add all input secrets and C values in order
        // msg = secret_0 || C_0 || ... || secret_n || C_n
        for proof in &self.inputs {
            msg.push_str(&proof.secret.to_string());
            msg.push_str(&proof.c.to_hex());
        }

        // Add all output amounts and B_ values in order (if any)
        // msg = ... || amount_0 || B_0 || ... || amount_m || B_m
        if let Some(outputs) = &self.outputs {
            for output in outputs {
                msg.push_str(&output.amount.to_string());
                msg.push_str(&output.blinded_secret.to_hex());
            }
        }

        // Add quote ID
        // msg = ... || quote_id
        msg.push_str(&self.quote.to_string());

        msg
    }
}

/// Melt Method Settings
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MeltMethodSettings {
    /// Payment Method e.g. bolt11
    pub method: PaymentMethod,
    /// Currency Unit e.g. sat
    pub unit: CurrencyUnit,
    /// Min Amount
    pub min_amount: Option<Amount>,
    /// Max Amount
    pub max_amount: Option<Amount>,
    /// Options
    pub options: Option<MeltMethodOptions>,
}

impl Serialize for MeltMethodSettings {
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

        let mut amountless_in_top_level = false;
        if let Some(MeltMethodOptions::Bolt11 { amountless }) = &self.options {
            if *amountless {
                num_fields += 1;
                amountless_in_top_level = true;
            }
        }

        let mut state = serializer.serialize_struct("MeltMethodSettings", num_fields)?;

        state.serialize_field("method", &self.method)?;
        state.serialize_field("unit", &self.unit)?;

        if let Some(min_amount) = &self.min_amount {
            state.serialize_field("min_amount", min_amount)?;
        }

        if let Some(max_amount) = &self.max_amount {
            state.serialize_field("max_amount", max_amount)?;
        }

        // If there's an amountless flag in Bolt11 options, add it at the top level
        if amountless_in_top_level {
            state.serialize_field("amountless", &true)?;
        }

        state.end()
    }
}

struct MeltMethodSettingsVisitor;

impl<'de> Visitor<'de> for MeltMethodSettingsVisitor {
    type Value = MeltMethodSettings;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a MeltMethodSettings structure")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut method: Option<PaymentMethod> = None;
        let mut unit: Option<CurrencyUnit> = None;
        let mut min_amount: Option<Amount> = None;
        let mut max_amount: Option<Amount> = None;
        let mut amountless: Option<bool> = None;

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
                "amountless" => {
                    if amountless.is_some() {
                        return Err(de::Error::duplicate_field("amountless"));
                    }
                    amountless = Some(map.next_value()?);
                }
                "options" => {
                    // If there are explicit options, they take precedence, except the amountless
                    // field which we will handle specially
                    let options: Option<MeltMethodOptions> = map.next_value()?;

                    if let Some(MeltMethodOptions::Bolt11 {
                        amountless: amountless_from_options,
                    }) = options
                    {
                        // If we already found a top-level amountless, use that instead
                        if amountless.is_none() {
                            amountless = Some(amountless_from_options);
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

        // Create options based on the method and the amountless flag
        let options = if method == PaymentMethod::Known(KnownMethod::Bolt11) && amountless.is_some()
        {
            amountless.map(|amountless| MeltMethodOptions::Bolt11 { amountless })
        } else {
            None
        };

        Ok(MeltMethodSettings {
            method,
            unit,
            min_amount,
            max_amount,
            options,
        })
    }
}

impl<'de> Deserialize<'de> for MeltMethodSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(MeltMethodSettingsVisitor)
    }
}

/// Mint Method settings options
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(untagged)]
pub enum MeltMethodOptions {
    /// Bolt11 Options
    Bolt11 {
        /// Mint supports paying bolt11 amountless
        amountless: bool,
    },
}

impl Settings {
    /// Create new [`Settings`]
    pub fn new(methods: Vec<MeltMethodSettings>, disabled: bool) -> Self {
        Self { methods, disabled }
    }

    /// Get [`MeltMethodSettings`] for unit method pair
    pub fn get_settings(
        &self,
        unit: &CurrencyUnit,
        method: &PaymentMethod,
    ) -> Option<MeltMethodSettings> {
        for method_settings in self.methods.iter() {
            if method_settings.method.eq(method) && method_settings.unit.eq(unit) {
                return Some(method_settings.clone());
            }
        }

        None
    }

    /// Remove [`MeltMethodSettings`] for unit method pair
    pub fn remove_settings(
        &mut self,
        unit: &CurrencyUnit,
        method: &PaymentMethod,
    ) -> Option<MeltMethodSettings> {
        self.methods
            .iter()
            .position(|settings| settings.method.eq(method) && settings.unit.eq(unit))
            .map(|index| self.methods.remove(index))
    }
}

/// Melt Settings
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema), schema(as = nut05::Settings))]
pub struct Settings {
    /// Methods to melt
    pub methods: Vec<MeltMethodSettings>,
    /// Minting disabled
    pub disabled: bool,
}

impl Settings {
    /// Supported nut05 methods
    pub fn supported_methods(&self) -> Vec<&PaymentMethod> {
        self.methods.iter().map(|a| &a.method).collect()
    }

    /// Supported nut05 units
    pub fn supported_units(&self) -> Vec<&CurrencyUnit> {
        self.methods.iter().map(|s| &s.unit).collect()
    }
}

/// Custom payment method melt quote request
///
/// This is a generic request type for melting tokens with custom payment methods.
///
/// The `extra` field allows payment-method-specific fields to be included
/// without being nested. When serialized, extra fields merge into the parent JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MeltQuoteCustomRequest {
    /// Custom payment method name
    pub method: String,
    /// Payment request string (method-specific format)
    pub request: String,
    /// Currency unit
    pub unit: CurrencyUnit,
    /// Extra payment-method-specific fields
    ///
    /// These fields are flattened into the JSON representation, allowing
    /// custom payment methods to include additional data.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Value::is_null")]
    #[cfg_attr(feature = "swagger", schema(value_type = Object, additional_properties = true))]
    pub extra: serde_json::Value,
}

/// Custom payment method melt quote response
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
///   "fee_reserve": 10,
///   "custom_field": "value"
/// }
/// ```
///
/// This separation enables proper validation layering: the mint verifies
/// well-defined fields (amount, fee_reserve, state, etc.) while passing extra
/// through to the gRPC payment processor for method-specific validation.
///
/// It also provides a clean upgrade path: when a payment method becomes speced,
/// its fields can be promoted from `extra` to well-defined struct fields without
/// breaking existing clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
#[serde(bound = "Q: Serialize + for<'a> Deserialize<'a>")]
pub struct MeltQuoteCustomResponse<Q> {
    /// Quote ID
    pub quote: Q,
    /// Amount to be melted
    pub amount: Amount,
    /// Fee reserve required
    pub fee_reserve: Amount,
    /// Quote State
    pub state: QuoteState,
    /// Unix timestamp until the quote is valid
    pub expiry: u64,
    /// Payment preimage (if payment completed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_preimage: Option<String>,
    /// Change (blinded signatures for overpaid amount)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<Vec<BlindSignature>>,
    /// Payment request (optional, for reference)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    /// Currency unit
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<CurrencyUnit>,
    /// Extra payment-method-specific fields
    ///
    /// These fields are flattened into the JSON representation, allowing
    /// custom payment methods to include additional data without nesting.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Value::is_null")]
    #[cfg_attr(feature = "swagger", schema(value_type = Object, additional_properties = true))]
    pub extra: serde_json::Value,
}

#[cfg(feature = "mint")]
impl<Q: ToString> MeltQuoteCustomResponse<Q> {
    /// Convert the MeltQuoteCustomResponse with a quote type Q to a String
    pub fn to_string_id(&self) -> MeltQuoteCustomResponse<String> {
        MeltQuoteCustomResponse {
            quote: self.quote.to_string(),
            amount: self.amount,
            fee_reserve: self.fee_reserve,
            state: self.state,
            expiry: self.expiry,
            payment_preimage: self.payment_preimage.clone(),
            change: self.change.clone(),
            request: self.request.clone(),
            unit: self.unit.clone(),
            extra: self.extra.clone(),
        }
    }
}

#[cfg(feature = "mint")]
impl From<MeltQuoteCustomResponse<QuoteId>> for MeltQuoteCustomResponse<String> {
    fn from(value: MeltQuoteCustomResponse<QuoteId>) -> Self {
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
    fn test_melt_method_settings_top_level_amountless() {
        // Create JSON with top-level amountless
        let json_str = r#"{
            "method": "bolt11",
            "unit": "sat",
            "min_amount": 0,
            "max_amount": 10000,
            "amountless": true
        }"#;

        // Deserialize it
        let settings: MeltMethodSettings = from_str(json_str).unwrap();

        // Check that amountless was correctly moved to options
        assert_eq!(settings.method, PaymentMethod::Known(KnownMethod::Bolt11));
        assert_eq!(settings.unit, CurrencyUnit::Sat);
        assert_eq!(settings.min_amount, Some(Amount::from(0)));
        assert_eq!(settings.max_amount, Some(Amount::from(10000)));

        match settings.options {
            Some(MeltMethodOptions::Bolt11 { amountless }) => {
                assert!(amountless);
            }
            _ => panic!("Expected Bolt11 options with amountless = true"),
        }

        // Serialize it back
        let serialized = to_string(&settings).unwrap();
        let parsed: serde_json::Value = from_str(&serialized).unwrap();

        // Verify the amountless is at the top level
        assert_eq!(parsed["amountless"], json!(true));
    }

    #[test]
    fn test_both_amountless_locations() {
        // Create JSON with amountless in both places (top level and in options)
        let json_str = r#"{
            "method": "bolt11",
            "unit": "sat",
            "min_amount": 0,
            "max_amount": 10000,
            "amountless": true,
            "options": {
                "amountless": false
            }
        }"#;

        // Deserialize it - top level should take precedence
        let settings: MeltMethodSettings = from_str(json_str).unwrap();

        match settings.options {
            Some(MeltMethodOptions::Bolt11 { amountless }) => {
                assert!(amountless, "Top-level amountless should take precedence");
            }
            _ => panic!("Expected Bolt11 options with amountless = true"),
        }
    }
}
