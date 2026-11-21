//! 22 Blind Auth

use std::fmt;

use bitcoin::base64::engine::general_purpose::{self, GeneralPurposeConfig};
use bitcoin::base64::engine::GeneralPurpose;
use bitcoin::base64::{alphabet, Engine};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::nut21::ProtectedEndpoint;
use crate::dhke::hash_secret_to_curve;
use crate::nuts::nut01::{DhkePoint, ProofId, SecretKey};
use crate::nuts::nut02::KeySetVersion;
use crate::nuts::nut10::nutroot::{sign_digest, verify_witness, NutrootWitness, RawNutrootWitness};
use crate::secret::Secret;
use crate::util::hex;
use crate::{BlindedMessage, Id, Proof, ProofDleq};

/// NUT22 Error
#[derive(Debug, Error)]
pub enum Error {
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
    /// A v3 BAT carries no witness
    #[error("Blind auth token witness missing")]
    MissingWitness,
    /// A v3 BAT's witness does not sign the request
    #[error("Blind auth token witness invalid")]
    InvalidWitness,
    /// TLV error
    #[error(transparent)]
    Tlv(#[from] crate::nuts::nut00::tlv::Error),
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthToken {
    /// Clear Auth token
    ClearAuth(String),
    /// Blind Auth token
    BlindAuth(Box<BlindAuthToken>),
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

/// Tag of the authorized-request message
pub const AUTHORIZED_REQUEST_TAG: &str = "Cashu_AuthorizedRequest";

/// The request a v3 BAT authorizes (NUT-22): one authorized-request
/// container `0xF1` with the method, target and body hash
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AuthorizedRequest {
    /// HTTP method, uppercase
    pub method: String,
    /// Path relative to the mint URL plus any query, exactly as sent
    pub target: String,
    /// `SHA256` of the body bytes as sent (of the empty string if none)
    pub body_hash: [u8; 32],
}

impl AuthorizedRequest {
    /// The request for a method, target and body as sent
    pub fn new(method: &str, target: &str, body: &[u8]) -> Self {
        use bitcoin::hashes::sha256::Hash as Sha256Hash;
        use bitcoin::hashes::Hash;

        Self {
            method: method.to_ascii_uppercase(),
            target: target.to_string(),
            body_hash: Sha256Hash::hash(body).to_byte_array(),
        }
    }

    /// The transcript: one `0xF1` container
    pub fn transcript(&self) -> Result<Vec<u8>, Error> {
        use crate::nuts::nut00::tlv::write_record;

        let mut fields = Vec::new();
        write_record(&mut fields, 0x01, self.method.as_bytes())?;
        write_record(&mut fields, 0x02, self.target.as_bytes())?;
        write_record(&mut fields, 0x03, &self.body_hash)?;
        let mut transcript = Vec::new();
        write_record(&mut transcript, 0xF1, &fields)?;
        Ok(transcript)
    }

    /// `request_digest = SHA256(transcript)`
    pub fn request_digest(&self) -> Result<[u8; 32], Error> {
        use bitcoin::hashes::sha256::Hash as Sha256Hash;
        use bitcoin::hashes::Hash;

        Ok(Sha256Hash::hash(&self.transcript()?).to_byte_array())
    }

    /// `tagged_hash("Cashu_AuthorizedRequest", request_digest)`: what the
    /// BAT's witness signs
    pub fn message(&self) -> Result<[u8; 32], Error> {
        Ok(crate::util::tagged_hash::tagged_hash(
            AUTHORIZED_REQUEST_TAG,
            &self.request_digest()?,
        ))
    }
}

/// Auth Proofs
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuthProof {
    /// `Keyset id`
    #[serde(rename = "id")]
    pub keyset_id: Id,
    /// Secret message
    pub secret: Secret,
    /// Unblinded signature
    #[serde(rename = "C")]
    pub c: DhkePoint,
    /// Auth Proof Dleq
    pub dleq: Option<ProofDleq>,
    /// Key-path witness over the authorized request (v3 BATs)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witness: Option<RawNutrootWitness>,
}

impl<'de> Deserialize<'de> for AuthProof {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct EncodedAuthProof {
            #[serde(rename = "id")]
            keyset_id: Id,
            secret: Secret,
            #[serde(rename = "C")]
            c: String,
            #[serde(default)]
            dleq: Option<ProofDleq>,
            #[serde(default)]
            witness: Option<String>,
        }

        let encoded = EncodedAuthProof::deserialize(deserializer)?;
        let version = encoded.keyset_id.get_version();
        let c = DhkePoint::from_hex_for_version(encoded.c, version)
            .map_err(serde::de::Error::custom)?;
        // Only v3 BATs carry a witness; pre-v3 ones keep their old shape.
        let witness = match version {
            KeySetVersion::Version02 => encoded
                .witness
                .map(RawNutrootWitness::new)
                .transpose()
                .map_err(serde::de::Error::custom)?,
            _ => None,
        };
        Ok(Self {
            keyset_id: encoded.keyset_id,
            secret: encoded.secret,
            c,
            dleq: encoded.dleq,
            witness,
        })
    }
}

impl AuthProof {
    /// Y of AuthProof
    pub fn y(&self) -> Result<ProofId, Error> {
        Ok(hash_secret_to_curve(&self.secret, self.keyset_id.get_version())?.into())
    }

    /// Sign the request a v3 BAT authorizes, by the key path of its secret
    pub fn sign_request(
        &mut self,
        request: &AuthorizedRequest,
        secret_key: &SecretKey,
    ) -> Result<(), Error> {
        let signature = sign_digest(secret_key, &request.message()?);
        self.witness = Some(NutrootWitness::key_path(signature).into());
        Ok(())
    }

    /// Verify a v3 BAT's witness over the request it authorizes
    ///
    /// The witness must be a key-path witness: one signature by the
    /// secret's key.
    pub fn verify_request(&self, request: &AuthorizedRequest) -> Result<(), Error> {
        let witness = self
            .witness
            .as_ref()
            .ok_or(Error::MissingWitness)?
            .parse()
            .map_err(|_| Error::InvalidWitness)?;
        if !witness.is_key_path() {
            return Err(Error::InvalidWitness);
        }
        let secret = self.secret.v3_point().map_err(|_| Error::InvalidWitness)?;
        verify_witness(&secret, &witness, &request.message()?, 0).map_err(|_| Error::InvalidWitness)
    }
}

impl From<AuthProof> for Proof {
    fn from(value: AuthProof) -> Self {
        Self {
            amount: 1.into(),
            keyset_id: value.keyset_id,
            secret: value.secret,
            c: value.c,
            witness: value.witness.map(crate::nuts::Witness::Nutroot),
            dleq: value.dleq,
            p2pk_e: None,
            spend_info: None,
        }
    }
}

impl TryFrom<Proof> for AuthProof {
    type Error = Error;
    fn try_from(value: Proof) -> Result<Self, Self::Error> {
        Ok(Self {
            keyset_id: value.keyset_id,
            secret: value.secret,
            c: value.c,
            dleq: value.dleq,
            witness: value.witness.and_then(|w| w.as_nutroot().cloned()),
        })
    }
}

/// Blind Auth Token
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlindAuthToken {
    /// [AuthProof]
    pub auth_proof: AuthProof,
    /// The request the token arrived with, set by the server so the mint can
    /// verify a v3 BAT's witness; never serialized
    #[serde(skip)]
    pub request: Option<AuthorizedRequest>,
    /// The key of a v3 BAT, held by the wallet until the client signs the
    /// request it sends; never serialized
    #[serde(skip)]
    pub signing_key: Option<SecretKey>,
}

impl BlindAuthToken {
    /// Create new [ `BlindAuthToken`]
    pub fn new(auth_proof: AuthProof) -> Self {
        BlindAuthToken {
            auth_proof,
            request: None,
            signing_key: None,
        }
    }

    /// Sign the request this v3 BAT authorizes and drop its key
    ///
    /// `body` must be the exact bytes sent. A token without a key (pre-v3)
    /// is left unchanged.
    pub fn authorize(&mut self, request: &AuthorizedRequest) -> Result<(), Error> {
        if let Some(key) = self.signing_key.take() {
            self.auth_proof.sign_request(request, &key)?;
        }
        Ok(())
    }

    /// Remove DLEQ
    ///
    /// We do not send the DLEQ to the mint as it links redemption and creation
    pub fn without_dleq(&self) -> Self {
        Self {
            auth_proof: AuthProof {
                keyset_id: self.auth_proof.keyset_id,
                secret: self.auth_proof.secret.clone(),
                c: self.auth_proof.c,
                dleq: None,
                witness: self.auth_proof.witness.clone(),
            },
            request: self.request.clone(),
            signing_key: self.signing_key.clone(),
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

        Ok(BlindAuthToken {
            auth_proof,
            request: None,
            signing_key: None,
        })
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
            keyset_id: Id::from_bytes(&[0, 1, 2, 3, 4, 5, 6, 7]).expect("valid id"),
            secret: Secret::generate(),
            c: secret_key.public_key().into(),
            dleq: None,
            witness: None,
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
            AuthToken::BlindAuth(Box::new(blind_auth.clone())).to_string(),
            blind_auth_string
        );
        assert_eq!(
            AuthToken::BlindAuth(Box::new(blind_auth)).header_key(),
            "Blind-auth"
        );
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

    /// tests/22-tests.md request transcripts
    #[test]
    fn authorized_request_vectors() {
        use bitcoin::hashes::sha256::Hash as Sha256Hash;
        use bitcoin::hashes::Hash;
        use bitcoin::secp256k1::{schnorr, Message};

        let key3 = SecretKey::from_slice(&[[0u8; 31].as_slice(), &[3]].concat()).unwrap();
        for (method, target, body, transcript, request_digest, digest, signature) in [
            (
                "POST",
                "/v1/swap",
                b"illustrative request body".as_slice(),
                "f10035010004504f53540200082f76312f73776170030020bc14236ec9e2bf6d961268b7463d7be83e01554adfd063361e9e3ae985edce19",
                "b4832c2f7390f933e17d2c70e2a4b99306782fd5f0ee2e5675cb96f67abb5556",
                "aaa50f1820f34bc7109b375c9a41e444ce362aabbb5aa6a18b2dbff75d03a54d",
                "775fdc83744bd6f9b2b051ade53cd979f5a3552cb1f53c13be694a559ef2adc6feaf69b1901a4c0dce20fc17bdc3ae20aa077a703ab7d370f47a4c491de60260",
            ),
            (
                "GET",
                "/v1/mint/quote/bolt11/quote123?b=2&a=1&q=a%20b",
                b"".as_slice(),
                "f1005a01000347455402002e2f76312f6d696e742f71756f74652f626f6c7431312f71756f74653132333f623d3226613d3126713d6125323062030020e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                "ca24b57145758188f31dc08588de1862dcc3aba143fa8b0dcffb29b423ce9142",
                "ccddd0edd33747425ab506ed56ab5c0f8e83e3fd24b67282239cec87b2e98ec2",
                "0621ea88520bbf8e0519e0bbc5653040b1a10cac36a4891ba9fe354fed2b1990b8a68d918e024083c91fe875695c965c54aac75b830e180381073f63a684d980",
            ),
        ] {
            let request = AuthorizedRequest::new(method, target, body);
            assert_eq!(hex::encode(request.transcript().unwrap()), transcript);
            assert_eq!(hex::encode(request.request_digest().unwrap()), request_digest);
            let message = request.message().unwrap();
            assert_eq!(hex::encode(message), digest);

            let keypair = bitcoin::secp256k1::Keypair::from_secret_key(
                &crate::SECP256K1,
                key3.as_secp256k1(),
            );
            let zero_aux = crate::SECP256K1.sign_schnorr_with_aux_rand(
                &Message::from_digest(message),
                &keypair,
                &[0; 32],
            );
            assert_eq!(hex::encode(zero_aux.serialize()), signature);

            // A v3 BAT with this secret verifies the vector's witness.
            let mut proof = AuthProof {
                keyset_id: Id::from_bytes(&[[2u8].as_slice(), &[1; 32]].concat()).unwrap(),
                secret: Secret::from_point(&key3.public_key()),
                c: crate::nuts::nut01::BlsG1PublicKey::hash_to_curve(b"c").into(),
                dleq: None,
                witness: Some(
                    NutrootWitness::key_path(
                        schnorr::Signature::from_slice(&hex::decode(signature).unwrap())
                            .unwrap(),
                    )
                    .into(),
                ),
            };
            proof.verify_request(&request).unwrap();

            // Another request, or another body, does not.
            let other = AuthorizedRequest::new(method, target, b"tampered");
            assert!(proof.verify_request(&other).is_err());
            proof.witness = None;
            assert!(matches!(
                proof.verify_request(&request),
                Err(Error::MissingWitness)
            ));
            proof.sign_request(&request, &key3).unwrap();
            proof.verify_request(&request).unwrap();

            // The witness survives the header encoding.
            let token = BlindAuthToken::new(proof.clone());
            let decoded = BlindAuthToken::from_str(&token.to_string()).unwrap();
            assert_eq!(decoded.auth_proof, proof);
        }
        assert_eq!(
            hex::encode(Sha256Hash::hash(AUTHORIZED_REQUEST_TAG.as_bytes()).to_byte_array()),
            "00cb1b132a603b708bb3b1f56c64d64647b7bbc378f136fc53f66dc21ef325ec"
        );
    }
}
