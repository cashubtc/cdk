//! Send saga types

use core::fmt;

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
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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

impl fmt::Debug for SendOperationData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendOperationData")
            .field("amount", &self.amount)
            .field("memo", &self.memo)
            .field("counter_start", &self.counter_start)
            .field("counter_end", &self.counter_end)
            .field("token", &self.token.as_ref().map(|_| "[redacted]"))
            .field("proofs", &self.proofs.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::SendOperationData;
    use crate::nuts::{Id, Proof, SecretKey};
    use crate::secret::Secret;
    use crate::Amount;

    const SECRET_MARKER: &str = "super_secret_spending_material_xyz";
    const TOKEN_MARKER: &str = "cashuB_super_secret_bearer_token_marker";

    #[allow(clippy::use_debug)]
    #[test]
    fn send_operation_data_debug_does_not_leak_spending_secrets() {
        let keyset_id = Id::from_str("00deadbeef123456").expect("valid keyset id");
        let proof = Proof::new(
            Amount::from(1),
            keyset_id,
            Secret::new(SECRET_MARKER),
            SecretKey::generate().public_key(),
        );

        let data = SendOperationData {
            amount: Amount::from(1),
            memo: None,
            counter_start: None,
            counter_end: None,
            token: Some(TOKEN_MARKER.to_string()),
            proofs: Some(vec![proof]),
        };

        let debug_output = format!("{data:?}");
        assert!(
            !debug_output.contains(SECRET_MARKER),
            "SendOperationData Debug leaked a proof spending secret: {debug_output}"
        );
        assert!(
            !debug_output.contains(TOKEN_MARKER),
            "SendOperationData Debug leaked the bearer token: {debug_output}"
        );
    }
}
