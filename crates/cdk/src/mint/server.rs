//! Server-side RPC interface for a mint.
//!
//! [`MintServer`] is the server-side counterpart of the wallet's
//! [`MintConnector`](crate::wallet::MintConnector): it exposes the same RPC
//! surface but is implemented by whatever serves requests (here, [`Mint`]).
//! It speaks [`QuoteId`], which serializes as the plain string the wire
//! carries, so both transports share one interface and only a client holding
//! an id it has not parsed yet needs the `String` spelling.
//!
//! cdk-axum routes every request through it and keeps what is HTTP: parsing a
//! body whose type the path's method selects, auth, the NUT-19 cache, and
//! answering with each variant's payload rather than the externally tagged
//! enum. The `String` conversions live in `DirectMintConnection`, the
//! in-process [`MintConnector`](crate::wallet::MintConnector) adapter, because
//! that is the one place the representation changes.

use std::slice;

use async_trait::async_trait;
use cdk_common::{
    MeltQuoteCreateResponse, MeltQuoteRequest, MeltQuoteResponse, MintQuoteRequest,
    MintQuoteResponse,
};

use super::{Mint, MintInput, PendingMelt, QuoteId};
use crate::ensure_cdk;
use crate::error::Error;
use crate::nuts::{
    BatchCheckMintQuoteRequest, BatchMintRequest, CheckStateRequest, CheckStateResponse, Id,
    KeySet, KeysetResponse, MeltRequest, MintInfo, MintRequest, MintResponse, PaymentMethod,
    RestoreRequest, RestoreResponse, SwapRequest, SwapResponse,
};
use crate::util::unix_time;

/// Server-side equivalent of [`MintConnector`](crate::wallet::MintConnector).
///
/// Every method maps one-to-one to a mint RPC.
///
/// A `method` argument names the route the caller is speaking on. It is
/// caller-supplied, so an implementor must reject a request whose quotes were
/// not created for that method before touching any state: auth is scoped per
/// method, and a mismatch would otherwise let a token for one method drive
/// another method's quotes. Only the calls that reach an existing quote take
/// it, plus [`MintServer::post_melt_quote`], whose request carries a method of
/// its own; a mint-quote request takes its method from the route and so has
/// nothing to disagree with.
#[async_trait]
pub trait MintServer: Send + Sync {
    /// Active mint keys [NUT-01]
    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error>;
    /// Keyset keys for a specific keyset [NUT-01]
    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error>;
    /// Keysets [NUT-02]
    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error>;
    /// Mint quote [NUT-04, NUT-23, NUT-25]
    async fn post_mint_quote(
        &self,
        request: MintQuoteRequest,
    ) -> Result<MintQuoteResponse<QuoteId>, Error>;
    /// Mint quote status
    async fn get_mint_quote_status(
        &self,
        method: &PaymentMethod,
        quote_id: &QuoteId,
    ) -> Result<MintQuoteResponse<QuoteId>, Error>;
    /// Mint tokens [NUT-04]
    async fn post_mint(
        &self,
        method: &PaymentMethod,
        request: MintRequest<QuoteId>,
    ) -> Result<MintResponse, Error>;
    /// Batch check mint quote status [NUT-29]
    async fn post_batch_check_mint_quote_status(
        &self,
        method: &PaymentMethod,
        request: BatchCheckMintQuoteRequest<QuoteId>,
    ) -> Result<Vec<MintQuoteResponse<QuoteId>>, Error>;
    /// Batch mint tokens [NUT-29]
    async fn post_batch_mint(
        &self,
        method: &PaymentMethod,
        request: BatchMintRequest<QuoteId>,
    ) -> Result<MintResponse, Error>;
    /// Melt quote [NUT-05]
    async fn post_melt_quote(
        &self,
        method: &PaymentMethod,
        request: MeltQuoteRequest,
    ) -> Result<MeltQuoteCreateResponse<QuoteId>, Error>;
    /// Melt quote status
    async fn get_melt_quote_status(
        &self,
        method: &PaymentMethod,
        quote_id: &QuoteId,
    ) -> Result<MeltQuoteResponse<QuoteId>, Error>;
    /// Melt [NUT-05, NUT-08]
    ///
    /// Answers before the payment settles, so the caller decides whether to
    /// return the pending response (the NUT-05 async flow, which the request's
    /// `prefer_async` flag asks for and
    /// [`PendingMelt::requires_async_response`] demands) or to await the
    /// handle.
    async fn post_melt(
        &self,
        method: &PaymentMethod,
        request: MeltRequest<QuoteId>,
    ) -> Result<PendingMelt, Error>;
    /// Swap [NUT-03, NUT-06]
    async fn post_swap(&self, request: SwapRequest) -> Result<SwapResponse, Error>;
    /// Mint info [NUT-06]
    async fn get_mint_info(&self) -> Result<MintInfo, Error>;
    /// Spendable check [NUT-07]
    async fn post_check_state(
        &self,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error>;
    /// Restore [NUT-13]
    async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error>;
}

/// Implements [`MintServer`] for a [`Mint`], which already works in
/// [`QuoteId`], so each method adds only the scoping the caller-supplied
/// `method` calls for.
#[async_trait]
impl MintServer for Mint {
    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error> {
        Ok(self.pubkeys().keysets)
    }

    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
        self.keyset(&keyset_id).ok_or(Error::UnknownKeySet)
    }

    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error> {
        Ok(self.keysets())
    }

    async fn post_mint_quote(
        &self,
        request: MintQuoteRequest,
    ) -> Result<MintQuoteResponse<QuoteId>, Error> {
        self.get_mint_quote(request).await
    }

    async fn get_mint_quote_status(
        &self,
        method: &PaymentMethod,
        quote_id: &QuoteId,
    ) -> Result<MintQuoteResponse<QuoteId>, Error> {
        self.check_mint_quotes(slice::from_ref(quote_id), Some(method))
            .await?
            .into_iter()
            .next()
            .ok_or(Error::UnknownQuote)
    }

    async fn post_mint(
        &self,
        method: &PaymentMethod,
        request: MintRequest<QuoteId>,
    ) -> Result<MintResponse, Error> {
        self.ensure_mint_quote_methods(method.clone(), slice::from_ref(&request.quote))
            .await?;

        self.process_mint_request(MintInput::Single(request)).await
    }

    async fn post_batch_check_mint_quote_status(
        &self,
        method: &PaymentMethod,
        request: BatchCheckMintQuoteRequest<QuoteId>,
    ) -> Result<Vec<MintQuoteResponse<QuoteId>>, Error> {
        self.validate_batch_quote_ids(&request.quotes).await?;

        self.check_mint_quotes(&request.quotes, Some(method)).await
    }

    async fn post_batch_mint(
        &self,
        method: &PaymentMethod,
        request: BatchMintRequest<QuoteId>,
    ) -> Result<MintResponse, Error> {
        self.validate_batch_quote_ids(&request.quotes).await?;
        self.ensure_mint_quote_methods(method.clone(), &request.quotes)
            .await?;

        self.process_mint_request(MintInput::Batch(request)).await
    }

    async fn post_melt_quote(
        &self,
        method: &PaymentMethod,
        request: MeltQuoteRequest,
    ) -> Result<MeltQuoteCreateResponse<QuoteId>, Error> {
        ensure_cdk!(&request.method() == method, Error::InvalidPaymentMethod);

        self.get_melt_quote(request).await
    }

    async fn get_melt_quote_status(
        &self,
        method: &PaymentMethod,
        quote_id: &QuoteId,
    ) -> Result<MeltQuoteResponse<QuoteId>, Error> {
        self.check_melt_quote(quote_id, Some(method)).await
    }

    async fn post_melt(
        &self,
        method: &PaymentMethod,
        request: MeltRequest<QuoteId>,
    ) -> Result<PendingMelt, Error> {
        self.ensure_melt_quote_method(method.clone(), request.quote())
            .await?;

        self.melt(&request).await
    }

    async fn post_swap(&self, request: SwapRequest) -> Result<SwapResponse, Error> {
        self.process_swap_request(request).await
    }

    async fn get_mint_info(&self) -> Result<MintInfo, Error> {
        Ok(self.mint_info().await?.time(unix_time()))
    }

    async fn post_check_state(
        &self,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        self.check_state(&request).await
    }

    async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        self.restore(request).await
    }
}
