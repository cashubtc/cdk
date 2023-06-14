//! Client to connet to mint
use std::fmt;

use bitcoin::Amount;
use serde_json::Value;
use url::Url;

pub use crate::Invoice;
use crate::{
    keyset::{Keys, MintKeySets},
    types::{
        BlindedMessage, BlindedMessages, CheckFeesRequest, CheckFeesResponse,
        CheckSpendableRequest, CheckSpendableResponse, MeltRequest, MeltResponse, MintInfo,
        MintRequest, PostMintResponse, Proof, RequestMintResponse, SplitRequest, SplitResponse,
    },
    utils,
};
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub enum Error {
    InvoiceNotPaid,
    LightingWalletNotResponding(Option<String>),
    /// Parse Url Error
    UrlParseError(url::ParseError),
    /// Serde Json error
    SerdeJsonError(serde_json::Error),
    ///  Min req error
    MinReqError(minreq::Error),
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

impl From<minreq::Error> for Error {
    fn from(err: minreq::Error) -> Error {
        Error::MinReqError(err)
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
            Error::MinReqError(err) => write!(f, "{}", err),
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
        let keys = minreq::get(url).send()?.json::<Value>()?;

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
    pub async fn get_keysets(&self) -> Result<MintKeySets, Error> {
        let url = self.mint_url.join("keysets")?;
        let res = minreq::get(url).send()?.json::<Value>()?;

        let response: Result<MintKeySets, serde_json::Error> = serde_json::from_value(res.clone());

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

        let res = minreq::get(url).send()?.json::<Value>()?;

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

        let res = minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<Value>()?;

        let response: Result<PostMintResponse, serde_json::Error> =
            serde_json::from_value(res.clone());

        match response {
            Ok(res) => Ok(res),
            Err(_) => Err(Error::from_json(&res.to_string())?),
        }
    }

    /// Check Max expected fee [NUT-05]
    pub async fn check_fees(&self, invoice: Invoice) -> Result<CheckFeesResponse, Error> {
        let url = self.mint_url.join("checkfees")?;

        let request = CheckFeesRequest { pr: invoice };

        let res = minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<Value>()?;

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
        invoice: Invoice,
        outputs: Option<Vec<BlindedMessage>>,
    ) -> Result<MeltResponse, Error> {
        let url = self.mint_url.join("melt")?;

        let request = MeltRequest {
            proofs,
            pr: invoice,
            outputs,
        };

        let value = minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<Value>()?;

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

        let res = minreq::post(url)
            .with_json(&split_request)?
            .send()?
            .json::<Value>()?;

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
        proofs: &Vec<Proof>,
    ) -> Result<CheckSpendableResponse, Error> {
        let url = self.mint_url.join("check")?;
        let request = CheckSpendableRequest {
            proofs: proofs.to_owned(),
        };

        let res = minreq::post(url)
            .with_json(&request)?
            .send()?
            .json::<Value>()?;

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
        let res = minreq::get(url).send()?.json::<Value>()?;

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
