//! Split
// https://github.com/cashubtc/nuts/blob/main/06.md
use serde::{Deserialize, Serialize};

use crate::nuts::nut00::{BlindedMessage, Proofs};
use crate::Amount;

#[cfg(feature = "wallet")]
use crate::nuts::nut00::wallet::BlindedMessages;

use super::nut00::BlindedSignature;

#[cfg(feature = "wallet")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitPayload {
    pub blinded_messages: BlindedMessages,
    pub split_payload: SplitRequest,
}

/// Split Request [NUT-06]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitRequest {
    pub proofs: Proofs,
    pub outputs: Vec<BlindedMessage>,
}

impl SplitRequest {
    pub fn proofs_amount(&self) -> Amount {
        self.proofs.iter().map(|proof| proof.amount).sum()
    }
    pub fn output_amount(&self) -> Amount {
        self.outputs.iter().map(|proof| proof.amount).sum()
    }
}

/// Split Response [NUT-06]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitResponse {
    /// Promises
    pub promises: Vec<BlindedSignature>,
}

impl SplitResponse {
    pub fn new(promises: Vec<BlindedSignature>) -> SplitResponse {
        SplitResponse { promises }
    }

    pub fn promises_amount(&self) -> Amount {
        self.promises
            .iter()
            .map(|BlindedSignature { amount, .. }| *amount)
            .sum::<Amount>()
    }
}
