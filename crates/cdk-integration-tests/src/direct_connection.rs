//! In-process [`MintConnector`] backed directly by a [`Mint`], for tests.
//!
//! `DirectMintConnection` is the client-side counterpart to a local mint: it
//! implements [`MintConnector`] by forwarding to the mint's
//! [`MintServer`](cdk::mint::MintServer) implementation, so no HTTP transport is
//! involved. All wire-type conversions live in `MintServer`; this type only
//! forwards. It is a test helper, so it lives here rather than in `cdk`.

use std::fmt::{Debug, Formatter};
use std::sync::Arc;

use async_trait::async_trait;
use cdk::mint::{Mint, MintServer};
use cdk::nuts::{
    BatchCheckMintQuoteRequest, BatchMintRequest, CheckStateRequest, CheckStateResponse, Id,
    KeySet, KeysetResponse, MeltRequest, MintInfo, MintRequest, MintResponse, PaymentMethod,
    RestoreRequest, RestoreResponse, SwapRequest, SwapResponse,
};
use cdk::wallet::{AuthWallet, LnurlPayInvoiceResponse, LnurlPayResponse, MintConnector};
use cdk::Error;
use cdk_common::stream_channel::{StreamRx, StreamTx};
use cdk_common::{
    MeltQuoteCreateResponse, MeltQuoteRequest, MeltQuoteResponse, MintQuoteRequest,
    MintQuoteResponse,
};
use tokio::sync::RwLock;

/// A [`MintConnector`] that talks to a [`Mint`] in-process instead of over HTTP.
pub struct DirectMintConnection {
    /// The wrapped mint.
    pub mint: Mint,
    auth_wallet: Arc<RwLock<Option<AuthWallet>>>,
}

impl DirectMintConnection {
    /// Create a new connection wrapping the given [`Mint`].
    pub fn new(mint: Mint) -> Self {
        Self {
            mint,
            auth_wallet: Arc::new(RwLock::new(None)),
        }
    }
}

impl Debug for DirectMintConnection {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DirectMintConnection")
    }
}

#[async_trait]
impl MintConnector for DirectMintConnection {
    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, Error> {
        Err(Error::Custom(
            "DNS TXT resolution is not supported by DirectMintConnection".to_string(),
        ))
    }

    async fn fetch_lnurl_pay_request(&self, _url: &str) -> Result<LnurlPayResponse, Error> {
        Err(Error::Custom(
            "Lightning address is not supported by DirectMintConnection".to_string(),
        ))
    }

    async fn fetch_lnurl_invoice(&self, _url: &str) -> Result<LnurlPayInvoiceResponse, Error> {
        Err(Error::Custom(
            "Lightning address is not supported by DirectMintConnection".to_string(),
        ))
    }

    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error> {
        self.mint.get_mint_keys().await
    }

    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
        self.mint.get_mint_keyset(keyset_id).await
    }

    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error> {
        self.mint.get_mint_keysets().await
    }

    async fn post_mint_quote(
        &self,
        request: MintQuoteRequest,
    ) -> Result<MintQuoteResponse<String>, Error> {
        self.mint.post_mint_quote(request).await
    }

    async fn get_mint_quote_status(
        &self,
        method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MintQuoteResponse<String>, Error> {
        self.mint.get_mint_quote_status(method, quote_id).await
    }

    async fn post_mint(
        &self,
        method: &PaymentMethod,
        request: MintRequest<String>,
    ) -> Result<MintResponse, Error> {
        self.mint.post_mint(method, request).await
    }

    async fn post_batch_check_mint_quote_status(
        &self,
        method: &PaymentMethod,
        request: BatchCheckMintQuoteRequest<String>,
    ) -> Result<Vec<MintQuoteResponse<String>>, Error> {
        self.mint
            .post_batch_check_mint_quote_status(method, request)
            .await
    }

    async fn post_batch_mint(
        &self,
        method: &PaymentMethod,
        request: BatchMintRequest<String>,
    ) -> Result<MintResponse, Error> {
        self.mint.post_batch_mint(method, request).await
    }

    async fn post_melt_quote(
        &self,
        request: MeltQuoteRequest,
    ) -> Result<MeltQuoteCreateResponse<String>, Error> {
        self.mint.post_melt_quote(request).await
    }

    async fn get_melt_quote_status(
        &self,
        method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MeltQuoteResponse<String>, Error> {
        self.mint.get_melt_quote_status(method, quote_id).await
    }

    async fn post_melt(
        &self,
        method: &PaymentMethod,
        request: MeltRequest<String>,
    ) -> Result<MeltQuoteResponse<String>, Error> {
        self.mint.post_melt(method, request).await
    }

    async fn post_swap(&self, request: SwapRequest) -> Result<SwapResponse, Error> {
        self.mint.post_swap(request).await
    }

    async fn get_mint_info(&self) -> Result<MintInfo, Error> {
        self.mint.get_mint_info().await
    }

    async fn post_check_state(
        &self,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        self.mint.post_check_state(request).await
    }

    async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        self.mint.post_restore(request).await
    }

    async fn get_auth_wallet(&self) -> Option<AuthWallet> {
        self.auth_wallet.read().await.clone()
    }

    async fn set_auth_wallet(&self, wallet: Option<AuthWallet>) {
        *self.auth_wallet.write().await = wallet;
    }

    async fn open_stream(&self) -> Result<(StreamTx, StreamRx), Error> {
        self.mint.open_stream().await
    }
}
