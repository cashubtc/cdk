//! Types for `cashu-rs`

use std::{collections::HashMap, str::FromStr};

use base64::{engine::general_purpose, Engine as _};
use bitcoin::Amount;
use lightning_invoice::Invoice;
use rand::Rng;
use secp256k1::{PublicKey, SecretKey};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;

use crate::{dhke::blind_message, error::Error, serde_utils::serde_url, utils::split_amount};

/// Blinded Message [NUT-00]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlindedMessage {
    /// Amount in satoshi
    #[serde(with = "bitcoin::amount::serde::as_sat")]
    pub amount: Amount,
    /// encrypted secret message (B_)
    #[serde(rename = "B_")]
    pub b: PublicKey,
}

/// Blinded Messages [NUT-00]
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BlindedMessages {
    /// Blinded messages
    pub blinded_messages: Vec<BlindedMessage>,
    /// Secrets
    pub secrets: Vec<Vec<u8>>,
    /// Rs
    pub rs: Vec<SecretKey>,
    /// Amounts
    pub amounts: Vec<Amount>,
}

impl BlindedMessages {
    pub fn random(amount: Amount) -> Result<Self, Error> {
        let mut blinded_messages = BlindedMessages::default();

        let mut rng = rand::thread_rng();
        for amount in split_amount(amount) {
            let bytes: [u8; 32] = rng.gen();
            let (blinded, r) = blind_message(&bytes, None)?;

            let blinded_message = BlindedMessage { amount, b: blinded };

            blinded_messages.secrets.push(bytes.to_vec());
            blinded_messages.blinded_messages.push(blinded_message);
            blinded_messages.rs.push(r);
            blinded_messages.amounts.push(amount);
        }

        Ok(blinded_messages)
    }

    pub fn blank() -> Result<Self, Error> {
        let mut blinded_messages = BlindedMessages::default();

        let mut rng = rand::thread_rng();
        for _i in 0..4 {
            let bytes: [u8; 32] = rng.gen();
            let (blinded, r) = blind_message(&bytes, None)?;

            let blinded_message = BlindedMessage {
                amount: Amount::ZERO,
                b: blinded,
            };

            blinded_messages.secrets.push(bytes.to_vec());
            blinded_messages.blinded_messages.push(blinded_message);
            blinded_messages.rs.push(r);
            blinded_messages.amounts.push(Amount::ZERO);
        }

        Ok(blinded_messages)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitPayload {
    pub keep_blinded_messages: BlindedMessages,
    pub send_blinded_messages: BlindedMessages,
    pub split_payload: SplitRequest,
}

/// Promise (BlindedSignature) [NIP-00]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Promise {
    pub id: String,
    #[serde(with = "bitcoin::amount::serde::as_sat")]
    pub amount: Amount,
    /// blinded signature (C_) on the secret message `B_` of [BlindedMessage]
    #[serde(rename = "C_")]
    pub c: String,
}

/// Proofs [NUT-00]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proof {
    /// Amount in satoshi
    #[serde(with = "bitcoin::amount::serde::as_sat")]
    pub amount: Amount,
    /// Secret message
    pub secret: String,
    /// Unblinded signature
    #[serde(rename = "C")]
    pub c: String,
    /// `Keyset id`
    pub id: Option<String>,
    /// P2SHScript that specifies the spending condition for this Proof
    pub script: Option<String>,
}

/// Mint Keys [NIP-01]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintKeys(pub HashMap<u64, String>);

/// Mint Keysets [NIP-02]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintKeySets {
    /// set of public keys that the mint generates
    pub keysets: Vec<String>,
}

/// Mint request response [NUT-03]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestMintResponse {
    /// Bolt11 payment request
    pub pr: Invoice,
    /// Hash of Invoice
    pub hash: String,
}

/// Post Mint Request [NIP-04]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintRequest {
    pub outputs: Vec<BlindedMessage>,
}

/// Post Mint Response [NUT-05]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostMintResponse {
    pub promises: Vec<Promise>,
}

/// Check Fees Response [NUT-05]

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckFeesResponse {
    /// Expected Mac Fee in satoshis    
    #[serde(with = "bitcoin::amount::serde::as_sat")]
    pub fee: Amount,
}

/// Check Fees request [NUT-05]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckFeesRequest {
    /// Lighting Invoice
    pub pr: Invoice,
}

/// Melt Request [NUT-05]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltRequest {
    pub proofs: Vec<Proof>,
    /// bollt11
    pub pr: Invoice,
    /// Blinded Message that can be used to return change [NUT-08]
    /// Amount feild of blindedMessages `SHOULD` be set to zero
    pub outputs: Option<Vec<BlindedMessage>>,
}

/// Melt Response [NUT-05]
/// Lightning fee return [NUT-08] if change is defined
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltResposne {
    pub paid: bool,
    pub preimage: String,
    pub change: Option<Promise>,
}

/// Split Request [NUT-06]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitRequest {
    #[serde(with = "bitcoin::amount::serde::as_sat")]
    pub amount: Amount,
    pub proofs: Vec<Proof>,
    pub outputs: Vec<BlindedMessage>,
}

/// Split Response [NUT-06]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitResponse {
    /// Promises to keep
    pub fst: Vec<Promise>,
    /// Promises to send
    pub snd: Vec<Promise>,
}

/// Check spendabale request [NUT-07]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckSpendableRequest {
    pub proofs: Vec<Proof>,
}

/// Check Spendable Response [NUT-07]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckSpendableResponse {
    /// booleans indicating whether the provided Proof is still spendable.
    /// In same order as provided proofs
    pub spendable: Vec<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofsStatus {
    pub spendable: Vec<Proof>,
    pub spent: Vec<Proof>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendProofs {
    pub change_proofs: Vec<Proof>,
    pub send_proofs: Vec<Proof>,
}

/// Mint Version
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintVersion {
    pub name: String,
    pub version: String,
}

impl Serialize for MintVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let combined = format!("{}/{}", self.name, self.version);
        serializer.serialize_str(&combined)
    }
}

impl<'de> Deserialize<'de> for MintVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let combined = String::deserialize(deserializer)?;
        let parts: Vec<&str> = combined.split('/').collect();
        if parts.len() != 2 {
            return Err(serde::de::Error::custom("Invalid input string"));
        }
        Ok(MintVersion {
            name: parts[0].to_string(),
            version: parts[1].to_string(),
        })
    }
}

/// Mint Info [NIP-09]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintInfo {
    /// name of the mint and should be recognizable
    pub name: String,
    /// hex pubkey of the mint
    pub pubkey: String,
    /// implementation name and the version running
    pub version: MintVersion,
    /// short description of the mint
    pub description: String,
    /// long description
    pub description_long: String,
    /// contact methods to reach the mint operator
    pub contact: HashMap<String, String>,
    /// shows which NUTs the mint supports
    pub nuts: Vec<String>,
    /// message of the day that the wallet must display to the user
    pub motd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    #[serde(with = "serde_url")]
    pub mint: Url,
    pub proofs: Vec<Proof>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenData {
    pub token: Vec<Token>,
    pub memo: Option<String>,
}

impl FromStr for TokenData {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !s.starts_with("cashuA") {
            return Err(Error::UnsupportedToken);
        }

        let s = s.replace("cashuA", "");
        let decoded = general_purpose::STANDARD.decode(s)?;
        let decoded_str = String::from_utf8(decoded)?;
        println!("decode: {:?}", decoded_str);
        let token: TokenData = serde_json::from_str(&decoded_str)?;
        Ok(token)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proof_seralize() {
        let proof = "[{\"id\":\"DSAl9nvvyfva\",\"amount\":2,\"secret\":\"EhpennC9qB3iFlW8FZ_pZw\",\"C\":\"02c020067db727d586bc3183aecf97fcb800c3f4cc4759f69c626c9db5d8f5b5d4\"},{\"id\":\"DSAl9nvvyfva\",\"amount\":8,\"secret\":\"TmS6Cv0YT5PU_5ATVKnukw\",\"C\":\"02ac910bef28cbe5d7325415d5c263026f15f9b967a079ca9779ab6e5c2db133a7\"}]";
        let proof: Vec<Proof> = serde_json::from_str(proof).unwrap();

        assert_eq!(proof[0].clone().id.unwrap(), "DSAl9nvvyfva");
    }

    #[test]
    fn test_token_from_str() {
        let token = "cashuAeyJ0b2tlbiI6W3sibWludCI6Imh0dHBzOi8vODMzMy5zcGFjZTozMzM4IiwicHJvb2ZzIjpbeyJpZCI6IkRTQWw5bnZ2eWZ2YSIsImFtb3VudCI6Miwic2VjcmV0IjoiRWhwZW5uQzlxQjNpRmxXOEZaX3BadyIsIkMiOiIwMmMwMjAwNjdkYjcyN2Q1ODZiYzMxODNhZWNmOTdmY2I4MDBjM2Y0Y2M0NzU5ZjY5YzYyNmM5ZGI1ZDhmNWI1ZDQifSx7ImlkIjoiRFNBbDludnZ5ZnZhIiwiYW1vdW50Ijo4LCJzZWNyZXQiOiJUbVM2Q3YwWVQ1UFVfNUFUVktudWt3IiwiQyI6IjAyYWM5MTBiZWYyOGNiZTVkNzMyNTQxNWQ1YzI2MzAyNmYxNWY5Yjk2N2EwNzljYTk3NzlhYjZlNWMyZGIxMzNhNyJ9XX1dLCJtZW1vIjoiVGhhbmt5b3UuIn0=";
        let token = TokenData::from_str(token).unwrap();

        assert_eq!(
            token.token[0].mint,
            Url::from_str("https://8333.space:3338").unwrap()
        );
        assert_eq!(token.token[0].proofs[0].clone().id.unwrap(), "DSAl9nvvyfva");
    }
}
