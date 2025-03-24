//! Mint in memory database use std::collections::HashMap;

use std::collections::HashMap;

use async_trait::async_trait;
use cashu::{AuthRequired, ProtectedEndpoint};

use crate::database::Error;
use crate::mint::MintKeySetInfo;
use crate::nuts::nut07::State;
use crate::nuts::{AuthProof, BlindSignature, Id, PublicKey};

/// Mint Database trait
#[async_trait]
pub trait MintAuthDatabase {
    /// Mint Database Error
    type Err: Into<Error> + From<Error>;
    /// Add Active Keyset
    async fn set_active_keyset(&self, id: Id) -> Result<(), Self::Err>;
    /// Get Active Keyset
    async fn get_active_keyset_id(&self) -> Result<Option<Id>, Self::Err>;

    /// Add [`MintKeySetInfo`]
    async fn add_keyset_info(&self, keyset: MintKeySetInfo) -> Result<(), Self::Err>;
    /// Get [`MintKeySetInfo`]
    async fn get_keyset_info(&self, id: &Id) -> Result<Option<MintKeySetInfo>, Self::Err>;
    /// Get [`MintKeySetInfo`]s
    async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Self::Err>;

    /// Add spent [`Proofs`]
    async fn add_proof(&self, proof: AuthProof) -> Result<(), Self::Err>;
    /// Get [`Proofs`] state
    async fn get_proofs_states(&self, ys: &[PublicKey]) -> Result<Vec<Option<State>>, Self::Err>;
    /// Get [`Proofs`] state
    async fn update_proof_state(
        &self,
        y: &PublicKey,
        proofs_state: State,
    ) -> Result<Option<State>, Self::Err>;

    /// Add [`BlindSignature`]
    async fn add_blind_signatures(
        &self,
        blinded_messages: &[PublicKey],
        blind_signatures: &[BlindSignature],
    ) -> Result<(), Self::Err>;
    /// Get [`BlindSignature`]s
    async fn get_blind_signatures(
        &self,
        blinded_messages: &[PublicKey],
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err>;

    /// Add protected endpoints
    async fn add_protected_endpoints(
        &self,
        protected_endpoints: HashMap<ProtectedEndpoint, AuthRequired>,
    ) -> Result<(), Self::Err>;
    /// Removed Protected endpoints
    async fn remove_protected_endpoints(
        &self,
        protected_endpoints: Vec<ProtectedEndpoint>,
    ) -> Result<(), Self::Err>;
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
