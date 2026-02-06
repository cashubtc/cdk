//! Swap saga types

use cashu::BlindedMessage;
use serde::{Deserialize, Serialize};

use crate::{Amount, Error};

/// States specific to swap saga (wallet-side)
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwapSagaState {
    /// Input proofs reserved, swap request prepared, ready to execute
    ProofsReserved,
    /// Swap request sent to mint, awaiting signatures for new proofs
    SwapRequested,
}

impl std::fmt::Display for SwapSagaState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SwapSagaState::ProofsReserved => write!(f, "proofs_reserved"),
            SwapSagaState::SwapRequested => write!(f, "swap_requested"),
        }
    }
}

impl std::str::FromStr for SwapSagaState {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "proofs_reserved" => Ok(SwapSagaState::ProofsReserved),
            "swap_requested" => Ok(SwapSagaState::SwapRequested),
            _ => Err(Error::InvalidOperationState),
        }
    }
}

/// Operation-specific data for Swap operations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapOperationData {
    /// Input amount
    pub input_amount: Amount,
    /// Output amount
    pub output_amount: Amount,
    /// Derivation counter start
    pub counter_start: Option<u32>,
    /// Derivation counter end
    pub counter_end: Option<u32>,
    /// Blinded messages for recovery
    ///
    /// Stored so that if a crash occurs after the mint accepts the swap,
    /// we can use these to query the mint for signatures and reconstruct proofs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blinded_messages: Option<Vec<BlindedMessage>>,
}
