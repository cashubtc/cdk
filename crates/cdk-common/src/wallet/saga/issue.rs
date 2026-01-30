//! Issue (mint) saga types

use cashu::BlindedMessage;
use serde::{Deserialize, Serialize};

use crate::Error;

/// States specific to mint (issue) saga
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueSagaState {
    /// Pre-mint secrets created and counter incremented, ready to request signatures
    SecretsPrepared,
    /// Mint request sent to mint, awaiting signatures for new proofs
    MintRequested,
}

impl std::fmt::Display for IssueSagaState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IssueSagaState::SecretsPrepared => write!(f, "secrets_prepared"),
            IssueSagaState::MintRequested => write!(f, "mint_requested"),
        }
    }
}

impl std::str::FromStr for IssueSagaState {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "secrets_prepared" => Ok(IssueSagaState::SecretsPrepared),
            "mint_requested" => Ok(IssueSagaState::MintRequested),
            _ => Err(Error::InvalidOperationState),
        }
    }
}

/// Operation-specific data for Mint operations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintOperationData {
    /// Quote ID
    pub quote_id: String,
    /// Amount to mint
    pub amount: crate::Amount,
    /// Derivation counter start
    pub counter_start: Option<u32>,
    /// Derivation counter end
    pub counter_end: Option<u32>,
    /// Blinded messages for recovery
    ///
    /// Stored so that if a crash occurs after the mint accepts the request,
    /// we can use these to query the mint for signatures and reconstruct proofs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blinded_messages: Option<Vec<BlindedMessage>>,
}
