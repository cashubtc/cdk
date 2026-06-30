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
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MeltRequest<Q> {
    /// Quote ID
    quote: Q,
    /// Proofs
    inputs: Proofs,
    /// Blinded Message that can be used to return change [NUT-08]
    /// Amount field of BlindedMessages `SHOULD` be set to zero
    outputs: Option<Vec<BlindedMessage>>,
    /// Whether the client prefers asynchronous processing
    #[serde(default)]
    prefer_async: bool,
    /// Selected fee option index for onchain melts
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fee_index: Option<u32>,
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
            fee_index: value.fee_index,
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
            fee_index: None,
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

    /// Set the selected fee option index for onchain melts.
    pub fn fee_index(mut self, fee_index: u32) -> Self {
        self.fee_index = Some(fee_index);
        self
    }

    /// Get the selected fee option index for onchain melts.
    pub fn selected_fee_index(&self) -> Option<u32> {
        self.fee_index
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
pub struct MeltMethodSettings {
    /// Payment Method e.g. bolt11
    pub method: PaymentMethod,
    /// Currency Unit e.g. sat
    pub unit: CurrencyUnit,
    /// Human-readable name for the payment method.
    ///
    /// If null or omitted on the wire, wallets should derive it from `method`
    /// by replacing `_` and `-` with spaces and title-casing each word.
    pub method_name: Option<String>,
    /// Min Amount
    pub min_amount: Option<Amount>,
    /// Max Amount
    pub max_amount: Option<Amount>,
    /// Options
    pub options: Option<MeltMethodOptions>,
}

impl MeltMethodSettings {
    /// Human-readable payment method name.
    ///
    /// Returns the explicit `method_name` when present. If it is null or omitted,
    /// derives the name from `method` by replacing `_` and `-` with spaces and
    /// title-casing each word.
    pub fn method_name(&self) -> String {
        self.method_name
            .clone()
            .unwrap_or_else(|| self.method.derived_method_name())
    }
}

impl Serialize for MeltMethodSettings {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut num_fields = 2; // method and unit are always present
        if self.min_amount.is_some() {
            num_fields += 1;
        }
        if self.max_amount.is_some() {
            num_fields += 1;
        }
        if self.method_name.is_some() {
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

        if let Some(method_name) = &self.method_name {
            state.serialize_field("method_name", method_name)?;
        }

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
        let mut method_name: Option<String> = None;
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
                "method_name" => {
                    if method_name.is_some() {
                        return Err(de::Error::duplicate_field("method_name"));
                    }
                    method_name = map.next_value()?;
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
            method_name,
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
///   "method": "custom",
///   "state": "UNPAID",
///   "amount": 1000,
///   "fee_reserve": 10,
///   "custom_field": "value"
/// }
/// ```
///
/// This separation enables proper validation layering: the mint verifies
/// well-defined fields (amount, optional fee_reserve, state, etc.) while
/// passing extra through to the gRPC payment processor for method-specific validation.
///
/// It also provides a clean upgrade path: when a payment method becomes speced,
/// its fields can be promoted from `extra` to well-defined struct fields without
/// breaking existing clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "Q: Serialize + for<'a> Deserialize<'a>")]
pub struct MeltQuoteCustomResponse<Q> {
    /// Quote ID
    pub quote: Q,
    /// Payment method
    pub method: PaymentMethod,
    /// Amount to be melted
    pub amount: Amount,
    /// Fee reserve required, if provided
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fee_reserve: Option<Amount>,
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
    pub extra: serde_json::Value,
}

impl<Q: ToString> MeltQuoteCustomResponse<Q> {
    /// Convert the MeltQuoteCustomResponse with a quote type Q to a String
    pub fn to_string_id(&self) -> MeltQuoteCustomResponse<String> {
        MeltQuoteCustomResponse {
            quote: self.quote.to_string(),
            method: self.method.clone(),
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
            method: value.method,
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

    const MELT_REQUEST_JSON: &str = r#"{
        "quote": "quote-1",
        "inputs": [
            {
                "amount": 2,
                "id": "00bfa73302d12ffd",
                "secret": "[\"P2PK\",{\"nonce\":\"c7f280eb55c1e8564e03db06973e94bc9b666d9e1ca42ad278408fe625950303\",\"data\":\"030d8acedfe072c9fa449a1efe0817157403fbec460d8e79f957966056e5dd76c1\",\"tags\":[[\"sigflag\",\"SIG_ALL\"]]}]",
                "C": "02c97ee3d1db41cf0a3ddb601724be8711a032950811bf326f8219c50c4808d3cd"
            },
            {
                "amount": 4,
                "id": "00bfa73302d12ffd",
                "secret": "[\"P2PK\",{\"nonce\":\"d7f280eb55c1e8564e03db06973e94bc9b666d9e1ca42ad278408fe625950304\",\"data\":\"030d8acedfe072c9fa449a1efe0817157403fbec460d8e79f957966056e5dd76c1\",\"tags\":[[\"sigflag\",\"SIG_ALL\"]]}]",
                "C": "02c97ee3d1db41cf0a3ddb601724be8711a032950811bf326f8219c50c4808d3cd"
            }
        ],
        "outputs": [
            {
                "amount": 3,
                "id": "00bfa73302d12ffd",
                "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
            }
        ]
    }"#;

    #[test]
    fn test_quote_state_display_outputs_wire_values() {
        assert_eq!(QuoteState::Unpaid.to_string(), "UNPAID");
        assert_eq!(QuoteState::Paid.to_string(), "PAID");
        assert_eq!(QuoteState::Pending.to_string(), "PENDING");
        assert_eq!(QuoteState::Unknown.to_string(), "UNKNOWN");
        assert_eq!(QuoteState::Failed.to_string(), "FAILED");
    }

    #[test]
    fn test_quote_state_from_str_accepts_wire_values() {
        assert_eq!("UNPAID".parse::<QuoteState>().unwrap(), QuoteState::Unpaid);
        assert_eq!("PAID".parse::<QuoteState>().unwrap(), QuoteState::Paid);
        assert_eq!(
            "PENDING".parse::<QuoteState>().unwrap(),
            QuoteState::Pending
        );
        assert_eq!(
            "UNKNOWN".parse::<QuoteState>().unwrap(),
            QuoteState::Unknown
        );
        assert_eq!("FAILED".parse::<QuoteState>().unwrap(), QuoteState::Failed);
    }

    #[test]
    fn test_quote_state_from_str_rejects_unknown_value() {
        assert!(matches!(
            "unknown".parse::<QuoteState>(),
            Err(Error::UnknownState)
        ));
    }

    #[test]
    fn test_melt_request_inputs_outputs_and_amounts() {
        let req: MeltRequest<String> = from_str(MELT_REQUEST_JSON).unwrap();

        let inputs = req.inputs();
        assert_eq!(inputs.len(), 2, "expected 2 inputs");
        assert_eq!(u64::from(inputs[0].amount), 2);
        assert_eq!(u64::from(inputs[1].amount), 4);
        assert_eq!(req.inputs_amount().unwrap(), Amount::from(6));

        let outputs = req.outputs().as_ref().expect("expected change outputs");
        assert_eq!(outputs.len(), 1, "expected 1 output");
        assert_eq!(u64::from(outputs[0].amount), 3);
        assert_eq!(req.output_amount(), Some(Amount::from(3)));
    }

    #[test]
    fn test_melt_request_preserves_async_preference() {
        let req: MeltRequest<String> = from_str(MELT_REQUEST_JSON).unwrap();
        assert!(!req.is_prefer_async());

        assert!(req.prefer_async(true).is_prefer_async());
    }

    #[test]
    fn test_melt_request_preserves_selected_fee_index() {
        let req: MeltRequest<String> = from_str(MELT_REQUEST_JSON).unwrap();
        assert_eq!(req.selected_fee_index(), None);

        assert_eq!(req.fee_index(42).selected_fee_index(), Some(42));
    }

    #[test]
    fn test_melt_method_settings_cbor_roundtrip() {
        let settings = [
            MeltMethodSettings {
                method: PaymentMethod::Known(KnownMethod::Bolt11),
                unit: CurrencyUnit::Sat,
                method_name: None,
                min_amount: None,
                max_amount: None,
                options: None,
            },
            MeltMethodSettings {
                method: PaymentMethod::Known(KnownMethod::Bolt11),
                unit: CurrencyUnit::Sat,
                method_name: None,
                min_amount: Some(Amount::from(1)),
                max_amount: None,
                options: None,
            },
            MeltMethodSettings {
                method: PaymentMethod::Known(KnownMethod::Bolt11),
                unit: CurrencyUnit::Sat,
                method_name: None,
                min_amount: None,
                max_amount: Some(Amount::from(1000)),
                options: None,
            },
            MeltMethodSettings {
                method: PaymentMethod::Known(KnownMethod::Bolt11),
                unit: CurrencyUnit::Sat,
                method_name: Some("Lightning".to_string()),
                min_amount: None,
                max_amount: None,
                options: Some(MeltMethodOptions::Bolt11 { amountless: true }),
            },
        ];

        for settings in settings {
            let mut encoded = Vec::new();
            ciborium::into_writer(&settings, &mut encoded).expect("serialize settings as CBOR");

            let decoded: MeltMethodSettings =
                ciborium::from_reader(&encoded[..]).expect("deserialize settings from CBOR");

            assert_eq!(decoded, settings);
        }
    }

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
        assert_eq!(settings.method_name, None);
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

    #[test]
    fn test_melt_method_settings_options_amountless() {
        let json_str = r#"{
            "method": "bolt11",
            "unit": "sat",
            "options": {
                "amountless": true
            }
        }"#;

        let settings: MeltMethodSettings = from_str(json_str).unwrap();

        assert_eq!(
            settings.options,
            Some(MeltMethodOptions::Bolt11 { amountless: true })
        );
    }

    #[test]
    fn test_melt_method_settings_non_bolt11_ignores_amountless() {
        let json_str = r#"{
            "method": "onchain",
            "unit": "sat",
            "amountless": true
        }"#;

        let settings: MeltMethodSettings = from_str(json_str).unwrap();

        assert_eq!(settings.method, PaymentMethod::Known(KnownMethod::Onchain));
        assert_eq!(settings.options, None);
    }

    #[test]
    fn test_settings_get_and_remove_match_method_and_unit() {
        let bolt11_sat = MeltMethodSettings {
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            unit: CurrencyUnit::Sat,
            method_name: None,
            min_amount: Some(Amount::from(1)),
            max_amount: None,
            options: None,
        };
        let bolt11_usd = MeltMethodSettings {
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            unit: CurrencyUnit::Usd,
            method_name: None,
            min_amount: Some(Amount::from(2)),
            max_amount: None,
            options: None,
        };
        let onchain_sat = MeltMethodSettings {
            method: PaymentMethod::Known(KnownMethod::Onchain),
            unit: CurrencyUnit::Sat,
            method_name: None,
            min_amount: Some(Amount::from(3)),
            max_amount: None,
            options: None,
        };

        let mut settings = Settings::new(
            vec![bolt11_sat.clone(), bolt11_usd.clone(), onchain_sat.clone()],
            false,
        );

        assert_eq!(
            settings.get_settings(
                &CurrencyUnit::Usd,
                &PaymentMethod::Known(KnownMethod::Bolt11)
            ),
            Some(bolt11_usd.clone())
        );

        assert_eq!(
            settings.remove_settings(
                &CurrencyUnit::Usd,
                &PaymentMethod::Known(KnownMethod::Bolt11)
            ),
            Some(bolt11_usd)
        );
        assert_eq!(settings.methods, vec![bolt11_sat, onchain_sat]);
    }

    #[test]
    fn test_settings_supported_methods_and_units() {
        let settings = Settings::new(
            vec![
                MeltMethodSettings {
                    method: PaymentMethod::Known(KnownMethod::Bolt11),
                    unit: CurrencyUnit::Sat,
                    method_name: None,
                    min_amount: None,
                    max_amount: None,
                    options: None,
                },
                MeltMethodSettings {
                    method: PaymentMethod::Known(KnownMethod::Onchain),
                    unit: CurrencyUnit::Usd,
                    method_name: None,
                    min_amount: None,
                    max_amount: None,
                    options: None,
                },
            ],
            false,
        );

        assert_eq!(
            settings.supported_methods(),
            vec![
                &PaymentMethod::Known(KnownMethod::Bolt11),
                &PaymentMethod::Known(KnownMethod::Onchain),
            ]
        );
        assert_eq!(
            settings.supported_units(),
            vec![&CurrencyUnit::Sat, &CurrencyUnit::Usd]
        );
    }

    #[test]
    fn test_melt_method_settings_method_name_round_trip() {
        let json_str = r#"{
            "method": "bolt11",
            "unit": "sat",
            "method_name": "Lightning",
            "min_amount": 0,
            "max_amount": 10000
        }"#;

        let settings: MeltMethodSettings = from_str(json_str).unwrap();

        assert_eq!(settings.method_name, Some("Lightning".to_string()));
        assert_eq!(settings.method_name(), "Lightning");

        let serialized = to_string(&settings).unwrap();
        let parsed: serde_json::Value = from_str(&serialized).unwrap();

        assert_eq!(parsed["method_name"], json!("Lightning"));
    }

    #[test]
    fn test_melt_method_settings_null_method_name_deserializes_as_none() {
        let json_str = r#"{
            "method": "bolt11",
            "unit": "sat",
            "method_name": null
        }"#;

        let settings: MeltMethodSettings = from_str(json_str).unwrap();

        assert_eq!(settings.method_name, None);
        assert_eq!(settings.method_name(), "Bolt11");

        let serialized = to_string(&settings).unwrap();
        let parsed: serde_json::Value = from_str(&serialized).unwrap();

        assert!(parsed.get("method_name").is_none());
    }

    #[test]
    fn test_melt_method_settings_omitted_method_name_uses_derived_name() {
        let json_str = r#"{
            "method": "apple-pay",
            "unit": "usd"
        }"#;

        let settings: MeltMethodSettings = from_str(json_str).unwrap();

        assert_eq!(settings.method_name, None);
        assert_eq!(settings.method_name(), "Apple Pay");
    }

    #[test]
    fn test_melt_quote_custom_response_fee_reserve_optional() {
        let json_str = r#"{
            "quote": "abc123",
            "method": "cashapp",
            "state": "UNPAID",
            "amount": 1000,
            "expiry": 1234567890,
            "custom_field": "value"
        }"#;

        let response: MeltQuoteCustomResponse<String> = from_str(json_str).unwrap();

        assert_eq!(response.fee_reserve, None);
        assert_eq!(
            response.method,
            PaymentMethod::Custom("cashapp".to_string())
        );
        assert_eq!(response.extra["custom_field"], json!("value"));

        let serialized = to_string(&response).unwrap();
        let parsed: serde_json::Value = from_str(&serialized).unwrap();

        assert!(parsed.get("fee_reserve").is_none());
    }

    #[test]
    fn test_melt_quote_custom_response_serializes_fee_reserve_when_present() {
        let response = MeltQuoteCustomResponse {
            quote: "abc123".to_string(),
            method: PaymentMethod::Custom("custom".to_string()),
            amount: Amount::from(1000),
            fee_reserve: Some(Amount::from(10)),
            state: QuoteState::Unpaid,
            expiry: 1234567890,
            payment_preimage: None,
            change: None,
            request: None,
            unit: None,
            extra: serde_json::Value::Null,
        };

        let serialized = to_string(&response).unwrap();
        let parsed: serde_json::Value = from_str(&serialized).unwrap();

        assert_eq!(parsed["fee_reserve"], json!(10));
        assert_eq!(parsed["method"], json!("custom"));
    }
}
