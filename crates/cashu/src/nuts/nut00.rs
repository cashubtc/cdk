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
    /// Amount in satoshi
    pub amount: Amount,
    /// encrypted secret message (B_)
    #[serde(rename = "B_")]
    pub b: PublicKey,
    #[serde(rename = "id")]
    pub keyset_id: Id,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CurrencyUnit {
    #[default]
    Sat,
    Custom(String),
}

impl FromStr for CurrencyUnit {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "sat" => Ok(Self::Sat),
            _ => Ok(Self::Custom(s.to_string())),
        }
    }
}

impl fmt::Display for CurrencyUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CurrencyUnit::Sat => write!(f, "sat"),
            CurrencyUnit::Custom(unit) => write!(f, "{}", unit),
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
            let fee_reserve = bitcoin::Amount::from_sat(fee_reserve.to_sat());

            let count = (fee_reserve
                .to_float_in(bitcoin::Denomination::Satoshi)
                .log2()
                .ceil() as u64)
                .max(1);

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

            (amount.to_sat(), self.token[0].mint.to_string())
        }
    }

    impl FromStr for Token {
        type Err = error::wallet::Error;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            let s = if s.starts_with("cashuA") {
                s.replace("cashuA", "")
            } else if s.starts_with("cashuB") {
                s.replace("cashuB", "")
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
            write!(f, "cashuB{}", encoded)
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
    pub id: Id,
    pub amount: Amount,
    /// blinded signature (C_) on the secret message `B_` of [BlindedMessage]
    #[serde(rename = "C_")]
    pub c: PublicKey,
}

/// Proofs [NUT-00]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proof {
    /// `Keyset id`
    pub id: Id,
    /// Amount in satoshi
    pub amount: Amount,
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
            id: Some(proof.id),
        }
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
        pub amount: Option<Amount>,
        /// Secret message
        pub secret: Secret,
        /// Unblinded signature
        #[serde(rename = "C")]
        pub c: Option<PublicKey>,
        /// `Keyset id`
        pub id: Option<Id>,
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
        let proof = "[{\"id\":\"DSAl9nvvyfva\",\"amount\":2,\"secret\":\"EhpennC9qB3iFlW8FZ_pZw\",\"C\":\"02c020067db727d586bc3183aecf97fcb800c3f4cc4759f69c626c9db5d8f5b5d4\"},{\"id\":\"DSAl9nvvyfva\",\"amount\":8,\"secret\":\"TmS6Cv0YT5PU_5ATVKnukw\",\"C\":\"02ac910bef28cbe5d7325415d5c263026f15f9b967a079ca9779ab6e5c2db133a7\"}]";
        let proof: Proofs = serde_json::from_str(proof).unwrap();

        assert_eq!(proof[0].clone().id, Id::from_str("DSAl9nvvyfva").unwrap());
    }

    #[test]
    fn test_token_str_round_trip() {
        let token_str = "cashuAeyJ0b2tlbiI6W3sibWludCI6Imh0dHBzOi8vODMzMy5zcGFjZTozMzM4IiwicHJvb2ZzIjpbeyJpZCI6IkRTQWw5bnZ2eWZ2YSIsImFtb3VudCI6Miwic2VjcmV0IjoiRWhwZW5uQzlxQjNpRmxXOEZaX3BadyIsIkMiOiIwMmMwMjAwNjdkYjcyN2Q1ODZiYzMxODNhZWNmOTdmY2I4MDBjM2Y0Y2M0NzU5ZjY5YzYyNmM5ZGI1ZDhmNWI1ZDQifSx7ImlkIjoiRFNBbDludnZ5ZnZhIiwiYW1vdW50Ijo4LCJzZWNyZXQiOiJUbVM2Q3YwWVQ1UFVfNUFUVktudWt3IiwiQyI6IjAyYWM5MTBiZWYyOGNiZTVkNzMyNTQxNWQ1YzI2MzAyNmYxNWY5Yjk2N2EwNzljYTk3NzlhYjZlNWMyZGIxMzNhNyJ9XX1dLCJtZW1vIjoiVGhhbmsgeW91LiJ9";

        let token = Token::from_str(token_str).unwrap();

        assert_eq!(
            token.token[0].mint,
            UncheckedUrl::from_str("https://8333.space:3338").unwrap()
        );
        assert_eq!(
            token.token[0].proofs[0].clone().id,
            Id::from_str("DSAl9nvvyfva").unwrap()
        );

        let encoded = &token.to_string();

        let token_data = Token::from_str(encoded).unwrap();

        assert_eq!(token_data, token);
    }

    #[test]
    fn test_token_with_and_without_padding() {
        let proof = "[{\"id\":\"DSAl9nvvyfva\",\"amount\":2,\"secret\":\"EhpennC9qB3iFlW8FZ_pZw\",\"C\":\"02c020067db727d586bc3183aecf97fcb800c3f4cc4759f69c626c9db5d8f5b5d4\"},{\"id\":\"DSAl9nvvyfva\",\"amount\":8,\"secret\":\"TmS6Cv0YT5PU_5ATVKnukw\",\"C\":\"02ac910bef28cbe5d7325415d5c263026f15f9b967a079ca9779ab6e5c2db133a7\"}]";
        let proof: Proofs = serde_json::from_str(proof).unwrap();
        let token = Token::new(
            UncheckedUrl::from_str("https://localhost:5000/cashu").unwrap(),
            proof,
            None,
            None,
        )
        .unwrap();

        let _token = Token::from_str(&token.to_string()).unwrap();

        let _token = Token::from_str("cashuAeyJ0b2tlbiI6W3sicHJvb2ZzIjpbeyJpZCI6IjBOSTNUVUFzMVNmeSIsImFtb3VudCI6MSwic2VjcmV0IjoiVE92cGVmZGxSZ0EzdlhMN05pM2MvRE1oY29URXNQdnV4eFc0Rys2dXVycz0iLCJDIjoiMDNiZThmMzQwOTMxYTI4ZTlkMGRmNGFmMWQwMWY1ZTcxNTFkMmQ1M2RiN2Y0ZDAyMWQzZGUwZmRiMDNjZGY4ZTlkIn1dLCJtaW50IjoiaHR0cHM6Ly9sZWdlbmQubG5iaXRzLmNvbS9jYXNodS9hcGkvdjEvNGdyOVhjbXozWEVrVU53aUJpUUdvQyJ9XX0").unwrap();
    }

    #[test]
    fn test_blank_blinded_messages() {
        // TODO: Need to update id to new type in proof
        let b = PreMintSecrets::blank(Id::from_str("").unwrap(), Amount::from_sat(1000)).unwrap();
        assert_eq!(b.len(), 10);

        // TODO: Need to update id to new type in proof
        let b = PreMintSecrets::blank(Id::from_str("").unwrap(), Amount::from_sat(1)).unwrap();
        assert_eq!(b.len(), 1);
    }
}
