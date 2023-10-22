//! Notation and Models
// https://github.com/cashubtc/nuts/blob/main/00.md

use serde::{Deserialize, Serialize};

use super::nut01::PublicKey;
use super::nut02::Id;
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
}

#[cfg(feature = "wallet")]
pub mod wallet {
    use std::str::FromStr;

    use base64::engine::general_purpose;
    use base64::Engine as _;
    use serde::{Deserialize, Serialize};
    use url::Url;

    use super::MintProofs;
    use crate::dhke::blind_message;
    use crate::error::wallet;
    use crate::nuts::nut00::{BlindedMessage, Proofs};
    use crate::nuts::nut01;
    use crate::secret::Secret;
    use crate::url::UncheckedUrl;
    use crate::utils::split_amount;
    use crate::{error, Amount};

    /// Blinded Messages [NUT-00]
    #[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
    pub struct BlindedMessages {
        /// Blinded messages
        pub blinded_messages: Vec<BlindedMessage>,
        /// Secrets
        pub secrets: Vec<Secret>,
        /// Rs
        pub rs: Vec<nut01::SecretKey>,
        /// Amounts
        pub amounts: Vec<Amount>,
    }

    impl BlindedMessages {
        /// Outputs for speceifed amount with random secret
        pub fn random(amount: Amount) -> Result<Self, wallet::Error> {
            let mut blinded_messages = BlindedMessages::default();

            for amount in split_amount(amount) {
                let secret = Secret::new();
                let (blinded, r) = blind_message(secret.as_bytes(), None)?;

                let blinded_message = BlindedMessage { amount, b: blinded };

                blinded_messages.secrets.push(secret);
                blinded_messages.blinded_messages.push(blinded_message);
                blinded_messages.rs.push(r.into());
                blinded_messages.amounts.push(amount);
            }

            Ok(blinded_messages)
        }

        /// Blank Outputs used for NUT-08 change
        pub fn blank(fee_reserve: Amount) -> Result<Self, wallet::Error> {
            let mut blinded_messages = BlindedMessages::default();

            let fee_reserve = bitcoin::Amount::from_sat(fee_reserve.to_sat());

            let count = (fee_reserve
                .to_float_in(bitcoin::Denomination::Satoshi)
                .log2()
                .ceil() as u64)
                .max(1);

            for _i in 0..count {
                let secret = Secret::new();
                let (blinded, r) = blind_message(secret.as_bytes(), None)?;

                let blinded_message = BlindedMessage {
                    amount: Amount::ZERO,
                    b: blinded,
                };

                blinded_messages.secrets.push(secret);
                blinded_messages.blinded_messages.push(blinded_message);
                blinded_messages.rs.push(r.into());
                blinded_messages.amounts.push(Amount::ZERO);
            }

            Ok(blinded_messages)
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Token {
        pub token: Vec<MintProofs>,
        pub memo: Option<String>,
    }

    impl Token {
        pub fn new(
            mint_url: UncheckedUrl,
            proofs: Proofs,
            memo: Option<String>,
        ) -> Result<Self, wallet::Error> {
            if proofs.is_empty() {
                return Err(wallet::Error::ProofsRequired);
            }

            // Check Url is valid
            let _: Url = (&mint_url).try_into()?;

            Ok(Self {
                token: vec![MintProofs::new(mint_url, proofs)],
                memo,
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
            if !s.starts_with("cashuA") {
                return Err(wallet::Error::UnsupportedToken);
            }

            let s = s.replace("cashuA", "");
            let decoded = general_purpose::STANDARD.decode(s)?;
            let decoded_str = String::from_utf8(decoded)?;
            let token: Token = serde_json::from_str(&decoded_str)?;
            Ok(token)
        }
    }

    impl Token {
        pub fn convert_to_string(&self) -> Result<String, wallet::Error> {
            let json_string = serde_json::to_string(self)?;
            let encoded = general_purpose::STANDARD.encode(json_string);
            Ok(format!("cashuA{}", encoded))
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
    /// Amount in satoshi
    pub amount: Amount,
    /// Secret message
    pub secret: Secret,
    /// Unblinded signature
    #[serde(rename = "C")]
    pub c: PublicKey,
    /// `Keyset id`
    pub id: Option<Id>,
}

/// List of proofs
pub type Proofs = Vec<Proof>;

impl From<Proof> for mint::Proof {
    fn from(proof: Proof) -> Self {
        Self {
            amount: Some(proof.amount),
            secret: proof.secret,
            c: Some(proof.c),
            id: proof.id,
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

    pub fn mint_proofs_from_proofs(proofs: super::Proofs) -> Proofs {
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

        assert_eq!(
            proof[0].clone().id.unwrap(),
            Id::try_from_base64("DSAl9nvvyfva").unwrap()
        );
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
            token.token[0].proofs[0].clone().id.unwrap(),
            Id::try_from_base64("DSAl9nvvyfva").unwrap()
        );

        let encoded = &token.convert_to_string().unwrap();

        let token_data = Token::from_str(encoded).unwrap();

        assert_eq!(token_data, token);
    }

    #[test]
    fn test_blank_blinded_messages() {
        let b = BlindedMessages::blank(Amount::from_sat(1000)).unwrap();
        assert_eq!(b.blinded_messages.len(), 10);

        let b = BlindedMessages::blank(Amount::from_sat(1)).unwrap();
        assert_eq!(b.blinded_messages.len(), 1);
    }
}
