//! Send saga types

use serde::{Deserialize, Serialize};

use crate::nuts::Proofs;
use crate::{Amount, Error};

/// States specific to send saga
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendSagaState {
    /// Proofs selected and reserved for sending, ready to create token
    ProofsReserved,
    /// Token created and ready to share, proofs marked as pending spent awaiting claim
    TokenCreated,
    /// Rollback in progress, reclaiming proofs via swap (transient state)
    RollingBack,
}

impl std::fmt::Display for SendSagaState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendSagaState::ProofsReserved => write!(f, "proofs_reserved"),
            SendSagaState::TokenCreated => write!(f, "token_created"),
            SendSagaState::RollingBack => write!(f, "rolling_back"),
        }
    }
}

impl std::str::FromStr for SendSagaState {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "proofs_reserved" => Ok(SendSagaState::ProofsReserved),
            "token_created" => Ok(SendSagaState::TokenCreated),
            "rolling_back" => Ok(SendSagaState::RollingBack),
            _ => Err(Error::InvalidOperationState),
        }
    }
}

/// Operation-specific data for Send operations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendOperationData {
    /// Target amount to send
    pub amount: Amount,
    /// Memo for the send
    pub memo: Option<String>,
    /// Derivation counter start
    pub counter_start: Option<u32>,
    /// Derivation counter end
    pub counter_end: Option<u32>,
    /// Token data (when in Pending/Finalized state)
    pub token: Option<String>,
    /// Proofs being sent
    pub proofs: Option<Proofs>,
}
