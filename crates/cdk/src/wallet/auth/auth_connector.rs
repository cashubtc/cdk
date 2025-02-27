use std::fmt::Debug;

use async_trait::async_trait;
use cdk_common::MintInfo;

use super::Error;
use crate::nuts::{Id, KeySet, KeysetResponse, MintAuthRequest, MintBolt11Response};

/// Interface that connects a wallet to a mint. Typically represents an [HttpClient].
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait AuthMintConnector: Debug {
    /// Get Mint Info [NUT-06]
    async fn get_mint_info(&self) -> Result<MintInfo, Error>;
    /// Get Blind Auth Keyset
    async fn get_mint_blind_auth_keyset(&self, keyset_id: Id) -> Result<KeySet, Error>;
    /// Get Blind Auth keysets
    async fn get_mint_blind_auth_keysets(&self) -> Result<KeysetResponse, Error>;
    /// Post mint blind auth
    async fn post_mint_blind_auth(
        &self,
        request: MintAuthRequest,
    ) -> Result<MintBolt11Response, Error>;
}
