//! Types for `cashu-crab`

use serde::{Deserialize, Serialize};

use crate::nuts::nut00::{mint, Proofs};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofsStatus {
    pub spendable: mint::Proofs,
    pub spent: mint::Proofs,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendProofs {
    pub change_proofs: Proofs,
    pub send_proofs: Proofs,
}
