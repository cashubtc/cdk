use std::str::FromStr;
use std::sync::Arc;

use cashu::nuts::nut00::wallet::Token as TokenSdk;

use crate::error::Result;
use crate::MintProofs;
use crate::Proof;

pub struct Token {
    token: TokenSdk,
}

impl Token {
    pub fn new(mint: String, proofs: Vec<Arc<Proof>>, memo: Option<String>) -> Result<Self> {
        let mint = url::Url::from_str(&mint)?;
        let proofs = proofs.into_iter().map(|p| p.as_ref().into()).collect();
        Ok(Self {
            token: TokenSdk::new(mint, proofs, memo)?,
        })
    }

    pub fn token(&self) -> Vec<Arc<MintProofs>> {
        self.token
            .token
            .clone()
            .into_iter()
            .map(|p| Arc::new(p.into()))
            .collect()
    }

    pub fn memo(&self) -> Option<String> {
        self.token.memo.clone()
    }

    pub fn from_string(token: String) -> Result<Self> {
        Ok(Self {
            token: TokenSdk::from_str(&token)?,
        })
    }

    pub fn as_string(&self) -> Result<String> {
        Ok(self.token.convert_to_string()?)
    }
}
