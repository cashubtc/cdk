//! Specific Subscription for the cdk crate
use serde::de::{DeserializeOwned, Error as DeError};
use serde::{Deserialize, Serialize};

use super::PublicKey;
use crate::nut00::KnownMethod;
use crate::nut25::MeltQuoteBolt12Response;
use crate::nut30::{MeltQuoteOnchainResponse, MintQuoteOnchainResponse};
use crate::nuts::{
    CurrencyUnit, MeltQuoteBolt11Response, MeltQuoteCustomResponse, MintQuoteBolt11Response,
    MintQuoteCustomResponse, PaymentMethod, ProofState,
};
use crate::quote_id::QuoteIdError;
use crate::MintQuoteBolt12Response;

pub mod ws;

/// Subscription Parameter according to the standard
#[derive(Debug, Clone, Serialize, Eq, PartialEq, Hash, Deserialize)]
#[serde(bound = "I: DeserializeOwned + Serialize")]
pub struct Params<I> {
    /// Kind
    pub kind: Kind,
    /// Filters
    pub filters: Vec<String>,
    /// Subscription Id
    #[serde(rename = "subId")]
    pub id: I,
}

/// Check state Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SupportedSettings {
    /// Supported methods
    pub supported: Vec<SupportedMethods>,
}

/// Supported WS Methods
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SupportedMethods {
    /// Payment Method
    pub method: PaymentMethod,
    /// Unit
    pub unit: CurrencyUnit,
    /// Command
    pub commands: Vec<WsCommand>,
}

impl SupportedMethods {
    /// Create [`SupportedMethods`]
    pub fn new(method: PaymentMethod, unit: CurrencyUnit, commands: Vec<WsCommand>) -> Self {
        Self {
            method,
            unit,
            commands,
        }
    }

    /// Create [`SupportedMethods`] for Bolt11 with all supported commands
    pub fn default_bolt11(unit: CurrencyUnit) -> Self {
        let commands = vec![
            WsCommand::Bolt11MintQuote,
            WsCommand::Bolt11MeltQuote,
            WsCommand::ProofState,
        ];

        Self {
            method: PaymentMethod::Known(KnownMethod::Bolt11),
            unit,
            commands,
        }
    }

    /// Create [`SupportedMethods`] for Bolt12 with all supported commands
    pub fn default_bolt12(unit: CurrencyUnit) -> Self {
        let commands = vec![
            WsCommand::Bolt12MintQuote,
            WsCommand::Bolt12MeltQuote,
            WsCommand::ProofState,
        ];

        Self {
            method: PaymentMethod::Known(KnownMethod::Bolt12),
            unit,
            commands,
        }
    }

    /// Create [`SupportedMethods`] for custom payment method with all supported commands
    pub fn default_custom(method: PaymentMethod, unit: CurrencyUnit) -> Self {
        let method_name = method.to_string();
        let commands = vec![
            WsCommand::Custom(format!("{}_mint_quote", method_name)),
            WsCommand::Custom(format!("{}_melt_quote", method_name)),
            WsCommand::ProofState,
        ];

        Self {
            method,
            unit,
            commands,
        }
    }
}

impl WsCommand {
    /// Create a custom mint quote command for a payment method
    pub fn custom_mint_quote(method: &str) -> Self {
        WsCommand::Custom(format!("{}_mint_quote", method))
    }

    /// Create a custom melt quote command for a payment method
    pub fn custom_melt_quote(method: &str) -> Self {
        WsCommand::Custom(format!("{}_melt_quote", method))
    }
}

/// WebSocket commands supported by the Cashu mint
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WsCommand {
    /// Command to request a Lightning invoice for minting tokens
    Bolt11MintQuote,
    /// Command to request a Lightning payment for melting tokens
    Bolt11MeltQuote,
    /// Websocket support for Bolt12 Mint Quote
    Bolt12MintQuote,
    /// Websocket support for Bolt12 Melt Quote
    Bolt12MeltQuote,
    /// Command to check the state of a proof
    ProofState,
    /// Custom payment method command
    Custom(String),
}

impl Serialize for WsCommand {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let s = match self {
            WsCommand::Bolt11MintQuote => "bolt11_mint_quote",
            WsCommand::Bolt11MeltQuote => "bolt11_melt_quote",
            WsCommand::Bolt12MintQuote => "bolt12_mint_quote",
            WsCommand::Bolt12MeltQuote => "bolt12_melt_quote",
            WsCommand::ProofState => "proof_state",
            WsCommand::Custom(custom) => custom.as_str(),
        };
        serializer.serialize_str(s)
    }
}

impl<'de> Deserialize<'de> for WsCommand {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "bolt11_mint_quote" => WsCommand::Bolt11MintQuote,
            "bolt11_melt_quote" => WsCommand::Bolt11MeltQuote,
            "bolt12_mint_quote" => WsCommand::Bolt12MintQuote,
            "bolt12_melt_quote" => WsCommand::Bolt12MeltQuote,
            "proof_state" => WsCommand::ProofState,
            custom => WsCommand::Custom(custom.to_string()),
        })
    }
}

impl<T> From<MintQuoteBolt12Response<T>> for NotificationPayload<T>
where
    T: Clone,
{
    fn from(mint_quote: MintQuoteBolt12Response<T>) -> NotificationPayload<T> {
        NotificationPayload::MintQuoteBolt12Response(mint_quote)
    }
}

impl<T> From<MeltQuoteBolt12Response<T>> for NotificationPayload<T>
where
    T: Clone,
{
    fn from(melt_quote: MeltQuoteBolt12Response<T>) -> NotificationPayload<T> {
        NotificationPayload::MeltQuoteBolt12Response(melt_quote)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(bound(serialize = "T: Serialize + DeserializeOwned"))]
#[serde(untagged)]
/// Subscription response
///
/// Deserialization is implemented below with explicit field discrimination
/// because quote response payloads for different payment methods have
/// overlapping fields.
pub enum NotificationPayload<T>
where
    T: Clone,
{
    /// Proof State
    ProofState(ProofState),
    /// Mint Quote Onchain Response
    MintQuoteOnchainResponse(MintQuoteOnchainResponse<T>),
    /// Melt Quote Onchain Response
    MeltQuoteOnchainResponse(MeltQuoteOnchainResponse<T>),
    /// Mint Quote Bolt12 Response
    MintQuoteBolt12Response(MintQuoteBolt12Response<T>),
    /// Melt Quote Bolt11 Response
    MeltQuoteBolt11Response(MeltQuoteBolt11Response<T>),
    /// Mint Quote Bolt11 Response
    MintQuoteBolt11Response(MintQuoteBolt11Response<T>),
    /// Melt Quote Bolt12 Response
    MeltQuoteBolt12Response(MeltQuoteBolt12Response<T>),
    /// Custom Mint Quote Response (method, response)
    CustomMintQuoteResponse(String, MintQuoteCustomResponse<T>),
    /// Custom Melt Quote Response (method, response)
    CustomMeltQuoteResponse(String, MeltQuoteCustomResponse<T>),
}

fn fill_response_method<E>(value: &mut serde_json::Value, method: &str) -> Result<(), E>
where
    E: DeError,
{
    if let serde_json::Value::Object(object) = value {
        match object.get("method") {
            Some(serde_json::Value::String(existing))
                if PaymentMethod::new(existing.to_string())
                    == PaymentMethod::new(method.to_string()) => {}
            Some(serde_json::Value::String(existing)) => {
                return Err(E::custom(format!(
                    "notification payload method {existing} does not match kind method {method}"
                )));
            }
            Some(_) => {
                return Err(E::custom("notification payload method must be a string"));
            }
            None => {
                object.insert(
                    "method".to_string(),
                    serde_json::Value::String(method.to_string()),
                );
            }
        }
    }

    Ok(())
}

fn fill_kind_response_method<E>(
    mut value: serde_json::Value,
    method: &str,
) -> Result<serde_json::Value, E>
where
    E: DeError,
{
    fill_response_method::<E>(&mut value, method)?;
    Ok(value)
}

/// Deserialize a notification payload using the subscription kind that produced
/// it.
///
/// NUT-17 notification payloads are not self-describing. Quote responses share
/// many fields and tolerate unknown fields, so deserializing the payload without
/// the subscription kind can silently select the wrong variant.
pub fn deserialize_payload_for_kind<T, E>(
    kind: &Kind,
    value: serde_json::Value,
) -> Result<NotificationPayload<T>, E>
where
    T: Clone + Serialize + DeserializeOwned,
    E: DeError,
{
    fn from_value<V, E>(value: serde_json::Value) -> Result<V, E>
    where
        V: DeserializeOwned,
        E: DeError,
    {
        serde_json::from_value(value).map_err(E::custom)
    }

    fn custom_response<V, E>(
        kind: &str,
        expected_method: &str,
        mut value: serde_json::Value,
    ) -> Result<(String, V), E>
    where
        V: DeserializeOwned,
        E: DeError,
    {
        match &mut value {
            serde_json::Value::Array(items) if items.len() == 2 => {
                let payload_method = items
                    .first()
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| E::custom("custom notification method must be a string"))?
                    .to_string();
                if payload_method != expected_method {
                    return Err(E::custom(format!(
                        "custom notification method {payload_method} does not match kind {kind}"
                    )));
                }

                let response = items
                    .get_mut(1)
                    .ok_or_else(|| E::custom("custom notification payload is missing response"))?;
                if !response.is_object() {
                    return Err(E::custom("custom notification response must be an object"));
                }
                fill_response_method::<E>(response, expected_method)?;

                from_value(value)
            }
            serde_json::Value::Array(_) => {
                Err(E::custom("custom notification payload must have two items"))
            }
            serde_json::Value::Object(_) => {
                fill_response_method::<E>(&mut value, expected_method)?;
                from_value(value).map(|response| (expected_method.to_string(), response))
            }
            _ => Err(E::custom("custom notification response must be an object")),
        }
    }

    match kind {
        Kind::ProofState => from_value(value).map(NotificationPayload::ProofState),
        Kind::Bolt11MintQuote => fill_kind_response_method::<E>(value, "bolt11")
            .and_then(from_value)
            .map(NotificationPayload::MintQuoteBolt11Response),
        Kind::Bolt11MeltQuote => fill_kind_response_method::<E>(value, "bolt11")
            .and_then(from_value)
            .map(NotificationPayload::MeltQuoteBolt11Response),
        Kind::Bolt12MintQuote => fill_kind_response_method::<E>(value, "bolt12")
            .and_then(from_value)
            .map(NotificationPayload::MintQuoteBolt12Response),
        Kind::Bolt12MeltQuote => fill_kind_response_method::<E>(value, "bolt12")
            .and_then(from_value)
            .map(NotificationPayload::MeltQuoteBolt12Response),
        Kind::OnchainMintQuote => fill_kind_response_method::<E>(value, "onchain")
            .and_then(from_value)
            .map(NotificationPayload::MintQuoteOnchainResponse),
        Kind::OnchainMeltQuote => fill_kind_response_method::<E>(value, "onchain")
            .and_then(from_value)
            .map(NotificationPayload::MeltQuoteOnchainResponse),
        Kind::Custom(method) => {
            if let Some(expected_method) = method.strip_suffix("_mint_quote") {
                custom_response(method, expected_method, value).map(|(method, response)| {
                    NotificationPayload::CustomMintQuoteResponse(method, response)
                })
            } else if let Some(expected_method) = method.strip_suffix("_melt_quote") {
                custom_response(method, expected_method, value).map(|(method, response)| {
                    NotificationPayload::CustomMeltQuoteResponse(method, response)
                })
            } else {
                Err(E::custom(format!(
                    "unsupported custom notification kind: {method}"
                )))
            }
        }
    }
}

impl<'de, T> Deserialize<'de> for NotificationPayload<T>
where
    T: Clone + Serialize + DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let _ = serde_json::Value::deserialize(deserializer)?;
        Err(D::Error::custom(
            "notification payloads require subscription kind context; use \
             nut17::deserialize_payload_for_kind",
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, Hash, Serialize)]
#[serde(bound = "T: Serialize + DeserializeOwned")]
/// A parsed notification
pub enum NotificationId<T>
where
    T: Clone,
{
    /// ProofState id is a Pubkey
    ProofState(PublicKey),
    /// MeltQuote id is an QuoteId
    MeltQuoteBolt11(T),
    /// MintQuote id is an QuoteId
    MintQuoteBolt11(T),
    /// MintQuote id is an QuoteId
    MintQuoteBolt12(T),
    /// MintQuote id is an QuoteId
    MeltQuoteBolt12(T),
    /// MintQuote id is an QuoteId
    MintQuoteOnchain(T),
    /// MintQuote id is an QuoteId
    MeltQuoteOnchain(T),
    /// MintQuote id is an QuoteId
    MintQuoteCustom(String, T),
    /// MintQuote id is an QuoteId
    MeltQuoteCustom(String, T),
}

/// Kind
#[derive(Debug, Clone, Eq, Ord, PartialOrd, PartialEq, Hash)]
pub enum Kind {
    /// Bolt 11 Melt Quote
    Bolt11MeltQuote,
    /// Bolt 11 Mint Quote
    Bolt11MintQuote,
    /// Proof State
    ProofState,
    /// Bolt 12 Mint Quote
    Bolt12MintQuote,
    /// Bolt 12 Melt Quote
    Bolt12MeltQuote,
    /// Onchain Mint Quote
    OnchainMintQuote,
    /// Onchain Melt Quote
    OnchainMeltQuote,
    /// Custom
    Custom(String),
}

impl Serialize for Kind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let s = match self {
            Kind::Bolt11MintQuote => "bolt11_mint_quote",
            Kind::Bolt11MeltQuote => "bolt11_melt_quote",
            Kind::Bolt12MintQuote => "bolt12_mint_quote",
            Kind::Bolt12MeltQuote => "bolt12_melt_quote",
            Kind::OnchainMintQuote => "onchain_mint_quote",
            Kind::OnchainMeltQuote => "onchain_melt_quote",
            Kind::ProofState => "proof_state",
            Kind::Custom(custom) => custom.as_str(),
        };
        serializer.serialize_str(s)
    }
}

impl<'de> Deserialize<'de> for Kind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "bolt11_mint_quote" => Kind::Bolt11MintQuote,
            "bolt11_melt_quote" => Kind::Bolt11MeltQuote,
            "bolt12_mint_quote" => Kind::Bolt12MintQuote,
            "bolt12_melt_quote" => Kind::Bolt12MeltQuote,
            "onchain_mint_quote" => Kind::OnchainMintQuote,
            "onchain_melt_quote" => Kind::OnchainMeltQuote,
            "proof_state" => Kind::ProofState,
            custom => Kind::Custom(custom.to_string()),
        })
    }
}

impl<I> AsRef<I> for Params<I> {
    fn as_ref(&self) -> &I {
        &self.id
    }
}

/// Parsing error
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Uuid Error: {0}")]
    /// Uuid Error
    QuoteId(#[from] QuoteIdError),

    #[error("PublicKey Error: {0}")]
    /// PublicKey Error
    PublicKey(#[from] crate::nuts::nut01::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nuts::nut00::{CurrencyUnit, KnownMethod, PaymentMethod};
    use crate::nuts::nut01::PublicKey;
    use crate::nuts::{MeltQuoteState, MintQuoteState};
    use crate::Amount;

    #[test]
    fn notification_payload_onchain_mint_roundtrip() {
        let resp: MintQuoteOnchainResponse<String> = MintQuoteOnchainResponse {
            quote: "abc".to_string(),
            request: "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".to_string(),
            unit: CurrencyUnit::Sat,
            method: PaymentMethod::Known(KnownMethod::Onchain),
            expiry: Some(1701704757),
            pubkey: PublicKey::from_hex(
                "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
            )
            .unwrap(),
            amount_paid: Amount::from(100_000),
            amount_issued: Amount::from(0),
            updated_at: 0,
            payjoin: None,
        };
        let payload: NotificationPayload<String> =
            NotificationPayload::MintQuoteOnchainResponse(resp.clone());

        let encoded = serde_json::to_string(&payload).unwrap();
        let decoded = deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::OnchainMintQuote,
            serde_json::from_str(&encoded).unwrap(),
        )
        .unwrap();

        match decoded {
            NotificationPayload::MintQuoteOnchainResponse(r) => {
                assert_eq!(r, resp);
            }
            other => panic!("expected MintQuoteOnchainResponse, got {:?}", other),
        }
    }

    #[test]
    fn notification_payload_bolt12_mint_roundtrip() {
        // Ensure a Bolt12 payload (with `amount`) still decodes as Bolt12 and
        // is not swallowed by the field-name overlap with the Onchain variant.
        let resp: MintQuoteBolt12Response<String> = MintQuoteBolt12Response {
            quote: "abc".to_string(),
            request: "lno1...".to_string(),
            amount: Some(Amount::from(100_000)),
            unit: CurrencyUnit::Sat,
            method: PaymentMethod::Known(KnownMethod::Bolt12),
            expiry: Some(1701704757),
            pubkey: PublicKey::from_hex(
                "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
            )
            .unwrap(),
            amount_paid: Amount::from(0),
            amount_issued: Amount::from(0),
            updated_at: 0,
        };
        let payload: NotificationPayload<String> =
            NotificationPayload::MintQuoteBolt12Response(resp.clone());

        let encoded = serde_json::to_string(&payload).unwrap();
        let decoded = deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::Bolt12MintQuote,
            serde_json::from_str(&encoded).unwrap(),
        )
        .unwrap();

        match decoded {
            NotificationPayload::MintQuoteBolt12Response(r) => {
                assert_eq!(r, resp);
            }
            other => panic!("expected MintQuoteBolt12Response, got {:?}", other),
        }
    }

    #[test]
    fn notification_payload_bolt11_mint_with_pubkey_roundtrip() {
        let resp: MintQuoteBolt11Response<String> = MintQuoteBolt11Response {
            quote: "abc".to_string(),
            request: "lnbc...".to_string(),
            amount: Some(Amount::from(100_000)),
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::BOLT11,
            amount_paid: Amount::from(0),
            amount_issued: Amount::from(0),
            updated_at: 0,
            state: MintQuoteState::Unpaid,
            expiry: Some(1701704757),
            pubkey: Some(
                PublicKey::from_hex(
                    "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
                )
                .unwrap(),
            ),
        };
        let payload: NotificationPayload<String> =
            NotificationPayload::MintQuoteBolt11Response(resp.clone());

        let encoded = serde_json::to_string(&payload).unwrap();
        let decoded = deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::Bolt11MintQuote,
            serde_json::from_str(&encoded).unwrap(),
        )
        .unwrap();

        match decoded {
            NotificationPayload::MintQuoteBolt11Response(r) => {
                assert_eq!(r, resp);
            }
            other => panic!("expected MintQuoteBolt11Response, got {:?}", other),
        }
    }

    #[test]
    fn notification_payload_onchain_melt_roundtrip() {
        let resp: MeltQuoteOnchainResponse<String> = MeltQuoteOnchainResponse {
            quote: "abc".to_string(),
            amount: Amount::from(100_000),
            unit: CurrencyUnit::Sat,
            method: PaymentMethod::Known(KnownMethod::Onchain),
            state: MeltQuoteState::Pending,
            expiry: 1701704757,
            request: "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".to_string(),
            fee_options: vec![crate::nut30::MeltQuoteOnchainFeeOption {
                fee_index: 0,
                fee_reserve: Amount::from(5_000),
                estimated_blocks: 1,
            }],
            selected_fee_index: Some(0),
            outpoint: Some("3b7f3b85:2".to_string()),
            change: None,
            payjoin: None,
        };
        let payload: NotificationPayload<String> =
            NotificationPayload::MeltQuoteOnchainResponse(resp.clone());

        let encoded = serde_json::to_string(&payload).unwrap();
        let decoded = deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::OnchainMeltQuote,
            serde_json::from_str(&encoded).unwrap(),
        )
        .unwrap();

        match decoded {
            NotificationPayload::MeltQuoteOnchainResponse(r) => {
                assert_eq!(r, resp);
            }
            other => panic!("expected MeltQuoteOnchainResponse, got {:?}", other),
        }
    }

    #[test]
    fn notification_payload_bolt12_melt_roundtrip() {
        let resp: MeltQuoteBolt12Response<String> = MeltQuoteBolt12Response {
            quote: "abc".to_string(),
            amount: Amount::from(100_000),
            fee_reserve: Amount::from(10),
            state: MeltQuoteState::Pending,
            expiry: 1701704757,
            payment_preimage: None,
            change: None,
            request: Some("lno1...".to_string()),
            unit: Some(CurrencyUnit::Sat),
            method: PaymentMethod::Known(KnownMethod::Bolt12),
        };
        let payload: NotificationPayload<String> =
            NotificationPayload::MeltQuoteBolt12Response(resp.clone());

        let encoded = serde_json::to_string(&payload).unwrap();
        let decoded = deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::Bolt12MeltQuote,
            serde_json::from_str(&encoded).unwrap(),
        )
        .unwrap();

        match decoded {
            NotificationPayload::MeltQuoteBolt12Response(r) => {
                assert_eq!(r, resp);
            }
            other => panic!("expected MeltQuoteBolt12Response, got {:?}", other),
        }
    }

    #[test]
    fn notification_payload_custom_arrays_require_method_and_object_response() {
        let custom_mint = r#"[
            "paypal",
            {
                "quote": "abc",
                "request": "pay://abc",
                "amount": 10,
                "amount_paid": 0,
                "amount_issued": 0,
                "unit": "sat",
                "expiry": 1701704757
            }
        ]"#;
        let mint_kind = Kind::Custom("paypal_mint_quote".to_string());
        let payload = deserialize_payload_for_kind::<String, serde_json::Error>(
            &mint_kind,
            serde_json::from_str(custom_mint).unwrap(),
        )
        .unwrap();
        match payload {
            NotificationPayload::CustomMintQuoteResponse(method, response) => {
                assert_eq!(method, "paypal");
                assert_eq!(response.quote, "abc");
                assert_eq!(response.method, PaymentMethod::Custom("paypal".to_string()));
            }
            other => panic!("expected CustomMintQuoteResponse, got {:?}", other),
        }

        let custom_melt = r#"[
            "paypal",
            {
                "quote": "abc",
                "amount": 10,
                "fee_reserve": 1,
                "state": "PENDING",
                "expiry": 1701704757,
                "request": "pay://abc",
                "unit": "sat"
            }
        ]"#;
        let melt_kind = Kind::Custom("paypal_melt_quote".to_string());
        let payload = deserialize_payload_for_kind::<String, serde_json::Error>(
            &melt_kind,
            serde_json::from_str(custom_melt).unwrap(),
        )
        .unwrap();
        match payload {
            NotificationPayload::CustomMeltQuoteResponse(method, response) => {
                assert_eq!(method, "paypal");
                assert_eq!(response.quote, "abc");
                assert_eq!(response.method, PaymentMethod::Custom("paypal".to_string()));
            }
            other => panic!("expected CustomMeltQuoteResponse, got {:?}", other),
        }

        assert!(deserialize_payload_for_kind::<String, serde_json::Error>(
            &mint_kind,
            serde_json::json!(["paypal"]),
        )
        .is_err());
        assert!(deserialize_payload_for_kind::<String, serde_json::Error>(
            &mint_kind,
            serde_json::json!(["paypal", "not an object"]),
        )
        .is_err());
        assert!(deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::Custom("paypal".to_string()),
            serde_json::json!({}),
        )
        .is_err());
    }

    #[test]
    fn notification_payload_onchain_mint_tolerates_unknown_fields() {
        // A newer mint may extend the onchain mint quote response with
        // additional fields; classification and decoding must still succeed.
        let encoded = r#"{
            "quote": "abc",
            "request": "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
            "unit": "sat",
            "expiry": 1701704757,
            "pubkey": "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
            "amount_paid": 0,
            "amount_issued": 0,
            "some_future_extension": {"nested": true}
        }"#;

        let decoded = deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::OnchainMintQuote,
            serde_json::from_str(encoded).unwrap(),
        )
        .unwrap();

        match decoded {
            NotificationPayload::MintQuoteOnchainResponse(r) => {
                assert_eq!(r.quote, "abc");
            }
            other => panic!("expected MintQuoteOnchainResponse, got {:?}", other),
        }
    }

    #[test]
    fn notification_payload_onchain_mint_future_state_field_uses_kind() {
        let encoded = r#"{
            "quote": "abc",
            "request": "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
            "unit": "sat",
            "expiry": 1701704757,
            "pubkey": "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
            "amount_paid": 0,
            "amount_issued": 0,
            "state": "future-extension"
        }"#;

        let decoded = deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::OnchainMintQuote,
            serde_json::from_str(encoded).unwrap(),
        )
        .unwrap();

        match decoded {
            NotificationPayload::MintQuoteOnchainResponse(r) => {
                assert_eq!(r.quote, "abc");
            }
            other => panic!("expected MintQuoteOnchainResponse, got {:?}", other),
        }
    }

    #[test]
    fn notification_payload_bolt12_mint_tolerates_unknown_fields() {
        let encoded = r#"{
            "quote": "abc",
            "request": "lno1...",
            "amount": 100,
            "unit": "sat",
            "expiry": 1701704757,
            "pubkey": "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
            "amount_paid": 0,
            "amount_issued": 0,
            "some_future_extension": {"nested": true}
        }"#;

        let decoded = deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::Bolt12MintQuote,
            serde_json::from_str(encoded).unwrap(),
        )
        .unwrap();

        match decoded {
            NotificationPayload::MintQuoteBolt12Response(r) => {
                assert_eq!(r.quote, "abc");
            }
            other => panic!("expected MintQuoteBolt12Response, got {:?}", other),
        }
    }

    #[test]
    fn notification_payload_custom_tuple_uses_kind() {
        let encoded = serde_json::json!([
            "foo",
            {
                "quote": "abc",
                "request": "custom-request",
                "amount": 100,
                "amount_paid": 0,
                "amount_issued": 0,
                "unit": "sat",
                "expiry": 1701704757
            }
        ]);

        let decoded = deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::Custom("foo_mint_quote".to_string()),
            encoded,
        )
        .unwrap();

        match decoded {
            NotificationPayload::CustomMintQuoteResponse(method, r) => {
                assert_eq!(method, "foo");
                assert_eq!(r.quote, "abc");
            }
            other => panic!("expected CustomMintQuoteResponse, got {:?}", other),
        }
    }

    #[test]
    fn notification_payload_kind_decoder_accepts_case_insensitive_method() {
        let payload = serde_json::json!({
            "quote": "abc",
            "request": "lnbc1...",
            "amount": 100,
            "unit": "sat",
            "method": "BOLT11",
            "state": "UNPAID",
            "expiry": 1701704757
        });

        let decoded = deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::Bolt11MintQuote,
            payload,
        )
        .unwrap();

        match decoded {
            NotificationPayload::MintQuoteBolt11Response(response) => {
                assert_eq!(response.method, PaymentMethod::Known(KnownMethod::Bolt11));
            }
            other => panic!("expected MintQuoteBolt11Response, got {:?}", other),
        }
    }

    #[test]
    fn notification_payload_rejects_method_that_does_not_match_kind() {
        let bolt12_payload = serde_json::json!({
            "quote": "melt-quote",
            "amount": 21,
            "fee_reserve": 1,
            "method": "bolt11",
            "state": "PAID",
            "expiry": 1234
        });

        assert!(deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::Bolt12MeltQuote,
            bolt12_payload,
        )
        .is_err());

        let custom_payload = serde_json::json!({
            "quote": "custom-quote",
            "request": "custom-request",
            "method": "bar",
            "amount": 100,
            "amount_paid": 0,
            "amount_issued": 0,
            "unit": "sat",
            "expiry": 1701704757
        });

        assert!(deserialize_payload_for_kind::<String, serde_json::Error>(
            &Kind::Custom("foo_mint_quote".to_string()),
            custom_payload,
        )
        .is_err());
    }
    #[test]
    fn notification_payload_without_kind_context_errors() {
        let encoded = r#"{
            "quote": "abc",
            "request": "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
            "unit": "sat",
            "expiry": 1701704757,
            "pubkey": "03d56ce4e446a85bbdaa547b4ec2b073d40ff802831352b8272b7dd7a4de5a7cac",
            "amount_paid": 0,
            "amount_issued": 0
        }"#;

        let err = serde_json::from_str::<NotificationPayload<String>>(encoded).unwrap_err();

        assert!(err
            .to_string()
            .contains("require subscription kind context"));
    }
}
