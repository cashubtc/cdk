use std::str::FromStr;
use std::sync::Arc;

use cashu::nuts::nut00::wallet::Token as TokenSdk;
use cashu::url::UncheckedUrl;

use crate::error::Result;
use crate::{MintProofs, Proof};

pub struct Token {
    inner: TokenSdk,
}

impl Token {
    pub fn new(mint: String, proofs: Vec<Arc<Proof>>, memo: Option<String>) -> Result<Self> {
        let mint = UncheckedUrl::from_str(&mint)?;
        let proofs = proofs.into_iter().map(|p| p.as_ref().into()).collect();
        Ok(Self {
            inner: TokenSdk::new(mint, proofs, memo)?,
        })
    }

    pub fn token(&self) -> Vec<Arc<MintProofs>> {
        self.inner
            .token
            .clone()
            .into_iter()
            .map(|p| Arc::new(p.into()))
            .collect()
    }

    pub fn memo(&self) -> Option<String> {
        self.inner.memo.clone()
    }

    pub fn from_string(token: String) -> Result<Self> {
        Ok(Self {
            inner: TokenSdk::from_str(&token)?,
        })
    }

    pub fn as_string(&self) -> Result<String> {
        Ok(self.inner.convert_to_string()?)
    }
}

impl From<cashu::nuts::nut00::wallet::Token> for Token {
    fn from(inner: cashu::nuts::nut00::wallet::Token) -> Token {
        Token { inner }
    }
}
