//! Signatory mod
//!
//! This module abstract all the key related operations, defining an interface for the necessary
//! operations, to be implemented by the different signatory implementations.
//!
//! There is an in memory implementation, when the keys are stored in memory, in the same process,
//! but it is isolated from the rest of the application, and they communicate through a channel with
//! the defined API.
use cdk_common::error::Error;
use cdk_common::mint::MintKeySetInfo;
use cdk_common::{
    BlindSignature, BlindedMessage, CurrencyUnit, Id, KeySet, Keys, MintKeySet, Proof, PublicKey,
};

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
///
/// TODO: Change argument to accept a vector of Amount instead of max_order.
#[derive(Debug, Clone)]
pub struct RotateKeyArguments {
    /// Unit
    pub unit: CurrencyUnit,
    /// Max order
    pub amounts: Vec<u64>,
    /// Input fee
    pub input_fee_ppk: u64,
}

#[derive(Debug, Clone)]
/// Signatory keysets
pub struct SignatoryKeysets {
    /// The public key
    pub pubkey: PublicKey,
    /// The list of keysets
    pub keysets: Vec<SignatoryKeySet>,
}

#[derive(Debug, Clone)]
/// SignatoryKeySet
///
/// This struct is used to represent a keyset and its info, pretty much all the information but the
/// private key, that will never leave the signatory
pub struct SignatoryKeySet {
    /// The keyset Id
    pub id: Id,
    /// The Currency Unit
    pub unit: CurrencyUnit,
    /// Whether to set it as active or not
    pub active: bool,
    /// The list of public keys
    pub keys: Keys,
    /// Amounts supported by the keyset
    pub amounts: Vec<u64>,
    /// Information about the fee per public key
    pub input_fee_ppk: u64,
    /// Final expiry of the keyset (unix timestamp in the future)
    pub final_expiry: Option<u64>,
}

impl From<&SignatoryKeySet> for KeySet {
    fn from(val: &SignatoryKeySet) -> Self {
        val.to_owned().into()
    }
}

impl From<SignatoryKeySet> for KeySet {
    fn from(val: SignatoryKeySet) -> Self {
        KeySet {
            id: val.id,
            unit: val.unit,
            keys: val.keys,
            final_expiry: val.final_expiry,
        }
    }
}

impl From<&SignatoryKeySet> for MintKeySetInfo {
    fn from(val: &SignatoryKeySet) -> Self {
        val.to_owned().into()
    }
}

impl From<SignatoryKeySet> for MintKeySetInfo {
    fn from(val: SignatoryKeySet) -> Self {
        MintKeySetInfo {
            id: val.id,
            unit: val.unit,
            active: val.active,
            input_fee_ppk: val.input_fee_ppk,
            derivation_path: Default::default(),
            derivation_path_index: Default::default(),
            max_order: 0,
            amounts: val.amounts,
            final_expiry: val.final_expiry,
            valid_from: 0,
        }
    }
}

impl From<&(MintKeySetInfo, MintKeySet)> for SignatoryKeySet {
    fn from((info, key): &(MintKeySetInfo, MintKeySet)) -> Self {
        Self {
            id: info.id,
            unit: key.unit.clone(),
            active: info.active,
            input_fee_ppk: info.input_fee_ppk,
            amounts: info.amounts.clone(),
            keys: key.keys.clone().into(),
            final_expiry: key.final_expiry,
        }
    }
}

#[async_trait::async_trait]
/// Signatory trait
pub trait Signatory {
    /// The Signatory implementation name. This may be exposed, so being as discrete as possible is
    /// advised.
    fn name(&self) -> String;

    /// Blind sign a message.
    ///
    /// The message can be for a coin or an auth token.
    async fn blind_sign(
        &self,
        blinded_messages: Vec<BlindedMessage>,
    ) -> Result<Vec<BlindSignature>, Error>;

    /// Verify [`Proof`] meets conditions and is signed
    async fn verify_proofs(&self, proofs: Vec<Proof>) -> Result<(), Error>;

    /// Retrieve the list of all mint keysets
    async fn keysets(&self) -> Result<SignatoryKeysets, Error>;

    /// Add current keyset to inactive keysets
    /// Generate new keyset
    async fn rotate_keyset(&self, args: RotateKeyArguments) -> Result<SignatoryKeySet, Error>;
}
