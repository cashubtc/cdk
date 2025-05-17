//! NUT-18: Payment Requests
//!
//! <https://github.com/cashubtc/nuts/blob/main/18.md>

use std::fmt;
use std::ops::Not;
use std::str::FromStr;

use bitcoin::base64::engine::{general_purpose, GeneralPurpose};
use bitcoin::base64::{alphabet, Engine};
use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::ser::{SerializeTuple, Serializer};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{CurrencyUnit, Nut10Secret, Proofs, SpendingConditions};
use crate::mint_url::MintUrl;
use crate::nuts::nut10::Kind;
use crate::Amount;

const PAYMENT_REQUEST_PREFIX: &str = "creqA";

/// NUT18 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid Prefix
    #[error("Invalid Prefix")]
    InvalidPrefix,
    /// Ciborium error
    #[error(transparent)]
    CiboriumError(#[from] ciborium::de::Error<std::io::Error>),
    /// Base64 error
    #[error(transparent)]
    Base64Error(#[from] bitcoin::base64::DecodeError),
}

/// Transport Type
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransportType {
    /// Nostr
    #[serde(rename = "nostr")]
    Nostr,
    /// Http post
    #[serde(rename = "post")]
    HttpPost,
}

impl fmt::Display for TransportType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use serde::ser::Error;
        let t = serde_json::to_string(self).map_err(|e| fmt::Error::custom(e.to_string()))?;
        write!(f, "{t}")
    }
}

impl FromStr for TransportType {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "nostr" => Ok(Self::Nostr),
            "post" => Ok(Self::HttpPost),
            _ => Err(Error::InvalidPrefix),
        }
    }
}

impl FromStr for Transport {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let decode_config = general_purpose::GeneralPurposeConfig::new()
            .with_decode_padding_mode(bitcoin::base64::engine::DecodePaddingMode::Indifferent);
        let decoded = GeneralPurpose::new(&alphabet::URL_SAFE, decode_config).decode(s)?;

        Ok(ciborium::from_reader(&decoded[..])?)
    }
}

/// Transport
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transport {
    /// Type
    #[serde(rename = "t")]
    pub _type: TransportType,
    /// Target
    #[serde(rename = "a")]
    pub target: String,
    /// Tags
    #[serde(rename = "g")]
    pub tags: Option<Vec<Vec<String>>>,
}

impl Transport {
    /// Create a new TransportBuilder
    pub fn builder() -> TransportBuilder {
        TransportBuilder::default()
    }
}

/// Builder for Transport
#[derive(Debug, Default, Clone)]
pub struct TransportBuilder {
    _type: Option<TransportType>,
    target: Option<String>,
    tags: Option<Vec<Vec<String>>>,
}

impl TransportBuilder {
    /// Set transport type
    pub fn transport_type(mut self, transport_type: TransportType) -> Self {
        self._type = Some(transport_type);
        self
    }

    /// Set target
    pub fn target<S: Into<String>>(mut self, target: S) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Add a tag
    pub fn add_tag(mut self, tag: Vec<String>) -> Self {
        self.tags.get_or_insert_with(Vec::new).push(tag);
        self
    }

    /// Set tags
    pub fn tags(mut self, tags: Vec<Vec<String>>) -> Self {
        self.tags = Some(tags);
        self
    }

    /// Build the Transport
    pub fn build(self) -> Result<Transport, &'static str> {
        let _type = self._type.ok_or("Transport type is required")?;
        let target = self.target.ok_or("Target is required")?;

        Ok(Transport {
            _type,
            target,
            tags: self.tags,
        })
    }
}

impl AsRef<String> for Transport {
    fn as_ref(&self) -> &String {
        &self.target
    }
}

/// Secret Data without nonce for payment requests
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SecretDataRequest {
    /// Expresses the spending condition specific to each kind
    pub data: String,
    /// Additional data committed to and can be used for feature extensions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<Vec<String>>>,
}

/// Nut10Secret without nonce for payment requests
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Nut10SecretRequest {
    /// Kind of the spending condition
    pub kind: Kind,
    /// Secret Data without nonce
    pub secret_data: SecretDataRequest,
}

impl Nut10SecretRequest {
    /// Create a new Nut10SecretRequest
    pub fn new<S, V>(kind: Kind, data: S, tags: Option<V>) -> Self
    where
        S: Into<String>,
        V: Into<Vec<Vec<String>>>,
    {
        let secret_data = SecretDataRequest {
            data: data.into(),
            tags: tags.map(|v| v.into()),
        };

        Self { kind, secret_data }
    }
}

impl From<Nut10Secret> for Nut10SecretRequest {
    fn from(secret: Nut10Secret) -> Self {
        let secret_data = SecretDataRequest {
            data: secret.secret_data.data,
            tags: secret.secret_data.tags,
        };

        Self {
            kind: secret.kind,
            secret_data,
        }
    }
}

impl From<Nut10SecretRequest> for Nut10Secret {
    fn from(value: Nut10SecretRequest) -> Self {
        Self::new(value.kind, value.secret_data.data, value.secret_data.tags)
    }
}

impl From<SpendingConditions> for Nut10SecretRequest {
    fn from(conditions: SpendingConditions) -> Self {
        match conditions {
            SpendingConditions::P2PKConditions { data, conditions } => {
                Self::new(Kind::P2PK, data.to_hex(), conditions)
            }
            SpendingConditions::HTLCConditions { data, conditions } => {
                Self::new(Kind::HTLC, data.to_string(), conditions)
            }
        }
    }
}

impl Serialize for Nut10SecretRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Create a tuple representing the struct fields
        let secret_tuple = (&self.kind, &self.secret_data);

        // Serialize the tuple as a JSON array
        let mut s = serializer.serialize_tuple(2)?;

        s.serialize_element(&secret_tuple.0)?;
        s.serialize_element(&secret_tuple.1)?;
        s.end()
    }
}

// Custom visitor for deserializing Secret
struct SecretVisitor;

impl<'de> Visitor<'de> for SecretVisitor {
    type Value = Nut10SecretRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a tuple with two elements: [Kind, SecretData]")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        // Deserialize the kind (first element)
        let kind = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(0, &self))?;

        // Deserialize the secret_data (second element)
        let secret_data = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(1, &self))?;

        // Make sure there are no additional elements
        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
            return Err(de::Error::invalid_length(3, &self));
        }

        Ok(Nut10SecretRequest { kind, secret_data })
    }
}

impl<'de> Deserialize<'de> for Nut10SecretRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(SecretVisitor)
    }
}

/// Payment Request
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentRequest {
    /// `Payment id`
    #[serde(rename = "i")]
    pub payment_id: Option<String>,
    /// Amount
    #[serde(rename = "a")]
    pub amount: Option<Amount>,
    /// Unit
    #[serde(rename = "u")]
    pub unit: Option<CurrencyUnit>,
    /// Single use
    #[serde(rename = "s")]
    pub single_use: Option<bool>,
    /// Mints
    #[serde(rename = "m")]
    pub mints: Option<Vec<MintUrl>>,
    /// Description
    #[serde(rename = "d")]
    pub description: Option<String>,
    /// Transport
    #[serde(rename = "t")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transports: Option<Vec<Transport>>,
    /// Nut10
    pub nut10: Option<Nut10SecretRequest>,
}

impl PaymentRequest {
    /// Create a new PaymentRequestBuilder
    pub fn builder() -> PaymentRequestBuilder {
        PaymentRequestBuilder::default()
    }
}

/// Builder for PaymentRequest
#[derive(Debug, Default, Clone)]
pub struct PaymentRequestBuilder {
    payment_id: Option<String>,
    amount: Option<Amount>,
    unit: Option<CurrencyUnit>,
    single_use: Option<bool>,
    mints: Option<Vec<MintUrl>>,
    description: Option<String>,
    transports: Vec<Transport>,
    nut10: Option<Nut10SecretRequest>,
}

impl PaymentRequestBuilder {
    /// Set payment ID
    pub fn payment_id<S>(mut self, payment_id: S) -> Self
    where
        S: Into<String>,
    {
        self.payment_id = Some(payment_id.into());
        self
    }

    /// Set amount
    pub fn amount<A>(mut self, amount: A) -> Self
    where
        A: Into<Amount>,
    {
        self.amount = Some(amount.into());
        self
    }

    /// Set unit
    pub fn unit(mut self, unit: CurrencyUnit) -> Self {
        self.unit = Some(unit);
        self
    }

    /// Set single use flag
    pub fn single_use(mut self, single_use: bool) -> Self {
        self.single_use = Some(single_use);
        self
    }

    /// Add a mint URL
    pub fn add_mint(mut self, mint_url: MintUrl) -> Self {
        self.mints.get_or_insert_with(Vec::new).push(mint_url);
        self
    }

    /// Set mints
    pub fn mints(mut self, mints: Vec<MintUrl>) -> Self {
        self.mints = Some(mints);
        self
    }

    /// Set description
    pub fn description<S: Into<String>>(mut self, description: S) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Add a transport
    pub fn add_transport(mut self, transport: Transport) -> Self {
        self.transports.push(transport);
        self
    }

    /// Set transports
    pub fn transports(mut self, transports: Vec<Transport>) -> Self {
        self.transports = transports;
        self
    }

    /// Set Nut10 secret
    pub fn nut10(mut self, nut10: Nut10SecretRequest) -> Self {
        self.nut10 = Some(nut10);
        self
    }

    /// Build the PaymentRequest
    pub fn build(self) -> PaymentRequest {
        let transports = self.transports.is_empty().not().then_some(self.transports);

        PaymentRequest {
            payment_id: self.payment_id,
            amount: self.amount,
            unit: self.unit,
            single_use: self.single_use,
            mints: self.mints,
            description: self.description,
            transports,
            nut10: self.nut10,
        }
    }
}

impl AsRef<Option<String>> for PaymentRequest {
    fn as_ref(&self) -> &Option<String> {
        &self.payment_id
    }
}

impl fmt::Display for PaymentRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use serde::ser::Error;
        let mut data = Vec::new();
        ciborium::into_writer(self, &mut data).map_err(|e| fmt::Error::custom(e.to_string()))?;
        let encoded = general_purpose::URL_SAFE.encode(data);
        write!(f, "{PAYMENT_REQUEST_PREFIX}{encoded}")
    }
}

impl FromStr for PaymentRequest {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s
            .strip_prefix(PAYMENT_REQUEST_PREFIX)
            .ok_or(Error::InvalidPrefix)?;

        let decode_config = general_purpose::GeneralPurposeConfig::new()
            .with_decode_padding_mode(bitcoin::base64::engine::DecodePaddingMode::Indifferent);
        let decoded = GeneralPurpose::new(&alphabet::URL_SAFE, decode_config).decode(s)?;

        Ok(ciborium::from_reader(&decoded[..])?)
    }
}

/// Payment Request
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentRequestPayload {
    /// Id
    pub id: Option<String>,
    /// Memo
    pub memo: Option<String>,
    /// Mint
    pub mint: MintUrl,
    /// Unit
    pub unit: CurrencyUnit,
    /// Proofs
    pub proofs: Proofs,
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use lightning_invoice::Bolt11Invoice;

    use super::*;

    const PAYMENT_REQUEST: &str = "creqApWF0gaNhdGVub3N0cmFheKlucHJvZmlsZTFxeTI4d3VtbjhnaGo3dW45ZDNzaGp0bnl2OWtoMnVld2Q5aHN6OW1od2RlbjV0ZTB3ZmprY2N0ZTljdXJ4dmVuOWVlaHFjdHJ2NWhzenJ0aHdkZW41dGUwZGVoaHh0bnZkYWtxcWd5ZGFxeTdjdXJrNDM5eWtwdGt5c3Y3dWRoZGh1NjhzdWNtMjk1YWtxZWZkZWhrZjBkNDk1Y3d1bmw1YWeBgmFuYjE3YWloYjdhOTAxNzZhYQphdWNzYXRhbYF4Imh0dHBzOi8vbm9mZWVzLnRlc3RudXQuY2FzaHUuc3BhY2U=";

    #[test]
    fn test_decode_payment_req() {
        let req = PaymentRequest::from_str(PAYMENT_REQUEST).expect("valid payment request");

        assert_eq!(&req.payment_id.unwrap(), "b7a90176");
        assert_eq!(req.amount.unwrap(), 10.into());
        assert_eq!(req.unit.clone().unwrap(), CurrencyUnit::Sat);
        assert_eq!(
            req.mints.unwrap(),
            vec![MintUrl::from_str("https://nofees.testnut.cashu.space").expect("valid mint url")]
        );
        assert_eq!(req.unit.unwrap(), CurrencyUnit::Sat);

        let transport = req.transports.unwrap();
        let transport = transport.first().unwrap();

        let expected_transport = Transport {_type: TransportType::Nostr, target: "nprofile1qy28wumn8ghj7un9d3shjtnyv9kh2uewd9hsz9mhwden5te0wfjkccte9curxven9eehqctrv5hszrthwden5te0dehhxtnvdakqqgydaqy7curk439ykptkysv7udhdhu68sucm295akqefdehkf0d495cwunl5".to_string(), tags: Some(vec![vec!["n".to_string(), "17".to_string()]])};

        assert_eq!(transport, &expected_transport);
    }

    #[test]
    fn test_roundtrip_payment_req() {
        let transport = Transport {_type: TransportType::Nostr, target: "nprofile1qy28wumn8ghj7un9d3shjtnyv9kh2uewd9hsz9mhwden5te0wfjkccte9curxven9eehqctrv5hszrthwden5te0dehhxtnvdakqqgydaqy7curk439ykptkysv7udhdhu68sucm295akqefdehkf0d495cwunl5".to_string(), tags: Some(vec![vec!["n".to_string(), "17".to_string()]])};

        let request = PaymentRequest {
            payment_id: Some("b7a90176".to_string()),
            amount: Some(10.into()),
            unit: Some(CurrencyUnit::Sat),
            single_use: None,
            mints: Some(vec!["https://nofees.testnut.cashu.space"
                .parse()
                .expect("valid mint url")]),
            description: None,
            transports: Some(vec![transport.clone()]),
            nut10: None,
        };

        let request_str = request.to_string();

        let req = PaymentRequest::from_str(&request_str).expect("valid payment request");

        assert_eq!(&req.payment_id.unwrap(), "b7a90176");
        assert_eq!(req.amount.unwrap(), 10.into());
        assert_eq!(req.unit.clone().unwrap(), CurrencyUnit::Sat);
        assert_eq!(
            req.mints.unwrap(),
            vec![MintUrl::from_str("https://nofees.testnut.cashu.space").expect("valid mint url")]
        );
        assert_eq!(req.unit.unwrap(), CurrencyUnit::Sat);

        let t = req.transports.unwrap();
        let t = t.first().unwrap();
        assert_eq!(&transport, t);
    }

    #[test]
    fn test_payment_request_builder() {
        let transport = Transport {
            _type: TransportType::Nostr,
            target: "nprofile1qy28wumn8ghj7un9d3shjtnyv9kh2uewd9hsz9mhwden5te0wfjkccte9curxven9eehqctrv5hszrthwden5te0dehhxtnvdakqqgydaqy7curk439ykptkysv7udhdhu68sucm295akqefdehkf0d495cwunl5".to_string(), 
            tags: Some(vec![vec!["n".to_string(), "17".to_string()]])
        };

        let mint_url =
            MintUrl::from_str("https://nofees.testnut.cashu.space").expect("valid mint url");

        // Build a payment request using the builder pattern
        let request = PaymentRequest::builder()
            .payment_id("b7a90176")
            .amount(Amount::from(10))
            .unit(CurrencyUnit::Sat)
            .add_mint(mint_url.clone())
            .add_transport(transport.clone())
            .build();

        // Verify the built request
        assert_eq!(&request.payment_id.clone().unwrap(), "b7a90176");
        assert_eq!(request.amount.unwrap(), 10.into());
        assert_eq!(request.unit.clone().unwrap(), CurrencyUnit::Sat);
        assert_eq!(request.mints.clone().unwrap(), vec![mint_url]);

        let t = request.transports.clone().unwrap();
        let t = t.first().unwrap();
        assert_eq!(&transport, t);

        // Test serialization and deserialization
        let request_str = request.to_string();
        let req = PaymentRequest::from_str(&request_str).expect("valid payment request");

        assert_eq!(req.payment_id, request.payment_id);
        assert_eq!(req.amount, request.amount);
        assert_eq!(req.unit, request.unit);
    }

    #[test]
    fn test_transport_builder() {
        // Build a transport using the builder pattern
        let transport = Transport::builder()
            .transport_type(TransportType::Nostr)
            .target("nprofile1qy28wumn8ghj7un9d3shjtnyv9kh2uewd9hsz9mhwden5te0wfjkccte9curxven9eehqctrv5hszrthwden5te0dehhxtnvdakqqgydaqy7curk439ykptkysv7udhdhu68sucm295akqefdehkf0d495cwunl5")
            .add_tag(vec!["n".to_string(), "17".to_string()])
            .build()
            .expect("Valid transport");

        // Verify the built transport
        assert_eq!(transport._type, TransportType::Nostr);
        assert_eq!(transport.target, "nprofile1qy28wumn8ghj7un9d3shjtnyv9kh2uewd9hsz9mhwden5te0wfjkccte9curxven9eehqctrv5hszrthwden5te0dehhxtnvdakqqgydaqy7curk439ykptkysv7udhdhu68sucm295akqefdehkf0d495cwunl5");
        assert_eq!(
            transport.tags,
            Some(vec![vec!["n".to_string(), "17".to_string()]])
        );

        // Test error case - missing required fields
        let result = TransportBuilder::default().build();
        assert!(result.is_err());
    }

    #[test]
    fn test_nut10_secret_request() {
        use crate::nuts::nut10::Kind;

        // Create a Nut10SecretRequest
        let secret_request = Nut10SecretRequest::new(
            Kind::P2PK,
            "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198",
            Some(vec![vec!["key".to_string(), "value".to_string()]]),
        );

        // Convert to a full Nut10Secret
        let full_secret: Nut10Secret = secret_request.clone().into();

        // Check conversion
        assert_eq!(full_secret.kind, Kind::P2PK);
        assert_eq!(
            full_secret.secret_data.data,
            "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198"
        );
        assert_eq!(
            full_secret.secret_data.tags,
            Some(vec![vec!["key".to_string(), "value".to_string()]])
        );

        // Convert back to Nut10SecretRequest
        let converted_back = Nut10SecretRequest::from(full_secret);

        // Check round-trip conversion
        assert_eq!(converted_back.kind, secret_request.kind);
        assert_eq!(
            converted_back.secret_data.data,
            secret_request.secret_data.data
        );
        assert_eq!(
            converted_back.secret_data.tags,
            secret_request.secret_data.tags
        );

        // Test in PaymentRequest builder
        let payment_request = PaymentRequest::builder()
            .payment_id("test123")
            .amount(Amount::from(100))
            .nut10(secret_request.clone())
            .build();

        assert_eq!(payment_request.nut10, Some(secret_request));
    }

    #[test]
    fn test_nut10_secret_request_multiple_mints() {
        let mint_urls = [
            "https://8333.space:3338",
            "https://mint.minibits.cash/Bitcoin",
            "https://antifiat.cash",
            "https://mint.macadamia.cash",
        ]
        .iter()
        .map(|m| MintUrl::from_str(m).unwrap())
        .collect();

        let payment_request = PaymentRequestBuilder::default()
            .unit(CurrencyUnit::Sat)
            .amount(10)
            .mints(mint_urls)
            .build();

        let payment_request_str = payment_request.to_string();

        let r = PaymentRequest::from_str(&payment_request_str).unwrap();

        assert_eq!(payment_request, r);
    }

    #[test]
    fn test_nut10_secret_request_htlc() {
        let bolt11 = "lnbc100n1p5z3a63pp56854ytysg7e5z9fl3w5mgvrlqjfcytnjv8ff5hm5qt6gl6alxesqdqqcqzzsxqyz5vqsp5p0x0dlhn27s63j4emxnk26p7f94u0lyarnfp5yqmac9gzy4ngdss9qxpqysgqne3v0hnzt2lp0hc69xpzckk0cdcar7glvjhq60lsrfe8gejdm8c564prrnsft6ctxxyrewp4jtezrq3gxxqnfjj0f9tw2qs9y0lslmqpfu7et9";

        let bolt11 = Bolt11Invoice::from_str(bolt11).unwrap();

        let nut10 = SpendingConditions::HTLCConditions {
            data: bolt11.payment_hash().clone(),
            conditions: None,
        };

        let payment_request = PaymentRequestBuilder::default()
            .unit(CurrencyUnit::Sat)
            .amount(10)
            .nut10(nut10.into())
            .build();

        let payment_request_str = payment_request.to_string();

        let r = PaymentRequest::from_str(&payment_request_str).unwrap();

        assert_eq!(payment_request, r);
    }

    #[test]
    fn test_nut10_secret_request_p2pk() {
        // Use a public key for P2PK condition
        let pubkey_hex = "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198";

        // Create P2PK spending conditions
        let nut10 = SpendingConditions::P2PKConditions {
            data: crate::nuts::PublicKey::from_str(pubkey_hex).unwrap(),
            conditions: None,
        };

        // Build payment request with P2PK condition
        let payment_request = PaymentRequestBuilder::default()
            .unit(CurrencyUnit::Sat)
            .amount(10)
            .payment_id("test-p2pk-id")
            .description("P2PK locked payment")
            .nut10(nut10.into())
            .build();

        // Convert to string representation
        let payment_request_str = payment_request.to_string();

        // Parse back from string
        let decoded_request = PaymentRequest::from_str(&payment_request_str).unwrap();

        // Verify round-trip serialization
        assert_eq!(payment_request, decoded_request);

        // Verify the P2PK data was preserved correctly
        if let Some(nut10_secret) = decoded_request.nut10 {
            assert_eq!(nut10_secret.kind, Kind::P2PK);
            assert_eq!(nut10_secret.secret_data.data, pubkey_hex);
        } else {
            panic!("NUT10 secret data missing in decoded payment request");
        }
    }
}
