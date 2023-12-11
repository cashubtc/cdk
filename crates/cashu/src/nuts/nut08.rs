//! Lightning fee return
// https://github.com/cashubtc/nuts/blob/main/08.md

use serde::{Deserialize, Serialize};

use super::{BlindedMessage, BlindedSignature, Proofs};
use crate::Amount;

/// Melt Bolt11 Request [NUT-08]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltBolt11Request {
    /// Quote ID
    pub quote: String,
    /// Proofs
    pub inputs: Proofs,
    /// Blinded Message that can be used to return change [NUT-08]
    /// Amount field of blindedMessages `SHOULD` be set to zero
    pub outputs: Option<Vec<BlindedMessage>>,
}

impl MeltBolt11Request {
    pub fn proofs_amount(&self) -> Amount {
        self.inputs.iter().map(|proof| proof.amount).sum()
    }

    pub fn output_amount(&self) -> Option<Amount> {
        self.outputs
            .as_ref()
            .map(|o| o.iter().map(|proof| proof.amount).sum())
    }
}

/// Melt Response [NUT-08]
/// Lightning fee return [NUT-08] if change is defined
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltBolt11Response {
    pub paid: bool,
    // REVIEW: https://github.com/cashubtc/nuts/pull/55#discussion_r1419991818
    pub proof: String,
    pub change: Option<Vec<BlindedSignature>>,
}

impl MeltBolt11Response {
    pub fn change_amount(&self) -> Option<Amount> {
        self.change
            .as_ref()
            .map(|c| c.iter().map(|b| b.amount).sum())
    }
}
