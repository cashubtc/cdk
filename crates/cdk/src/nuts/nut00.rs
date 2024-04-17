//! NUT-00: Notation and Models
//!
//! <https://github.com/cashubtc/nuts/blob/main/00.md>

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::string::FromUtf8Error;

use base64::engine::{general_purpose, GeneralPurpose};
use base64::{alphabet, Engine as _};
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;
use url::Url;

use crate::dhke::blind_message;
use crate::nuts::nut01::{PublicKey, SecretKey};
use crate::nuts::nut11::{witness_deserialize, witness_serialize, Signatures};
use crate::nuts::nut12::BlindSignatureDleq;
use crate::nuts::{Id, P2PKConditions, ProofDleq};
use crate::secret::Secret;
use crate::url::UncheckedUrl;
use crate::Amount;

/// List of [Proof]
pub type Proofs = Vec<Proof>;

#[derive(Debug, Error)]
pub enum Error {
    /// Proofs required
    #[error("Proofs required in token")]
    ProofsRequired,
    /// Unsupported token
    #[error("Unsupported token")]
    UnsupportedToken,
    /// Invalid Url
    #[error("Invalid Url")]
    InvalidUrl,
    /// Serde Json error
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
    /// Utf8 parse error
    #[error(transparent)]
    Utf8ParseError(#[from] FromUtf8Error),
    /// Base64 error
    #[error(transparent)]
    Base64Error(#[from] base64::DecodeError),
    /// Parse Url Error
    #[error(transparent)]
    UrlParseError(#[from] url::ParseError),
    /// CDK error
    #[error(transparent)]
    Cdk(#[from] crate::error::Error),
    /// NUT11 error
    #[error(transparent)]
    NUT11(#[from] crate::nuts::nut11::Error),
}

/// Blinded Message (also called `output`)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlindedMessage {
    /// Amount
    ///
    /// The value for the requested [BlindSignature]
    pub amount: Amount,
    /// Keyset ID
    ///
    /// ID from which we expect a signature.
    #[serde(rename = "id")]
    pub keyset_id: Id,
    /// Blinded secret message (B_)
    ///
    /// The blinded secret message generated by the sender.
    #[serde(rename = "B_")]
    pub blinded_secret: PublicKey,
    /// Witness
    ///
    /// <https://github.com/cashubtc/nuts/blob/main/11.md>
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    //#[serde(serialize_with = "witness_serialize")]
    //#[serde(deserialize_with = "witness_deserialize")]
    pub witness: Option<Signatures>,
}

impl BlindedMessage {
    /// Compose new blinded message
    #[inline]
    pub fn new(amount: Amount, keyset_id: Id, blinded_secret: PublicKey) -> Self {
        Self {
            amount,
            keyset_id,
            blinded_secret,
            witness: None,
        }
    }

    /// Add witness
    pub fn witness(mut self, witness: Signatures) -> Self {
        self.witness = Some(witness);
        self
    }
}

/// Blind Signature (also called `promise`)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlindSignature {
    /// Amount
    ///
    /// The value of the blinded token.
    pub amount: Amount,
    /// Keyset ID
    ///
    /// ID of the mint keys that signed the token.
    #[serde(rename = "id")]
    pub keyset_id: Id,
    /// Blinded signature (C_)
    ///
    /// The blinded signature on the secret message `B_` of [BlindedMessage].
    #[serde(rename = "C_")]
    pub c: PublicKey,
    /// DLEQ Proof
    ///
    /// <https://github.com/cashubtc/nuts/blob/main/12.md>
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dleq: Option<BlindSignatureDleq>,
}

/// Proofs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proof {
    /// Amount
    pub amount: Amount,
    /// `Keyset id`
    #[serde(rename = "id")]
    pub keyset_id: Id,
    /// Secret message
    pub secret: Secret,
    /// Unblinded signature
    #[serde(rename = "C")]
    pub c: PublicKey,
    /// Witness
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(serialize_with = "witness_serialize")]
    #[serde(deserialize_with = "witness_deserialize")]
    pub witness: Option<Signatures>,
    /// DLEQ Proof
    pub dleq: Option<ProofDleq>,
}

impl Proof {
    pub fn new(amount: Amount, keyset_id: Id, secret: Secret, c: PublicKey) -> Self {
        Proof {
            amount,
            keyset_id,
            secret,
            c,
            witness: None,
            dleq: None,
        }
    }
}

impl Hash for Proof {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.secret.hash(state);
    }
}

impl Ord for Proof {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.amount.cmp(&other.amount)
    }
}

impl PartialOrd for Proof {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum CurrencyUnit {
    #[default]
    Sat,
    Usd,
    Custom(String),
}

impl<S> From<S> for CurrencyUnit
where
    S: AsRef<str>,
{
    fn from(currency: S) -> Self {
        match currency.as_ref() {
            "sat" => Self::Sat,
            "usd" => Self::Usd,
            o => Self::Custom(o.to_string()),
        }
    }
}

impl fmt::Display for CurrencyUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CurrencyUnit::Sat => write!(f, "sat"),
            CurrencyUnit::Usd => write!(f, "usd"),
            CurrencyUnit::Custom(unit) => write!(f, "{unit}"),
        }
    }
}

impl Serialize for CurrencyUnit {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for CurrencyUnit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let currency: String = String::deserialize(deserializer)?;
        Ok(Self::from(currency))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum PaymentMethod {
    #[default]
    Bolt11,
    Custom(String),
}

impl<S> From<S> for PaymentMethod
where
    S: AsRef<str>,
{
    fn from(method: S) -> Self {
        match method.as_ref() {
            "bolt11" => Self::Bolt11,
            o => Self::Custom(o.to_string()),
        }
    }
}

impl fmt::Display for PaymentMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PaymentMethod::Bolt11 => write!(f, "bolt11"),
            PaymentMethod::Custom(unit) => write!(f, "{}", unit),
        }
    }
}

impl Serialize for PaymentMethod {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for PaymentMethod {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let payment_method: String = String::deserialize(deserializer)?;
        Ok(Self::from(payment_method))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreMint {
    /// Blinded message
    pub blinded_message: BlindedMessage,
    /// Secret
    pub secret: Secret,
    /// R
    pub r: SecretKey,
    /// Amount
    pub amount: Amount,
}

impl Ord for PreMint {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.amount.cmp(&other.amount)
    }
}

impl PartialOrd for PreMint {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct PreMintSecrets {
    pub secrets: Vec<PreMint>,
}

impl PreMintSecrets {
    /// Outputs for speceifed amount with random secret
    pub fn random(keyset_id: Id, amount: Amount) -> Result<Self, Error> {
        let amount_split = amount.split();

        let mut output = Vec::with_capacity(amount_split.len());

        for amount in amount_split {
            let secret = Secret::generate();
            let (blinded, r) = blind_message(&secret.to_bytes(), None)?;

            let blinded_message = BlindedMessage::new(amount, keyset_id, blinded);

            output.push(PreMint {
                secret,
                blinded_message,
                r,
                amount,
            });
        }

        Ok(PreMintSecrets { secrets: output })
    }

    pub fn from_secrets(
        keyset_id: Id,
        amounts: Vec<Amount>,
        secrets: Vec<Secret>,
    ) -> Result<Self, Error> {
        let mut output = Vec::with_capacity(secrets.len());

        for (secret, amount) in secrets.into_iter().zip(amounts) {
            let (blinded, r) = blind_message(&secret.to_bytes(), None)?;

            let blinded_message = BlindedMessage::new(amount, keyset_id, blinded);

            output.push(PreMint {
                secret,
                blinded_message,
                r,
                amount,
            });
        }

        Ok(PreMintSecrets { secrets: output })
    }

    /// Blank Outputs used for NUT-08 change
    pub fn blank(keyset_id: Id, fee_reserve: Amount) -> Result<Self, Error> {
        let count = ((u64::from(fee_reserve) as f64).log2().ceil() as u64).max(1);

        let mut output = Vec::with_capacity(count as usize);

        for _i in 0..count {
            let secret = Secret::generate();
            let (blinded, r) = blind_message(&secret.to_bytes(), None)?;

            let blinded_message = BlindedMessage::new(Amount::ZERO, keyset_id, blinded);

            output.push(PreMint {
                secret,
                blinded_message,
                r,
                amount: Amount::ZERO,
            })
        }

        Ok(PreMintSecrets { secrets: output })
    }

    pub fn with_p2pk_conditions(
        keyset_id: Id,
        amount: Amount,
        conditions: P2PKConditions,
    ) -> Result<Self, Error> {
        let amount_split = amount.split();

        let mut output = Vec::with_capacity(amount_split.len());

        for amount in amount_split {
            let secret: Secret = conditions.clone().try_into()?;
            let (blinded, r) = blind_message(&secret.to_bytes(), None)?;

            let blinded_message = BlindedMessage::new(amount, keyset_id, blinded);

            output.push(PreMint {
                secret,
                blinded_message,
                r,
                amount,
            });
        }

        Ok(PreMintSecrets { secrets: output })
    }

    pub fn iter(&self) -> impl Iterator<Item = &PreMint> {
        self.secrets.iter()
    }

    pub fn len(&self) -> usize {
        self.secrets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }

    pub fn total_amount(&self) -> Amount {
        self.secrets
            .iter()
            .map(|PreMint { amount, .. }| *amount)
            .sum()
    }

    pub fn blinded_messages(&self) -> Vec<BlindedMessage> {
        self.iter().map(|pm| pm.blinded_message.clone()).collect()
    }

    pub fn secrets(&self) -> Vec<Secret> {
        self.iter().map(|pm| pm.secret.clone()).collect()
    }

    pub fn rs(&self) -> Vec<SecretKey> {
        self.iter().map(|pm| pm.r.clone()).collect()
    }

    pub fn amounts(&self) -> Vec<Amount> {
        self.iter().map(|pm| pm.amount).collect()
    }

    pub fn combine(&mut self, mut other: Self) {
        self.secrets.append(&mut other.secrets)
    }

    pub fn sort_secrets(&mut self) {
        self.secrets.sort();
    }
}

// Implement Iterator for PreMintSecrets
impl Iterator for PreMintSecrets {
    type Item = PreMint;

    fn next(&mut self) -> Option<Self::Item> {
        // Use the iterator of the vector
        self.secrets.pop()
    }
}

impl Ord for PreMintSecrets {
    fn cmp(&self, other: &Self) -> Ordering {
        self.secrets.cmp(&other.secrets)
    }
}

impl PartialOrd for PreMintSecrets {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    pub token: Vec<MintProofs>,
    /// Memo for token
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// Token Unit
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<CurrencyUnit>,
}

impl Token {
    pub fn new(
        mint_url: UncheckedUrl,
        proofs: Proofs,
        memo: Option<String>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Self, Error> {
        if proofs.is_empty() {
            return Err(Error::ProofsRequired);
        }

        // Check Url is valid
        let _: Url = (&mint_url).try_into().map_err(|_| Error::InvalidUrl)?;

        Ok(Self {
            token: vec![MintProofs::new(mint_url, proofs)],
            memo,
            unit,
        })
    }

    pub fn token_info(&self) -> (u64, String) {
        let mut amount = Amount::ZERO;

        for proofs in &self.token {
            for proof in &proofs.proofs {
                amount += proof.amount;
            }
        }

        (amount.into(), self.token[0].mint.to_string())
    }
}

impl FromStr for Token {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = if s.starts_with("cashuA") {
            s.replace("cashuA", "")
        } else {
            return Err(Error::UnsupportedToken);
        };

        let decode_config = general_purpose::GeneralPurposeConfig::new()
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent);
        let decoded = GeneralPurpose::new(&alphabet::STANDARD, decode_config).decode(s)?;
        let decoded_str = String::from_utf8(decoded)?;
        let token: Token = serde_json::from_str(&decoded_str)?;
        Ok(token)
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let json_string = serde_json::to_string(self).map_err(|_| fmt::Error)?;
        let encoded = general_purpose::STANDARD.encode(json_string);
        write!(f, "cashuA{}", encoded)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintProofs {
    pub mint: UncheckedUrl,
    pub proofs: Proofs,
}

impl MintProofs {
    pub fn new(mint_url: UncheckedUrl, proofs: Proofs) -> Self {
        Self {
            mint: mint_url,
            proofs,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_proof_serialize() {
        let proof = "[{\"id\":\"009a1f293253e41e\",\"amount\":2,\"secret\":\"407915bc212be61a77e3e6d2aeb4c727980bda51cd06a6afc29e2861768a7837\",\"C\":\"02bc9097997d81afb2cc7346b5e4345a9346bd2a506eb7958598a72f0cf85163ea\"},{\"id\":\"009a1f293253e41e\",\"amount\":8,\"secret\":\"fe15109314e61d7756b0f8ee0f23a624acaa3f4e042f61433c728c7057b931be\",\"C\":\"029e8e5050b890a7d6c0968db16bc1d5d5fa040ea1de284f6ec69d61299f671059\"}]";
        let proof: Proofs = serde_json::from_str(proof).unwrap();

        assert_eq!(
            proof[0].clone().keyset_id,
            Id::from_str("009a1f293253e41e").unwrap()
        );

        assert_eq!(proof.len(), 2);
    }

    #[test]
    fn test_token_str_round_trip() {
        let token_str = "cashuAeyJ0b2tlbiI6W3sibWludCI6Imh0dHBzOi8vODMzMy5zcGFjZTozMzM4IiwicHJvb2ZzIjpbeyJhbW91bnQiOjIsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6IjQwNzkxNWJjMjEyYmU2MWE3N2UzZTZkMmFlYjRjNzI3OTgwYmRhNTFjZDA2YTZhZmMyOWUyODYxNzY4YTc4MzciLCJDIjoiMDJiYzkwOTc5OTdkODFhZmIyY2M3MzQ2YjVlNDM0NWE5MzQ2YmQyYTUwNmViNzk1ODU5OGE3MmYwY2Y4NTE2M2VhIn0seyJhbW91bnQiOjgsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6ImZlMTUxMDkzMTRlNjFkNzc1NmIwZjhlZTBmMjNhNjI0YWNhYTNmNGUwNDJmNjE0MzNjNzI4YzcwNTdiOTMxYmUiLCJDIjoiMDI5ZThlNTA1MGI4OTBhN2Q2YzA5NjhkYjE2YmMxZDVkNWZhMDQwZWExZGUyODRmNmVjNjlkNjEyOTlmNjcxMDU5In1dfV0sInVuaXQiOiJzYXQiLCJtZW1vIjoiVGhhbmsgeW91LiJ9";

        let token = Token::from_str(token_str).unwrap();

        assert_eq!(
            token.token[0].mint,
            UncheckedUrl::from_str("https://8333.space:3338").unwrap()
        );
        assert_eq!(
            token.token[0].proofs[0].clone().keyset_id,
            Id::from_str("009a1f293253e41e").unwrap()
        );
        assert_eq!(token.unit.clone().unwrap(), CurrencyUnit::Sat);

        let encoded = &token.to_string();

        let token_data = Token::from_str(encoded).unwrap();

        assert_eq!(token_data, token);
    }

    #[test]
    fn test_blank_blinded_messages() {
        // TODO: Need to update id to new type in proof
        let b = PreMintSecrets::blank(
            Id::from_str("009a1f293253e41e").unwrap(),
            Amount::from(1000),
        )
        .unwrap();
        assert_eq!(b.len(), 10);

        // TODO: Need to update id to new type in proof
        let b = PreMintSecrets::blank(Id::from_str("009a1f293253e41e").unwrap(), Amount::from(1))
            .unwrap();
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn incorrect_tokens() {
        let incorrect_prefix = "casshuAeyJ0b2tlbiI6W3sibWludCI6Imh0dHBzOi8vODMzMy5zcGFjZTozMzM4IiwicHJvb2ZzIjpbeyJhbW91bnQiOjIsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6IjQwNzkxNWJjMjEyYmU2MWE3N2UzZTZkMmFlYjRjNzI3OTgwYmRhNTFjZDA2YTZhZmMyOWUyODYxNzY4YTc4MzciLCJDIjoiMDJiYzkwOTc5OTdkODFhZmIyY2M3MzQ2YjVlNDM0NWE5MzQ2YmQyYTUwNmViNzk1ODU5OGE3MmYwY2Y4NTE2M2VhIn0seyJhbW91bnQiOjgsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6ImZlMTUxMDkzMTRlNjFkNzc1NmIwZjhlZTBmMjNhNjI0YWNhYTNmNGUwNDJmNjE0MzNjNzI4YzcwNTdiOTMxYmUiLCJDIjoiMDI5ZThlNTA1MGI4OTBhN2Q2YzA5NjhkYjE2YmMxZDVkNWZhMDQwZWExZGUyODRmNmVjNjlkNjEyOTlmNjcxMDU5In1dfV0sInVuaXQiOiJzYXQiLCJtZW1vIjoiVGhhbmsgeW91LiJ9";

        let incorrect_prefix_token = Token::from_str(incorrect_prefix);

        assert!(incorrect_prefix_token.is_err());

        let no_prefix = "eyJ0b2tlbiI6W3sibWludCI6Imh0dHBzOi8vODMzMy5zcGFjZTozMzM4IiwicHJvb2ZzIjpbeyJhbW91bnQiOjIsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6IjQwNzkxNWJjMjEyYmU2MWE3N2UzZTZkMmFlYjRjNzI3OTgwYmRhNTFjZDA2YTZhZmMyOWUyODYxNzY4YTc4MzciLCJDIjoiMDJiYzkwOTc5OTdkODFhZmIyY2M3MzQ2YjVlNDM0NWE5MzQ2YmQyYTUwNmViNzk1ODU5OGE3MmYwY2Y4NTE2M2VhIn0seyJhbW91bnQiOjgsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6ImZlMTUxMDkzMTRlNjFkNzc1NmIwZjhlZTBmMjNhNjI0YWNhYTNmNGUwNDJmNjE0MzNjNzI4YzcwNTdiOTMxYmUiLCJDIjoiMDI5ZThlNTA1MGI4OTBhN2Q2YzA5NjhkYjE2YmMxZDVkNWZhMDQwZWExZGUyODRmNmVjNjlkNjEyOTlmNjcxMDU5In1dfV0sInVuaXQiOiJzYXQiLCJtZW1vIjoiVGhhbmsgeW91LiJ9";

        let no_prefix_token = Token::from_str(no_prefix);

        assert!(no_prefix_token.is_err());

        let correct_token = "cashuAeyJ0b2tlbiI6W3sibWludCI6Imh0dHBzOi8vODMzMy5zcGFjZTozMzM4IiwicHJvb2ZzIjpbeyJhbW91bnQiOjIsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6IjQwNzkxNWJjMjEyYmU2MWE3N2UzZTZkMmFlYjRjNzI3OTgwYmRhNTFjZDA2YTZhZmMyOWUyODYxNzY4YTc4MzciLCJDIjoiMDJiYzkwOTc5OTdkODFhZmIyY2M3MzQ2YjVlNDM0NWE5MzQ2YmQyYTUwNmViNzk1ODU5OGE3MmYwY2Y4NTE2M2VhIn0seyJhbW91bnQiOjgsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6ImZlMTUxMDkzMTRlNjFkNzc1NmIwZjhlZTBmMjNhNjI0YWNhYTNmNGUwNDJmNjE0MzNjNzI4YzcwNTdiOTMxYmUiLCJDIjoiMDI5ZThlNTA1MGI4OTBhN2Q2YzA5NjhkYjE2YmMxZDVkNWZhMDQwZWExZGUyODRmNmVjNjlkNjEyOTlmNjcxMDU5In1dfV0sInVuaXQiOiJzYXQiLCJtZW1vIjoiVGhhbmsgeW91LiJ9";

        let correct_token = Token::from_str(correct_token);

        assert!(correct_token.is_ok());
    }
}
