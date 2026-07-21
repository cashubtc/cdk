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
use crate::nut26::CREQ_B_HRP;
use crate::nuts::{CurrencyUnit, Proofs};
use crate::Amount;

const PAYMENT_REQUEST_PREFIX: &str = "creqA";

/// Payment method accepted by the receiver for a payment request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SupportedMethod {
    /// Payment method name, such as `bolt11`, `bolt12`, or `onchain`.
    #[serde(rename = "mn")]
    pub method: String,
    /// Additional fee in the request unit for payments from non-preferred mints.
    #[serde(rename = "mf")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee: Option<Amount>,
}

impl SupportedMethod {
    /// Create a supported method without an additional method fee.
    pub fn new<S>(method: S) -> Self
    where
        S: Into<String>,
    {
        Self {
            method: method.into(),
            fee: None,
        }
    }

    /// Create a supported method with an additional method fee.
    pub fn with_fee<S, A>(method: S, fee: A) -> Self
    where
        S: Into<String>,
        A: Into<Amount>,
    {
        Self {
            method: method.into(),
            fee: Some(fee.into()),
        }
    }
}

/// NUT-18 payment request.
///
/// A receiver creates this request to tell a payer how much to send, which
/// mints and payment methods are acceptable, and which transports can deliver
/// the resulting [`PaymentRequestPayload`].
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentRequest {
    /// Payment id to include in the payment payload.
    #[serde(rename = "i")]
    pub payment_id: Option<String>,
    /// Requested amount net of input fees.
    ///
    /// If this is set, [`Self::unit`] must also be set.
    #[serde(rename = "a")]
    pub amount: Option<Amount>,
    /// Unit of the requested amount.
    #[serde(rename = "u")]
    pub unit: Option<CurrencyUnit>,
    /// Whether this request is intended for a single payment.
    #[serde(rename = "s")]
    pub single_use: Option<bool>,
    /// Mint URLs the receiver accepts or prefers.
    ///
    /// If non-empty and [`Self::mint_preferred`] is omitted or `false`, this
    /// list is strict and the payer must only send proofs from these mints. If
    /// [`Self::mint_preferred`] is `true`, this list is advisory and other
    /// mints may be used.
    #[serde(rename = "m")]
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub mints: Vec<MintUrl>,
    /// Whether [`Self::mints`] is preferred instead of strict.
    ///
    /// `true` means the payer should prefer the listed mints but may send from
    /// others. `false` or omitted means the mint list is strict. Ignored when
    /// [`Self::mints`] is empty.
    #[serde(rename = "mp")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mint_preferred: Option<bool>,
    /// Payment methods the payer's mint must support.
    ///
    /// If non-empty, the payer must send ecash from a mint that supports at
    /// least one listed method. Each method can carry a fee that only applies
    /// to payments from non-preferred mints, or from any mint if no mint list is
    /// set.
    #[serde(rename = "sm")]
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub supported_methods: Vec<SupportedMethod>,
    /// Human-readable description for the payer to display.
    #[serde(rename = "d")]
    pub description: Option<String>,
    /// Transports for delivering the payment payload, sorted by preference.
    ///
    /// An empty list means the payment is expected to be delivered in-band by
    /// the surrounding protocol.
    #[serde(rename = "t")]
    #[serde(skip_serializing_if = "Vec::is_empty", default = "Vec::default")]
    pub transports: Vec<Transport>,
    /// Optional NUT-10 locking condition requested for the payment proofs.
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
        // Check if it's a bech32m format (CREQ-B) - case insensitive
        if s.to_lowercase().starts_with(CREQ_B_HRP) {
            // Use the bech32 decoding from NUT-26
            return Self::from_bech32_string(s).map_err(Error::Nut26Error);
        }

        // Otherwise, try the legacy CBOR format (CREQ-A)
        let s = s
            .strip_prefix(PAYMENT_REQUEST_PREFIX)
            .ok_or(Error::InvalidPrefix)?;

        let decode_config = general_purpose::GeneralPurposeConfig::new()
            .with_decode_padding_mode(bitcoin::base64::engine::DecodePaddingMode::Indifferent);
        let decoded = match GeneralPurpose::new(&alphabet::URL_SAFE, decode_config).decode(s) {
            Ok(decoded) => decoded,
            Err(url_safe_err) => {
                let decode_config = general_purpose::GeneralPurposeConfig::new()
                    .with_decode_padding_mode(
                        bitcoin::base64::engine::DecodePaddingMode::Indifferent,
                    );
                GeneralPurpose::new(&alphabet::STANDARD, decode_config)
                    .decode(s)
                    .map_err(|_| url_safe_err)?
            }
        };

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
    mints: Vec<MintUrl>,
    mint_preferred: Option<bool>,
    supported_methods: Vec<SupportedMethod>,
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

    /// Set requested amount.
    ///
    /// Call [`Self::unit`] as well to produce a spec-valid fixed-amount
    /// request.
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
        self.mints.push(mint_url);
        self
    }

    /// Set mint URLs the receiver accepts or prefers.
    ///
    /// Unless [`Self::mint_preferred`] is set to `true`, a non-empty list is
    /// strict.
    pub fn mints(mut self, mints: Vec<MintUrl>) -> Self {
        self.mints = mints;
        self
    }

    /// Set whether the mint list is preferred instead of strict.
    ///
    /// `true` means the payer should prefer listed mints but may use other
    /// mints. `false` means the payer must only use listed mints. Omit this
    /// field to get the same strict behavior as `false`.
    pub fn mint_preferred(mut self, mint_preferred: bool) -> Self {
        self.mint_preferred = Some(mint_preferred);
        self
    }

    /// Set payment methods the payer's mint must support.
    pub fn supported_methods(mut self, methods: Vec<SupportedMethod>) -> Self {
        self.supported_methods = methods;
        self
    }

    /// Add a payment method the payer's mint must support.
    pub fn add_supported_method(mut self, method: SupportedMethod) -> Self {
        self.supported_methods.push(method);
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
            mint_preferred: self.mint_preferred,
            supported_methods: self.supported_methods,
            description: self.description,
            transports: self.transports,
            nut10: self.nut10,
        }
    }
}

/// Payment Request Payload
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

    const PAYMENT_REQUEST: &str = "creqAp2FpaGI3YTkwMTc2YWEKYXVjc2F0YXP2YW2BeCJodHRwczovL25vZmVlcy50ZXN0bnV0LmNhc2h1LnNwYWNlYWT2YXSBo2F0ZW5vc3RyYWF4qW5wcm9maWxlMXFxc2dtNnFmYTNjOGR0ejJmdnpodmZxZWFjbXdtMGU1MHBlM2s1dGZtdnBqam1uMHZqN20ydGdwejNtaHh1ZTY5dWhoeWV0dnY5dWp1ZXJwZDQ2aHh0bmZkdXEzd2Ftbnd2YXo3dG1qdjRreHo3Znc4cWVueHZld3dkY3h6Y205OXVxczZhbW53dmF6N3Rtd2RhZWp1bXIwZHM0bGpoN25hZ4GCYW5iMTc=";

    #[test]
    fn test_decode_payment_req() {
        let req = PaymentRequest::from_str(PAYMENT_REQUEST).expect("valid payment request");

        assert_eq!(&req.payment_id.unwrap(), "b7a90176");
        assert_eq!(req.amount.unwrap(), 10.into());
        assert_eq!(req.unit.clone().unwrap(), CurrencyUnit::Sat);
        assert_eq!(
            req.mints,
            vec![MintUrl::from_str("https://nofees.testnut.cashu.space").expect("valid mint url")]
        );
        assert_eq!(req.unit.unwrap(), CurrencyUnit::Sat);

        let transport = req.transports.first().unwrap();

        let expected_transport = Transport {_type: TransportType::Nostr, target: "nprofile1qqsgm6qfa3c8dtz2fvzhvfqeacmwm0e50pe3k5tfmvpjjmn0vj7m2tgpz3mhxue69uhhyetvv9ujuerpd46hxtnfduq3wamnwvaz7tmjv4kxz7fw8qenxvewwdcxzcm99uqs6amnwvaz7tmwdaejumr0ds4ljh7n".to_string(), tags: vec![vec!["n".to_string(), "17".to_string()]]};

        assert_eq!(transport, &expected_transport);
    }

    #[test]
    fn test_roundtrip_payment_req() {
        let transport = Transport {_type: TransportType::Nostr, target: "nprofile1qqsgm6qfa3c8dtz2fvzhvfqeacmwm0e50pe3k5tfmvpjjmn0vj7m2tgpz3mhxue69uhhyetvv9ujuerpd46hxtnfduq3wamnwvaz7tmjv4kxz7fw8qenxvewwdcxzcm99uqs6amnwvaz7tmwdaejumr0ds4ljh7n".to_string(), tags: vec![vec!["n".to_string(), "17".to_string()]]};

        let request = PaymentRequest {
            payment_id: Some("b7a90176".to_string()),
            amount: Some(10.into()),
            unit: Some(CurrencyUnit::Sat),
            single_use: None,
            mints: vec!["https://nofees.testnut.cashu.space"
                .parse()
                .expect("valid mint url")],
            mint_preferred: None,
            supported_methods: vec![],
            description: None,
            transports: vec![transport.clone()],
            nut10: None,
        };

        let request_str = request.to_string();

        assert_eq!(request_str, PAYMENT_REQUEST);

        let req = PaymentRequest::from_str(&request_str).expect("valid payment request");

        assert_eq!(&req.payment_id.unwrap(), "b7a90176");
        assert_eq!(req.amount.unwrap(), 10.into());
        assert_eq!(req.unit.clone().unwrap(), CurrencyUnit::Sat);
        assert_eq!(
            req.mints,
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
            target: "nprofile1qqsgm6qfa3c8dtz2fvzhvfqeacmwm0e50pe3k5tfmvpjjmn0vj7m2tgpz3mhxue69uhhyetvv9ujuerpd46hxtnfduq3wamnwvaz7tmjv4kxz7fw8qenxvewwdcxzcm99uqs6amnwvaz7tmwdaejumr0ds4ljh7n".to_string(),
            tags: vec![vec!["n".to_string(), "17".to_string()]]
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
        assert_eq!(request.mints.clone(), vec![mint_url]);

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
            .target("nprofile1qqsgm6qfa3c8dtz2fvzhvfqeacmwm0e50pe3k5tfmvpjjmn0vj7m2tgpz3mhxue69uhhyetvv9ujuerpd46hxtnfduq3wamnwvaz7tmjv4kxz7fw8qenxvewwdcxzcm99uqs6amnwvaz7tmwdaejumr0ds4ljh7n")
            .add_tag(vec!["n".to_string(), "17".to_string()])
            .build()
            .expect("Valid transport");

        // Verify the built transport
        assert_eq!(transport._type, TransportType::Nostr);
        assert_eq!(transport.target, "nprofile1qqsgm6qfa3c8dtz2fvzhvfqeacmwm0e50pe3k5tfmvpjjmn0vj7m2tgpz3mhxue69uhhyetvv9ujuerpd46hxtnfduq3wamnwvaz7tmjv4kxz7fw8qenxvewwdcxzcm99uqs6amnwvaz7tmwdaejumr0ds4ljh7n");
        assert_eq!(
            transport.tags,
            vec![vec!["n".to_string(), "17".to_string()]]
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
    fn test_mint_preferred_serializes_as_mp() {
        let payment_request = PaymentRequestBuilder::default()
            .mints(vec![MintUrl::from_str("https://mint.example.com").unwrap()])
            .mint_preferred(true)
            .build();

        let value = serde_json::to_value(&payment_request).unwrap();

        assert_eq!(value.get("mp"), Some(&serde_json::Value::Bool(true)));
        assert!(value.get("ms").is_none());

        let decoded: PaymentRequest = serde_json::from_value(serde_json::json!({
            "m": ["https://mint.example.com"],
            "mp": false
        }))
        .unwrap();
        assert_eq!(decoded.mint_preferred, Some(false));
    }

    #[test]
    fn test_supported_methods_serialize_as_method_objects() {
        let payment_request = PaymentRequestBuilder::default()
            .add_supported_method(SupportedMethod::new("bolt11"))
            .add_supported_method(SupportedMethod::with_fee("bolt12", 5))
            .build();

        let value = serde_json::to_value(&payment_request).unwrap();
        assert_eq!(
            value.get("sm"),
            Some(&serde_json::json!([
                { "mn": "bolt11" },
                { "mn": "bolt12", "mf": 5 }
            ]))
        );

        let decoded: PaymentRequest = serde_json::from_value(serde_json::json!({
            "sm": [
                { "mn": "bolt11" },
                { "mn": "bolt12", "mf": 5 }
            ]
        }))
        .unwrap();
        assert_eq!(
            decoded.supported_methods,
            vec![
                SupportedMethod::new("bolt11"),
                SupportedMethod::with_fee("bolt12", 5),
            ]
        );
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
                    "a": "nprofile1qqsgm6qfa3c8dtz2fvzhvfqeacmwm0e50pe3k5tfmvpjjmn0vj7m2tgpz3mhxue69uhhyetvv9ujuerpd46hxtnfduq3wamnwvaz7tmjv4kxz7fw8qenxvewwdcxzcm99uqs6amnwvaz7tmwdaejumr0ds4ljh7n",
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
            payment_request_cloned.mints,
            vec![MintUrl::from_str("https://8333.space:3338").unwrap()]
        );

        let transport = payment_request.transports.first().unwrap();
        assert_eq!(transport._type, TransportType::Nostr);
        assert_eq!(transport.target, "nprofile1qqsgm6qfa3c8dtz2fvzhvfqeacmwm0e50pe3k5tfmvpjjmn0vj7m2tgpz3mhxue69uhhyetvv9ujuerpd46hxtnfduq3wamnwvaz7tmjv4kxz7fw8qenxvewwdcxzcm99uqs6amnwvaz7tmwdaejumr0ds4ljh7n");
        assert_eq!(
            transport.tags,
            vec![vec!["n".to_string(), "17".to_string()]]
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
            decoded_from_spec.mints,
            vec![MintUrl::from_str("https://8333.space:3338").unwrap()]
        );
    }

    #[test]
    fn test_complete_payment_request() {
        // Complete payment request with all optional fields included
        let expected_encoded = "creqAqGF0gaNhdGRwb3N0YWF4G2h0dHBzOi8vYXBpLmV4YW1wbGUuY29tL3BheWFn92FpaDQ4NDBmNTFlYWEZA+hhdWNzYXRhbYF4GGh0dHBzOi8vbWludC5leGFtcGxlLmNvbWFkcFByb2R1Y3QgcHVyY2hhc2Vhc/VlbnV0MTCjYWtkUDJQS2FkeEIwM2JhZjBjM2FjMjIwMzY2YzJjMzk3YmY5MzA1NzljNDE2MzQzNTU4NGY1NzNiMTA5MTA5ODdjNTQ0YzU5ZTYxZjFhdIGCZ3B1cnBvc2Vnb2ZmbGluZQ==";

        let decoded_from_spec = PaymentRequest::from_str(expected_encoded).unwrap();

        assert_eq!(decoded_from_spec.payment_id, Some("4840f51e".to_string()));
        assert_eq!(decoded_from_spec.amount, Some(Amount::from(1000)));
        assert_eq!(decoded_from_spec.unit, Some(CurrencyUnit::Sat));
        assert_eq!(decoded_from_spec.single_use, Some(true));
        assert_eq!(
            decoded_from_spec.mints,
            vec![MintUrl::from_str("https://mint.example.com").unwrap()]
        );
        assert_eq!(
            decoded_from_spec.description,
            Some("Product purchase".to_string())
        );
        assert_eq!(decoded_from_spec.transports.len(), 1);
        assert_eq!(
            decoded_from_spec.transports[0]._type,
            TransportType::HttpPost
        );
        assert_eq!(
            decoded_from_spec.transports[0].target,
            "https://api.example.com/pay"
        );

        let nut10 = decoded_from_spec.nut10.expect("nut10");
        assert_eq!(nut10.kind, Kind::P2PK);
        assert_eq!(
            nut10.data,
            "03baf0c3ac220366c2c397bf930579c4163435584f573b10910987c544c59e61f1"
        );
        assert_eq!(
            nut10.tags,
            Some(vec![vec!["purpose".to_string(), "offline".to_string()]])
        );
    }

    #[test]
    fn test_http_transport_payment_request() {
        // HTTP POST transport payment request
        let expected_encoded = "creqApWF0gaNhdGRwb3N0YWF4H2h0dHBzOi8vYXBpLmV4YW1wbGUuY29tL3JlY2VpdmVhZ/dhaWhhMmMxMmY0NWFhGDJhdWNzYXRhbYF4GWh0dHBzOi8vY2FzaHUuZXhhbXBsZS5jb20=";

        let decoded_from_spec = PaymentRequest::from_str(expected_encoded).unwrap();

        assert_eq!(decoded_from_spec.payment_id, Some("a2c12f45".to_string()));
        assert_eq!(decoded_from_spec.amount, Some(Amount::from(50)));
        assert_eq!(decoded_from_spec.unit, Some(CurrencyUnit::Sat));
        assert_eq!(
            decoded_from_spec.mints,
            vec![MintUrl::from_str("https://cashu.example.com").unwrap()]
        );
        assert_eq!(decoded_from_spec.transports.len(), 1);
        assert_eq!(
            decoded_from_spec.transports[0]._type,
            TransportType::HttpPost
        );
        assert_eq!(
            decoded_from_spec.transports[0].target,
            "https://api.example.com/receive"
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
            payment_request_cloned.mints,
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
            vec![
                vec!["n".to_string(), "17".to_string()],
                vec!["n".to_string(), "9735".to_string()]
            ]
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
            payment_request_cloned.mints,
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
            payment_request_cloned.mints,
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

    #[test]
    fn test_preferred_mint_list_with_supported_methods() {
        // Preferred mint list with supported methods and per-method fee
        let expected_encoded = "creqApmFpdXByZWZlcnJlZF9mZWVfbWV0aG9kc2FhGGRhdWNzYXRhbYF4GGh0dHBzOi8vbWludC5leGFtcGxlLmNvbWJtcPVic22CoWJtbmZib2x0MTGiYm1uZmJvbHQxMmJtZgU=";

        let decoded_from_spec = PaymentRequest::from_str(expected_encoded).unwrap();

        assert_eq!(
            decoded_from_spec.payment_id,
            Some("preferred_fee_methods".to_string())
        );
        assert_eq!(decoded_from_spec.amount, Some(Amount::from(100)));
        assert_eq!(decoded_from_spec.unit, Some(CurrencyUnit::Sat));
        assert_eq!(
            decoded_from_spec.mints,
            vec![MintUrl::from_str("https://mint.example.com").unwrap()]
        );
        assert_eq!(decoded_from_spec.mint_preferred, Some(true));
        assert_eq!(
            decoded_from_spec.supported_methods,
            vec![
                SupportedMethod::new("bolt11"),
                SupportedMethod::with_fee("bolt12", 5),
            ]
        );
    }

    #[test]
    fn test_from_str_handles_both_formats() {
        // Create a payment request
        let payment_request = PaymentRequest {
            payment_id: Some("test456".to_string()),
            amount: Some(Amount::from(100)),
            unit: Some(CurrencyUnit::Sat),
            single_use: None,
            mints: vec![MintUrl::from_str("https://mint.example.com").unwrap()],
            mint_preferred: None,
            supported_methods: vec![],
            description: Some("Test both formats".to_string()),
            transports: vec![],
            nut10: None,
        };

        // Test CBOR format (CREQ-A) - from Display trait
        let cbor_encoded = payment_request.to_string();
        assert!(cbor_encoded.starts_with("creqA"));
        let decoded_cbor =
            PaymentRequest::from_str(&cbor_encoded).expect("Should decode CBOR format");
        assert_eq!(decoded_cbor.payment_id, payment_request.payment_id);
        assert_eq!(decoded_cbor.amount, payment_request.amount);
        assert_eq!(decoded_cbor.unit, payment_request.unit);
        assert_eq!(decoded_cbor.description, payment_request.description);

        // Test bech32 format (CREQ-B)
        let bech32_encoded = payment_request
            .to_bech32_string()
            .expect("Should encode to bech32");
        assert!(bech32_encoded.to_uppercase().starts_with("CREQB"));
        let decoded_bech32 =
            PaymentRequest::from_str(&bech32_encoded).expect("Should decode bech32 format");
        assert_eq!(decoded_bech32.payment_id, payment_request.payment_id);
        assert_eq!(decoded_bech32.amount, payment_request.amount);
        assert_eq!(decoded_bech32.unit, payment_request.unit);
        assert_eq!(decoded_bech32.description, payment_request.description);

        // Test case insensitivity for bech32
        let bech32_lowercase = bech32_encoded.to_lowercase();
        let decoded_lowercase =
            PaymentRequest::from_str(&bech32_lowercase).expect("Should decode lowercase bech32");
        assert_eq!(decoded_lowercase.payment_id, payment_request.payment_id);

        let bech32_uppercase = bech32_encoded.to_uppercase();
        let decoded_uppercase =
            PaymentRequest::from_str(&bech32_uppercase).expect("Should decode uppercase bech32");
        assert_eq!(decoded_uppercase.payment_id, payment_request.payment_id);
    }

    #[test]
    fn builder_preserves_optional_fields_and_as_ref_targets_payment_id() {
        let mint = MintUrl::from_str("https://mint.example.com").unwrap();
        let transport = Transport::builder()
            .transport_type(TransportType::HttpPost)
            .target("https://wallet.example.com/callback")
            .build()
            .unwrap();

        let request = PaymentRequest::builder()
            .payment_id("payment-123")
            .amount(Amount::from(21))
            .unit(CurrencyUnit::Sat)
            .single_use(true)
            .mints(vec![mint.clone()])
            .description("coffee")
            .transports(vec![transport.clone()])
            .build();

        assert_eq!(request.as_ref().as_deref(), Some("payment-123"));
        assert_eq!(request.amount, Some(Amount::from(21)));
        assert_eq!(request.unit, Some(CurrencyUnit::Sat));
        assert_eq!(request.single_use, Some(true));
        assert_eq!(request.mints, vec![mint]);
        assert_eq!(request.description.as_deref(), Some("coffee"));
        assert_eq!(request.transports, vec![transport]);
    }
}
