//! NUT-XX transaction response types.

use serde::{Deserialize, Serialize};

use crate::melt::MeltQuoteResponse;
use crate::mint_quote::MintQuoteResponse;
use crate::nuts::{BlindSignature, TransactionState};

/// `POST /v1/transaction` and `GET /v1/transaction/{digest}` response.
///
/// Quote responses are carried as JSON values so each keeps its own NUT-04/05
/// wire shape rather than an enum tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionResponse {
    /// Transaction digest, lowercase hex
    pub digest: String,
    /// State
    pub state: TransactionState,
    /// One signature per blinded output in request order, empty unless `PAID`
    pub signatures: Vec<BlindSignature>,
    /// NUT-05 response per melt quote output, in request order
    pub melt_quotes: Vec<serde_json::Value>,
    /// NUT-04 response of the change quote once `PAID` with positive change
    pub change_quote: Option<serde_json::Value>,
}

/// A melt quote response in its per-method wire shape.
pub fn melt_quote_json<Q>(response: MeltQuoteResponse<Q>) -> Result<serde_json::Value, crate::Error>
where
    Q: Serialize + serde::de::DeserializeOwned,
{
    Ok(match response {
        MeltQuoteResponse::Bolt11(r) => serde_json::to_value(r)?,
        MeltQuoteResponse::Bolt12(r) => serde_json::to_value(r)?,
        MeltQuoteResponse::Onchain(r) => serde_json::to_value(r)?,
        MeltQuoteResponse::Custom((_, r)) => serde_json::to_value(r)?,
    })
}

/// A mint quote response in its per-method wire shape.
pub fn mint_quote_json<Q>(response: MintQuoteResponse<Q>) -> Result<serde_json::Value, crate::Error>
where
    Q: Serialize + serde::de::DeserializeOwned,
{
    Ok(match response {
        MintQuoteResponse::Bolt11(r) => serde_json::to_value(r)?,
        MintQuoteResponse::Bolt12(r) => serde_json::to_value(r)?,
        MintQuoteResponse::Onchain(r) => serde_json::to_value(r)?,
        MintQuoteResponse::Custom { response, .. } => serde_json::to_value(response)?,
    })
}
