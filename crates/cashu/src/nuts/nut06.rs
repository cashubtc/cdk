//! Split
// https://github.com/cashubtc/nuts/blob/main/06.md
use serde::{Deserialize, Serialize};

use super::nut00::BlindedSignature;
#[cfg(feature = "wallet")]
use crate::nuts::nut00::wallet::BlindedMessages;
use crate::nuts::nut00::{BlindedMessage, Proofs};
use crate::Amount;

#[cfg(feature = "wallet")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SplitPayload {
    pub blinded_messages: BlindedMessages,
    pub split_payload: SplitRequest,
}

/// Split Request [NUT-06]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitRequest {
    // TODO: This should be deprecated
    pub amount: Option<Amount>,
    /// Proofs that are to be spent in `Split`
    pub proofs: Proofs,
    /// Blinded Messages for Mint to sign
    pub outputs: Vec<BlindedMessage>,
}

impl SplitRequest {
    pub fn new(proofs: Proofs, outputs: Vec<BlindedMessage>) -> Self {
        Self {
            amount: None,
            proofs,
            outputs,
        }
    }

    /// Total value of proofs in `SplitRequest`
    pub fn proofs_amount(&self) -> Amount {
        self.proofs.iter().map(|proof| proof.amount).sum()
    }

    /// Total value of outputs in `SplitRequest`
    pub fn output_amount(&self) -> Amount {
        self.outputs.iter().map(|proof| proof.amount).sum()
    }
}

/// Split Response [NUT-06]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitResponse {
    /// Promises to keep
    // TODO: This should be deprecated
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fst: Option<Vec<BlindedSignature>>,
    /// Promises to send
    // TODO: This should be deprecated
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

    // TODO: This should be deprecated
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

    // TODO: This should be deprecated
    pub fn change_amount(&self) -> Option<Amount> {
        self.fst.as_ref().map(|fst| {
            fst.iter()
                .map(|BlindedSignature { amount, .. }| *amount)
                .sum()
        })
    }

    // TODO: This should be deprecated
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
