//! State types for the Mint (Issue) saga.
//!
//! Each state is a distinct type that holds the data relevant to that stage
//! of the mint operation. The type state pattern ensures that only valid
//! operations are available at each stage.

use cdk_common::wallet::WalletSaga;
use uuid::Uuid;

use crate::nuts::{Id, PaymentMethod, PreMintSecrets, Proofs};
use crate::wallet::MintQuote;

/// Type alias for MintRequest with String quote ID
pub type MintRequestString = crate::nuts::MintRequest<String>;

/// Initial state - operation ID assigned, no work done yet.
#[derive(Debug)]
pub struct Initial {
    /// Unique operation identifier for tracking and crash recovery
    pub operation_id: Uuid,
}

/// Prepared state - quote validated, premint secrets created, ready to execute.
#[derive(Debug)]
pub struct Prepared {
    /// Unique operation identifier
    pub operation_id: Uuid,
    /// Quote ID being minted
    pub quote_id: String,
    /// Quote information
    pub quote_info: MintQuote,
    /// Active keyset ID
    pub active_keyset_id: Id,
    /// Premint secrets
    pub premint_secrets: PreMintSecrets,
    /// Mint request ready to send
    pub mint_request: MintRequestString,
    /// Payment method (Bolt11 or Bolt12)
    pub payment_method: PaymentMethod,
    /// Persisted saga for optimistic locking and recovery
    pub saga: WalletSaga,
}

/// Finalized state - mint completed successfully, proofs available.
#[derive(Debug)]
pub struct Finalized {
    /// Minted proofs
    pub proofs: Proofs,
}
