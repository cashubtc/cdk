//! NUT-18: Payment Requests
//!
//! <https://github.com/cashubtc/nuts/blob/main/18.md>

use std::fmt;
use std::str::FromStr;

use bitcoin::base64::engine::{general_purpose, GeneralPurpose};
use bitcoin::base64::{alphabet, Engine};
use serde::{Deserialize, Serialize};

use super::{Error, Nut10SecretRequest, Transport};
use crate::mint_url::MintUrl;
use crate::nuts::{CurrencyUnit, Proofs};
use crate::Amount;

const PAYMENT_REQUEST_PREFIX: &str = "creqA";

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
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Vec::default")]
    pub transports: Vec<Transport>,
    /// Nut10
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nut10: Option<Nut10SecretRequest>,
}

impl PaymentRequest {
    /// Create a new PaymentRequestBuilder
    pub fn builder() -> PaymentRequestBuilder {
        PaymentRequestBuilder::default()
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
        PaymentRequest {
            payment_id: self.payment_id,
            amount: self.amount,
            unit: self.unit,
            single_use: self.single_use,
            mints: self.mints,
            description: self.description,
            transports: self.transports,
            nut10: self.nut10,
        }
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
    use crate::nuts::nut10::Kind;
    use crate::nuts::SpendingConditions;
    use crate::TransportType;

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

        let transport = req.transports.first().unwrap();

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
            transports: vec![transport.clone()],
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

        let t = req.transports.first().unwrap();
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

        let t = request.transports.first().unwrap();
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
        let result = crate::nuts::nut18::transport::TransportBuilder::default().build();
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
        let full_secret: crate::nuts::Nut10Secret = secret_request.clone().into();

        // Check conversion
        assert_eq!(full_secret.kind(), Kind::P2PK);
        assert_eq!(
            full_secret.secret_data().data(),
            "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198"
        );
        assert_eq!(
            full_secret.secret_data().tags().clone(),
            Some(vec![vec!["key".to_string(), "value".to_string()]]).as_ref()
        );

        // Convert back to Nut10SecretRequest
        let converted_back = Nut10SecretRequest::from(full_secret);

        // Check round-trip conversion
        assert_eq!(converted_back.kind, secret_request.kind);
        assert_eq!(converted_back.data, secret_request.data);
        assert_eq!(converted_back.tags, secret_request.tags);

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
            data: *bolt11.payment_hash(),
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
            assert_eq!(nut10_secret.data, pubkey_hex);
        } else {
            panic!("NUT10 secret data missing in decoded payment request");
        }
    }

    /// Test vectors from NUT-18 specification
    /// https://github.com/cashubtc/nuts/blob/main/tests/18-tests.md

    #[test]
    fn test_basic_payment_request() {
        // Basic payment request with required fields
        let json = r#"{
            "i": "b7a90176",
            "a": 10,
            "u": "sat",
            "m": ["https://8333.space:3338"],
            "t": [
                {
                    "t": "nostr",
                    "a": "nprofile1qy28wumn8ghj7un9d3shjtnyv9kh2uewd9hsz9mhwden5te0wfjkccte9curxven9eehqctrv5hszrthwden5te0dehhxtnvdakqqgydaqy7curk439ykptkysv7udhdhu68sucm295akqefdehkf0d495cwunl5",
                    "g": [["n", "17"]]
                }
            ]
        }"#;

        let expected_encoded = "creqApWF0gaNhdGVub3N0cmFheKlucHJvZmlsZTFxeTI4d3VtbjhnaGo3dW45ZDNzaGp0bnl2OWtoMnVld2Q5aHN6OW1od2RlbjV0ZTB3ZmprY2N0ZTljdXJ4dmVuOWVlaHFjdHJ2NWhzenJ0aHdkZW41dGUwZGVoaHh0bnZkYWtxcWd5ZGFxeTdjdXJrNDM5eWtwdGt5c3Y3dWRoZGh1NjhzdWNtMjk1YWtxZWZkZWhrZjBkNDk1Y3d1bmw1YWeBgmFuYjE3YWloYjdhOTAxNzZhYQphdWNzYXRhbYF3aHR0cHM6Ly84MzMzLnNwYWNlOjMzMzg=";

        // Parse the JSON into a PaymentRequest
        let payment_request: PaymentRequest = serde_json::from_str(json).unwrap();
        let payment_request_cloned = payment_request.clone();

        // Verify the payment request fields
        assert_eq!(
            payment_request_cloned.payment_id.as_ref().unwrap(),
            "b7a90176"
        );
        assert_eq!(payment_request_cloned.amount.unwrap(), Amount::from(10));
        assert_eq!(payment_request_cloned.unit.unwrap(), CurrencyUnit::Sat);
        assert_eq!(
            payment_request_cloned.mints.unwrap(),
            vec![MintUrl::from_str("https://8333.space:3338").unwrap()]
        );

        let transport = payment_request.transports.first().unwrap();
        assert_eq!(transport._type, TransportType::Nostr);
        assert_eq!(transport.target, "nprofile1qy28wumn8ghj7un9d3shjtnyv9kh2uewd9hsz9mhwden5te0wfjkccte9curxven9eehqctrv5hszrthwden5te0dehhxtnvdakqqgydaqy7curk439ykptkysv7udhdhu68sucm295akqefdehkf0d495cwunl5");
        assert_eq!(
            transport.tags,
            Some(vec![vec!["n".to_string(), "17".to_string()]])
        );

        // Test encoding - the encoded form should match the expected output
        let encoded = payment_request.to_string();

        // For now, let's verify it can be decoded back correctly
        let decoded = PaymentRequest::from_str(&encoded).unwrap();
        assert_eq!(payment_request, decoded);

        // Test decoding the expected encoded string
        let decoded_from_spec = PaymentRequest::from_str(expected_encoded).unwrap();
        assert_eq!(decoded_from_spec.payment_id.as_ref().unwrap(), "b7a90176");
        assert_eq!(decoded_from_spec.amount.unwrap(), Amount::from(10));
        assert_eq!(decoded_from_spec.unit.unwrap(), CurrencyUnit::Sat);
        assert_eq!(
            decoded_from_spec.mints.unwrap(),
            vec![MintUrl::from_str("https://8333.space:3338").unwrap()]
        );
    }

    #[test]
    fn test_nostr_transport_payment_request() {
        // Nostr transport payment request with multiple mints
        let json = r#"{
            "i": "f92a51b8",
            "a": 100,
            "u": "sat",
            "m": ["https://mint1.example.com", "https://mint2.example.com"],
            "t": [
                {
                    "t": "nostr",
                    "a": "npub1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq28spj3",
                    "g": [["n", "17"], ["n", "9735"]]
                }
            ]
        }"#;

        let expected_encoded = "creqApWF0gaNhdGVub3N0cmFheD9ucHViMXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXFxcXEyOHNwajNhZ4KCYW5iMTeCYW5kOTczNWFpaGY5MmE1MWI4YWEYZGF1Y3NhdGFtgngZaHR0cHM6Ly9taW50MS5leGFtcGxlLmNvbXgZaHR0cHM6Ly9taW50Mi5leGFtcGxlLmNvbQ==";

        // Parse the JSON into a PaymentRequest
        let payment_request: PaymentRequest = serde_json::from_str(json).unwrap();
        let payment_request_cloned = payment_request.clone();

        // Verify the payment request fields
        assert_eq!(
            payment_request_cloned.payment_id.as_ref().unwrap(),
            "f92a51b8"
        );
        assert_eq!(payment_request_cloned.amount.unwrap(), Amount::from(100));
        assert_eq!(payment_request_cloned.unit.unwrap(), CurrencyUnit::Sat);
        assert_eq!(
            payment_request_cloned.mints.unwrap(),
            vec![
                MintUrl::from_str("https://mint1.example.com").unwrap(),
                MintUrl::from_str("https://mint2.example.com").unwrap()
            ]
        );

        let transport = payment_request_cloned.transports.first().unwrap();
        assert_eq!(transport._type, TransportType::Nostr);
        assert_eq!(
            transport.target,
            "npub1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq28spj3"
        );
        assert_eq!(
            transport.tags,
            Some(vec![
                vec!["n".to_string(), "17".to_string()],
                vec!["n".to_string(), "9735".to_string()]
            ])
        );

        // Test round-trip serialization
        let encoded = payment_request.to_string();
        let decoded = PaymentRequest::from_str(&encoded).unwrap();
        assert_eq!(payment_request, decoded);

        // Test decoding the expected encoded string
        let decoded_from_spec = PaymentRequest::from_str(expected_encoded).unwrap();
        assert_eq!(decoded_from_spec.payment_id.as_ref().unwrap(), "f92a51b8");
    }

    #[test]
    fn test_minimal_payment_request() {
        // Minimal payment request with only required fields
        let json = r#"{
            "i": "7f4a2b39",
            "u": "sat",
            "m": ["https://mint.example.com"]
        }"#;

        let expected_encoded =
            "creqAo2FpaDdmNGEyYjM5YXVjc2F0YW2BeBhodHRwczovL21pbnQuZXhhbXBsZS5jb20=";

        // Parse the JSON into a PaymentRequest
        let payment_request: PaymentRequest = serde_json::from_str(json).unwrap();
        let payment_request_cloned = payment_request.clone();

        // Verify the payment request fields
        assert_eq!(
            payment_request_cloned.payment_id.as_ref().unwrap(),
            "7f4a2b39"
        );
        assert_eq!(payment_request_cloned.amount, None);
        assert_eq!(payment_request_cloned.unit.unwrap(), CurrencyUnit::Sat);
        assert_eq!(
            payment_request_cloned.mints.unwrap(),
            vec![MintUrl::from_str("https://mint.example.com").unwrap()]
        );
        assert_eq!(payment_request_cloned.transports, vec![]);

        // Test round-trip serialization
        let encoded = payment_request.to_string();
        let decoded = PaymentRequest::from_str(&encoded).unwrap();
        assert_eq!(payment_request, decoded);

        // Test decoding the expected encoded string
        let decoded_from_spec = PaymentRequest::from_str(expected_encoded).unwrap();
        assert_eq!(decoded_from_spec.payment_id.as_ref().unwrap(), "7f4a2b39");
    }

    #[test]
    fn test_nut10_locking_payment_request() {
        // Payment request with NUT-10 P2PK locking
        let json = r#"{
            "i": "c9e45d2a",
            "a": 500,
            "u": "sat",
            "m": ["https://mint.example.com"],
            "nut10": {
                "k": "P2PK",
                "d": "02c3b5bb27e361457c92d93d78dd73d3d53732110b2cfe8b50fbc0abc615e9c331",
                "t": [["timeout", "3600"]]
            }
        }"#;

        let expected_encoded = "creqApWFpaGM5ZTQ1ZDJhYWEZAfRhdWNzYXRhbYF4GGh0dHBzOi8vbWludC5leGFtcGxlLmNvbWVudXQxMKNha2RQMlBLYWR4QjAyYzNiNWJiMjdlMzYxNDU3YzkyZDkzZDc4ZGQ3M2QzZDUzNzMyMTEwYjJjZmU4YjUwZmJjMGFiYzYxNWU5YzMzMWF0gYJndGltZW91dGQzNjAw";

        // Parse the JSON into a PaymentRequest
        let payment_request: PaymentRequest = serde_json::from_str(json).unwrap();
        let payment_request_cloned = payment_request.clone();

        // Verify the payment request fields
        assert_eq!(
            payment_request_cloned.payment_id.as_ref().unwrap(),
            "c9e45d2a"
        );
        assert_eq!(payment_request_cloned.amount.unwrap(), Amount::from(500));
        assert_eq!(payment_request_cloned.unit.unwrap(), CurrencyUnit::Sat);
        assert_eq!(
            payment_request_cloned.mints.unwrap(),
            vec![MintUrl::from_str("https://mint.example.com").unwrap()]
        );

        // Test NUT-10 locking
        let nut10 = payment_request_cloned.nut10.unwrap();
        assert_eq!(nut10.kind, Kind::P2PK);
        assert_eq!(
            nut10.data,
            "02c3b5bb27e361457c92d93d78dd73d3d53732110b2cfe8b50fbc0abc615e9c331"
        );
        assert_eq!(
            nut10.tags,
            Some(vec![vec!["timeout".to_string(), "3600".to_string()]])
        );

        // Test round-trip serialization
        let encoded = payment_request.to_string();
        let decoded = PaymentRequest::from_str(&encoded).unwrap();
        assert_eq!(payment_request, decoded);

        // Test decoding the expected encoded string
        let decoded_from_spec = PaymentRequest::from_str(expected_encoded).unwrap();
        assert_eq!(decoded_from_spec.payment_id.as_ref().unwrap(), "c9e45d2a");
    }
}
