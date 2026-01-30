//! Receive saga types

use cashu::BlindedMessage;
use serde::{Deserialize, Serialize};

use crate::{Amount, Error};

/// States specific to receive saga
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiveSagaState {
    /// Input proofs validated and stored as pending, ready to swap for new proofs
    ProofsPending,
    /// Swap request sent to mint, awaiting signatures for new proofs
    SwapRequested,
}

impl std::fmt::Display for ReceiveSagaState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReceiveSagaState::ProofsPending => write!(f, "proofs_pending"),
            ReceiveSagaState::SwapRequested => write!(f, "swap_requested"),
        }
    }
}

impl std::str::FromStr for ReceiveSagaState {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "proofs_pending" => Ok(ReceiveSagaState::ProofsPending),
            "swap_requested" => Ok(ReceiveSagaState::SwapRequested),
            _ => Err(Error::InvalidOperationState),
        }
    }
}

/// Operation-specific data for Receive operations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiveOperationData {
    /// Token to receive
    pub token: Option<String>,
    /// Derivation counter start
    pub counter_start: Option<u32>,
    /// Derivation counter end
    pub counter_end: Option<u32>,
    /// Amount received
    pub amount: Option<Amount>,
    /// Blinded messages for recovery
    ///
    /// Stored so that if a crash occurs after the mint accepts the swap,
    /// we can use these to query the mint for signatures and reconstruct proofs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blinded_messages: Option<Vec<BlindedMessage>>,
}
