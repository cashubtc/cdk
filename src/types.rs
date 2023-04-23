//! Types for `cashu-rs`

use std::collections::HashMap;

use bitcoin::Amount;
use lightning_invoice::Invoice;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Blinded Message [NUT-00]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlindedMessage {
    /// Amount in satoshi
    #[serde(with = "bitcoin::amount::serde::as_sat")]
    pub amount: Amount,
    /// encrypted secret message (B_)
    #[serde(rename = "B_")]
    pub b: String,
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
    pub fst: Vec<BlindedMessage>,
    /// Promises to send
    pub snd: Vec<BlindedMessage>,
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

/// Mint Version
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintVersion {
    name: String,
    version: String,
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
