//! Spendable Check
// https://github.com/cashubtc/nuts/blob/main/07.md

use serde::{Deserialize, Serialize};

use crate::secret::Secret;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum State {
    Spent,
    Unspent,
    Pending,
}

/// Check spendabale request [NUT-07]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckStateRequest {
    pub secrets: Vec<Secret>,
}

/// Proof state [NUT-07]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofState {
    /// Secret of proof
    pub secret: Secret,
    /// State of proof
    pub state: State,
    /// Witness data if it is supplied
    pub witness: Option<String>,
}

/// Check Spendable Response [NUT-07]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckStateResponse {
    pub states: Vec<ProofState>,
}

/// Spendable Settings
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    supported: bool,
}
