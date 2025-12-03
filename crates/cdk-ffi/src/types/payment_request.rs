//! Payment Request FFI types (NUT-18)

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::amount::{Amount, CurrencyUnit};
use crate::error::FfiError;

/// Transport type for payment request delivery
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum TransportType {
    /// Nostr transport (privacy-preserving)
    Nostr,
    /// HTTP POST transport
    HttpPost,
}

impl From<cdk::nuts::TransportType> for TransportType {
    fn from(t: cdk::nuts::TransportType) -> Self {
        match t {
            cdk::nuts::TransportType::Nostr => TransportType::Nostr,
            cdk::nuts::TransportType::HttpPost => TransportType::HttpPost,
        }
    }
}

impl From<TransportType> for cdk::nuts::TransportType {
    fn from(t: TransportType) -> Self {
        match t {
            TransportType::Nostr => cdk::nuts::TransportType::Nostr,
            TransportType::HttpPost => cdk::nuts::TransportType::HttpPost,
        }
    }
}

/// Transport for payment request delivery
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct Transport {
    /// Transport type
    pub transport_type: TransportType,
    /// Target (e.g., nprofile for Nostr, URL for HTTP)
    pub target: String,
    /// Optional tags
    pub tags: Option<Vec<Vec<String>>>,
}

impl From<cdk::nuts::Transport> for Transport {
    fn from(t: cdk::nuts::Transport) -> Self {
        Self {
            transport_type: t._type.into(),
            target: t.target,
            tags: t.tags,
        }
    }
}

impl From<Transport> for cdk::nuts::Transport {
    fn from(t: Transport) -> Self {
        Self {
            _type: t.transport_type.into(),
            target: t.target,
            tags: t.tags,
        }
    }
}

/// NUT-18 Payment Request
///
/// A payment request that can be shared to request Cashu tokens.
/// Encoded as a string with the `creqA` prefix.
#[derive(uniffi::Object)]
pub struct PaymentRequest {
    inner: cdk::nuts::PaymentRequest,
}

impl PaymentRequest {
    /// Create from inner CDK type
    pub(crate) fn from_inner(inner: cdk::nuts::PaymentRequest) -> Self {
        Self { inner }
    }

    /// Get inner reference
    pub(crate) fn inner(&self) -> &cdk::nuts::PaymentRequest {
        &self.inner
    }
}

#[uniffi::export]
impl PaymentRequest {
    /// Parse a payment request from its encoded string representation
    #[uniffi::constructor]
    pub fn from_string(encoded: String) -> Result<Arc<Self>, FfiError> {
        use std::str::FromStr;
        let inner = cdk::nuts::PaymentRequest::from_str(&encoded)
            .map_err(|e| FfiError::Generic { msg: e.to_string() })?;
        Ok(Arc::new(Self { inner }))
    }

    /// Encode the payment request to a string
    pub fn to_string_encoded(&self) -> String {
        self.inner.to_string()
    }

    /// Get the payment ID
    pub fn payment_id(&self) -> Option<String> {
        self.inner.payment_id.clone()
    }

    /// Get the requested amount
    pub fn amount(&self) -> Option<Amount> {
        self.inner.amount.map(|a| a.into())
    }

    /// Get the currency unit
    pub fn unit(&self) -> Option<CurrencyUnit> {
        self.inner.unit.clone().map(|u| u.into())
    }

    /// Get whether this is a single-use request
    pub fn single_use(&self) -> Option<bool> {
        self.inner.single_use
    }

    /// Get the list of acceptable mint URLs
    pub fn mints(&self) -> Option<Vec<String>> {
        self.inner
            .mints
            .as_ref()
            .map(|mints| mints.iter().map(|m| m.to_string()).collect())
    }

    /// Get the description
    pub fn description(&self) -> Option<String> {
        self.inner.description.clone()
    }

    /// Get the transports for delivering the payment
    pub fn transports(&self) -> Vec<Transport> {
        self.inner
            .transports
            .iter()
            .cloned()
            .map(|t| t.into())
            .collect()
    }
}

/// Parameters for creating a NUT-18 payment request
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct CreateRequestParams {
    /// Optional amount to request (in smallest unit for the currency)
    pub amount: Option<u64>,
    /// Currency unit (e.g., "sat", "msat", "usd")
    pub unit: String,
    /// Optional description for the request
    pub description: Option<String>,
    /// Optional public keys for P2PK spending conditions (hex-encoded)
    pub pubkeys: Option<Vec<String>>,
    /// Required number of signatures for multisig (defaults to 1)
    pub num_sigs: u64,
    /// Optional HTLC hash (hex-encoded SHA-256)
    pub hash: Option<String>,
    /// Optional HTLC preimage (alternative to hash)
    pub preimage: Option<String>,
    /// Transport type: "nostr", "http", or "none"
    pub transport: String,
    /// HTTP URL for HTTP transport (required if transport is "http")
    pub http_url: Option<String>,
    /// Nostr relay URLs (required if transport is "nostr")
    pub nostr_relays: Option<Vec<String>>,
}

impl Default for CreateRequestParams {
    fn default() -> Self {
        Self {
            amount: None,
            unit: "sat".to_string(),
            description: None,
            pubkeys: None,
            num_sigs: 1,
            hash: None,
            preimage: None,
            transport: "none".to_string(),
            http_url: None,
            nostr_relays: None,
        }
    }
}

impl From<CreateRequestParams> for cdk::wallet::payment_request::CreateRequestParams {
    fn from(params: CreateRequestParams) -> Self {
        Self {
            amount: params.amount,
            unit: params.unit,
            description: params.description,
            pubkeys: params.pubkeys,
            num_sigs: params.num_sigs,
            hash: params.hash,
            preimage: params.preimage,
            transport: params.transport,
            http_url: params.http_url,
            nostr_relays: params.nostr_relays,
        }
    }
}

impl From<cdk::wallet::payment_request::CreateRequestParams> for CreateRequestParams {
    fn from(params: cdk::wallet::payment_request::CreateRequestParams) -> Self {
        Self {
            amount: params.amount,
            unit: params.unit,
            description: params.description,
            pubkeys: params.pubkeys,
            num_sigs: params.num_sigs,
            hash: params.hash,
            preimage: params.preimage,
            transport: params.transport,
            http_url: params.http_url,
            nostr_relays: params.nostr_relays,
        }
    }
}

/// Decode a payment request from its encoded string representation
#[uniffi::export]
pub fn decode_payment_request(encoded: String) -> Result<Arc<PaymentRequest>, FfiError> {
    PaymentRequest::from_string(encoded)
}

/// Encode CreateRequestParams to JSON string
#[uniffi::export]
pub fn encode_create_request_params(params: CreateRequestParams) -> Result<String, FfiError> {
    Ok(serde_json::to_string(&params)?)
}

/// Decode CreateRequestParams from JSON string
#[uniffi::export]
pub fn decode_create_request_params(json: String) -> Result<CreateRequestParams, FfiError> {
    Ok(serde_json::from_str(&json)?)
}

/// Information needed to wait for an incoming Nostr payment
///
/// Returned by `create_request` when the transport is `nostr`. Pass this to
/// `wait_for_nostr_payment` to connect, subscribe, and receive the incoming
/// payment on the specified relays.
#[derive(uniffi::Object)]
pub struct NostrWaitInfo {
    inner: cdk::wallet::payment_request::NostrWaitInfo,
}

impl NostrWaitInfo {
    /// Create from inner CDK type
    pub(crate) fn from_inner(inner: cdk::wallet::payment_request::NostrWaitInfo) -> Self {
        Self { inner }
    }

    /// Get inner reference
    pub(crate) fn inner(&self) -> &cdk::wallet::payment_request::NostrWaitInfo {
        &self.inner
    }
}

#[uniffi::export]
impl NostrWaitInfo {
    /// Get the Nostr relays to connect to
    pub fn relays(&self) -> Vec<String> {
        self.inner.relays.clone()
    }

    /// Get the recipient public key as a hex string
    pub fn pubkey(&self) -> String {
        self.inner.pubkey.to_hex()
    }
}

/// Result of creating a payment request
///
/// Contains the payment request and optionally the Nostr wait info
/// if the transport was set to "nostr".
#[derive(uniffi::Record)]
pub struct CreateRequestResult {
    /// The payment request to share with the payer
    pub payment_request: Arc<PaymentRequest>,
    /// Nostr wait info (present when transport is "nostr")
    pub nostr_wait_info: Option<Arc<NostrWaitInfo>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYMENT_REQUEST: &str = "creqApWF0gaNhdGVub3N0cmFheKlucHJvZmlsZTFxeTI4d3VtbjhnaGo3dW45ZDNzaGp0bnl2OWtoMnVld2Q5aHN6OW1od2RlbjV0ZTB3ZmprY2N0ZTljdXJ4dmVuOWVlaHFjdHJ2NWhzenJ0aHdkZW41dGUwZGVoaHh0bnZkYWtxcWd5ZGFxeTdjdXJrNDM5eWtwdGt5c3Y3dWRoZGh1NjhzdWNtMjk1YWtxZWZkZWhrZjBkNDk1Y3d1bmw1YWeBgmFuYjE3YWloYjdhOTAxNzZhYQphdWNzYXRhbYF4Imh0dHBzOi8vbm9mZWVzLnRlc3RudXQuY2FzaHUuc3BhY2U=";

    #[test]
    fn test_decode_payment_request() {
        let req = PaymentRequest::from_string(PAYMENT_REQUEST.to_string()).unwrap();

        assert_eq!(req.payment_id().unwrap(), "b7a90176");
        assert_eq!(req.amount().unwrap().value, 10);
        assert!(matches!(req.unit().unwrap(), CurrencyUnit::Sat));

        let mints = req.mints().unwrap();
        assert_eq!(mints.len(), 1);
        assert_eq!(mints[0], "https://nofees.testnut.cashu.space");

        let transports = req.transports();
        assert_eq!(transports.len(), 1);
        assert!(matches!(transports[0].transport_type, TransportType::Nostr));
    }

    #[test]
    fn test_roundtrip_payment_request() {
        let req = PaymentRequest::from_string(PAYMENT_REQUEST.to_string()).unwrap();
        let encoded = req.to_string_encoded();
        let decoded = PaymentRequest::from_string(encoded).unwrap();

        assert_eq!(req.payment_id(), decoded.payment_id());
        assert_eq!(
            req.amount().map(|a| a.value),
            decoded.amount().map(|a| a.value)
        );
    }

    #[test]
    fn test_transport_conversion() {
        let ffi_transport = Transport {
            transport_type: TransportType::Nostr,
            target: "nprofile1...".to_string(),
            tags: Some(vec![vec!["n".to_string(), "17".to_string()]]),
        };

        let cdk_transport: cdk::nuts::Transport = ffi_transport.clone().into();
        let back: Transport = cdk_transport.into();

        assert_eq!(ffi_transport.transport_type, back.transport_type);
        assert_eq!(ffi_transport.target, back.target);
        assert_eq!(ffi_transport.tags, back.tags);
    }

    #[test]
    fn test_create_request_params_default() {
        let params = CreateRequestParams::default();

        assert_eq!(params.unit, "sat");
        assert_eq!(params.num_sigs, 1);
        assert_eq!(params.transport, "none");
        assert!(params.amount.is_none());
    }

    #[test]
    fn test_create_request_params_serialization() {
        let params = CreateRequestParams {
            amount: Some(100),
            unit: "sat".to_string(),
            description: Some("Test payment".to_string()),
            transport: "http".to_string(),
            http_url: Some("https://example.com/callback".to_string()),
            ..Default::default()
        };

        let json = encode_create_request_params(params.clone()).unwrap();
        let decoded = decode_create_request_params(json).unwrap();

        assert_eq!(params.amount, decoded.amount);
        assert_eq!(params.unit, decoded.unit);
        assert_eq!(params.description, decoded.description);
    }
}
