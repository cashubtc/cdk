//! Wallet client

use std::fmt::Debug;

use async_trait::async_trait;

use super::Error;
use crate::nuts::nutxx1::MintAuthRequest;
use crate::nuts::{
    AuthToken, CheckStateRequest, CheckStateResponse, Id, KeySet, KeysetResponse,
    MeltBolt11Request, MeltQuoteBolt11Request, MeltQuoteBolt11Response, MintBolt11Request,
    MintBolt11Response, MintInfo, MintQuoteBolt11Request, MintQuoteBolt11Response, RestoreRequest,
    RestoreResponse, SwapRequest, SwapResponse,
};

mod http_client;

pub use http_client::HttpClient;

/// Interface that connects a wallet to a mint. Typically represents an [HttpClient].
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MintConnector: Debug {
    /// Get Active Mint Keys [NUT-01]
    async fn get_mint_keys(&self, auth_token: Option<AuthToken>) -> Result<Vec<KeySet>, Error>;
    /// Get Keyset Keys [NUT-01]
    async fn get_mint_keyset(
        &self,
        keyset_id: Id,
        auth_token: Option<AuthToken>,
    ) -> Result<KeySet, Error>;
    /// Get Keysets [NUT-02]
    async fn get_mint_keysets(
        &self,
        auth_token: Option<AuthToken>,
    ) -> Result<KeysetResponse, Error>;
    /// Mint Quote [NUT-04]
    async fn post_mint_quote(
        &self,
        request: MintQuoteBolt11Request,
        auth_token: Option<AuthToken>,
    ) -> Result<MintQuoteBolt11Response<String>, Error>;
    /// Mint Quote status
    async fn get_mint_quote_status(
        &self,
        quote_id: &str,
        auth_token: Option<AuthToken>,
    ) -> Result<MintQuoteBolt11Response<String>, Error>;
    /// Mint Tokens [NUT-04]
    async fn post_mint(
        &self,
        request: MintBolt11Request<String>,
        auth_token: Option<AuthToken>,
    ) -> Result<MintBolt11Response, Error>;
    /// Melt Quote [NUT-05]
    async fn post_melt_quote(
        &self,
        request: MeltQuoteBolt11Request,
        auth_token: Option<AuthToken>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error>;
    /// Melt Quote Status
    async fn get_melt_quote_status(
        &self,
        quote_id: &str,
        auth_token: Option<AuthToken>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error>;
    /// Melt [NUT-05]
    /// [Nut-08] Lightning fee return if outputs defined
    async fn post_melt(
        &self,
        request: MeltBolt11Request<String>,
        auth_token: Option<AuthToken>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error>;
    /// Split Token [NUT-06]
    async fn post_swap(
        &self,
        request: SwapRequest,
        auth_token: Option<AuthToken>,
    ) -> Result<SwapResponse, Error>;
    /// Get Mint Info [NUT-06]
    async fn get_mint_info(&self) -> Result<MintInfo, Error>;
    /// Spendable check [NUT-07]
    async fn post_check_state(
        &self,
        request: CheckStateRequest,
        auth_token: Option<AuthToken>,
    ) -> Result<CheckStateResponse, Error>;
    /// Restore request [NUT-13]
    async fn post_restore(
        &self,
        request: RestoreRequest,
        auth_token: Option<AuthToken>,
    ) -> Result<RestoreResponse, Error>;

    /// Get Blind Auth keys
    async fn get_mint_blind_auth_keys(&self) -> Result<Vec<KeySet>, Error>;
    /// Get Blind Auth Keyset
    async fn get_mint_blind_auth_keyset(&self, keyset_id: Id) -> Result<KeySet, Error>;
    /// Get Blind Auth keysets
    async fn get_mint_blind_auth_keysets(&self) -> Result<KeysetResponse, Error>;
    /// Post mint blind auth
    async fn post_mint_blind_auth(
        &self,
        request: MintAuthRequest,
        auth_token: AuthToken,
    ) -> Result<MintBolt11Response, Error>;
}
