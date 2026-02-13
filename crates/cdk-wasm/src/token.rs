//! WASM token bindings

use std::collections::BTreeSet;
use std::str::FromStr;

use wasm_bindgen::prelude::*;

use crate::error::WasmError;
use crate::types::{Amount, MintUrl, Proofs};

/// WASM-compatible Token
#[wasm_bindgen]
pub struct Token {
    pub(crate) inner: cdk::nuts::Token,
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Token({})", self.inner)
    }
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl FromStr for Token {
    type Err = WasmError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let token = cdk::nuts::Token::from_str(s)
            .map_err(|e| WasmError::internal(format!("Invalid token: {}", e)))?;
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

#[wasm_bindgen]
impl Token {
    /// Create a new Token from string
    #[wasm_bindgen(js_name = "fromString")]
    pub fn from_string(encoded_token: String) -> Result<Token, WasmError> {
        let token = cdk::nuts::Token::from_str(&encoded_token)
            .map_err(|e| WasmError::internal(format!("Invalid token: {}", e)))?;
        Ok(Token { inner: token })
    }

    /// Get the total value of the token
    pub fn value(&self) -> Result<Amount, WasmError> {
        Ok(self.inner.value()?.into())
    }

    /// Get the memo from the token
    pub fn memo(&self) -> Option<String> {
        self.inner.memo().clone()
    }

    /// Get the mint URL
    #[wasm_bindgen(js_name = "mintUrl")]
    pub fn mint_url(&self) -> Result<JsValue, WasmError> {
        let url: MintUrl = self.inner.mint_url()?.into();
        serde_wasm_bindgen::to_value(&url).map_err(WasmError::internal)
    }

    /// Get proofs from the token (simplified - no keyset filtering)
    #[wasm_bindgen(js_name = "proofsSimple")]
    pub fn proofs_simple(&self) -> Result<JsValue, WasmError> {
        let empty_keysets = vec![];
        let proofs = self.inner.proofs(&empty_keysets)?;
        let ffi_proofs: Proofs = proofs.into_iter().map(|p| p.into()).collect();
        serde_wasm_bindgen::to_value(&ffi_proofs).map_err(WasmError::internal)
    }

    /// Convert token to raw bytes
    #[wasm_bindgen(js_name = "toRawBytes")]
    pub fn to_raw_bytes(&self) -> Result<Vec<u8>, WasmError> {
        Ok(self.inner.to_raw_bytes()?)
    }

    /// Encode token to string representation
    pub fn encode(&self) -> String {
        self.to_string()
    }

    /// Decode token from raw bytes
    #[wasm_bindgen(js_name = "fromRawBytes")]
    pub fn from_raw_bytes(bytes: Vec<u8>) -> Result<Token, WasmError> {
        let token = cdk::nuts::Token::try_from(&bytes)?;
        Ok(Token { inner: token })
    }

    /// Decode token from string representation
    pub fn decode(encoded_token: String) -> Result<Token, WasmError> {
        encoded_token.parse()
    }

    /// Return all P2PK pubkeys referenced by this token's spending conditions
    #[wasm_bindgen(js_name = "p2pkPubkeys")]
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

    /// Return all locktimes from spending conditions (sorted ascending)
    pub fn locktimes(&self) -> Vec<u64> {
        self.inner
            .locktimes()
            .map(|s| s.into_iter().collect())
            .unwrap_or_default()
    }
}
