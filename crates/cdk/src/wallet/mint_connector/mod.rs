//! Wallet client

use std::fmt::Debug;

use async_trait::async_trait;
use cdk_common::{MeltQuoteBolt12Request, MintQuoteBolt12Request, MintQuoteBolt12Response};

use super::Error;
use crate::nuts::{
    CheckStateRequest, CheckStateResponse, Id, KeySet, KeysetResponse, MeltQuoteBolt11Request,
    MeltQuoteBolt11Response, MeltRequest, MintInfo, MintQuoteBolt11Request,
    MintQuoteBolt11Response, MintQuoteMiningShareRequest, MintQuoteMiningShareResponse,
    MintRequest, MintResponse, RestoreRequest, RestoreResponse, SwapRequest, SwapResponse,
};
#[cfg(feature = "auth")]
use crate::wallet::AuthWallet;

pub mod http_client;
pub mod transport;

/// Auth HTTP Client with async transport
#[cfg(feature = "auth")]
pub type AuthHttpClient = http_client::AuthHttpClient<transport::Async>;
/// Default Http Client with async transport (non-Tor)
pub type HttpClient = http_client::HttpClient<transport::Async>;
/// Tor Http Client with async transport (only when `tor` feature is enabled and not on wasm32)
#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
pub type TorHttpClient = http_client::HttpClient<transport::tor_transport::TorAsync>;

/// Interface that connects a wallet to a mint. Typically represents an [HttpClient].
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MintConnector: Debug {
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    /// Resolve the DNS record getting the TXT value
    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, Error>;

    /// Get Active Mint Keys [NUT-01]
    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error>;
    /// Get Keyset Keys [NUT-01]
    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error>;
    /// Get Keysets [NUT-02]
    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error>;
    /// Mint Quote [NUT-04]
    async fn post_mint_quote(
        &self,
        request: MintQuoteBolt11Request,
    ) -> Result<MintQuoteBolt11Response<String>, Error>;
    /// Mint Quote status
    async fn get_mint_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteBolt11Response<String>, Error>;
    /// Mint Tokens [NUT-04]
    async fn post_mint(&self, request: MintRequest<String>) -> Result<MintResponse, Error>;
    /// Melt Quote [NUT-05]
    async fn post_melt_quote(
        &self,
        request: MeltQuoteBolt11Request,
    ) -> Result<MeltQuoteBolt11Response<String>, Error>;
    /// Melt Quote Status
    async fn get_melt_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response<String>, Error>;
    /// Melt [NUT-05]
    /// [Nut-08] Lightning fee return if outputs defined
    async fn post_melt(
        &self,
        request: MeltRequest<String>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error>;
    /// Split Token [NUT-06]
    async fn post_swap(&self, request: SwapRequest) -> Result<SwapResponse, Error>;
    /// Get Mint Info [NUT-06]
    async fn get_mint_info(&self) -> Result<MintInfo, Error>;
    /// Spendable check [NUT-07]
    async fn post_check_state(
        &self,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error>;
    /// Restore request [NUT-13]
    async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error>;

    /// Get the auth wallet for the client
    #[cfg(feature = "auth")]
    async fn get_auth_wallet(&self) -> Option<AuthWallet>;

    /// Set auth wallet on client
    #[cfg(feature = "auth")]
    async fn set_auth_wallet(&self, wallet: Option<AuthWallet>);
    /// Mint Quote [NUT-04]
    async fn post_mint_bolt12_quote(
        &self,
        request: MintQuoteBolt12Request,
    ) -> Result<MintQuoteBolt12Response<String>, Error>;
    /// Mint Quote status
    async fn get_mint_quote_bolt12_status(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteBolt12Response<String>, Error>;
    /// Melt Quote [NUT-23]
    async fn post_melt_bolt12_quote(
        &self,
        request: MeltQuoteBolt12Request,
    ) -> Result<MeltQuoteBolt11Response<String>, Error>;
    /// Melt Quote Status [NUT-23]
    async fn get_melt_bolt12_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response<String>, Error>;
    /// Melt [NUT-23]
    async fn post_melt_bolt12(
        &self,
        request: MeltRequest<String>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error>;

    /// Mint Quote for Mining Share [NUT-XX]
    async fn post_mint_quote_mining_share(
        &self,
        request: MintQuoteMiningShareRequest,
    ) -> Result<MintQuoteMiningShareResponse<String>, Error>;

    /// Mint Quote status for Mining Share [NUT-XX]
    async fn get_mint_quote_status_mining_share(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteMiningShareResponse<String>, Error>;

    /// Mint Tokens for Mining Share [NUT-XX]
    async fn post_mint_mining_share(
        &self,
        request: MintRequest<String>,
    ) -> Result<MintResponse, Error>;
}
