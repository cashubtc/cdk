//! Melt saga types

use cashu::BlindedMessage;
use serde::{Deserialize, Serialize};

use crate::{Amount, Error};

/// States specific to melt saga
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeltSagaState {
    /// Proofs reserved and quote locked, ready to initiate payment
    ProofsReserved,
    /// Melt request sent to mint, Lightning payment initiated
    MeltRequested,
    /// Lightning payment in progress, awaiting confirmation from network
    PaymentPending,
}

impl std::fmt::Display for MeltSagaState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeltSagaState::ProofsReserved => write!(f, "proofs_reserved"),
            MeltSagaState::MeltRequested => write!(f, "melt_requested"),
            MeltSagaState::PaymentPending => write!(f, "payment_pending"),
        }
    }
}

impl std::str::FromStr for MeltSagaState {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "proofs_reserved" => Ok(MeltSagaState::ProofsReserved),
            "melt_requested" => Ok(MeltSagaState::MeltRequested),
            "payment_pending" => Ok(MeltSagaState::PaymentPending),
            _ => Err(Error::InvalidOperationState),
        }
    }
}

/// Operation-specific data for Melt operations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeltOperationData {
    /// Quote ID
    pub quote_id: String,
    /// Amount to melt
    pub amount: Amount,
    /// Fee reserve
    pub fee_reserve: Amount,
    /// Derivation counter start
    pub counter_start: Option<u32>,
    /// Derivation counter end
    pub counter_end: Option<u32>,
    /// Change amount (if any)
    pub change_amount: Option<Amount>,
    /// Blinded messages for change recovery
    ///
    /// Stored so that if a crash occurs after the mint accepts the melt,
    /// we can use these to query the mint for change signatures and reconstruct proofs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_blinded_messages: Option<Vec<BlindedMessage>>,
}
