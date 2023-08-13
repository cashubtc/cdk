//! Client to connet to mint
use std::fmt;

use gloo::net::http::Request;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::nuts::nut00::{wallet::BlindedMessages, BlindedMessage, Proof};
use crate::nuts::nut01::Keys;
use crate::nuts::nut03::RequestMintResponse;
use crate::nuts::nut04::{MintRequest, PostMintResponse};
use crate::nuts::nut05::{CheckFeesRequest, CheckFeesResponse};
use crate::nuts::nut06::{SplitRequest, SplitResponse};
use crate::nuts::nut07::{CheckSpendableRequest, CheckSpendableResponse};
use crate::nuts::nut08::{MeltRequest, MeltResponse};
use crate::nuts::nut09::MintInfo;
use crate::nuts::*;
use crate::utils;
use crate::Amount;
pub use crate::Bolt11Invoice;

#[derive(Debug)]
pub enum Error {
    InvoiceNotPaid,
    LightingWalletNotResponding(Option<String>),
    /// Parse Url Error
    UrlParseError(url::ParseError),
    /// Serde Json error
    SerdeJsonError(serde_json::Error),
    /// Gloo error
    GlooError(String),
    /// Custom Error
    Custom(String),
}

impl From<url::ParseError> for Error {
    fn from(err: url::ParseError) -> Error {
        Error::UrlParseError(err)
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Error {
        Error::SerdeJsonError(err)
    }
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvoiceNotPaid => write!(f, "Invoice not paid"),
            Error::LightingWalletNotResponding(mint) => {
                write!(
                    f,
                    "Lightning Wallet not responding: {}",
                    mint.clone().unwrap_or("".to_string())
                )
            }
            Error::UrlParseError(err) => write!(f, "{}", err),
            Error::SerdeJsonError(err) => write!(f, "{}", err),
            Error::GlooError(err) => write!(f, "{}", err),
            Error::Custom(message) => write!(f, "{}", message),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintErrorResponse {
    code: u32,
    error: String,
}

impl Error {
    pub fn from_json(json: &str) -> Result<Self, Error> {
        let mint_res: MintErrorResponse = serde_json::from_str(json)?;

        let mint_error = match mint_res.error {
            error if error.starts_with("Lightning invoice not paid yet.") => Error::InvoiceNotPaid,
            error if error.starts_with("Lightning wallet not responding") => {
                let mint = utils::extract_url_from_error(&error);
                Error::LightingWalletNotResponding(mint)
            }
            error => Error::Custom(error),
        };
        Ok(mint_error)
    }
}

#[derive(Debug, Clone)]
pub struct Client {
    pub mint_url: Url,
}

impl Client {
    pub fn new(mint_url: &str) -> Result<Self, Error> {
        // HACK
        let mut mint_url = String::from(mint_url);
        if !mint_url.ends_with('/') {
            mint_url.push('/');
        }
        let mint_url = Url::parse(&mint_url)?;
        Ok(Self { mint_url })
    }

    /// Get Mint Keys [NUT-01]
    pub async fn get_keys(&self) -> Result<Keys, Error> {
        let url = self.mint_url.join("keys")?;
        let keys = Request::get(url.as_str())
            .send()
            .await
            .map_err(|err| Error::GlooError(err.to_string()))?
            .json::<Value>()
            .await
            .map_err(|err| Error::GlooError(err.to_string()))?;

        let keys: Keys = serde_json::from_str(&keys.to_string())?;
        /*
                let keys: BTreeMap<u64, String> = match serde_json::from_value(keys.clone()) {
                    Ok(keys) => keys,
                    Err(_err) => {
                        return Err(Error::CustomError(format!(
                            "url: {}, {}",
                            url,
                            serde_json::to_string(&keys)?
                        )))
                    }
                };

                let mint_keys: BTreeMap<u64, PublicKey> = keys
                    .into_iter()
                    .filter_map(|(k, v)| {
                        let key = hex::decode(v).ok()?;
                        let public_key = PublicKey::from_sec1_bytes(&key).ok()?;
                        Some((k, public_key))
                    })
                    .collect();
        */
        Ok(keys)
    }

    /// Get Keysets [NUT-02]
    pub async fn get_keysets(&self) -> Result<nut02::Response, Error> {
        let url = self.mint_url.join("keysets")?;
        let res = Request::get(url.as_str())
            .send()
            .await
            .map_err(|err| Error::GlooError(err.to_string()))?
            .json::<Value>()
            .await
            .map_err(|err| Error::GlooError(err.to_string()))?;

        let response: Result<nut02::Response, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::from_json(&res.to_string())?),
        }
    }

    /// Request Mint [NUT-03]
    pub async fn request_mint(&self, amount: Amount) -> Result<RequestMintResponse, Error> {
        let mut url = self.mint_url.join("mint")?;
        url.query_pairs_mut()
            .append_pair("amount", &amount.to_sat().to_string());

        let res = Request::get(url.as_str())
            .send()
            .await
            .map_err(|err| Error::GlooError(err.to_string()))?
            .json::<Value>()
            .await
            .map_err(|err| Error::GlooError(err.to_string()))?;

        let response: Result<RequestMintResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::from_json(&res.to_string())?),
        }
    }

    /// Mint Tokens [NUT-04]
    pub async fn mint(
        &self,
        blinded_messages: BlindedMessages,
        hash: &str,
    ) -> Result<PostMintResponse, Error> {
        let mut url = self.mint_url.join("mint")?;
        url.query_pairs_mut().append_pair("hash", hash);

        let request = MintRequest {
            outputs: blinded_messages.blinded_messages,
        };

        let res = Request::post(url.as_str())
            .json(&request)
            .map_err(|err| Error::GlooError(err.to_string()))?
            .send()
            .await
            .map_err(|err| Error::GlooError(err.to_string()))?
            .json::<Value>()
            .await
            .map_err(|err| Error::GlooError(err.to_string()))?;

        let response: Result<PostMintResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::from_json(&res.to_string())?),
        }
    }

    /// Check Max expected fee [NUT-05]
    pub async fn check_fees(&self, invoice: Bolt11Invoice) -> Result<CheckFeesResponse, Error> {
        let url = self.mint_url.join("checkfees")?;

        let request = CheckFeesRequest { pr: invoice };

        let res = Request::post(url.as_str())
            .json(&request)
            .map_err(|err| Error::GlooError(err.to_string()))?
            .send()
            .await
            .map_err(|err| Error::GlooError(err.to_string()))?
            .json::<Value>()
            .await
            .map_err(|err| Error::GlooError(err.to_string()))?;

        let response: Result<CheckFeesResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::from_json(&res.to_string())?),
        }
    }

    /// Melt [NUT-05]
    /// [Nut-08] Lightning fee return if outputs defined
    pub async fn melt(
        &self,
        proofs: Vec<Proof>,
        invoice: Bolt11Invoice,
        outputs: Option<Vec<BlindedMessage>>,
    ) -> Result<MeltResponse, Error> {
        let url = self.mint_url.join("melt")?;

        let request = MeltRequest {
            proofs,
            pr: invoice,
            outputs,
        };

        let value = Request::post(url.as_str())
            .json(&request)
            .map_err(|err| Error::GlooError(err.to_string()))?
            .send()
            .await
            .map_err(|err| Error::GlooError(err.to_string()))?
            .json::<Value>()
            .await
            .map_err(|err| Error::GlooError(err.to_string()))?;

        let response: Result<MeltResponse, serde_json::Error> =
            serde_json::from_value(value.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::from_json(&value.to_string())?),
        }
    }

    /// Split Token [NUT-06]
    pub async fn split(&self, split_request: SplitRequest) -> Result<SplitResponse, Error> {
        let url = self.mint_url.join("split")?;

        let res = Request::post(url.as_str())
            .json(&split_request)
            .map_err(|err| Error::GlooError(err.to_string()))?
            .send()
            .await
            .map_err(|err| Error::GlooError(err.to_string()))?
            .json::<Value>()
            .await
            .map_err(|err| Error::GlooError(err.to_string()))?;

        let response: Result<SplitResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::from_json(&res.to_string())?),
        }
    }

    /// Spendable check [NUT-07]
    pub async fn check_spendable(
        &self,
        proofs: &Vec<nut00::mint::Proof>,
    ) -> Result<CheckSpendableResponse, Error> {
        let url = self.mint_url.join("check")?;
        let request = CheckSpendableRequest {
            proofs: proofs.to_owned(),
        };

        let res = Request::post(url.as_str())
            .json(&request)
            .map_err(|err| Error::GlooError(err.to_string()))?
            .send()
            .await
            .map_err(|err| Error::GlooError(err.to_string()))?
            .json::<Value>()
            .await
            .map_err(|err| Error::GlooError(err.to_string()))?;

        let response: Result<CheckSpendableResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::from_json(&res.to_string())?),
        }
    }

    /// Get Mint Info [NUT-09]
    pub async fn get_info(&self) -> Result<MintInfo, Error> {
        let url = self.mint_url.join("info")?;
        let res = Request::get(url.as_str())
            .send()
            .await
            .map_err(|err| Error::GlooError(err.to_string()))?
            .json::<Value>()
            .await
            .map_err(|err| Error::GlooError(err.to_string()))?;

        let response: Result<MintInfo, serde_json::Error> = serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::from_json(&res.to_string())?),
        }
    }
}

#[cfg(test)]
mod tests {

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
}
