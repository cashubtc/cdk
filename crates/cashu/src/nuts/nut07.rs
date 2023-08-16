//! Spendable Check
// https://github.com/cashubtc/nuts/blob/main/07.md

use serde::{Deserialize, Serialize};

use super::nut00::mint;

/// Check spendabale request [NUT-07]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckSpendableRequest {
    pub proofs: mint::Proofs,
}

/// Check Spendable Response [NUT-07]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckSpendableResponse {
    /// booleans indicating whether the provided Proof is still spendable.
    /// In same order as provided proofs
    pub spendable: Vec<bool>,
    pub pending: Vec<bool>,
}
