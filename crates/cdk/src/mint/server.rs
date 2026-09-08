//! Server-side RPC interface for a mint.
//!
//! [`MintServer`] is the server-side counterpart of the wallet's
//! [`MintConnector`](crate::wallet::MintConnector): it exposes the same RPC
//! surface but is implemented by whatever serves requests (here, [`Mint`]).
//! It speaks the wire types (`String` quote IDs), so it is the single home for
//! the `String` <-> [`QuoteId`] conversions that both the HTTP server and an
//! in-process connector rely on.

use std::slice;
use std::str::FromStr;

use async_trait::async_trait;
use cdk_common::nut00::KnownMethod;
use cdk_common::stream_channel::{StreamRx, StreamTx};
use cdk_common::task::spawn;
use cdk_common::{
    MeltQuoteCreateResponse, MeltQuoteRequest, MeltQuoteResponse, MintQuoteRequest,
    MintQuoteResponse,
};

use super::{Mint, MintInput, QuoteId};
use crate::error::Error;
use crate::nuts::{
    BatchCheckMintQuoteRequest, BatchMintRequest, CheckStateRequest, CheckStateResponse, Id,
    KeySet, KeysetResponse, MeltRequest, MintInfo, MintRequest, MintResponse, PaymentMethod,
    RestoreRequest, RestoreResponse, SwapRequest, SwapResponse,
};
use crate::util::unix_time;

/// Server-side equivalent of [`MintConnector`](crate::wallet::MintConnector).
///
/// Every method maps one-to-one to a mint RPC and speaks the wire types, so an
/// implementor performs any internal representation conversions (notably
/// `String` <-> [`QuoteId`]) itself.
///
/// A `method` argument names the route the caller is speaking on. It is
/// caller-supplied, so an implementor must reject a request whose quotes were
/// not created for that method before touching any state: auth is scoped per
/// method, and a mismatch would otherwise let a token for one method drive
/// another method's quotes.
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
    ) -> Result<MintQuoteResponse<String>, Error>;
    /// Mint quote status
    async fn get_mint_quote_status(
        &self,
        method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MintQuoteResponse<String>, Error>;
    /// Mint tokens [NUT-04]
    async fn post_mint(
        &self,
        method: &PaymentMethod,
        request: MintRequest<String>,
    ) -> Result<MintResponse, Error>;
    /// Batch check mint quote status [NUT-29]
    async fn post_batch_check_mint_quote_status(
        &self,
        method: &PaymentMethod,
        request: BatchCheckMintQuoteRequest<String>,
    ) -> Result<Vec<MintQuoteResponse<String>>, Error>;
    /// Batch mint tokens [NUT-29]
    async fn post_batch_mint(
        &self,
        method: &PaymentMethod,
        request: BatchMintRequest<String>,
    ) -> Result<MintResponse, Error>;
    /// Melt quote [NUT-05]
    async fn post_melt_quote(
        &self,
        request: MeltQuoteRequest,
    ) -> Result<MeltQuoteCreateResponse<String>, Error>;
    /// Melt quote status
    async fn get_melt_quote_status(
        &self,
        method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MeltQuoteResponse<String>, Error>;
    /// Melt [NUT-05, NUT-08]
    async fn post_melt(
        &self,
        method: &PaymentMethod,
        request: MeltRequest<String>,
    ) -> Result<MeltQuoteResponse<String>, Error>;
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

    /// Open a raw bidirectional stream to a client.
    ///
    /// The stream carries opaque `String` messages; the content (a NUT-17
    /// subscription session or anything else) is layered on top and is not the
    /// server's concern. The returned halves are the server's end of a duplex
    /// whose peer a transport connects to a client. Implementations without
    /// streaming support should return [`Error::StreamingNotSupported`].
    async fn open_stream(&self) -> Result<(StreamTx, StreamRx), Error>;
}

/// Implements the generic [`MintServer`] (wire types, `String` quote IDs) for a
/// [`Mint`] that uses [`QuoteId`]s internally, converting requests and
/// responses between the `String` and [`QuoteId`] variants as necessary.
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
    ) -> Result<MintQuoteResponse<String>, Error> {
        match request {
            MintQuoteRequest::Bolt11(req) => match self.get_mint_quote(req.into()).await? {
                MintQuoteResponse::Bolt11(r) => Ok(MintQuoteResponse::Bolt11(r.to_string_id())),
                _ => Err(Error::InvalidPaymentMethod),
            },
            MintQuoteRequest::Bolt12(req) => match self.get_mint_quote(req.into()).await? {
                MintQuoteResponse::Bolt12(r) => Ok(MintQuoteResponse::Bolt12(r.to_string_id())),
                _ => Err(Error::InvalidPaymentMethod),
            },
            MintQuoteRequest::Onchain(req) => match self.get_mint_quote(req.into()).await? {
                MintQuoteResponse::Onchain(r) => Ok(MintQuoteResponse::Onchain(r.to_string_id())),
                _ => Err(Error::InvalidPaymentMethod),
            },
            MintQuoteRequest::Custom { method, request } => {
                match self
                    .get_mint_quote(super::MintQuoteRequest::Custom { method, request })
                    .await?
                {
                    MintQuoteResponse::Custom { method, response } => {
                        Ok(MintQuoteResponse::Custom {
                            method,
                            response: response.to_string_id(),
                        })
                    }
                    _ => Err(Error::InvalidPaymentMethod),
                }
            }
        }
    }

    async fn get_mint_quote_status(
        &self,
        method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MintQuoteResponse<String>, Error> {
        let quote_ids = [QuoteId::from_str(quote_id)?];
        ensure_mint_quote_methods(self, &method, &quote_ids).await?;

        let response = self
            .check_mint_quotes(&quote_ids)
            .await?
            .first()
            .ok_or(Error::UnknownQuote)?
            .clone();

        match method {
            PaymentMethod::Known(KnownMethod::Bolt11) => match response {
                MintQuoteResponse::Bolt11(r) => Ok(MintQuoteResponse::Bolt11(r.to_string_id())),
                _ => Err(Error::InvalidPaymentMethod),
            },
            PaymentMethod::Known(KnownMethod::Bolt12) => match response {
                MintQuoteResponse::Bolt12(r) => Ok(MintQuoteResponse::Bolt12(r.to_string_id())),
                _ => Err(Error::InvalidPaymentMethod),
            },
            PaymentMethod::Known(KnownMethod::Onchain) => match response {
                MintQuoteResponse::Onchain(r) => Ok(MintQuoteResponse::Onchain(r.to_string_id())),
                _ => Err(Error::InvalidPaymentMethod),
            },
            PaymentMethod::Custom(_) => match response {
                MintQuoteResponse::Custom { method, response } => Ok(MintQuoteResponse::Custom {
                    method,
                    response: response.to_string_id(),
                }),
                _ => Err(Error::InvalidPaymentMethod),
            },
        }
    }

    async fn post_mint(
        &self,
        method: &PaymentMethod,
        request: MintRequest<String>,
    ) -> Result<MintResponse, Error> {
        let request_id: MintRequest<QuoteId> = request.try_into()?;
        ensure_mint_quote_methods(self, method, slice::from_ref(&request_id.quote)).await?;

        self.process_mint_request(MintInput::Single(request_id))
            .await
    }

    async fn post_batch_check_mint_quote_status(
        &self,
        method: &PaymentMethod,
        request: BatchCheckMintQuoteRequest<String>,
    ) -> Result<Vec<MintQuoteResponse<String>>, Error> {
        let quote_ids = request
            .quotes
            .iter()
            .map(|s| QuoteId::from_str(s))
            .collect::<Result<Vec<_>, _>>()?;
        ensure_mint_quote_methods(self, method, &quote_ids).await?;

        self.check_mint_quotes(&quote_ids)
            .await
            .map(|responses| responses.into_iter().map(Into::into).collect())
    }

    async fn post_batch_mint(
        &self,
        method: &PaymentMethod,
        request: BatchMintRequest<String>,
    ) -> Result<MintResponse, Error> {
        let quotes = request
            .quotes
            .iter()
            .map(|s| QuoteId::from_str(s))
            .collect::<Result<Vec<_>, _>>()?;
        ensure_mint_quote_methods(self, method, &quotes).await?;

        let request_id = BatchMintRequest {
            quotes,
            quote_amounts: request.quote_amounts,
            outputs: request.outputs,
            signatures: request.signatures,
        };

        self.process_mint_request(MintInput::Batch(request_id))
            .await
    }

    async fn post_melt_quote(
        &self,
        request: MeltQuoteRequest,
    ) -> Result<MeltQuoteCreateResponse<String>, Error> {
        match request {
            MeltQuoteRequest::Bolt11(req) => match self.get_melt_quote(req.into()).await? {
                MeltQuoteCreateResponse::Bolt11(r) => {
                    Ok(MeltQuoteCreateResponse::Bolt11(r.to_string_id()))
                }
                _ => Err(Error::InvalidPaymentMethod),
            },
            MeltQuoteRequest::Bolt12(req) => match self.get_melt_quote(req.into()).await? {
                MeltQuoteCreateResponse::Bolt12(r) => {
                    Ok(MeltQuoteCreateResponse::Bolt12(r.to_string_id()))
                }
                _ => Err(Error::InvalidPaymentMethod),
            },
            MeltQuoteRequest::Custom(req) => match self.get_melt_quote(req.into()).await? {
                MeltQuoteCreateResponse::Custom((method, r)) => {
                    Ok(MeltQuoteCreateResponse::Custom((method, r.to_string_id())))
                }
                _ => Err(Error::InvalidPaymentMethod),
            },
            MeltQuoteRequest::Onchain(req) => match self.get_melt_quote(req.into()).await? {
                MeltQuoteCreateResponse::Onchain(r) => {
                    Ok(MeltQuoteCreateResponse::Onchain(r.into()))
                }
                _ => Err(Error::InvalidPaymentMethod),
            },
        }
    }

    async fn get_melt_quote_status(
        &self,
        method: PaymentMethod,
        quote_id: &str,
    ) -> Result<MeltQuoteResponse<String>, Error> {
        let quote_id = QuoteId::from_str(quote_id)?;
        ensure_melt_quote_method(self, &method, &quote_id).await?;

        let response = self.check_melt_quote(&quote_id).await?;
        convert_melt_quote_response(method, response)
    }

    async fn post_melt(
        &self,
        method: &PaymentMethod,
        request: MeltRequest<String>,
    ) -> Result<MeltQuoteResponse<String>, Error> {
        let request_uuid: MeltRequest<QuoteId> = request.try_into()?;
        ensure_melt_quote_method(self, method, request_uuid.quote()).await?;

        let response = self.melt(&request_uuid).await?.await?;
        convert_melt_quote_response(method.clone(), response)
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

    async fn open_stream(&self) -> Result<(StreamTx, StreamRx), Error> {
        let (client_end, (mint_tx, mint_rx)) = cdk_common::stream_channel::in_memory_pair();
        let mint = self.clone();
        // The spawned runner relies on the client draining its `StreamRx`:
        // unlike the axum bridge there is no `STALL_TIMEOUT` here, so a
        // non-draining in-process consumer would block `serve_stream` on the
        // bounded channel until the client end is dropped.
        spawn(async move { mint.serve_stream(mint_tx, mint_rx).await });
        Ok(client_end)
    }
}

/// Rejects a request whose quotes were not created for the route's payment
/// method, so a caller cannot drive a quote through another method's endpoint.
async fn ensure_mint_quote_methods(
    mint: &Mint,
    method: &PaymentMethod,
    quote_ids: &[QuoteId],
) -> Result<(), Error> {
    for quote_id in quote_ids {
        if &mint.get_mint_quote_method(quote_id).await? != method {
            return Err(Error::InvalidPaymentMethod);
        }
    }

    Ok(())
}

/// Melt counterpart of [`ensure_mint_quote_methods`]; must run before the melt
/// executes, since a mismatch caught afterwards would report an error for a
/// payment that already settled.
async fn ensure_melt_quote_method(
    mint: &Mint,
    method: &PaymentMethod,
    quote_id: &QuoteId,
) -> Result<(), Error> {
    if &mint.get_melt_quote_method(quote_id).await? != method {
        return Err(Error::InvalidPaymentMethod);
    }

    Ok(())
}

/// Shared `QuoteId` -> `String` conversion for the melt-quote status and melt
/// responses, which dispatch identically on the payment method.
fn convert_melt_quote_response(
    method: PaymentMethod,
    response: MeltQuoteResponse<QuoteId>,
) -> Result<MeltQuoteResponse<String>, Error> {
    match method {
        PaymentMethod::Known(KnownMethod::Bolt11) => match response {
            MeltQuoteResponse::Bolt11(r) => Ok(MeltQuoteResponse::Bolt11(r.to_string_id())),
            _ => Err(Error::InvalidPaymentMethod),
        },
        PaymentMethod::Known(KnownMethod::Bolt12) => match response {
            MeltQuoteResponse::Bolt12(r) => Ok(MeltQuoteResponse::Bolt12(r.to_string_id())),
            _ => Err(Error::InvalidPaymentMethod),
        },
        PaymentMethod::Known(KnownMethod::Onchain) => match response {
            MeltQuoteResponse::Onchain(r) => Ok(MeltQuoteResponse::Onchain(r.into())),
            _ => Err(Error::InvalidPaymentMethod),
        },
        PaymentMethod::Custom(_) => match response {
            MeltQuoteResponse::Custom((quote_method, r)) => {
                Ok(MeltQuoteResponse::Custom((quote_method, r.to_string_id())))
            }
            _ => Err(Error::InvalidPaymentMethod),
        },
    }
}
