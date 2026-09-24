//! 22 Blind Auth

use std::fmt;

use bitcoin::base64::engine::general_purpose::{self, GeneralPurposeConfig};
use bitcoin::base64::engine::GeneralPurpose;
use bitcoin::base64::{alphabet, Engine};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::nut21::ProtectedEndpoint;
use crate::dhke::hash_to_curve_for_version;
use crate::secret::Secret;
use crate::util::hex;
use crate::{BlindedMessage, Id, Proof, ProofDleq, PublicKey};

/// NUT22 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid request-bound Nutroot authorization.
    #[error(transparent)]
    Nutroot(#[from] crate::nuts::nut10::nutroot::Error),
    /// Invalid Prefix
    #[error("Invalid prefix")]
    InvalidPrefix,
    /// Dleq proof not included
    #[error("Dleq Proof not included for auth proof")]
    DleqProofNotIncluded,
    /// Hex Error
    #[error(transparent)]
    HexError(#[from] hex::Error),
    /// Base64 error
    #[error(transparent)]
    Base64Error(#[from] bitcoin::base64::DecodeError),
    /// Serde Json error
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
    /// Utf8 parse error
    #[error(transparent)]
    Utf8ParseError(#[from] std::string::FromUtf8Error),
    /// DHKE error
    #[error(transparent)]
    DHKE(#[from] crate::dhke::Error),
}

/// Blind auth settings
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize)]
pub struct Settings {
    /// Max number of blind auth tokens that can be minted per request
    pub bat_max_mint: u64,
    /// Protected endpoints
    pub protected_endpoints: Vec<ProtectedEndpoint>,
}

impl Settings {
    /// Create new [`Settings`]
    pub fn new(bat_max_mint: u64, protected_endpoints: Vec<ProtectedEndpoint>) -> Self {
        Self {
            bat_max_mint,
            protected_endpoints,
        }
    }
}

// Custom deserializer for Settings to expand patterns in protected endpoints
impl<'de> Deserialize<'de> for Settings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use std::collections::HashSet;

        use super::nut21::matching_route_paths;

        // Define a temporary struct to deserialize the raw data
        #[derive(Deserialize)]
        struct RawSettings {
            bat_max_mint: u64,
            protected_endpoints: Vec<RawProtectedEndpoint>,
        }

        #[derive(Deserialize)]
        struct RawProtectedEndpoint {
            method: super::nut21::Method,
            path: String,
        }

        // Deserialize into the temporary struct
        let raw = RawSettings::deserialize(deserializer)?;

        // Process protected endpoints, expanding patterns if present
        let mut protected_endpoints = HashSet::new();

        for raw_endpoint in raw.protected_endpoints {
            let expanded_paths = matching_route_paths(&raw_endpoint.path).map_err(|e| {
                serde::de::Error::custom(format!("Invalid pattern '{}': {}", raw_endpoint.path, e))
            })?;

            for path in expanded_paths {
                protected_endpoints.insert(super::nut21::ProtectedEndpoint::new(
                    raw_endpoint.method,
                    path,
                ));
            }
        }

        // Create the final Settings struct
        Ok(Settings {
            bat_max_mint: raw.bat_max_mint,
            protected_endpoints: protected_endpoints.into_iter().collect(),
        })
    }
}

/// Auth Token
// Keep the existing public unboxed variants for API compatibility.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthToken {
    /// Clear Auth token
    ClearAuth(String),
    /// Blind Auth token
    BlindAuth(BlindAuthToken),
}

impl fmt::Display for AuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClearAuth(cat) => cat.fmt(f),
            Self::BlindAuth(bat) => bat.fmt(f),
        }
    }
}

impl AuthToken {
    /// Bind a version-02 BAT to the bytes the transport will actually send.
    pub fn bind_request(&mut self, method: &str, target: &str, body: &[u8]) -> Result<(), Error> {
        if let Self::BlindAuth(token) = self {
            token.sign_request(method, target, body)?;
        }
        Ok(())
    }

    /// Header key for auth token type
    pub fn header_key(&self) -> String {
        match self {
            Self::ClearAuth(_) => "Clear-auth".to_string(),
            Self::BlindAuth(_) => "Blind-auth".to_string(),
        }
    }
}

/// Required Auth
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AuthRequired {
    /// Clear Auth token
    Clear,
    /// Blind Auth token
    Blind,
}

/// Auth Proofs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthProof {
    /// Key-path witness over the exact authorized HTTP request for version 02.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witness: Option<String>,
    /// `Keyset id`
    #[serde(rename = "id")]
    pub keyset_id: Id,
    /// Secret message
    pub secret: Secret,
    /// Unblinded signature
    #[serde(rename = "C")]
    pub c: PublicKey,
    /// Auth Proof Dleq
    pub dleq: Option<ProofDleq>,
}

impl AuthProof {
    /// Y of AuthProof
    pub fn y(&self) -> Result<PublicKey, Error> {
        Ok(hash_to_curve_for_version(
            self.secret.as_bytes(),
            self.keyset_id.get_version(),
        )?)
    }
}

impl From<AuthProof> for Proof {
    fn from(value: AuthProof) -> Self {
        Self {
            amount: 1.into(),
            keyset_id: value.keyset_id,
            secret: value.secret,
            c: value.c,
            witness: value.witness.map(crate::nuts::Witness::NutrootWitness),
            dleq: value.dleq,
            p2pk_e: None,
            spend_info: None,
        }
    }
}

impl TryFrom<Proof> for AuthProof {
    type Error = Error;
    fn try_from(value: Proof) -> Result<Self, Self::Error> {
        let witness = match value.witness {
            Some(crate::nuts::Witness::NutrootWitness(raw)) => Some(raw),
            None => None,
            Some(_) => return Err(crate::nuts::nut10::nutroot::Error::InvalidWitness.into()),
        };
        Ok(Self {
            witness,
            keyset_id: value.keyset_id,
            secret: value.secret,
            c: value.c,
            dleq: value.dleq,
        })
    }
}

/// Blind Auth Token
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlindAuthToken {
    #[serde(skip)]
    signing_key: Option<bitcoin::secp256k1::SecretKey>,
    #[serde(skip)]
    request_digest: Option<[u8; 32]>,
    /// [AuthProof]
    pub auth_proof: AuthProof,
}

impl fmt::Debug for BlindAuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BlindAuthToken([REDACTED])")
    }
}

impl BlindAuthToken {
    /// Construct an outgoing token and retain its private key only in memory.
    /// Serialization never includes that key or a trusted request digest.
    pub fn from_proof(proof: Proof) -> Result<Self, Error> {
        let signing_key = if proof.keyset_id.get_version() == crate::nuts::KeySetVersion::Version02
        {
            Some(
                proof
                    .spend_info
                    .as_ref()
                    .ok_or(crate::nuts::nut10::nutroot::Error::InvalidSpendInfo)?
                    .key_path_key(&proof.secret.to_string(), None)?,
            )
        } else {
            None
        };
        Ok(Self {
            auth_proof: proof.try_into()?,
            signing_key,
            request_digest: None,
        })
    }

    /// Sign the exact outgoing HTTP request. A received token has no private key.
    pub fn sign_request(&mut self, method: &str, target: &str, body: &[u8]) -> Result<(), Error> {
        if self.auth_proof.keyset_id.get_version() != crate::nuts::KeySetVersion::Version02 {
            return Ok(());
        }
        let digest = crate::nuts::nut10::nutroot::authorized_request_digest(method, target, body)?;
        let key = self
            .signing_key
            .as_ref()
            .ok_or(crate::nuts::nut10::nutroot::Error::InvalidWitness)?;
        self.auth_proof.witness = Some(serde_json::to_string(
            &crate::nuts::nut10::nutroot::Witness::key_path(key, digest),
        )?);
        Ok(())
    }

    /// Set server-side context from the actual incoming request, never from
    /// client JSON. Adapters must call this before mint authorization.
    pub fn set_request_context(
        &mut self,
        method: &str,
        target: &str,
        body: &[u8],
    ) -> Result<(), Error> {
        self.request_digest = Some(crate::nuts::nut10::nutroot::authorized_request_digest(
            method, target, body,
        )?);
        Ok(())
    }

    /// Verify a version-02 key-path witness against server-computed context.
    pub fn verify_request(&self) -> Result<(), Error> {
        if self.auth_proof.keyset_id.get_version() != crate::nuts::KeySetVersion::Version02 {
            return Ok(());
        }
        let digest = self
            .request_digest
            .ok_or(crate::nuts::nut10::nutroot::Error::InvalidWitness)?;
        let raw = self
            .auth_proof
            .witness
            .as_ref()
            .ok_or(crate::nuts::nut10::nutroot::Error::InvalidWitness)?;
        if raw.len() > 4096 {
            return Err(crate::nuts::nut10::nutroot::Error::InvalidWitness.into());
        }
        let witness: crate::nuts::nut10::nutroot::Witness = serde_json::from_str(raw)?;
        if witness.leaf.is_some() || witness.control.is_some() {
            return Err(crate::nuts::nut10::nutroot::Error::InvalidWitness.into());
        }
        witness.verify(&self.auth_proof.secret.to_string(), digest, 0)?;
        Ok(())
    }

    /// Create new [ `BlindAuthToken`]
    pub fn new(auth_proof: AuthProof) -> Self {
        Self {
            auth_proof,
            signing_key: None,
            request_digest: None,
        }
    }

    /// Remove DLEQ
    ///
    /// We do not send the DLEQ to the mint as it links redemption and creation
    pub fn without_dleq(&self) -> Self {
        Self {
            signing_key: self.signing_key,
            request_digest: self.request_digest,
            auth_proof: AuthProof {
                witness: self.auth_proof.witness.clone(),
                keyset_id: self.auth_proof.keyset_id,
                secret: self.auth_proof.secret.clone(),
                c: self.auth_proof.c,
                dleq: None,
            },
        }
    }
}

impl fmt::Display for BlindAuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let json_string = serde_json::to_string(&self.auth_proof).map_err(|_| fmt::Error)?;
        let encoded = general_purpose::URL_SAFE.encode(json_string);
        write!(f, "authA{encoded}")
    }
}

impl std::str::FromStr for BlindAuthToken {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Check prefix and extract the base64 encoded part in one step
        let encoded = s.strip_prefix("authA").ok_or(Error::InvalidPrefix)?;

        // Decode the base64 URL-safe string (accept with or without padding)
        let decode_config = GeneralPurposeConfig::new()
            .with_decode_padding_mode(bitcoin::base64::engine::DecodePaddingMode::Indifferent);
        let json_string =
            GeneralPurpose::new(&alphabet::URL_SAFE, decode_config).decode(encoded)?;

        // Convert bytes to UTF-8 string
        let json_str = String::from_utf8(json_string)?;

        // Deserialize the JSON string into AuthProof
        let auth_proof: AuthProof = serde_json::from_str(&json_str)?;

        Ok(Self::new(auth_proof))
    }
}

/// Mint auth request [NUT-XX]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintAuthRequest {
    /// Outputs
    pub outputs: Vec<BlindedMessage>,
}

impl MintAuthRequest {
    /// Count of tokens
    pub fn amount(&self) -> u64 {
        self.outputs.len() as u64
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::str::FromStr;

    use super::super::nut21::{Method, RoutePath};
    use super::*;
    use crate::nut00::KnownMethod;
    use crate::{Amount, PaymentMethod, SecretKey};

    fn test_auth_proof() -> AuthProof {
        let secret_key = SecretKey::generate();

        AuthProof {
            witness: None,
            keyset_id: Id::from_bytes(&[0, 1, 2, 3, 4, 5, 6, 7]).expect("valid id"),
            secret: Secret::generate(),
            c: secret_key.public_key(),
            dleq: None,
        }
    }

    #[test]
    fn test_blind_auth_token_padding() {
        let token = BlindAuthToken::new(test_auth_proof());

        // Serialize (Display impl produces padded base64)
        let token_str = token.to_string();
        assert!(token_str.starts_with("authA"));

        // Parse with padding
        let parsed =
            BlindAuthToken::from_str(&token_str).expect("Failed to parse token with padding");
        assert_eq!(token, parsed);

        // Strip padding and parse again
        let token_no_pad = token_str.trim_end_matches('=');
        let parsed_no_pad =
            BlindAuthToken::from_str(token_no_pad).expect("Failed to parse token without padding");
        assert_eq!(token, parsed_no_pad);
    }

    #[test]
    fn test_auth_token_display_and_header_key_preserve_type() {
        let blind_auth = BlindAuthToken::new(test_auth_proof());
        let blind_auth_string = blind_auth.to_string();

        assert_eq!(
            AuthToken::ClearAuth("clear-token".to_string()).to_string(),
            "clear-token"
        );
        assert_eq!(
            AuthToken::ClearAuth(String::new()).header_key(),
            "Clear-auth"
        );
        assert_eq!(
            AuthToken::BlindAuth(blind_auth.clone()).to_string(),
            blind_auth_string
        );
        assert_eq!(AuthToken::BlindAuth(blind_auth).header_key(), "Blind-auth");
    }

    #[test]
    fn test_mint_auth_request_amount_counts_outputs() {
        let keyset_id = Id::from_bytes(&[0, 1, 2, 3, 4, 5, 6, 7]).expect("valid id");
        let blinded_secret = SecretKey::generate().public_key();
        let request = MintAuthRequest {
            outputs: vec![
                BlindedMessage::new(Amount::ONE, keyset_id, blinded_secret),
                BlindedMessage::new(Amount::ONE, keyset_id, blinded_secret),
            ],
        };

        assert_eq!(request.amount(), 2);
    }

    #[test]
    fn test_settings_deserialize_direct_paths() {
        let json = r#"{
            "bat_max_mint": 10,
            "protected_endpoints": [
                {
                    "method": "GET",
                    "path": "/v1/mint/bolt11"
                },
                {
                    "method": "POST",
                    "path": "/v1/swap"
                }
            ]
        }"#;

        let settings: Settings = serde_json::from_str(json).unwrap();

        assert_eq!(settings.bat_max_mint, 10);
        assert_eq!(settings.protected_endpoints.len(), 2);

        // Check that both paths are included
        let paths = settings
            .protected_endpoints
            .iter()
            .map(|ep| (ep.method, ep.path.clone()))
            .collect::<Vec<_>>();
        assert!(paths.contains(&(
            Method::Get,
            RoutePath::Mint(PaymentMethod::Known(KnownMethod::Bolt11).to_string())
        )));
        assert!(paths.contains(&(Method::Post, RoutePath::Swap)));
    }

    #[test]
    fn test_settings_deserialize_with_regex() {
        let json = r#"{
            "bat_max_mint": 5,
            "protected_endpoints": [
                {
                    "method": "GET",
                    "path": "/v1/mint/*"
                },
                {
                    "method": "POST",
                    "path": "/v1/swap"
                }
            ]
        }"#;

        let settings: Settings = serde_json::from_str(json).unwrap();

        assert_eq!(settings.bat_max_mint, 5);
        assert_eq!(settings.protected_endpoints.len(), 6); // 4 mint paths + wildcard + 1 swap path

        let expected_protected: HashSet<ProtectedEndpoint> = HashSet::from_iter(vec![
            ProtectedEndpoint::new(Method::Post, RoutePath::Swap),
            ProtectedEndpoint::new(
                Method::Get,
                RoutePath::Mint(PaymentMethod::Known(KnownMethod::Bolt11).to_string()),
            ),
            ProtectedEndpoint::new(
                Method::Get,
                RoutePath::MintQuote(PaymentMethod::Known(KnownMethod::Bolt11).to_string()),
            ),
            ProtectedEndpoint::new(
                Method::Get,
                RoutePath::MintQuote(PaymentMethod::Known(KnownMethod::Bolt12).to_string()),
            ),
            ProtectedEndpoint::new(
                Method::Get,
                RoutePath::Mint(PaymentMethod::Known(KnownMethod::Bolt12).to_string()),
            ),
            ProtectedEndpoint::new(Method::Get, RoutePath::Wildcard("/v1/mint/".to_string())),
        ]);

        let deserialized_protected = settings.protected_endpoints.into_iter().collect();

        assert_eq!(expected_protected, deserialized_protected);
    }

    #[test]
    fn test_settings_deserialize_invalid_regex() {
        let json = r#"{
            "bat_max_mint": 5,
            "protected_endpoints": [
                {
                    "method": "GET",
                    "path": "/*wildcard_start"
                }
            ]
        }"#;

        let result = serde_json::from_str::<Settings>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_settings_deserialize_unknown_exact_path() {
        let json = r#"{
            "bat_max_mint": 5,
            "protected_endpoints": [
                {
                    "method": "POST",
                    "path": "/v1/swp"
                }
            ]
        }"#;

        let result = serde_json::from_str::<Settings>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_settings_deserialize_all_paths() {
        let json = r#"{
            "bat_max_mint": 5,
            "protected_endpoints": [
                {
                    "method": "GET",
                    "path": "/v1/*"
                }
            ]
        }"#;

        let settings: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(
            settings.protected_endpoints.len(),
            RoutePath::all_known_paths().len() + 1
        );
    }
}

#[cfg(test)]
#[test]
fn nutroot_bat_binds_request_and_does_not_serialize_private_context() {
    use crate::nuts::nut10::nutroot::SpendInfo;
    let key = crate::SecretKey::from_slice(&[9; 32]).unwrap();
    let id = Id::from_bytes(&[vec![2], vec![1; 32]].concat()).unwrap();
    let c = crate::nuts::nut01::BlsG1PublicKey::hash_to_curve(b"signature").into();
    let mut proof = Proof::new(
        1.into(),
        id,
        key.public_key().to_string().parse().unwrap(),
        c,
    );
    proof.spend_info = Some(SpendInfo {
        bearer_key: Some(key.clone()),
        ..Default::default()
    });
    let mut outgoing = BlindAuthToken::from_proof(proof).unwrap();
    outgoing
        .sign_request("POST", "/v1/swap?x=1", b"{ }")
        .unwrap();
    let serialized = outgoing.without_dleq().to_string();
    let json = serde_json::to_string(&outgoing).unwrap();
    assert!(!json.contains(&key.to_secret_hex()));
    assert!(!json.contains("signing_key"));
    assert!(!json.contains("request_digest"));
    let mut incoming: BlindAuthToken = serialized.parse().unwrap();
    assert!(incoming.verify_request().is_err());
    incoming
        .set_request_context("POST", "/v1/swap?x=1", b"{ }")
        .unwrap();
    incoming.verify_request().unwrap();
    let restored: BlindAuthToken =
        serde_json::from_str(&serde_json::to_string(&incoming).unwrap()).unwrap();
    assert!(restored.verify_request().is_err());
    for (method, target, body) in [
        ("GET", "/v1/swap?x=1", &b"{ }"[..]),
        ("POST", "/v1/swap?x=2", &b"{ }"[..]),
        ("POST", "/v1/swap?x=1", &b"{}"[..]),
    ] {
        incoming.set_request_context(method, target, body).unwrap();
        assert!(incoming.verify_request().is_err());
    }
    assert!(incoming
        .sign_request("POST", "/v1/swap?x=1", b"{ }")
        .is_err());
}
