//! `MintConnector` suite over an in-process loopback that encodes every unary
//! call to `MintRpcRequest`, runs it through `cdk::rpc::dispatch`, and decodes
//! the `MintRpcResponse`. This exercises the shared RPC envelope (G3) end to end
//! and doubles as a reference for how a real transport's client and server halves
//! fit together, minus the byte channel.

use std::fmt::{Debug, Formatter};
use std::sync::Arc;

use async_trait::async_trait;
use cdk::mint::{Mint, MintServer};
use cdk::nuts::{
    BatchCheckMintQuoteRequest, BatchMintRequest, CheckStateRequest, CheckStateResponse, Id,
    KeySet, KeysetResponse, MeltRequest, MintInfo, MintRequest, MintResponse, PaymentMethod,
    RestoreRequest, RestoreResponse, SwapRequest, SwapResponse,
};
use cdk::rpc::{dispatch, MintRpcRequest, MintRpcResponse};
use cdk::wallet::{AuthWallet, LnurlPayInvoiceResponse, LnurlPayResponse, MintConnector};
use cdk::Error;
use cdk_common::stream_channel::{StreamRx, StreamTx};
use cdk_common::{
    MeltQuoteCreateResponse, MeltQuoteRequest, MeltQuoteResponse, MintQuoteRequest,
    MintQuoteResponse,
};
use cdk_integration_tests::mint_connector_test;
use tokio::sync::RwLock;

/// A connector that talks to the mint through the `MintRpcRequest`/`dispatch`
/// envelope, in-process (no bytes on a wire).
struct RpcConnector {
    mint: Mint,
    auth_wallet: Arc<RwLock<Option<AuthWallet>>>,
}

impl Debug for RpcConnector {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "RpcConnector")
    }
}

impl RpcConnector {
    fn new(mint: Mint) -> Self {
        Self {
            mint,
            auth_wallet: Arc::new(RwLock::new(None)),
        }
    }

    async fn call<T: serde::de::DeserializeOwned>(
        &self,
        request: MintRpcRequest,
    ) -> Result<T, Error> {
        // Round-trip through JSON both ways so the test exercises the real wire
        // encoding: the request enum's method/params tagging and the ok/err
        // response envelope, not just the inner payload.
        let bytes = serde_json::to_vec(&request).expect("encode request");
        let request: MintRpcRequest = serde_json::from_slice(&bytes).expect("decode request");
        // Gate the call the way a real transport must: `dispatch` performs no
        // auth, so we verify it here before dispatching, mirroring what the
        // cdk-axum routes do. Against the suite's no-auth mint this is a
        // pass-through, but it keeps this connector a faithful transport
        // reference rather than an auth bypass.
        if let Some(endpoint) = request.protected_endpoint() {
            let token = match self.get_auth_wallet().await {
                Some(wallet) => wallet.get_auth_for_request(&endpoint).await.ok().flatten(),
                None => None,
            };
            self.mint.verify_auth(token, &endpoint).await?;
        }
        let response = dispatch(&self.mint, request).await;
        let bytes = serde_json::to_vec(&response).expect("encode response");
        let response: MintRpcResponse = serde_json::from_slice(&bytes).expect("decode response");
        response.decode()
    }
}

#[async_trait]
impl MintConnector for RpcConnector {
    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, Error> {
        Err(Error::Custom("unsupported".to_string()))
    }

    async fn fetch_lnurl_pay_request(&self, _url: &str) -> Result<LnurlPayResponse, Error> {
        Err(Error::Custom("unsupported".to_string()))
    }

    async fn fetch_lnurl_invoice(&self, _url: &str) -> Result<LnurlPayInvoiceResponse, Error> {
        Err(Error::Custom("unsupported".to_string()))
    }

    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error> {
        self.call(MintRpcRequest::GetMintKeys).await
    }

    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
        self.call(MintRpcRequest::GetMintKeyset { keyset_id }).await
    }

    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error> {
        self.call(MintRpcRequest::GetMintKeysets).await
    }

    async fn post_mint_quote(
        &self,
        request: MintQuoteRequest,
    ) -> Result<MintQuoteResponse<String>, Error> {
        self.call(MintRpcRequest::PostMintQuote(request)).await
    }

    async fn get_mint_quote_status(
        &self,
        method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MintQuoteResponse<String>, Error> {
        self.call(MintRpcRequest::GetMintQuoteStatus {
            method,
            quote_id: quote_id.to_string(),
        })
        .await
    }

    async fn post_mint(
        &self,
        method: &PaymentMethod,
        request: MintRequest<String>,
    ) -> Result<MintResponse, Error> {
        self.call(MintRpcRequest::PostMint {
            method: method.clone(),
            request,
        })
        .await
    }

    async fn post_batch_check_mint_quote_status(
        &self,
        method: &PaymentMethod,
        request: BatchCheckMintQuoteRequest<String>,
    ) -> Result<Vec<MintQuoteResponse<String>>, Error> {
        self.call(MintRpcRequest::PostBatchCheckMintQuoteStatus {
            method: method.clone(),
            request,
        })
        .await
    }

    async fn post_batch_mint(
        &self,
        method: &PaymentMethod,
        request: BatchMintRequest<String>,
    ) -> Result<MintResponse, Error> {
        self.call(MintRpcRequest::PostBatchMint {
            method: method.clone(),
            request,
        })
        .await
    }

    async fn post_melt_quote(
        &self,
        request: MeltQuoteRequest,
    ) -> Result<MeltQuoteCreateResponse<String>, Error> {
        self.call(MintRpcRequest::PostMeltQuote(request)).await
    }

    async fn get_melt_quote_status(
        &self,
        method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MeltQuoteResponse<String>, Error> {
        self.call(MintRpcRequest::GetMeltQuoteStatus {
            method,
            quote_id: quote_id.to_string(),
        })
        .await
    }

    async fn post_melt(
        &self,
        method: &PaymentMethod,
        request: MeltRequest<String>,
    ) -> Result<MeltQuoteResponse<String>, Error> {
        self.call(MintRpcRequest::PostMelt {
            method: method.clone(),
            request,
        })
        .await
    }

    async fn post_swap(&self, request: SwapRequest) -> Result<SwapResponse, Error> {
        self.call(MintRpcRequest::PostSwap(request)).await
    }

    async fn get_mint_info(&self) -> Result<MintInfo, Error> {
        self.call(MintRpcRequest::GetMintInfo).await
    }

    async fn post_check_state(
        &self,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        self.call(MintRpcRequest::PostCheckState(request)).await
    }

    async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        self.call(MintRpcRequest::PostRestore(request)).await
    }

    async fn open_stream(&self) -> Result<(StreamTx, StreamRx), Error> {
        // A real transport would bridge its own bidi stream to
        // `mint.serve_stream`; in-process we reuse the mint's own duplex.
        self.mint.open_stream().await
    }

    async fn get_auth_wallet(&self) -> Option<AuthWallet> {
        self.auth_wallet.read().await.clone()
    }

    async fn set_auth_wallet(&self, wallet: Option<AuthWallet>) {
        *self.auth_wallet.write().await = wallet;
    }
}

async fn make_rpc(_test_name: &str) -> RpcConnector {
    RpcConnector::new(mint_connector_test::build_test_mint().await)
}

mint_connector_test!(make_rpc);
