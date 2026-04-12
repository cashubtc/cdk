//! Wallet client

use std::fmt::Debug;

use async_trait::async_trait;
use cdk_common::{
    MeltQuoteCreateResponse, MeltQuoteRequest, MeltQuoteResponse, MintQuoteRequest,
    MintQuoteResponse,
};

use super::Error;
// Re-export Lightning address types for trait implementers
pub use crate::lightning_address::{LnurlPayInvoiceResponse, LnurlPayResponse};
use crate::nuts::{
    BatchCheckMintQuoteRequest, BatchMintRequest, CheckStateRequest, CheckStateResponse, Id,
    KeySet, KeysetResponse, MeltRequest, MintInfo, MintQuoteBolt11Response, MintRequest,
    MintResponse, PaymentMethod, RestoreRequest, RestoreResponse, SwapRequest, SwapResponse,
};
use crate::wallet::AuthWallet;

pub mod http_client;
pub mod transport;

/// Auth HTTP Client with async transport
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

    /// Fetch Lightning address pay request data
    async fn fetch_lnurl_pay_request(
        &self,
        url: &str,
    ) -> Result<crate::lightning_address::LnurlPayResponse, Error>;

    /// Fetch invoice from Lightning address callback
    async fn fetch_lnurl_invoice(
        &self,
        url: &str,
    ) -> Result<crate::lightning_address::LnurlPayInvoiceResponse, Error>;

    /// Get Active Mint Keys [NUT-01]
    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error>;
    /// Get Keyset Keys [NUT-01]
    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error>;
    /// Get Keysets [NUT-02]
    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error>;
    /// Mint Quote [NUT-04, NUT-23, NUT-25]
    async fn post_mint_quote(
        &self,
        request: MintQuoteRequest,
    ) -> Result<MintQuoteResponse<String>, Error>;
    /// Mint Tokens [NUT-04]
    async fn post_mint(
        &self,
        method: &PaymentMethod,
        request: MintRequest<String>,
    ) -> Result<MintResponse, Error>;

    /// Batch check mint quote status [NUT-29]
    ///
    /// Checks the status of multiple mint quotes in a single request.
    /// The response type is `Vec<MintQuoteBolt11Response>` for bolt11 quotes.
    /// For other payment methods, the response is method-specific.
    async fn post_batch_check_mint_quote_status(
        &self,
        method: &PaymentMethod,
        request: BatchCheckMintQuoteRequest<String>,
    ) -> Result<Vec<MintQuoteBolt11Response<String>>, Error>;

    /// Batch mint tokens [NUT-29]
    ///
    /// Mints tokens for multiple quotes in a single atomic request.
    async fn post_batch_mint(
        &self,
        method: &PaymentMethod,
        request: BatchMintRequest<String>,
    ) -> Result<MintResponse, Error>;

    /// Melt Quote [NUT-05]
    async fn post_melt_quote(
        &self,
        request: MeltQuoteRequest,
    ) -> Result<MeltQuoteCreateResponse<String>, Error>;

    /// Mint Quote status with payment method
    async fn get_mint_quote_status(
        &self,
        method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MintQuoteResponse<String>, Error>;

    /// Melt [NUT-05]
    /// Melt Quote Status
    async fn get_melt_quote_status(
        &self,
        method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MeltQuoteResponse<String>, Error>;

    /// [Nut-08] Lightning fee return if outputs defined
    async fn post_melt(
        &self,
        method: &PaymentMethod,
        request: MeltRequest<String>,
    ) -> Result<MeltQuoteResponse<String>, Error>;

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
    async fn get_auth_wallet(&self) -> Option<AuthWallet>;

    /// Set auth wallet on client
    async fn set_auth_wallet(&self, wallet: Option<AuthWallet>);
}
