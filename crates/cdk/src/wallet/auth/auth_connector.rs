use std::fmt::Debug;

use async_trait::async_trait;
use cdk_common::{AuthToken, MintInfo};

use super::Error;
use crate::nuts::{
    CheckBlindAuthStateRequest, CheckBlindAuthStateResponse, Id, KeySet, KeysetResponse,
    MintAuthRequest, MintResponse, SpendBlindAuthRequest, SpendBlindAuthResponse,
};

/// Interface that connects a wallet to a mint. Typically represents an HttpClient.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait AuthMintConnector: Debug {
    /// Get the current auth token
    async fn get_auth_token(&self) -> Result<AuthToken, Error>;

    /// Set a new auth token
    async fn set_auth_token(&self, token: AuthToken) -> Result<(), Error>;

    /// Get Mint Info [NUT-06]
    async fn get_mint_info(&self) -> Result<MintInfo, Error>;
    /// Get Blind Auth Keyset
    async fn get_mint_blind_auth_keyset(&self, keyset_id: Id) -> Result<KeySet, Error>;
    /// Get Blind Auth keysets
    async fn get_mint_blind_auth_keysets(&self) -> Result<KeysetResponse, Error>;
    /// Post mint blind auth
    async fn post_mint_blind_auth(&self, request: MintAuthRequest) -> Result<MintResponse, Error>;

    /// Check blind auth state - verify BAT validity without spending
    async fn post_blind_auth_checkstate(
        &self,
        request: CheckBlindAuthStateRequest,
    ) -> Result<CheckBlindAuthStateResponse, Error>;

    /// Spend blind auth - mark BAT as spent
    async fn post_blind_auth_spend(
        &self,
        request: SpendBlindAuthRequest,
    ) -> Result<SpendBlindAuthResponse, Error>;
}
