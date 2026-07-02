//! State types for the Mint (Issue) saga.
//!
//! Each state is a distinct type that holds the data relevant to that stage
//! of the mint operation. The type state pattern ensures that only valid
//! operations are available at each stage.

use cdk_common::wallet::WalletSaga;
use uuid::Uuid;

use crate::nuts::{BatchMintRequest, Id, PaymentMethod, PreMintSecrets, Proofs};
use crate::wallet::MintQuote;

/// Type alias for MintRequest with String quote ID
pub type MintRequestString = crate::nuts::MintRequest<String>;

/// Initial state - operation ID assigned, no work done yet.
#[derive(Debug)]
pub struct Initial {
    /// Unique operation identifier for tracking and crash recovery
    pub operation_id: Uuid,
}

/// The mint request type - either single quote or batch.
#[derive(Debug)]
pub enum PreparedMintRequest {
    /// Single quote mint request (legacy NUT-04)
    Single {
        /// Quote ID being minted
        quote_id: String,
        /// Quote information
        quote_info: MintQuote,
        /// Mint request ready to send
        request: MintRequestString,
    },
    /// Batch mint request (NUT-29)
    Batch {
        /// Quote IDs being minted
        quote_ids: Vec<String>,
        /// Quote information for each quote
        quote_infos: Vec<MintQuote>,
        /// Batch mint request ready to send
        request: BatchMintRequest<String>,
    },
}

impl PreparedMintRequest {}

/// Prepared state - quote validated, premint secrets created, ready to execute.
#[derive(Debug)]
pub struct Prepared {
    /// Unique operation identifier
    pub operation_id: Uuid,
    /// Active keyset ID
    pub active_keyset_id: Id,
    /// Premint secrets
    pub premint_secrets: PreMintSecrets,
    /// Mint request (single or batch)
    pub mint_request: PreparedMintRequest,
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
