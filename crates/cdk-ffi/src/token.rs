//! FFI token bindings

use std::collections::BTreeSet;
use std::str::FromStr;

use crate::error::FfiError;
use crate::{Amount, CurrencyUnit, MintUrl, Proofs};

/// FFI-compatible Token
#[derive(Debug, uniffi::Object)]
pub struct Token {
    pub(crate) inner: cdk::nuts::Token,
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl FromStr for Token {
    type Err = FfiError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let token = cdk::nuts::Token::from_str(s)
            .map_err(|e| FfiError::InvalidToken { msg: e.to_string() })?;
        Ok(Token { inner: token })
    }
}

impl From<cdk::nuts::Token> for Token {
    fn from(token: cdk::nuts::Token) -> Self {
        Self { inner: token }
    }
}

impl From<Token> for cdk::nuts::Token {
    fn from(token: Token) -> Self {
        token.inner
    }
}

#[uniffi::export]
impl Token {
    /// Create a new Token from string
    #[uniffi::constructor]
    pub fn from_string(encoded_token: String) -> Result<Token, FfiError> {
        let token = cdk::nuts::Token::from_str(&encoded_token)
            .map_err(|e| FfiError::InvalidToken { msg: e.to_string() })?;
        Ok(Token { inner: token })
    }

    /// Get the total value of the token
    pub fn value(&self) -> Result<Amount, FfiError> {
        Ok(self.inner.value()?.into())
    }

    /// Get the memo from the token
    pub fn memo(&self) -> Option<String> {
        self.inner.memo().clone()
    }

    /// Get the currency unit
    pub fn unit(&self) -> Option<CurrencyUnit> {
        self.inner.unit().map(Into::into)
    }

    /// Get the mint URL
    pub fn mint_url(&self) -> Result<MintUrl, FfiError> {
        Ok(self.inner.mint_url()?.into())
    }

    /// Get proofs from the token (simplified - no keyset filtering for now)
    pub fn proofs_simple(&self) -> Result<Proofs, FfiError> {
        // For now, return empty keysets to get all proofs
        let empty_keysets = vec![];
        let proofs = self.inner.proofs(&empty_keysets)?;
        Ok(proofs.into_iter().map(|p| p.into()).collect())
    }

    /// Convert token to raw bytes
    pub fn to_raw_bytes(&self) -> Result<Vec<u8>, FfiError> {
        Ok(self.inner.to_raw_bytes()?)
    }

    /// Encode token to string representation
    pub fn encode(&self) -> String {
        self.to_string()
    }

    /// Decode token from string representation
    #[uniffi::constructor]
    pub fn decode(encoded_token: String) -> Result<Token, FfiError> {
        encoded_token.parse()
    }

    /// Return unique spending conditions across all proofs in this token
    pub fn spending_conditions(&self) -> Vec<crate::types::SpendingConditions> {
        self.inner
            .spending_conditions()
            .map(|set| set.into_iter().map(Into::into).collect())
            .unwrap_or_default()
    }

    /// Return all P2PK pubkeys referenced by this token's spending conditions
    pub fn p2pk_pubkeys(&self) -> Vec<String> {
        let set = self
            .inner
            .p2pk_pubkeys()
            .map(|keys| {
                keys.into_iter()
                    .map(|k| k.to_string())
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        set.into_iter().collect()
    }

    /// Return all refund pubkeys from P2PK spending conditions
    pub fn p2pk_refund_pubkeys(&self) -> Vec<String> {
        let set = self
            .inner
            .p2pk_refund_pubkeys()
            .map(|keys| {
                keys.into_iter()
                    .map(|k| k.to_string())
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        set.into_iter().collect()
    }

    /// Return all HTLC hashes from spending conditions
    pub fn htlc_hashes(&self) -> Vec<String> {
        let set = self
            .inner
            .htlc_hashes()
            .map(|hashes| {
                hashes
                    .into_iter()
                    .map(|h| h.to_string())
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default();
        set.into_iter().collect()
    }

    /// Return all locktimes from spending conditions (sorted ascending)
    pub fn locktimes(&self) -> Vec<u64> {
        self.inner
            .locktimes()
            .map(|s| s.into_iter().collect())
            .unwrap_or_default()
    }
}
