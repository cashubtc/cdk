//! NUT-12: Offline ecash signature validation
// TODO: link to nut

use serde::{Deserialize, Serialize};

use super::nut01::PublicKey;
use super::nut02::Id;
use crate::Amount;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DleqProof {
    e: String,
    s: String,
    r: Option<String>,
}

/// Promise (BlindedSignature) [NUT-12] with DLEQ proof
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlindedSignature {
    pub id: Id,
    pub amount: Amount,
    /// blinded signature (C_) on the secret message `B_` of [BlindedMessage]
    #[serde(rename = "C_")]
    pub c: PublicKey,
    pub dleq: Option<DleqProof>,
}
