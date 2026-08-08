//! State types for the Receive saga.
//!
//! Each state is a distinct type holding data relevant to that stage.
//! The typestate pattern ensures only valid operations are available at each stage.
//!
//! # Type State Flow
//!
//! ```text
//! Initial
//!   └─> prepare() -> Prepared
//!                      └─> execute() -> Finalized
//!                                         └─> amount(), into_amount()
//! ```

use core::fmt;
use std::collections::HashMap;

use bitcoin::XOnlyPublicKey;
use cdk_common::wallet::KeysetLoadPolicy;
use uuid::Uuid;

use crate::nuts::{Id, Proofs, SecretKey};
use crate::wallet::receive::ReceiveOptions;
use crate::Amount;

/// Initial state - operation ID assigned but no work done yet.
/// Only `prepare()` is available in this state.
#[derive(Debug)]
pub struct Initial {
    /// Unique operation identifier for tracking and crash recovery
    pub operation_id: Uuid,
    /// Policy controlling how keysets are loaded during this saga
    pub keyset_policy: KeysetLoadPolicy,
}

/// Prepared state - token has been parsed and proofs extracted.
/// `execute()` is available in this state.
pub struct Prepared {
    /// Unique operation identifier
    pub operation_id: Uuid,
    /// Options for the receive operation
    pub options: ReceiveOptions,
    /// Memo from the token (if any)
    pub memo: Option<String>,
    /// Token string (if any)
    pub token: Option<String>,
    /// Proofs extracted from the token (potentially signed for P2PK/HTLC)
    pub proofs: Proofs,
    /// Total amount of the incoming proofs
    pub proofs_amount: Amount,
    /// Active keyset ID for the swap
    pub active_keyset_id: Id,
    /// P2PK signing keys (from options + wallet database lookups)
    pub p2pk_signing_keys: HashMap<XOnlyPublicKey, SecretKey>,
}

impl fmt::Debug for Prepared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prepared")
            .field("operation_id", &self.operation_id)
            .field("options", &self.options)
            .field("memo", &self.memo)
            .field("token", &self.token.as_ref().map(|_| "[redacted]"))
            .field(
                "proof_amounts",
                &self
                    .proofs
                    .iter()
                    .map(|proof| proof.amount)
                    .collect::<Vec<_>>(),
            )
            .field("proofs_amount", &self.proofs_amount)
            .field("active_keyset_id", &self.active_keyset_id)
            .field("p2pk_signing_key_count", &self.p2pk_signing_keys.len())
            .finish()
    }
}

/// Finalized state - receive operation completed successfully.
/// The received amount can be retrieved from this state.
#[derive(Debug)]
pub struct Finalized {
    /// Total amount received (after fees)
    pub amount: Amount,
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use uuid::Uuid;

    use super::Prepared;
    use crate::secret::Secret;
    use crate::wallet::receive::ReceiveOptions;
    use crate::wallet::test_utils::{test_keyset_id, test_proof};
    use crate::Amount;

    const SECRET_MARKER: &str = "super_secret_spending_material_xyz";
    const TOKEN_MARKER: &str = "cashuB_super_secret_bearer_token_marker";
    const PREIMAGE_MARKER: &str = "super_secret_htlc_preimage_xyz";

    #[allow(clippy::use_debug)]
    #[test]
    fn prepared_debug_does_not_leak_receive_secrets() {
        let keyset_id = test_keyset_id();
        let mut proof = test_proof(keyset_id, 1);
        proof.secret = Secret::new(SECRET_MARKER);

        let state = Prepared {
            operation_id: Uuid::new_v4(),
            options: ReceiveOptions {
                preimages: vec![PREIMAGE_MARKER.to_string()],
                ..Default::default()
            },
            memo: None,
            token: Some(TOKEN_MARKER.to_string()),
            proofs: vec![proof],
            proofs_amount: Amount::from(1),
            active_keyset_id: keyset_id,
            p2pk_signing_keys: HashMap::new(),
        };

        let debug_output = format!("{state:?}");
        assert!(!debug_output.contains(SECRET_MARKER));
        assert!(!debug_output.contains(TOKEN_MARKER));
        assert!(!debug_output.contains(PREIMAGE_MARKER));
        assert!(debug_output.contains("[redacted]"));
    }
}
