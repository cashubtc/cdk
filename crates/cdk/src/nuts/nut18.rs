//! NUT-18: Payment Requests
//!
//! <https://github.com/cashubtc/nuts/blob/main/18.md>

use std::fmt;
use std::str::FromStr;

use bitcoin::base64::engine::{general_purpose, GeneralPurpose};
use bitcoin::base64::{alphabet, Engine};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{CurrencyUnit, Proofs};
use crate::mint_url::MintUrl;
use crate::Amount;

const PAYMENT_REQUEST_PREFIX: &str = "creqA";

/// NUT18 Error
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid Prefix
    #[error("Invalid Prefix")]
    InvalidPrefix,
    /// Ciborium error
    #[error(transparent)]
    CiboriumError(#[from] ciborium::de::Error<std::io::Error>),
    /// Base64 error
    #[error(transparent)]
    Base64Error(#[from] bitcoin::base64::DecodeError),
}

/// Transport Type
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransportType {
    /// Nostr
    #[serde(rename = "nostr")]
    Nostr,
    /// Http post
    #[serde(rename = "post")]
    HttpPost,
}

impl fmt::Display for TransportType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use serde::ser::Error;
        let t = serde_json::to_string(self).map_err(|e| fmt::Error::custom(e.to_string()))?;
        write!(f, "{}", t)
    }
}

impl FromStr for Transport {
    type Err = serde_json::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s)
    }
}

/// Transport
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transport {
    /// Type
    #[serde(rename = "t")]
    pub _type: TransportType,
    /// Target
    #[serde(rename = "a")]
    pub target: String,
    /// Tags
    #[serde(rename = "g")]
    pub tags: Option<Vec<Vec<String>>>,
}

/// Payment Request
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentRequest {
    /// `Payment id`
    #[serde(rename = "i")]
    pub payment_id: Option<String>,
    /// Amount
    #[serde(rename = "a")]
    pub amount: Option<Amount>,
    /// Unit
    #[serde(rename = "u")]
    pub unit: Option<CurrencyUnit>,
    /// Single use
    #[serde(rename = "s")]
    pub single_use: Option<bool>,
    /// Mints
    #[serde(rename = "m")]
    pub mints: Option<Vec<MintUrl>>,
    /// Description
    #[serde(rename = "d")]
    pub description: Option<String>,
    /// Transport
    #[serde(rename = "t")]
    pub transports: Vec<Transport>,
}

impl fmt::Display for PaymentRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use serde::ser::Error;
        let mut data = Vec::new();
        ciborium::into_writer(self, &mut data).map_err(|e| fmt::Error::custom(e.to_string()))?;
        let encoded = general_purpose::URL_SAFE.encode(data);
        write!(f, "{}{}", PAYMENT_REQUEST_PREFIX, encoded)
    }
}

impl FromStr for PaymentRequest {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s
            .strip_prefix(PAYMENT_REQUEST_PREFIX)
            .ok_or(Error::InvalidPrefix)?;

        let decode_config = general_purpose::GeneralPurposeConfig::new()
            .with_decode_padding_mode(bitcoin::base64::engine::DecodePaddingMode::Indifferent);
        let decoded = GeneralPurpose::new(&alphabet::URL_SAFE, decode_config).decode(s)?;

        Ok(ciborium::from_reader(&decoded[..])?)
    }
}

/// Payment Request
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentRequestPayload {
    /// Id
    pub id: Option<String>,
    /// Memo
    pub memo: Option<String>,
    /// Mint
    pub mint: MintUrl,
    /// Unit
    pub unit: CurrencyUnit,
    /// Proofs
    pub proofs: Proofs,
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    const PAYMENT_REQUEST: &str = "creqApWF0gaNhdGVub3N0cmFheKlucHJvZmlsZTFxeTI4d3VtbjhnaGo3dW45ZDNzaGp0bnl2OWtoMnVld2Q5aHN6OW1od2RlbjV0ZTB3ZmprY2N0ZTljdXJ4dmVuOWVlaHFjdHJ2NWhzenJ0aHdkZW41dGUwZGVoaHh0bnZkYWtxcWd5ZGFxeTdjdXJrNDM5eWtwdGt5c3Y3dWRoZGh1NjhzdWNtMjk1YWtxZWZkZWhrZjBkNDk1Y3d1bmw1YWeBgmFuYjE3YWloYjdhOTAxNzZhYQphdWNzYXRhbYF4Imh0dHBzOi8vbm9mZWVzLnRlc3RudXQuY2FzaHUuc3BhY2U=";

    #[test]
    fn test_decode_payment_req() -> anyhow::Result<()> {
        let req = PaymentRequest::from_str(PAYMENT_REQUEST)?;

        assert_eq!(&req.payment_id.unwrap(), "b7a90176");
        assert_eq!(req.amount.unwrap(), 10.into());
        assert_eq!(req.unit.clone().unwrap(), CurrencyUnit::Sat);
        assert_eq!(
            req.mints.unwrap(),
            vec![MintUrl::from_str("https://nofees.testnut.cashu.space")?]
        );
        assert_eq!(req.unit.unwrap(), CurrencyUnit::Sat);

        let transport = req.transports.first().unwrap();

        let expected_transport = Transport {_type: TransportType::Nostr, target: "nprofile1qy28wumn8ghj7un9d3shjtnyv9kh2uewd9hsz9mhwden5te0wfjkccte9curxven9eehqctrv5hszrthwden5te0dehhxtnvdakqqgydaqy7curk439ykptkysv7udhdhu68sucm295akqefdehkf0d495cwunl5".to_string(), tags: Some(vec![vec!["n".to_string(), "17".to_string()]])};

        assert_eq!(transport, &expected_transport);

        Ok(())
    }

    #[test]
    fn test_roundtrip_payment_req() -> anyhow::Result<()> {
        let transport = Transport {_type: TransportType::Nostr, target: "nprofile1qy28wumn8ghj7un9d3shjtnyv9kh2uewd9hsz9mhwden5te0wfjkccte9curxven9eehqctrv5hszrthwden5te0dehhxtnvdakqqgydaqy7curk439ykptkysv7udhdhu68sucm295akqefdehkf0d495cwunl5".to_string(), tags: Some(vec![vec!["n".to_string(), "17".to_string()]])};

        let request = PaymentRequest {
            payment_id: Some("b7a90176".to_string()),
            amount: Some(10.into()),
            unit: Some(CurrencyUnit::Sat),
            single_use: None,
            mints: Some(vec!["https://nofees.testnut.cashu.space".parse()?]),
            description: None,
            transports: vec![transport.clone()],
        };

        let request_str = request.to_string();

        let req = PaymentRequest::from_str(&request_str)?;

        assert_eq!(&req.payment_id.unwrap(), "b7a90176");
        assert_eq!(req.amount.unwrap(), 10.into());
        assert_eq!(req.unit.clone().unwrap(), CurrencyUnit::Sat);
        assert_eq!(
            req.mints.unwrap(),
            vec![MintUrl::from_str("https://nofees.testnut.cashu.space")?]
        );
        assert_eq!(req.unit.unwrap(), CurrencyUnit::Sat);

        let t = req.transports.first().unwrap();
        assert_eq!(&transport, t);

        Ok(())
    }
}
