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
use crate::nut00::KnownMethod;
#[cfg(feature = "mint")]
use crate::quote_id::QuoteId;
#[cfg(feature = "mint")]
use crate::quote_id::QuoteIdError;
use crate::util::serde_helpers::deserialize_empty_string_as_none;
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
#[serde(bound = "Q: Serialize + DeserializeOwned")]
pub struct MintRequest<Q> {
    /// Quote id
    pub quote: Q,
    /// Outputs
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
pub struct MintResponse {
    /// Blinded Signatures
    pub signatures: Vec<BlindSignature>,
}

/// Mint Method Settings
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
        let mut num_fields = 2; // method and unit are always present
        if self.min_amount.is_some() {
            num_fields += 1;
        }
        if self.max_amount.is_some() {
            num_fields += 1;
        }

        let mut description_in_top_level = false;
        let mut onchain_confirmations: Option<u32> = None;

        match &self.options {
            Some(MintMethodOptions::Bolt11 { description }) if *description => {
                num_fields += 1;
                description_in_top_level = true;
            }
            Some(MintMethodOptions::Onchain { confirmations }) => {
                onchain_confirmations = Some(*confirmations);
                num_fields += 1; // for the "options" field
            }
            _ => {}
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

        // Serialize onchain options as a nested "options" object
        if let Some(confirmations) = onchain_confirmations {
            #[derive(Serialize)]
            struct OnchainOptions {
                confirmations: u32,
            }
            state.serialize_field("options", &OnchainOptions { confirmations })?;
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
        let mut confirmations: Option<u32> = None;

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
                "confirmations" => {
                    return Err(de::Error::unknown_field("confirmations", &["options"]));
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

                    if let Some(MintMethodOptions::Onchain {
                        confirmations: conf_from_options,
                    }) = options
                    {
                        confirmations = Some(conf_from_options);
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
        let options = if method == PaymentMethod::Known(KnownMethod::Bolt11) {
            description.map(|desc| MintMethodOptions::Bolt11 { description: desc })
        } else if method == PaymentMethod::Known(KnownMethod::Onchain) {
            confirmations.map(|conf| MintMethodOptions::Onchain {
                confirmations: conf,
            })
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
#[serde(untagged)]
pub enum MintMethodOptions {
    /// Bolt11 Options
    Bolt11 {
        /// Mint supports setting bolt11 description
        description: bool,
    },
    /// Onchain Options
    Onchain {
        /// Minimum number of confirmations required
        confirmations: u32,
    },
    /// Custom Options
    Custom {},
}

/// Mint Settings
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
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
///   "amount": 1000,
///   "amount_paid": 0,
///   "amount_issued": 0,
///   "paypal_link": "https://paypal.me/merchant",
///   "paypal_email": "merchant@example.com"
/// }
/// ```
///
/// This separation enables proper validation layering: the mint verifies
/// well-defined fields (amount, unit, etc.) while passing extra through
/// to the gRPC payment processor for method-specific validation.
///
/// It also provides a clean upgrade path: when a payment method becomes speced,
/// its fields can be promoted from `extra` to well-defined struct fields without
/// breaking existing clients (e.g., bolt12's `amount_paid` and `amount_issued`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound = "Q: Serialize + for<'a> Deserialize<'a>")]
pub struct MintQuoteCustomResponse<Q> {
    /// Quote ID
    pub quote: Q,
    /// Payment request string (method-specific format)
    pub request: String,
    /// Amount
    pub amount: Option<Amount>,
    /// Amount that has been paid
    pub amount_paid: Amount,
    /// Amount that has been issued
    pub amount_issued: Amount,
    /// Currency unit
    pub unit: Option<CurrencyUnit>,
    /// Unix timestamp until the quote is valid
    pub expiry: Option<u64>,
    /// NUT-19 Pubkey
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_empty_string_as_none"
    )]
    pub pubkey: Option<PublicKey>,
    /// Extra payment-method-specific fields
    ///
    /// These fields are flattened into the JSON representation, allowing
    /// custom payment methods to include additional data without nesting.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Value::is_null")]
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
            amount_paid: self.amount_paid,
            amount_issued: self.amount_issued,
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
            amount_paid: value.amount_paid,
            amount_issued: value.amount_issued,
            unit: value.unit,
            expiry: value.expiry,
            pubkey: value.pubkey,
            extra: value.extra,
        }
    }
}
#[cfg(test)]
mod tests {
    use std::fmt;

    use serde::ser::{Impossible, SerializeStruct, Serializer};
    use serde_json::{from_str, json, to_string};

    use super::*;
    use crate::nut00::KnownMethod;

    #[derive(Debug)]
    struct FieldCountError(String);

    impl fmt::Display for FieldCountError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl std::error::Error for FieldCountError {}

    impl serde::ser::Error for FieldCountError {
        fn custom<T>(msg: T) -> Self
        where
            T: fmt::Display,
        {
            Self(msg.to_string())
        }
    }

    struct FieldCountSerializer;

    struct FieldCountStruct {
        declared: usize,
        actual: usize,
    }

    impl Serializer for FieldCountSerializer {
        type Ok = ();
        type Error = FieldCountError;
        type SerializeSeq = Impossible<Self::Ok, Self::Error>;
        type SerializeTuple = Impossible<Self::Ok, Self::Error>;
        type SerializeTupleStruct = Impossible<Self::Ok, Self::Error>;
        type SerializeTupleVariant = Impossible<Self::Ok, Self::Error>;
        type SerializeMap = Impossible<Self::Ok, Self::Error>;
        type SerializeStruct = FieldCountStruct;
        type SerializeStructVariant = Impossible<Self::Ok, Self::Error>;

        fn serialize_bool(self, _v: bool) -> Result<Self::Ok, Self::Error> {
            Err(FieldCountError("unsupported bool".to_string()))
        }

        fn serialize_i8(self, _v: i8) -> Result<Self::Ok, Self::Error> {
            Err(FieldCountError("unsupported i8".to_string()))
        }

        fn serialize_i16(self, _v: i16) -> Result<Self::Ok, Self::Error> {
            Err(FieldCountError("unsupported i16".to_string()))
        }

        fn serialize_i32(self, _v: i32) -> Result<Self::Ok, Self::Error> {
            Err(FieldCountError("unsupported i32".to_string()))
        }

        fn serialize_i64(self, _v: i64) -> Result<Self::Ok, Self::Error> {
            Err(FieldCountError("unsupported i64".to_string()))
        }

        fn serialize_u8(self, _v: u8) -> Result<Self::Ok, Self::Error> {
            Err(FieldCountError("unsupported u8".to_string()))
        }

        fn serialize_u16(self, _v: u16) -> Result<Self::Ok, Self::Error> {
            Err(FieldCountError("unsupported u16".to_string()))
        }

        fn serialize_u32(self, _v: u32) -> Result<Self::Ok, Self::Error> {
            Err(FieldCountError("unsupported u32".to_string()))
        }

        fn serialize_u64(self, _v: u64) -> Result<Self::Ok, Self::Error> {
            Err(FieldCountError("unsupported u64".to_string()))
        }

        fn serialize_f32(self, _v: f32) -> Result<Self::Ok, Self::Error> {
            Err(FieldCountError("unsupported f32".to_string()))
        }

        fn serialize_f64(self, _v: f64) -> Result<Self::Ok, Self::Error> {
            Err(FieldCountError("unsupported f64".to_string()))
        }

        fn serialize_char(self, _v: char) -> Result<Self::Ok, Self::Error> {
            Err(FieldCountError("unsupported char".to_string()))
        }

        fn serialize_str(self, _v: &str) -> Result<Self::Ok, Self::Error> {
            Err(FieldCountError("unsupported str".to_string()))
        }

        fn serialize_bytes(self, _v: &[u8]) -> Result<Self::Ok, Self::Error> {
            Err(FieldCountError("unsupported bytes".to_string()))
        }

        fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
            Err(FieldCountError("unsupported none".to_string()))
        }

        fn serialize_some<T>(self, _value: &T) -> Result<Self::Ok, Self::Error>
        where
            T: ?Sized + Serialize,
        {
            Err(FieldCountError("unsupported some".to_string()))
        }

        fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
            Err(FieldCountError("unsupported unit".to_string()))
        }

        fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
            Err(FieldCountError("unsupported unit struct".to_string()))
        }

        fn serialize_unit_variant(
            self,
            _name: &'static str,
            _variant_index: u32,
            _variant: &'static str,
        ) -> Result<Self::Ok, Self::Error> {
            Err(FieldCountError("unsupported unit variant".to_string()))
        }

        fn serialize_newtype_struct<T>(
            self,
            _name: &'static str,
            _value: &T,
        ) -> Result<Self::Ok, Self::Error>
        where
            T: ?Sized + Serialize,
        {
            Err(FieldCountError("unsupported newtype struct".to_string()))
        }

        fn serialize_newtype_variant<T>(
            self,
            _name: &'static str,
            _variant_index: u32,
            _variant: &'static str,
            _value: &T,
        ) -> Result<Self::Ok, Self::Error>
        where
            T: ?Sized + Serialize,
        {
            Err(FieldCountError("unsupported newtype variant".to_string()))
        }

        fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
            Err(FieldCountError("unsupported seq".to_string()))
        }

        fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Self::Error> {
            Err(FieldCountError("unsupported tuple".to_string()))
        }

        fn serialize_tuple_struct(
            self,
            _name: &'static str,
            _len: usize,
        ) -> Result<Self::SerializeTupleStruct, Self::Error> {
            Err(FieldCountError("unsupported tuple struct".to_string()))
        }

        fn serialize_tuple_variant(
            self,
            _name: &'static str,
            _variant_index: u32,
            _variant: &'static str,
            _len: usize,
        ) -> Result<Self::SerializeTupleVariant, Self::Error> {
            Err(FieldCountError("unsupported tuple variant".to_string()))
        }

        fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
            Err(FieldCountError("unsupported map".to_string()))
        }

        fn serialize_struct(
            self,
            _name: &'static str,
            len: usize,
        ) -> Result<Self::SerializeStruct, Self::Error> {
            Ok(FieldCountStruct {
                declared: len,
                actual: 0,
            })
        }

        fn serialize_struct_variant(
            self,
            _name: &'static str,
            _variant_index: u32,
            _variant: &'static str,
            _len: usize,
        ) -> Result<Self::SerializeStructVariant, Self::Error> {
            Err(FieldCountError("unsupported struct variant".to_string()))
        }
    }

    impl SerializeStruct for FieldCountStruct {
        type Ok = ();
        type Error = FieldCountError;

        fn serialize_field<T>(&mut self, _key: &'static str, _value: &T) -> Result<(), Self::Error>
        where
            T: ?Sized + Serialize,
        {
            self.actual += 1;
            Ok(())
        }

        fn end(self) -> Result<Self::Ok, Self::Error> {
            if self.actual == self.declared {
                Ok(())
            } else {
                Err(FieldCountError(format!(
                    "declared {} fields but serialized {}",
                    self.declared, self.actual
                )))
            }
        }
    }

    fn assert_mint_method_settings_field_count(settings: &MintMethodSettings) {
        settings.serialize(FieldCountSerializer).unwrap();
    }

    #[test]
    fn test_mint_request_total_amount() {
        let request: MintRequest<String> = from_str(
            r#"{
                "quote": "quote-id",
                "outputs": [
                    {
                        "amount": 2,
                        "id": "00bfa73302d12ffd",
                        "B_": "038ec853d65ae1b79b5cdbc2774150b2cb288d6d26e12958a16fb33c32d9a86c39"
                    },
                    {
                        "amount": 4,
                        "id": "00bfa73302d12ffd",
                        "B_": "02c97ee3d1db41cf0a3ddb601724be8711a032950811bf326f8219c50c4808d3cd"
                    }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(request.total_amount().unwrap(), Amount::from(6));
    }

    #[test]
    fn test_mint_method_settings_serialize_field_count() {
        assert_mint_method_settings_field_count(&MintMethodSettings {
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            unit: CurrencyUnit::Sat,
            min_amount: Some(Amount::from(1)),
            max_amount: Some(Amount::from(1000)),
            options: Some(MintMethodOptions::Bolt11 { description: true }),
        });

        assert_mint_method_settings_field_count(&MintMethodSettings {
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            unit: CurrencyUnit::Sat,
            min_amount: Some(Amount::from(1)),
            max_amount: None,
            options: None,
        });

        assert_mint_method_settings_field_count(&MintMethodSettings {
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            unit: CurrencyUnit::Sat,
            min_amount: None,
            max_amount: Some(Amount::from(1000)),
            options: None,
        });

        assert_mint_method_settings_field_count(&MintMethodSettings {
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            unit: CurrencyUnit::Sat,
            min_amount: None,
            max_amount: None,
            options: Some(MintMethodOptions::Bolt11 { description: true }),
        });

        assert_mint_method_settings_field_count(&MintMethodSettings {
            method: PaymentMethod::Known(KnownMethod::Onchain),
            unit: CurrencyUnit::Sat,
            min_amount: None,
            max_amount: None,
            options: Some(MintMethodOptions::Onchain { confirmations: 3 }),
        });
    }

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
    fn test_mint_method_settings_does_not_serialize_false_description() {
        let settings = MintMethodSettings {
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            unit: CurrencyUnit::Sat,
            min_amount: None,
            max_amount: None,
            options: Some(MintMethodOptions::Bolt11 { description: false }),
        };

        let serialized = to_string(&settings).unwrap();
        let parsed: serde_json::Value = from_str(&serialized).unwrap();

        assert_eq!(parsed["method"], json!("bolt11"));
        assert!(parsed.get("description").is_none());
        assert!(parsed.get("options").is_none());
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

    #[test]
    fn custom_mint_quote_response_has_no_typed_state() {
        let response = MintQuoteCustomResponse {
            quote: "abc123".to_string(),
            request: "paypal://pay?id=123".to_string(),
            amount: Some(Amount::from(1000)),
            amount_paid: Amount::ZERO,
            amount_issued: Amount::ZERO,
            unit: Some(CurrencyUnit::Sat),
            expiry: Some(9999999),
            pubkey: None,
            extra: serde_json::Value::Null,
        };

        let serialized = to_string(&response).unwrap();
        let parsed: serde_json::Value = from_str(&serialized).unwrap();

        assert!(parsed.get("state").is_none());
        assert_eq!(parsed["amount_paid"], json!(0));
        assert_eq!(parsed["amount_issued"], json!(0));
    }

    #[test]
    fn test_onchain_settings_nested_options_round_trip() {
        // NUT-26 spec format: confirmations nested inside "options"
        let json_str = r#"{
            "method": "onchain",
            "unit": "sat",
            "min_amount": 1000,
            "max_amount": 1000000,
            "options": {
                "confirmations": 3
            }
        }"#;

        let settings: MintMethodSettings = from_str(json_str).unwrap();

        assert_eq!(settings.method, PaymentMethod::Known(KnownMethod::Onchain));
        assert_eq!(settings.unit, CurrencyUnit::Sat);
        assert_eq!(settings.min_amount, Some(Amount::from(1000)));
        assert_eq!(settings.max_amount, Some(Amount::from(1000000)));

        match settings.options {
            Some(MintMethodOptions::Onchain { confirmations }) => {
                assert_eq!(confirmations, 3);
            }
            _ => panic!("Expected Onchain options with confirmations = 3"),
        }

        // Serialize it back and verify the nested "options" structure
        let serialized = to_string(&settings).unwrap();
        let parsed: serde_json::Value = from_str(&serialized).unwrap();

        assert_eq!(parsed["method"], json!("onchain"));
        assert_eq!(parsed["options"]["confirmations"], json!(3));
        // Verify confirmations is NOT at top level
        assert!(parsed.get("confirmations").is_none());
    }

    #[test]
    fn test_onchain_settings_top_level_confirmations_rejected() {
        let json_str = r#"{
            "method": "onchain",
            "unit": "sat",
            "confirmations": 6
        }"#;

        let err = from_str::<MintMethodSettings>(json_str).unwrap_err();
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn test_onchain_settings_top_level_and_nested_rejected() {
        let json_str = r#"{
            "method": "onchain",
            "unit": "sat",
            "confirmations": 6,
            "options": {
                "confirmations": 3
            }
        }"#;

        let err = from_str::<MintMethodSettings>(json_str).unwrap_err();
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn test_mint_method_settings_visitor_reports_expected_type() {
        let err = from_str::<MintMethodSettings>("[]").unwrap_err();

        assert!(err.to_string().contains("a MintMethodSettings structure"));
    }

    #[test]
    fn test_get_settings_requires_exact_method_and_unit_match() {
        let bolt11_msat = MintMethodSettings {
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            unit: CurrencyUnit::Msat,
            min_amount: Some(Amount::from(1)),
            max_amount: None,
            options: None,
        };
        let bolt12_sat = MintMethodSettings {
            method: PaymentMethod::Known(KnownMethod::Bolt12),
            unit: CurrencyUnit::Sat,
            min_amount: Some(Amount::from(2)),
            max_amount: None,
            options: None,
        };
        let bolt11_sat = MintMethodSettings {
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            unit: CurrencyUnit::Sat,
            min_amount: Some(Amount::from(3)),
            max_amount: None,
            options: None,
        };
        let settings = Settings::new(
            vec![bolt11_msat.clone(), bolt12_sat.clone(), bolt11_sat.clone()],
            false,
        );

        assert_eq!(
            settings.get_settings(&CurrencyUnit::Sat, &PaymentMethod::BOLT11),
            Some(bolt11_sat)
        );
        assert_eq!(
            settings.get_settings(
                &CurrencyUnit::Msat,
                &PaymentMethod::Known(KnownMethod::Bolt12)
            ),
            None
        );
    }

    #[test]
    fn test_supported_methods_and_units_preserve_configured_values() {
        let settings = Settings::new(
            vec![
                MintMethodSettings {
                    method: PaymentMethod::Known(KnownMethod::Bolt11),
                    unit: CurrencyUnit::Msat,
                    min_amount: Some(Amount::from(1)),
                    max_amount: None,
                    options: None,
                },
                MintMethodSettings {
                    method: PaymentMethod::Known(KnownMethod::Onchain),
                    unit: CurrencyUnit::Eur,
                    min_amount: None,
                    max_amount: Some(Amount::from(100)),
                    options: Some(MintMethodOptions::Onchain { confirmations: 3 }),
                },
            ],
            false,
        );

        let methods = settings.supported_methods();
        assert_eq!(methods.len(), 2);
        assert_eq!(methods[0], &PaymentMethod::Known(KnownMethod::Bolt11));
        assert_eq!(methods[1], &PaymentMethod::Known(KnownMethod::Onchain));

        let units = settings.supported_units();
        assert_eq!(units.len(), 2);
        assert_eq!(units[0], &CurrencyUnit::Msat);
        assert_eq!(units[1], &CurrencyUnit::Eur);
    }

    #[test]
    fn test_remove_settings_requires_exact_method_and_unit_match() {
        let bolt11_msat = MintMethodSettings {
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            unit: CurrencyUnit::Msat,
            min_amount: Some(Amount::from(1)),
            max_amount: None,
            options: None,
        };
        let bolt12_sat = MintMethodSettings {
            method: PaymentMethod::Known(KnownMethod::Bolt12),
            unit: CurrencyUnit::Sat,
            min_amount: Some(Amount::from(2)),
            max_amount: None,
            options: None,
        };
        let bolt11_sat = MintMethodSettings {
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            unit: CurrencyUnit::Sat,
            min_amount: Some(Amount::from(3)),
            max_amount: None,
            options: None,
        };
        let mut settings = Settings::new(
            vec![bolt11_msat.clone(), bolt12_sat.clone(), bolt11_sat.clone()],
            false,
        );

        assert_eq!(
            settings.remove_settings(&CurrencyUnit::Sat, &PaymentMethod::BOLT11),
            Some(bolt11_sat.clone())
        );
        assert_eq!(settings.methods, vec![bolt11_msat, bolt12_sat]);
        assert_eq!(
            settings.remove_settings(&CurrencyUnit::Sat, &PaymentMethod::BOLT11),
            None
        );
    }
}
