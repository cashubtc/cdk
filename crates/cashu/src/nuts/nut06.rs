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
    #[deprecated(since = "0.3.0", note = "mint does not need amount")]
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
        since = "0.3.0",
        note = "mint only response with one list of all promises"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fst: Option<Vec<BlindedSignature>>,
    /// Promises to send
    #[deprecated(
        since = "0.3.0",
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
        since = "0.3.0",
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
        since = "0.3.0",
        note = "mint only response with one list of all promises"
    )]
    pub fn change_amount(&self) -> Option<Amount> {
        self.fst.as_ref().map(|fst| {
            fst.iter()
                .map(|BlindedSignature { amount, .. }| *amount)
                .sum()
        })
    }

    #[deprecated(
        since = "0.3.0",
        note = "mint only response with one list of all promises"
    )]
    pub fn target_amount(&self) -> Option<Amount> {
        self.snd.as_ref().map(|snd| {
            snd.iter()
                .map(|BlindedSignature { amount, .. }| *amount)
                .sum()
        })
    }

    pub fn promises_amount(&self) -> Option<Amount> {
        self.promises.as_ref().map(|promises| {
            promises
                .iter()
                .map(|BlindedSignature { amount, .. }| *amount)
                .sum()
        })
    }
}
