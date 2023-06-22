//! Mint Tokens
// https://github.com/cashubtc/nuts/blob/main/04.md
use serde::{Deserialize, Serialize};

use super::nut00::{BlindedMessage, BlindedSignature};
use crate::Amount;

/// Post Mint Request [NUT-04]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintRequest {
    pub outputs: Vec<BlindedMessage>,
}

impl MintRequest {
    pub fn total_amount(&self) -> Amount {
        self.outputs
            .iter()
            .map(|BlindedMessage { amount, .. }| *amount)
            .sum()
    }
}

/// Post Mint Response [NUT-04]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostMintResponse {
    pub promises: Vec<BlindedSignature>,
}
