//! Signatory
//!
//! Types to define new signatory instances and their types

use std::collections::HashMap;

use bitcoin::bip32::DerivationPath;

use crate::nuts::nut00::{BlindSignature, BlindedMessage, Proof};
use crate::{CurrencyUnit, Id, KeySet, KeysResponse, KeysetResponse};

#[derive(thiserror::Error, Debug)]
pub enum Error {}

#[async_trait::async_trait]
/// Signatory trait
pub trait Signatory {
    /// Blind sign a message
    async fn blind_sign<B>(&self, blinded_message: B) -> Result<BlindSignature, Error>
    where
        B: Into<BlindedMessage>;

    /// Verify [`Proof`] meets conditions and is signed
    async fn verify_proof<P>(&self, proof: P) -> Result<(), Error>
    where
        P: Into<Proof>;

    /// Retrieve a keyset by id
    async fn keyset<I>(&self, keyset_id: I) -> Result<Option<KeySet>, Error>
    where
        I: Into<Id>;

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
