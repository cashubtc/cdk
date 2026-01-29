//! State types for the Send saga.
//!
//! Each state is a distinct type that holds the data relevant to that stage
//! of the send operation. The type state pattern ensures that only valid
//! operations are available at each stage.

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
#[derive(Debug)]
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
#[derive(Debug)]
pub struct TokenCreated {
    /// Unique operation identifier
    pub operation_id: Uuid,
    /// Proofs included in the token (needed for revocation/checking status)
    pub proofs: Proofs,
    /// The persisted saga for optimistic locking
    pub saga: WalletSaga,
}
