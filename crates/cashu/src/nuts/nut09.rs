//! Nut-09: Restore signatures

use serde::{Deserialize, Serialize};

use super::{BlindedMessage, BlindedSignature};

/// Restore Request [NUT-09]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreRequest {
    /// Outputs
    pub outputs: Vec<BlindedMessage>,
}

/// Restore Response [NUT-09]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreResponse {
    /// Outputs
    pub outputs: Vec<BlindedMessage>,
    /// Signatures
    pub signatures: Vec<BlindedSignature>,
}
