//! Notation and Models
// https://github.com/cashubtc/nuts/blob/main/00.md

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::{Id, Proofs, PublicKey};
use crate::error::Error;
use crate::secret::Secret;
use crate::url::UncheckedUrl;
use crate::Amount;

/// Blinded Message [NUT-00]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlindedMessage {
    /// Amount
    pub amount: Amount,
    /// Keyset Id
    #[serde(rename = "id")]
    pub keyset_id: Id,
    /// encrypted secret message (B_)
    #[serde(rename = "B_")]
    pub b: PublicKey,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum CurrencyUnit {
    #[default]
    Sat,
    Usd,
    Custom(String),
}

impl FromStr for CurrencyUnit {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "sat" => Ok(Self::Sat),
            "usd" => Ok(Self::Usd),
            _ => Ok(Self::Custom(s.to_string())),
        }
    }
}

impl fmt::Display for CurrencyUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CurrencyUnit::Sat => write!(f, "sat"),
            CurrencyUnit::Usd => write!(f, "usd"),
            CurrencyUnit::Custom(unit) => write!(f, "{}", unit),
        }
    }
}

#[derive(Default, Deserialize, Serialize, Debug, PartialEq, Eq, Clone, Hash)]
#[serde(rename_all = "lowercase")]
pub enum PaymentMethod {
    #[default]
    Bolt11,
    Custom(String),
}

impl FromStr for PaymentMethod {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "bolt11" => Ok(Self::Bolt11),
            _ => Ok(Self::Custom(s.to_string())),
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

#[cfg(feature = "wallet")]
pub mod wallet {
    use std::cmp::Ordering;
    use std::fmt;
    use std::str::FromStr;

    use base64::engine::{general_purpose, GeneralPurpose};
    use base64::{alphabet, Engine as _};
    use bip39::Mnemonic;
    use serde::{Deserialize, Serialize};
    use url::Url;

    use super::{CurrencyUnit, MintProofs};
    use crate::dhke::blind_message;
    use crate::error::wallet;
    use crate::nuts::{BlindedMessage, Id, Proofs, SecretKey};
    use crate::secret::Secret;
    use crate::url::UncheckedUrl;
    use crate::{error, Amount};

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
        secrets: Vec<PreMint>,
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

    impl PreMintSecrets {
        /// Outputs for speceifed amount with random secret
        pub fn random(keyset_id: Id, amount: Amount) -> Result<Self, wallet::Error> {
            let amount_split = amount.split();

            let mut output = Vec::with_capacity(amount_split.len());

            for amount in amount_split {
                let secret = Secret::new();
                let (blinded, r) = blind_message(secret.as_bytes(), None)?;

                let blinded_message = BlindedMessage {
                    amount,
                    b: blinded,
                    keyset_id,
                };

                output.push(PreMint {
                    secret,
                    blinded_message,
                    r: r.into(),
                    amount,
                });
            }

            Ok(PreMintSecrets { secrets: output })
        }

        /// Blank Outputs used for NUT-08 change
        pub fn blank(keyset_id: Id, fee_reserve: Amount) -> Result<Self, wallet::Error> {
            let count = ((u64::from(fee_reserve) as f64).log2().ceil() as u64).max(1);

            let mut output = Vec::with_capacity(count as usize);

            for _i in 0..count {
                let secret = Secret::new();
                let (blinded, r) = blind_message(secret.as_bytes(), None)?;

                let blinded_message = BlindedMessage {
                    amount: Amount::ZERO,
                    b: blinded,
                    keyset_id,
                };

                output.push(PreMint {
                    secret,
                    blinded_message,
                    r: r.into(),
                    amount: Amount::ZERO,
                })
            }

            Ok(PreMintSecrets { secrets: output })
        }

        /// Generate blinded messages from predetermined secrets and blindings
        /// factor
        /// TODO: Put behind feature
        pub fn from_seed(
            keyset_id: Id,
            counter: u64,
            mnemonic: &Mnemonic,
            amount: Amount,
        ) -> Result<Self, wallet::Error> {
            let mut pre_mint_secrets = PreMintSecrets::default();

            let mut counter = counter;

            for amount in amount.split() {
                let secret = Secret::from_seed(&mnemonic, keyset_id, counter);
                let blinding_factor = SecretKey::from_seed(&mnemonic, keyset_id, counter);

                let (blinded, r) = blind_message(secret.as_bytes(), Some(blinding_factor.into()))?;

                let blinded_message = BlindedMessage {
                    keyset_id,
                    amount,
                    b: blinded,
                };

                let pre_mint = PreMint {
                    blinded_message,
                    secret: secret.clone(),
                    r: r.into(),
                    amount: Amount::ZERO,
                };

                pre_mint_secrets.secrets.push(pre_mint);
                counter += 1;
            }

            Ok(pre_mint_secrets)
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
        ) -> Result<Self, wallet::Error> {
            if proofs.is_empty() {
                return Err(wallet::Error::ProofsRequired);
            }

            // Check Url is valid
            let _: Url = (&mint_url).try_into()?;

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
        type Err = error::wallet::Error;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            let s = if s.starts_with("cashuA") {
                s.replace("cashuA", "")
            } else {
                return Err(wallet::Error::UnsupportedToken);
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintProofs {
    pub mint: UncheckedUrl,
    pub proofs: Proofs,
}

#[cfg(feature = "wallet")]
impl MintProofs {
    fn new(mint_url: UncheckedUrl, proofs: Proofs) -> Self {
        Self {
            mint: mint_url,
            proofs,
        }
    }
}

/// Promise (BlindedSignature) [NUT-00]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlindedSignature {
    pub amount: Amount,
    /// Keyset Id
    #[serde(rename = "id")]
    pub keyset_id: Id,
    /// blinded signature (C_) on the secret message `B_` of [BlindedMessage]
    #[serde(rename = "C_")]
    pub c: PublicKey,
}

/// Proofs [NUT-00]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proof {
    /// Amount in satoshi
    pub amount: Amount,
    /// `Keyset id`
    #[serde(rename = "id")]
    pub keyset_id: Id,
    /// Secret message
    pub secret: Secret,
    /// Unblinded signature
    #[serde(rename = "C")]
    pub c: PublicKey,
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

impl From<Proof> for mint::Proof {
    fn from(proof: Proof) -> Self {
        Self {
            amount: Some(proof.amount),
            secret: proof.secret,
            c: Some(proof.c),
            keyset_id: Some(proof.keyset_id),
        }
    }
}

impl TryFrom<mint::Proof> for Proof {
    type Error = Error;

    fn try_from(mint_proof: mint::Proof) -> Result<Proof, Self::Error> {
        Ok(Self {
            keyset_id: mint_proof.keyset_id.ok_or(Error::MissingProofField)?,
            amount: mint_proof.amount.ok_or(Error::MissingProofField)?,
            secret: mint_proof.secret,
            c: mint_proof.c.ok_or(Error::MissingProofField)?,
        })
    }
}

pub mod mint {
    use serde::{Deserialize, Serialize};

    use super::PublicKey;
    use crate::nuts::nut02::Id;
    use crate::secret::Secret;
    use crate::Amount;

    /// Proofs [NUT-00]
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Proof {
        /// Amount in satoshi
        #[serde(skip_serializing)]
        pub amount: Option<Amount>,
        /// Secret message
        #[serde(skip_serializing)]
        pub secret: Secret,
        /// Unblinded signature
        #[serde(rename = "C")]
        pub c: Option<PublicKey>,
        /// `Keyset id`
        #[serde(skip_serializing)]
        #[serde(rename = "id")]
        pub keyset_id: Option<Id>,
    }

    /// List of proofs
    pub type Proofs = Vec<Proof>;

    pub fn mint_proofs_from_proofs(proofs: crate::nuts::Proofs) -> Proofs {
        proofs.iter().map(|p| p.to_owned().into()).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::wallet::*;
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
