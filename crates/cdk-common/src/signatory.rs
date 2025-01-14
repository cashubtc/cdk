//! Signatory mod
//!
//! This module abstract all the key related operations, defining an interface for the necessary
//! operations, to be implemented by the different signatory implementations.
//!
//! There is an in memory implementation, when the keys are stored in memory, in the same process,
//! but it is isolated from the rest of the application, and they communicate through a channel with
//! the defined API.
use std::collections::HashMap;

use bitcoin::bip32::DerivationPath;
use cashu::{
    BlindSignature, BlindedMessage, CurrencyUnit, Id, KeySet, KeysResponse, KeysetResponse, Proof,
};

use super::error::Error;

#[async_trait::async_trait]
/// Signatory trait
pub trait Signatory {
    /// Blind sign a message
    async fn blind_sign(&self, blinded_message: BlindedMessage) -> Result<BlindSignature, Error>;

    /// Verify [`Proof`] meets conditions and is signed
    async fn verify_proof(&self, proof: Proof) -> Result<(), Error>;

    /// Retrieve a keyset by id
    async fn keyset(&self, keyset_id: Id) -> Result<Option<KeySet>, Error>;

    /// Retrieve the public keys of a keyset
    async fn keyset_pubkeys(&self, keyset_id: Id) -> Result<KeysResponse, Error>;

    /// Retrieve the public keys of the active keyset for distribution to wallet
    /// clients
    async fn pubkeys(&self) -> Result<KeysResponse, Error>;

    /// Return a list of all supported keysets
    async fn keysets(&self) -> Result<KeysetResponse, Error>;

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    async fn rotate_keyset(
        &self,
        unit: CurrencyUnit,
        derivation_path_index: u32,
        max_order: u8,
        input_fee_ppk: u64,
        custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    ) -> Result<(), Error>;
}
