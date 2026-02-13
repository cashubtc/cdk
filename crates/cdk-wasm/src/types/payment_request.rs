//! Payment Request WASM types (NUT-18)

use serde::{Deserialize, Serialize};

use super::amount::{Amount, CurrencyUnit};
use super::mint::MintUrl;
use super::proof::Proof;

/// Transport type for payment request delivery
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransportType {
    Nostr,
    HttpPost,
}

impl From<cdk::nuts::TransportType> for TransportType {
    fn from(t: cdk::nuts::TransportType) -> Self {
        match t {
            cdk::nuts::TransportType::Nostr => TransportType::Nostr,
            cdk::nuts::TransportType::HttpPost => TransportType::HttpPost,
        }
    }
}

impl From<TransportType> for cdk::nuts::TransportType {
    fn from(t: TransportType) -> Self {
        match t {
            TransportType::Nostr => cdk::nuts::TransportType::Nostr,
            TransportType::HttpPost => cdk::nuts::TransportType::HttpPost,
        }
    }
}

/// Transport for payment request delivery
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transport {
    pub transport_type: TransportType,
    pub target: String,
    pub tags: Option<Vec<Vec<String>>>,
}

impl From<cdk::nuts::Transport> for Transport {
    fn from(t: cdk::nuts::Transport) -> Self {
        Self {
            transport_type: t._type.into(),
            target: t.target,
            tags: t.tags,
        }
    }
}

impl From<Transport> for cdk::nuts::Transport {
    fn from(t: Transport) -> Self {
        Self {
            _type: t.transport_type.into(),
            target: t.target,
            tags: t.tags,
        }
    }
}

/// NUT-18 Payment Request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentRequest {
    pub payment_id: Option<String>,
    pub amount: Option<Amount>,
    pub unit: Option<CurrencyUnit>,
    pub single_use: Option<bool>,
    pub mints: Option<Vec<String>>,
    pub description: Option<String>,
    pub transports: Vec<Transport>,
}

impl From<cdk::nuts::PaymentRequest> for PaymentRequest {
    fn from(req: cdk::nuts::PaymentRequest) -> Self {
        Self {
            payment_id: req.payment_id,
            amount: req.amount.map(Into::into),
            unit: req.unit.map(Into::into),
            single_use: req.single_use,
            mints: req
                .mints
                .map(|mints| mints.iter().map(|m| m.to_string()).collect()),
            description: req.description,
            transports: req.transports.into_iter().map(Into::into).collect(),
        }
    }
}

/// Parameters for creating a NUT-18 payment request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRequestParams {
    pub amount: Option<u64>,
    pub unit: String,
    pub description: Option<String>,
    pub pubkeys: Option<Vec<String>>,
    pub num_sigs: u64,
    pub hash: Option<String>,
    pub preimage: Option<String>,
    pub transport: String,
    pub http_url: Option<String>,
    pub nostr_relays: Option<Vec<String>>,
}

impl Default for CreateRequestParams {
    fn default() -> Self {
        Self {
            amount: None,
            unit: "sat".to_string(),
            description: None,
            pubkeys: None,
            num_sigs: 1,
            hash: None,
            preimage: None,
            transport: "none".to_string(),
            http_url: None,
            nostr_relays: None,
        }
    }
}

impl From<CreateRequestParams> for cdk::wallet::payment_request::CreateRequestParams {
    fn from(params: CreateRequestParams) -> Self {
        Self {
            amount: params.amount,
            unit: params.unit,
            description: params.description,
            pubkeys: params.pubkeys,
            num_sigs: params.num_sigs,
            hash: params.hash,
            preimage: params.preimage,
            transport: params.transport,
            http_url: params.http_url,
            nostr_relays: params.nostr_relays,
        }
    }
}

impl From<cdk::wallet::payment_request::CreateRequestParams> for CreateRequestParams {
    fn from(params: cdk::wallet::payment_request::CreateRequestParams) -> Self {
        Self {
            amount: params.amount,
            unit: params.unit,
            description: params.description,
            pubkeys: params.pubkeys,
            num_sigs: params.num_sigs,
            hash: params.hash,
            preimage: params.preimage,
            transport: params.transport,
            http_url: params.http_url,
            nostr_relays: params.nostr_relays,
        }
    }
}

/// Payment request payload sent over transport
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentRequestPayload {
    pub id: Option<String>,
    pub memo: Option<String>,
    pub mint: MintUrl,
    pub unit: CurrencyUnit,
    pub proofs: Vec<Proof>,
}
