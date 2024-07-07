//! Cashu Token
//!
//! <https://github.com/cashubtc/nuts/blob/main/00.md>

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use base64::engine::{general_purpose, GeneralPurpose};
use base64::{alphabet, Engine as _};
use serde::{Deserialize, Serialize};
use url::Url;

use super::{Error, Proof, ProofV4, Proofs};
use crate::nuts::{CurrencyUnit, Id};
use crate::url::UncheckedUrl;
use crate::Amount;

/// Token Enum
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Token {
    /// Token V3
    TokenV3(TokenV3),
    /// Token V4
    TokenV4(TokenV4),
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let token = match self {
            Self::TokenV3(token) => token.to_string(),
            Self::TokenV4(token) => token.to_string(),
        };

        write!(f, "{}", token)
    }
}

impl Token {
    /// Create new [`Token`]
    pub fn new(
        mint_url: UncheckedUrl,
        proofs: Proofs,
        memo: Option<String>,
        unit: Option<CurrencyUnit>,
    ) -> Self {
        let proofs = proofs
            .into_iter()
            .fold(HashMap::new(), |mut acc, val| {
                acc.entry(val.keyset_id)
                    .and_modify(|p: &mut Vec<Proof>| p.push(val.clone()))
                    .or_insert(vec![val.clone()]);
                acc
            })
            .into_iter()
            .map(|(id, proofs)| TokenV4Token::new(id, proofs))
            .collect();

        Token::TokenV4(TokenV4 {
            mint_url,
            unit,
            memo,
            token: proofs,
        })
    }

    /// Proofs in [`Token`]
    pub fn proofs(&self) -> HashMap<UncheckedUrl, Proofs> {
        match self {
            Self::TokenV3(token) => token.proofs(),
            Self::TokenV4(token) => token.proofs(),
        }
    }

    /// Total value of [`Token`]
    pub fn value(&self) -> Amount {
        match self {
            Self::TokenV3(token) => token.value(),
            Self::TokenV4(token) => token.value(),
        }
    }

    /// [`Token`] memo
    pub fn memo(&self) -> &Option<String> {
        match self {
            Self::TokenV3(token) => token.memo(),
            Self::TokenV4(token) => token.memo(),
        }
    }

    /// Unit
    pub fn unit(&self) -> &Option<CurrencyUnit> {
        match self {
            Self::TokenV3(token) => token.unit(),
            Self::TokenV4(token) => token.unit(),
        }
    }

    /// To v3 string
    pub fn to_v3_string(&self) -> String {
        let v3_token = match self {
            Self::TokenV3(token) => token.clone(),
            Self::TokenV4(token) => token.clone().into(),
        };

        v3_token.to_string()
    }
}

impl FromStr for Token {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (is_v3, s) = match (s.strip_prefix("cashuA"), s.strip_prefix("cashuB")) {
            (Some(s), None) => (true, s),
            (None, Some(s)) => (false, s),
            _ => return Err(Error::UnsupportedToken),
        };

        let decode_config = general_purpose::GeneralPurposeConfig::new()
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent);
        let decoded = GeneralPurpose::new(&alphabet::URL_SAFE, decode_config).decode(s)?;

        match is_v3 {
            true => {
                let decoded_str = String::from_utf8(decoded)?;
                let token: TokenV3 = serde_json::from_str(&decoded_str)?;
                Ok(Token::TokenV3(token))
            }
            false => {
                let token: TokenV4 = ciborium::from_reader(&decoded[..])?;
                Ok(Token::TokenV4(token))
            }
        }
    }
}

/// Token V3 Token
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenV3Token {
    /// Url of mint
    pub mint: UncheckedUrl,
    /// [`Proofs`]
    pub proofs: Proofs,
}

impl TokenV3Token {
    /// Create new [`TokenV3Token`]
    pub fn new(mint_url: UncheckedUrl, proofs: Proofs) -> Self {
        Self {
            mint: mint_url,
            proofs,
        }
    }
}

/// Token
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenV3 {
    /// Proofs in [`Token`] by mint
    pub token: Vec<TokenV3Token>,
    /// Memo for token
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// Token Unit
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<CurrencyUnit>,
}

impl TokenV3 {
    /// Create new [`Token`]
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
            token: vec![TokenV3Token::new(mint_url, proofs)],
            memo,
            unit,
        })
    }

    fn proofs(&self) -> HashMap<UncheckedUrl, Proofs> {
        let mut proofs: HashMap<UncheckedUrl, Proofs> = HashMap::new();

        for token in self.token.clone() {
            let mint_url = token.mint;
            let mut mint_proofs = token.proofs;

            proofs
                .entry(mint_url)
                .and_modify(|p| p.append(&mut mint_proofs))
                .or_insert(mint_proofs);
        }

        proofs
    }

    #[inline]
    fn value(&self) -> Amount {
        self.token
            .iter()
            .map(|t| t.proofs.iter().map(|p| p.amount).sum())
            .sum()
    }

    #[inline]
    fn memo(&self) -> &Option<String> {
        &self.memo
    }

    #[inline]
    fn unit(&self) -> &Option<CurrencyUnit> {
        &self.unit
    }
}

impl FromStr for TokenV3 {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.strip_prefix("cashuA").ok_or(Error::UnsupportedToken)?;

        let decode_config = general_purpose::GeneralPurposeConfig::new()
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent);
        let decoded = GeneralPurpose::new(&alphabet::URL_SAFE, decode_config).decode(s)?;
        let decoded_str = String::from_utf8(decoded)?;
        let token: TokenV3 = serde_json::from_str(&decoded_str)?;
        Ok(token)
    }
}

impl fmt::Display for TokenV3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let json_string = serde_json::to_string(self).map_err(|_| fmt::Error)?;
        let encoded = general_purpose::URL_SAFE.encode(json_string);
        write!(f, "cashuA{}", encoded)
    }
}

impl From<TokenV4> for TokenV3 {
    fn from(token: TokenV4) -> Self {
        let (mint_url, proofs) = token
            .proofs()
            .into_iter()
            .next()
            .expect("Token has no proofs");
        TokenV3 {
            token: vec![TokenV3Token::new(mint_url, proofs)],
            memo: token.memo,
            unit: token.unit,
        }
    }
}

/// Token V4
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenV4 {
    /// Mint Url
    #[serde(rename = "m")]
    pub mint_url: UncheckedUrl,
    /// Token Unit
    #[serde(rename = "u", skip_serializing_if = "Option::is_none")]
    pub unit: Option<CurrencyUnit>,
    /// Memo for token
    #[serde(rename = "d", skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// Proofs
    ///
    /// Proofs separated by keyset_id
    #[serde(rename = "t")]
    pub token: Vec<TokenV4Token>,
}

impl TokenV4 {
    /// Proofs from token
    pub fn proofs(&self) -> HashMap<UncheckedUrl, Proofs> {
        let mint_url = &self.mint_url;
        let mut proofs: HashMap<UncheckedUrl, Proofs> = HashMap::new();

        for token in self.token.clone() {
            let mut mint_proofs = token
                .proofs
                .iter()
                .map(|p| p.into_proof(&token.keyset_id))
                .collect();

            proofs
                .entry(mint_url.clone())
                .and_modify(|p| p.append(&mut mint_proofs))
                .or_insert(mint_proofs);
        }

        proofs
    }

    #[inline]
    fn value(&self) -> Amount {
        self.token
            .iter()
            .map(|t| t.proofs.iter().map(|p| p.amount).sum())
            .sum()
    }

    #[inline]
    fn memo(&self) -> &Option<String> {
        &self.memo
    }

    #[inline]
    fn unit(&self) -> &Option<CurrencyUnit> {
        &self.unit
    }
}

impl fmt::Display for TokenV4 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use serde::ser::Error;
        let mut data = Vec::new();
        ciborium::into_writer(self, &mut data).map_err(|e| fmt::Error::custom(e.to_string()))?;
        let encoded = general_purpose::URL_SAFE.encode(data);
        write!(f, "cashuB{}", encoded)
    }
}

impl FromStr for TokenV4 {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.strip_prefix("cashuB").ok_or(Error::UnsupportedToken)?;

        let decode_config = general_purpose::GeneralPurposeConfig::new()
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent);
        let decoded = GeneralPurpose::new(&alphabet::URL_SAFE, decode_config).decode(s)?;
        let token: TokenV4 = ciborium::from_reader(&decoded[..])?;
        Ok(token)
    }
}

impl TryFrom<TokenV3> for TokenV4 {
    type Error = Error;
    fn try_from(token: TokenV3) -> Result<Self, Self::Error> {
        let proofs = token.proofs();
        if proofs.len() != 1 {
            return Err(Error::UnsupportedToken);
        }

        let (mint_url, mint_proofs) = proofs.iter().next().expect("No proofs");

        let proofs = mint_proofs
            .iter()
            .fold(HashMap::new(), |mut acc, val| {
                acc.entry(val.keyset_id)
                    .and_modify(|p: &mut Vec<Proof>| p.push(val.clone()))
                    .or_insert(vec![val.clone()]);
                acc
            })
            .into_iter()
            .map(|(id, proofs)| TokenV4Token::new(id, proofs))
            .collect();

        Ok(TokenV4 {
            mint_url: mint_url.to_owned(),
            token: proofs,
            memo: token.memo,
            unit: token.unit,
        })
    }
}

/// Token V4 Token
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenV4Token {
    /// `Keyset id`
    #[serde(
        rename = "i",
        serialize_with = "serialize_v4_keyset_id",
        deserialize_with = "deserialize_v4_keyset_id"
    )]
    pub keyset_id: Id,
    /// Proofs
    #[serde(rename = "p")]
    pub proofs: Vec<ProofV4>,
}

fn serialize_v4_keyset_id<S>(keyset_id: &Id, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_bytes(&keyset_id.to_bytes())
}

fn deserialize_v4_keyset_id<'de, D>(deserializer: D) -> Result<Id, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let bytes = Vec::<u8>::deserialize(deserializer)?;
    Id::from_bytes(&bytes).map_err(serde::de::Error::custom)
}

impl TokenV4Token {
    /// Create new [`TokenV4Token`]
    pub fn new(keyset_id: Id, proofs: Proofs) -> Self {
        Self {
            keyset_id,
            proofs: proofs.into_iter().map(|p| p.into()).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::UncheckedUrl;

    #[test]
    fn test_token_padding() {
        let token_str_with_padding = "cashuAeyJ0b2tlbiI6W3sibWludCI6Imh0dHBzOi8vODMzMy5zcGFjZTozMzM4IiwicHJvb2ZzIjpbeyJhbW91bnQiOjIsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6IjQwNzkxNWJjMjEyYmU2MWE3N2UzZTZkMmFlYjRjNzI3OTgwYmRhNTFjZDA2YTZhZmMyOWUyODYxNzY4YTc4MzciLCJDIjoiMDJiYzkwOTc5OTdkODFhZmIyY2M3MzQ2YjVlNDM0NWE5MzQ2YmQyYTUwNmViNzk1ODU5OGE3MmYwY2Y4NTE2M2VhIn0seyJhbW91bnQiOjgsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6ImZlMTUxMDkzMTRlNjFkNzc1NmIwZjhlZTBmMjNhNjI0YWNhYTNmNGUwNDJmNjE0MzNjNzI4YzcwNTdiOTMxYmUiLCJDIjoiMDI5ZThlNTA1MGI4OTBhN2Q2YzA5NjhkYjE2YmMxZDVkNWZhMDQwZWExZGUyODRmNmVjNjlkNjEyOTlmNjcxMDU5In1dfV0sInVuaXQiOiJzYXQiLCJtZW1vIjoiVGhhbmsgeW91IHZlcnkgbXVjaC4ifQ==";

        let token = TokenV3::from_str(token_str_with_padding).unwrap();

        let token_str_without_padding = "cashuAeyJ0b2tlbiI6W3sibWludCI6Imh0dHBzOi8vODMzMy5zcGFjZTozMzM4IiwicHJvb2ZzIjpbeyJhbW91bnQiOjIsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6IjQwNzkxNWJjMjEyYmU2MWE3N2UzZTZkMmFlYjRjNzI3OTgwYmRhNTFjZDA2YTZhZmMyOWUyODYxNzY4YTc4MzciLCJDIjoiMDJiYzkwOTc5OTdkODFhZmIyY2M3MzQ2YjVlNDM0NWE5MzQ2YmQyYTUwNmViNzk1ODU5OGE3MmYwY2Y4NTE2M2VhIn0seyJhbW91bnQiOjgsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6ImZlMTUxMDkzMTRlNjFkNzc1NmIwZjhlZTBmMjNhNjI0YWNhYTNmNGUwNDJmNjE0MzNjNzI4YzcwNTdiOTMxYmUiLCJDIjoiMDI5ZThlNTA1MGI4OTBhN2Q2YzA5NjhkYjE2YmMxZDVkNWZhMDQwZWExZGUyODRmNmVjNjlkNjEyOTlmNjcxMDU5In1dfV0sInVuaXQiOiJzYXQiLCJtZW1vIjoiVGhhbmsgeW91IHZlcnkgbXVjaC4ifQ";

        let token_without = TokenV3::from_str(token_str_without_padding).unwrap();

        assert_eq!(token, token_without);
    }

    #[test]
    fn test_token_v4_str_round_trip() {
        let token_str = "cashuBpGF0gaJhaUgArSaMTR9YJmFwgaNhYQFhc3hAOWE2ZGJiODQ3YmQyMzJiYTc2ZGIwZGYxOTcyMTZiMjlkM2I4Y2MxNDU1M2NkMjc4MjdmYzFjYzk0MmZlZGI0ZWFjWCEDhhhUP_trhpXfStS6vN6So0qWvc2X3O4NfM-Y1HISZ5JhZGlUaGFuayB5b3VhbXVodHRwOi8vbG9jYWxob3N0OjMzMzhhdWNzYXQ=";
        let token = TokenV4::from_str(token_str).unwrap();

        assert_eq!(
            token.mint_url,
            UncheckedUrl::from_str("http://localhost:3338").unwrap()
        );
        assert_eq!(
            token.token[0].keyset_id,
            Id::from_str("00ad268c4d1f5826").unwrap()
        );

        let token: TokenV4 = token.try_into().unwrap();

        let encoded = &token.to_string();

        let token_data = TokenV4::from_str(encoded).unwrap();

        assert_eq!(token_data, token);
    }

    #[test]
    fn test_token_v4_multi_keyset() -> anyhow::Result<()> {
        let token_str_multi_keysets = "cashuBo2F0gqJhaUgA_9SLj17PgGFwgaNhYQFhc3hAYWNjMTI0MzVlN2I4NDg0YzNjZjE4NTAxNDkyMThhZjkwZjcxNmE1MmJmNGE1ZWQzNDdlNDhlY2MxM2Y3NzM4OGFjWCECRFODGd5IXVW-07KaZCvuWHk3WrnnpiDhHki6SCQh88-iYWlIAK0mjE0fWCZhcIKjYWECYXN4QDEzMjNkM2Q0NzA3YTU4YWQyZTIzYWRhNGU5ZjFmNDlmNWE1YjRhYzdiNzA4ZWIwZDYxZjczOGY0ODMwN2U4ZWVhY1ghAjRWqhENhLSsdHrr2Cw7AFrKUL9Ffr1XN6RBT6w659lNo2FhAWFzeEA1NmJjYmNiYjdjYzY0MDZiM2ZhNWQ1N2QyMTc0ZjRlZmY4YjQ0MDJiMTc2OTI2ZDNhNTdkM2MzZGNiYjU5ZDU3YWNYIQJzEpxXGeWZN5qXSmJjY8MzxWyvwObQGr5G1YCCgHicY2FtdWh0dHA6Ly9sb2NhbGhvc3Q6MzMzOGF1Y3NhdA==";

        let token = Token::from_str(token_str_multi_keysets).unwrap();
        let amount = token.value();

        assert_eq!(amount, Amount::from(4));

        let unit = token.unit().clone().unwrap();

        assert_eq!(CurrencyUnit::Sat, unit);

        match token {
            Token::TokenV4(token) => {
                let tokens: Vec<Id> = token.token.iter().map(|t| t.keyset_id).collect();

                assert_eq!(tokens.len(), 2);

                assert!(tokens.contains(&Id::from_str("00ffd48b8f5ecf80").unwrap()));
                assert!(tokens.contains(&Id::from_str("00ad268c4d1f5826").unwrap()));

                let mint_url = token.mint_url;

                assert_eq!("http://localhost:3338", &mint_url.to_string());
            }
            _ => {
                anyhow::bail!("Token should be a v4 token")
            }
        }

        Ok(())
    }

    #[test]
    fn test_token_str_round_trip() {
        let token_str = "cashuAeyJ0b2tlbiI6W3sibWludCI6Imh0dHBzOi8vODMzMy5zcGFjZTozMzM4IiwicHJvb2ZzIjpbeyJhbW91bnQiOjIsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6IjQwNzkxNWJjMjEyYmU2MWE3N2UzZTZkMmFlYjRjNzI3OTgwYmRhNTFjZDA2YTZhZmMyOWUyODYxNzY4YTc4MzciLCJDIjoiMDJiYzkwOTc5OTdkODFhZmIyY2M3MzQ2YjVlNDM0NWE5MzQ2YmQyYTUwNmViNzk1ODU5OGE3MmYwY2Y4NTE2M2VhIn0seyJhbW91bnQiOjgsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6ImZlMTUxMDkzMTRlNjFkNzc1NmIwZjhlZTBmMjNhNjI0YWNhYTNmNGUwNDJmNjE0MzNjNzI4YzcwNTdiOTMxYmUiLCJDIjoiMDI5ZThlNTA1MGI4OTBhN2Q2YzA5NjhkYjE2YmMxZDVkNWZhMDQwZWExZGUyODRmNmVjNjlkNjEyOTlmNjcxMDU5In1dfV0sInVuaXQiOiJzYXQiLCJtZW1vIjoiVGhhbmsgeW91LiJ9";

        let token = TokenV3::from_str(token_str).unwrap();
        assert_eq!(
            token.token[0].mint,
            UncheckedUrl::from_str("https://8333.space:3338").unwrap()
        );
        assert_eq!(
            token.token[0].proofs[0].clone().keyset_id,
            Id::from_str("009a1f293253e41e").unwrap()
        );
        assert_eq!(token.unit.unwrap(), CurrencyUnit::Sat);

        let encoded = &token.to_string();

        let token_data = TokenV3::from_str(encoded).unwrap();

        assert_eq!(token_data, token);
    }

    #[test]
    fn incorrect_tokens() {
        let incorrect_prefix = "casshuAeyJ0b2tlbiI6W3sibWludCI6Imh0dHBzOi8vODMzMy5zcGFjZTozMzM4IiwicHJvb2ZzIjpbeyJhbW91bnQiOjIsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6IjQwNzkxNWJjMjEyYmU2MWE3N2UzZTZkMmFlYjRjNzI3OTgwYmRhNTFjZDA2YTZhZmMyOWUyODYxNzY4YTc4MzciLCJDIjoiMDJiYzkwOTc5OTdkODFhZmIyY2M3MzQ2YjVlNDM0NWE5MzQ2YmQyYTUwNmViNzk1ODU5OGE3MmYwY2Y4NTE2M2VhIn0seyJhbW91bnQiOjgsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6ImZlMTUxMDkzMTRlNjFkNzc1NmIwZjhlZTBmMjNhNjI0YWNhYTNmNGUwNDJmNjE0MzNjNzI4YzcwNTdiOTMxYmUiLCJDIjoiMDI5ZThlNTA1MGI4OTBhN2Q2YzA5NjhkYjE2YmMxZDVkNWZhMDQwZWExZGUyODRmNmVjNjlkNjEyOTlmNjcxMDU5In1dfV0sInVuaXQiOiJzYXQiLCJtZW1vIjoiVGhhbmsgeW91LiJ9";

        let incorrect_prefix_token = TokenV3::from_str(incorrect_prefix);

        assert!(incorrect_prefix_token.is_err());

        let no_prefix = "eyJ0b2tlbiI6W3sibWludCI6Imh0dHBzOi8vODMzMy5zcGFjZTozMzM4IiwicHJvb2ZzIjpbeyJhbW91bnQiOjIsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6IjQwNzkxNWJjMjEyYmU2MWE3N2UzZTZkMmFlYjRjNzI3OTgwYmRhNTFjZDA2YTZhZmMyOWUyODYxNzY4YTc4MzciLCJDIjoiMDJiYzkwOTc5OTdkODFhZmIyY2M3MzQ2YjVlNDM0NWE5MzQ2YmQyYTUwNmViNzk1ODU5OGE3MmYwY2Y4NTE2M2VhIn0seyJhbW91bnQiOjgsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6ImZlMTUxMDkzMTRlNjFkNzc1NmIwZjhlZTBmMjNhNjI0YWNhYTNmNGUwNDJmNjE0MzNjNzI4YzcwNTdiOTMxYmUiLCJDIjoiMDI5ZThlNTA1MGI4OTBhN2Q2YzA5NjhkYjE2YmMxZDVkNWZhMDQwZWExZGUyODRmNmVjNjlkNjEyOTlmNjcxMDU5In1dfV0sInVuaXQiOiJzYXQiLCJtZW1vIjoiVGhhbmsgeW91LiJ9";

        let no_prefix_token = TokenV3::from_str(no_prefix);

        assert!(no_prefix_token.is_err());

        let correct_token = "cashuAeyJ0b2tlbiI6W3sibWludCI6Imh0dHBzOi8vODMzMy5zcGFjZTozMzM4IiwicHJvb2ZzIjpbeyJhbW91bnQiOjIsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6IjQwNzkxNWJjMjEyYmU2MWE3N2UzZTZkMmFlYjRjNzI3OTgwYmRhNTFjZDA2YTZhZmMyOWUyODYxNzY4YTc4MzciLCJDIjoiMDJiYzkwOTc5OTdkODFhZmIyY2M3MzQ2YjVlNDM0NWE5MzQ2YmQyYTUwNmViNzk1ODU5OGE3MmYwY2Y4NTE2M2VhIn0seyJhbW91bnQiOjgsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6ImZlMTUxMDkzMTRlNjFkNzc1NmIwZjhlZTBmMjNhNjI0YWNhYTNmNGUwNDJmNjE0MzNjNzI4YzcwNTdiOTMxYmUiLCJDIjoiMDI5ZThlNTA1MGI4OTBhN2Q2YzA5NjhkYjE2YmMxZDVkNWZhMDQwZWExZGUyODRmNmVjNjlkNjEyOTlmNjcxMDU5In1dfV0sInVuaXQiOiJzYXQiLCJtZW1vIjoiVGhhbmsgeW91LiJ9";

        let correct_token = TokenV3::from_str(correct_token);

        assert!(correct_token.is_ok());
    }
}
