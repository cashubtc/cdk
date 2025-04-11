//! Signatory mod
//!
//! This module abstract all the key related operations, defining an interface for the necessary
//! operations, to be implemented by the different signatory implementations.
//!
//! There is an in memory implementation, when the keys are stored in memory, in the same process,
//! but it is isolated from the rest of the application, and they communicate through a channel with
//! the defined API.
use cashu::{BlindSignature, BlindedMessage, CurrencyUnit, Id, KeySet, MintKeySet, Proof};
use cdk_common::error::Error;
use cdk_common::mint::MintKeySetInfo;

#[derive(Debug)]
/// Type alias to make the keyset info API more useful, queryable by unit and Id
pub enum KeysetIdentifier {
    /// Mint Keyset by unit
    Unit(CurrencyUnit),
    /// Mint Keyset by Id
    Id(Id),
}

impl From<Id> for KeysetIdentifier {
    fn from(id: Id) -> Self {
        Self::Id(id)
    }
}

impl From<CurrencyUnit> for KeysetIdentifier {
    fn from(unit: CurrencyUnit) -> Self {
        Self::Unit(unit)
    }
}

/// RotateKeyArguments
///
/// This struct is used to pass the arguments to the rotate_keyset function
#[derive(Debug, Clone)]
pub struct RotateKeyArguments {
    pub unit: CurrencyUnit,
    pub derivation_path_index: Option<u32>,
    pub max_order: u8,
    pub input_fee_ppk: u64,
}

#[derive(Debug, Clone)]
/// SignatoryKeySet
///
/// This struct is used to represent a keyset and its info, pretty much all the information but the
/// private key, that will never leave the signatory
pub struct SignatoryKeySet {
    /// KeySet
    pub key: KeySet,
    /// MintSetInfo
    pub info: MintKeySetInfo,
}

impl From<&(MintKeySetInfo, MintKeySet)> for SignatoryKeySet {
    fn from((info, key): &(MintKeySetInfo, MintKeySet)) -> Self {
        Self {
            key: key.clone().into(),
            info: info.clone(),
        }
    }
}

#[async_trait::async_trait]
/// Signatory trait
pub trait Signatory {
    /// Get all the mint keysets for authentication
    async fn auth_keysets(&self) -> Result<Option<Vec<SignatoryKeySet>>, Error>;

    /// Blind sign a message.
    ///
    /// The message can be for a coin or an auth token.
    async fn blind_sign(&self, blinded_message: BlindedMessage) -> Result<BlindSignature, Error>;

    /// Verify [`Proof`] meets conditions and is signed
    async fn verify_proof(&self, proofs: Proof) -> Result<(), Error>;

    /// Retrieve the list of all mint keysets
    async fn keysets(&self) -> Result<Vec<SignatoryKeySet>, Error>;

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    async fn rotate_keyset(&self, args: RotateKeyArguments) -> Result<MintKeySetInfo, Error>;
}
