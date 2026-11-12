//! NUT-XX transaction response types.

use serde::{Deserialize, Serialize};

use crate::melt::MeltQuoteResponse;
use crate::nuts::{BlindSignature, TransactionState};

/// `POST /v1/transaction` response.
///
/// Melt quote responses are carried as JSON values so each keeps its own NUT-05
/// wire shape rather than an enum tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionResponse {
    /// State
    pub state: TransactionState,
    /// One signature per blinded output in request order, empty unless `PAID`
    pub signatures: Vec<BlindSignature>,
    /// NUT-05 response per melt quote output, in request order
    pub melt_quotes: Vec<serde_json::Value>,
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
