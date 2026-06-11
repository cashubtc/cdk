//! State types for the Send saga.
//!
//! Each state is a distinct type that holds the data relevant to that stage
//! of the send operation. The type state pattern ensures that only valid
//! operations are available at each stage.

use core::fmt;

use cdk_common::wallet::WalletSaga;
use uuid::Uuid;

use crate::nuts::Proofs;
use crate::wallet::send::SendOptions;
use crate::Amount;

/// Initial state before any work is done.
#[derive(Debug)]
pub struct Initial {
    /// Unique operation identifier for tracking and crash recovery
    pub operation_id: Uuid,
}

/// Prepared state with proofs selected and reserved.
pub struct Prepared {
    /// Unique operation identifier
    pub operation_id: Uuid,
    /// Amount to send
    pub amount: Amount,
    /// Send options
    pub options: SendOptions,
    /// Proofs that need to be swapped before sending
    pub proofs_to_swap: Proofs,
    /// Fee for the swap operation
    pub swap_fee: Amount,
    /// Proofs that will be included in the token directly
    pub proofs_to_send: Proofs,
    /// Fee the recipient will pay to redeem the token
    pub send_fee: Amount,
    /// The persisted saga for optimistic locking
    pub saga: WalletSaga,
}

/// Token created state after send is confirmed.
pub struct TokenCreated {
    /// Unique operation identifier
    pub operation_id: Uuid,
    /// Proofs included in the token (needed for revocation/checking status)
    pub proofs: Proofs,
    /// The persisted saga for optimistic locking
    pub saga: WalletSaga,
}

impl fmt::Debug for Prepared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prepared")
            .field("operation_id", &self.operation_id)
            .field("amount", &self.amount)
            .field("options", &self.options)
            .field(
                "proofs_to_swap",
                &self
                    .proofs_to_swap
                    .iter()
                    .map(|p| p.amount)
                    .collect::<Vec<_>>(),
            )
            .field("swap_fee", &self.swap_fee)
            .field(
                "proofs_to_send",
                &self
                    .proofs_to_send
                    .iter()
                    .map(|p| p.amount)
                    .collect::<Vec<_>>(),
            )
            .field("send_fee", &self.send_fee)
            .field("saga_id", &self.saga.id)
            .field("saga_state", &self.saga.state)
            .field("saga_version", &self.saga.version)
            .finish()
    }
}

impl fmt::Debug for TokenCreated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenCreated")
            .field("operation_id", &self.operation_id)
            .field(
                "proofs",
                &self.proofs.iter().map(|p| p.amount).collect::<Vec<_>>(),
            )
            .field("saga_id", &self.saga.id)
            .field("saga_state", &self.saga.state)
            .field("saga_version", &self.saga.version)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use cdk_common::wallet::{
        OperationData, SendOperationData, SendSagaState, WalletSaga, WalletSagaState,
    };
    use cdk_common::CurrencyUnit;
    use uuid::Uuid;

    use super::{Prepared, TokenCreated};
    use crate::secret::Secret;
    use crate::wallet::send::SendOptions;
    use crate::wallet::test_utils::{test_keyset_id, test_mint_url, test_proof};
    use crate::Amount;

    const SECRET_MARKER: &str = "super_secret_spending_material_xyz";

    fn send_saga(
        operation_id: Uuid,
        state: SendSagaState,
        amount: Amount,
        proofs: Option<crate::nuts::Proofs>,
    ) -> WalletSaga {
        WalletSaga::new(
            operation_id,
            WalletSagaState::Send(state),
            amount,
            test_mint_url(),
            CurrencyUnit::Sat,
            OperationData::Send(SendOperationData {
                amount,
                memo: None,
                counter_start: None,
                counter_end: None,
                token: Some(SECRET_MARKER.to_string()),
                proofs,
            }),
        )
    }

    #[allow(clippy::use_debug)]
    #[test]
    fn prepared_debug_does_not_leak_secret() {
        let operation_id = Uuid::new_v4();
        let keyset_id = test_keyset_id();

        let mut proof = test_proof(keyset_id, 1);
        proof.secret = Secret::new(SECRET_MARKER);

        let state = Prepared {
            operation_id,
            amount: Amount::from(1),
            options: SendOptions::default(),
            proofs_to_swap: vec![proof.clone()],
            swap_fee: Amount::from(0),
            proofs_to_send: vec![proof.clone()],
            send_fee: Amount::from(0),
            saga: send_saga(
                operation_id,
                SendSagaState::ProofsReserved,
                Amount::from(1),
                Some(vec![proof]),
            ),
        };

        let debug_output = format!("{state:?}");
        assert!(
            !debug_output.contains(SECRET_MARKER),
            "Prepared Debug leaked the spending secret: {debug_output}"
        );
    }

    #[allow(clippy::use_debug)]
    #[test]
    fn token_created_debug_does_not_leak_secret() {
        let operation_id = Uuid::new_v4();
        let keyset_id = test_keyset_id();

        let mut proof = test_proof(keyset_id, 1);
        proof.secret = Secret::new(SECRET_MARKER);

        let state = TokenCreated {
            operation_id,
            proofs: vec![proof.clone()],
            saga: send_saga(
                operation_id,
                SendSagaState::TokenCreated,
                Amount::from(1),
                Some(vec![proof]),
            ),
        };

        let debug_output = format!("{state:?}");
        assert!(
            !debug_output.contains(SECRET_MARKER),
            "TokenCreated Debug leaked the spending secret: {debug_output}"
        );
    }
}
