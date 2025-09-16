//! Mint in memory database use std::collections::HashMap;

use std::collections::HashMap;

use async_trait::async_trait;
use cashu::{AuthRequired, ProtectedEndpoint};

use super::DbTransactionFinalizer;
use crate::database::Error;
use crate::mint::MintKeySetInfo;
use crate::nuts::nut07::State;
use crate::nuts::{AuthProof, BlindSignature, Id, PublicKey};

/// Mint Database transaction
#[async_trait]
pub trait MintAuthTransaction<Error>: DbTransactionFinalizer<Err = Error> {
    /// Add Active Keyset
    async fn set_active_keyset(&mut self, id: Id) -> Result<(), Error>;

    /// Add [`MintKeySetInfo`]
    async fn add_keyset_info(&mut self, keyset: MintKeySetInfo) -> Result<(), Error>;

    /// Add spent [`AuthProof`]
    async fn add_proof(&mut self, proof: AuthProof) -> Result<(), Error>;

    /// Update [`AuthProof`]s state
    async fn update_proof_state(
        &mut self,
        y: &PublicKey,
        proofs_state: State,
    ) -> Result<Option<State>, Error>;

    /// Add [`BlindSignature`]
    async fn add_blind_signatures(
        &mut self,
        blinded_messages: &[PublicKey],
        blind_signatures: &[BlindSignature],
    ) -> Result<(), Error>;

    /// Add protected endpoints
    async fn add_protected_endpoints(
        &mut self,
        protected_endpoints: HashMap<ProtectedEndpoint, AuthRequired>,
    ) -> Result<(), Error>;

    /// Removed Protected endpoints
    async fn remove_protected_endpoints(
        &mut self,
        protected_endpoints: Vec<ProtectedEndpoint>,
    ) -> Result<(), Error>;
}

/// Mint Database trait
#[async_trait]
pub trait MintAuthDatabase {
    /// Mint Database Error
    type Err: Into<Error> + From<Error>;

    /// Begins a transaction
    async fn begin_transaction<'a>(
        &'a self,
    ) -> Result<Box<dyn MintAuthTransaction<Self::Err> + Send + Sync + 'a>, Self::Err>;

    /// Get Active Keyset
    async fn get_active_keyset_id(&self) -> Result<Option<Id>, Self::Err>;

    /// Get [`MintKeySetInfo`]
    async fn get_keyset_info(&self, id: &Id) -> Result<Option<MintKeySetInfo>, Self::Err>;
    /// Get [`MintKeySetInfo`]s
    async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Self::Err>;

    /// Get [`AuthProof`] state
    async fn get_proofs_states(&self, ys: &[PublicKey]) -> Result<Vec<Option<State>>, Self::Err>;

    /// Get [`BlindSignature`]s
    async fn get_blind_signatures(
        &self,
        blinded_messages: &[PublicKey],
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err>;

    /// Get auth for protected_endpoint
    async fn get_auth_for_endpoint(
        &self,
        protected_endpoint: ProtectedEndpoint,
    ) -> Result<Option<AuthRequired>, Self::Err>;
    /// Get protected endpoints
    async fn get_auth_for_endpoints(
        &self,
    ) -> Result<HashMap<ProtectedEndpoint, Option<AuthRequired>>, Self::Err>;
}

/// Type alias for trait objects
pub type DynMintAuthDatabase =
    std::sync::Arc<dyn MintAuthDatabase<Err = super::Error> + Send + Sync>;
