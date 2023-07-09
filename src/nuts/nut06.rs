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
    #[deprecated(since = "0.1.5", note = "mint does not need amount")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<Amount>,
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
    #[deprecated(
        since = "0.1.5",
        note = "mint only response with one list of all promises"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fst: Option<Vec<BlindedSignature>>,
    /// Promises to send
    #[deprecated(
        since = "0.1.5",
        note = "mint only response with one list of all promises"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snd: Option<Vec<BlindedSignature>>,
    /// Promises
    pub promises: Option<Vec<BlindedSignature>>,
}

impl SplitResponse {
    pub fn new(promises: Vec<BlindedSignature>) -> SplitResponse {
        SplitResponse {
            fst: None,
            snd: None,
            promises: Some(promises),
        }
    }

    #[deprecated(
        since = "0.1.1",
        note = "mint only response with one list of all promises"
    )]
    pub fn new_from_amount(
        fst: Vec<BlindedSignature>,
        snd: Vec<BlindedSignature>,
    ) -> SplitResponse {
        Self {
            fst: Some(fst),
            snd: Some(snd),
            promises: None,
        }
    }

    #[deprecated(
        since = "0.1.1",
        note = "mint only response with one list of all promises"
    )]
    pub fn change_amount(&self) -> Option<Amount> {
        match &self.fst {
            Some(fst) => Some(
                fst.iter()
                    .map(|BlindedSignature { amount, .. }| *amount)
                    .sum(),
            ),
            None => None,
        }
    }

    #[deprecated(
        since = "0.1.1",
        note = "mint only response with one list of all promises"
    )]
    pub fn target_amount(&self) -> Option<Amount> {
        match &self.snd {
            Some(snd) => Some(
                snd.iter()
                    .map(|BlindedSignature { amount, .. }| *amount)
                    .sum(),
            ),
            None => None,
        }
    }

    pub fn promises_amount(&self) -> Option<Amount> {
        match &self.promises {
            Some(promises) => Some(
                promises
                    .iter()
                    .map(|BlindedSignature { amount, .. }| *amount)
                    .sum(),
            ),
            None => None,
        }
    }
}
