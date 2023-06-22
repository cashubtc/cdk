//! Split
// https://github.com/cashubtc/nuts/blob/main/06.md
use serde::{Deserialize, Serialize};

use crate::amount::Amount;
use crate::nuts::nut00::{BlindedMessage, BlindedMessages, Proofs};

use super::nut00::BlindedSignature;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitPayload {
    pub keep_blinded_messages: BlindedMessages,
    pub send_blinded_messages: BlindedMessages,
    pub split_payload: SplitRequest,
}

/// Split Request [NUT-06]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitRequest {
    pub amount: Amount,
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
    /// Promises to keep
    pub fst: Vec<BlindedSignature>,
    /// Promises to send
    pub snd: Vec<BlindedSignature>,
}

impl SplitResponse {
    pub fn change_amount(&self) -> Amount {
        self.fst
            .iter()
            .map(|BlindedSignature { amount, .. }| *amount)
            .sum()
    }

    pub fn target_amount(&self) -> Amount {
        self.snd
            .iter()
            .map(|BlindedSignature { amount, .. }| *amount)
            .sum()
    }
}
