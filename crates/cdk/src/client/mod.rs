//! Client to connet to mint

use async_trait::async_trait;
use thiserror::Error;
use url::Url;

use crate::error::ErrorResponse;
use crate::nuts::nut09::{RestoreRequest, RestoreResponse};
use crate::nuts::{
    BlindedMessage, CheckStateResponse, CurrencyUnit, Id, KeySet, KeysetResponse,
    MeltBolt11Response, MeltQuoteBolt11Response, MintBolt11Response, MintInfo,
    MintQuoteBolt11Response, PreMintSecrets, Proof, PublicKey, SwapRequest, SwapResponse,
};
use crate::Amount;

#[cfg(feature = "gloo")]
pub mod gloo_client;
#[cfg(not(target_arch = "wasm32"))]
pub mod minreq_client;

pub use crate::Bolt11Invoice;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Invoice not paid")]
    InvoiceNotPaid,
    #[error("Wallet not responding")]
    LightingWalletNotResponding(Option<String>),
    /// Parse Url Error
    #[error("`{0}`")]
    UrlParse(#[from] url::ParseError),
    /// Serde Json error
    #[error("`{0}`")]
    SerdeJson(#[from] serde_json::Error),
    /// Cashu Url Error
    #[error("`{0}`")]
    CashuUrl(#[from] crate::url::Error),
    ///  Min req error
    #[cfg(not(target_arch = "wasm32"))]
    #[error("`{0}`")]
    MinReq(#[from] minreq::Error),
    #[cfg(feature = "gloo")]
    #[error("`{0}`")]
    Gloo(String),
    #[error("Unknown Error response")]
    UnknownErrorResponse(crate::error::ErrorResponse),
    /// Custom Error
    #[error("`{0}`")]
    Custom(String),
}

impl From<ErrorResponse> for Error {
    fn from(err: ErrorResponse) -> Error {
        Self::UnknownErrorResponse(err)
    }
}

#[async_trait(?Send)]
pub trait Client {
    async fn get_mint_keys(&self, mint_url: Url) -> Result<Vec<KeySet>, Error>;

    async fn get_mint_keysets(&self, mint_url: Url) -> Result<KeysetResponse, Error>;

    async fn get_mint_keyset(&self, mint_url: Url, keyset_id: Id) -> Result<KeySet, Error>;

    async fn post_mint_quote(
        &self,
        mint_url: Url,
        amount: Amount,
        unit: CurrencyUnit,
    ) -> Result<MintQuoteBolt11Response, Error>;

    async fn post_mint(
        &self,
        mint_url: Url,
        quote: &str,
        premint_secrets: PreMintSecrets,
    ) -> Result<MintBolt11Response, Error>;

    async fn post_melt_quote(
        &self,
        mint_url: Url,
        unit: CurrencyUnit,
        request: Bolt11Invoice,
    ) -> Result<MeltQuoteBolt11Response, Error>;

    async fn post_melt(
        &self,
        mint_url: Url,
        quote: String,
        inputs: Vec<Proof>,
        outputs: Option<Vec<BlindedMessage>>,
    ) -> Result<MeltBolt11Response, Error>;

    // REVIEW: Should be consistent aboue passing in the Request struct or the
    // compnatants and making it within the function. Here the struct is passed
    // in but in check spendable and melt the compants are passed in
    async fn post_swap(
        &self,
        mint_url: Url,
        split_request: SwapRequest,
    ) -> Result<SwapResponse, Error>;

    async fn post_check_state(
        &self,
        mint_url: Url,
        ys: Vec<PublicKey>,
    ) -> Result<CheckStateResponse, Error>;

    async fn get_mint_info(&self, mint_url: Url) -> Result<MintInfo, Error>;

    async fn post_restore(
        &self,
        mint_url: Url,
        restore_request: RestoreRequest,
    ) -> Result<RestoreResponse, Error>;
}

#[cfg(any(not(target_arch = "wasm32"), feature = "gloo"))]
fn join_url(url: Url, paths: &[&str]) -> Result<Url, Error> {
    let mut url = url;
    for path in paths {
        if !url.path().ends_with('/') {
            url.path_segments_mut()
                .map_err(|_| Error::Custom("Url Path Segmants".to_string()))?
                .push(path);
        } else {
            url.path_segments_mut()
                .map_err(|_| Error::Custom("Url Path Segmants".to_string()))?
                .pop()
                .push(path);
        }
    }

    Ok(url)
}

#[cfg(test)]
mod tests {
    /*
    use super::*;

    #[test]
    fn test_decode_error() {
        let err = r#"{"code":0,"error":"Lightning invoice not paid yet."}"#;

        let error = Error::from_json(err).unwrap();

        match error {
            Error::InvoiceNotPaid => {}
            _ => panic!("Wrong error"),
        }

        let err = r#"{"code": 0, "error": "Lightning wallet not responding: Failed to connect to https://legend.lnbits.com due to: All connection attempts failed"}"#;
        let error = Error::from_json(err).unwrap();
        match error {
            Error::LightingWalletNotResponding(mint) => {
                assert_eq!(mint, Some("https://legend.lnbits.com".to_string()));
            }
            _ => panic!("Wrong error"),
        }
    }
    */
}
