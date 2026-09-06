//! FFI token bindings

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use crate::error::FfiError;
use crate::types::TokenUrEncoder;
use crate::{Amount, CurrencyUnit, KeySetInfo, MintUrl, Proofs};

/// FFI-compatible Token
#[derive(uniffi::Object)]
pub struct Token {
    pub(crate) inner: cdk::nuts::Token,
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Token")
            .field("encoded", &"[REDACTED]")
            .finish()
    }
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
            .map_err(|e| FfiError::internal(format!("Invalid token: {}", e)))?;
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
            .map_err(|e| FfiError::internal(format!("Invalid token: {}", e)))?;
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

    /// Get proofs from the token
    pub fn proofs(&self, mint_keysets: Vec<KeySetInfo>) -> Result<Proofs, FfiError> {
        let mint_keysets: Vec<_> = mint_keysets
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<_, _>>()?;
        let proofs = self.inner.proofs(&mint_keysets)?;
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

    /// Decode token from raw bytes
    #[uniffi::constructor]
    pub fn from_raw_bytes(bytes: Vec<u8>) -> Result<Token, FfiError> {
        let token = cdk::nuts::Token::try_from(&bytes)?;
        Ok(Token { inner: token })
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

    /// Create a NUT-16 UR encoder for displaying this token as an animated
    /// QR code
    ///
    /// `max_fragment_length` is the maximum number of payload bytes per QR
    /// frame; `None` selects a default suited to most QR scanners. Each
    /// `TokenUrEncoder::next_part` fragment is displayed as one QR frame.
    pub fn ur_encoder(
        &self,
        max_fragment_length: Option<u32>,
    ) -> Result<std::sync::Arc<TokenUrEncoder>, FfiError> {
        let max_fragment_length = max_fragment_length
            .map(|l| l as usize)
            .unwrap_or(cdk::nuts::nut16::DEFAULT_MAX_FRAGMENT_LENGTH);
        let encoder = cdk::nuts::nut16::TokenUrEncoder::new(&self.inner, max_fragment_length)?;
        Ok(std::sync::Arc::new(TokenUrEncoder::from_inner(encoder)))
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    const TOKEN: &str = "cashuBpGF0gaJhaUgArSaMTR9YJmFwgaNhYQFhc3hAOWE2ZGJiODQ3YmQyMzJiYTc2ZGIwZGYxOTcyMTZiMjlkM2I4Y2MxNDU1M2NkMjc4MjdmYzFjYzk0MmZlZGI0ZWFjWCEDhhhUP_trhpXfStS6vN6So0qWvc2X3O4NfM-Y1HISZ5JhZGlUaGFuayB5b3VhbXVodHRwOi8vbG9jYWxob3N0OjMzMzhhdWNzYXQ=";

    #[allow(clippy::use_debug)]
    #[test]
    fn token_debug_redacts_bearer_data() {
        let token = Token::from_str(TOKEN).expect("public test vector should parse");
        let secret = token
            .inner
            .proofs(&[])
            .expect("test token should expose its proof")
            .into_iter()
            .next()
            .expect("test token should contain a proof")
            .secret
            .to_string();
        let output = format!("{token:?}");
        assert!(!output.contains(&secret));
        assert!(!output.contains(TOKEN));
        assert!(output.contains("[REDACTED]"));
    }
}
