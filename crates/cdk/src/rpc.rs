//! Transport-neutral RPC envelope for the mint's unary API.
//!
//! [`MintRpcRequest`] is a tagged enum with one variant per [`MintServer`] RPC.
//! A non-HTTP transport (Iroh, an attested enclave channel, gRPC) carries these
//! as JSON: the client builds a variant, the server decodes it and calls
//! [`dispatch`], and the reply is a [`MintRpcResponse`]. This is the
//! transport-agnostic analog of the `cdk-axum` routes; the `method` tag replaces
//! the HTTP path, and [`MintRpcRequest::protected_endpoint`] replaces the
//! per-route `verify_auth` call.
//!
//! [`MintServer`]: crate::mint::MintServer

use cdk_common::{MeltQuoteRequest, MeltRequest, MintQuoteRequest};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{Error, ErrorResponse};
use crate::nuts::nut21::{Method, ProtectedEndpoint, RoutePath};
use crate::nuts::{
    BatchCheckMintQuoteRequest, BatchMintRequest, CheckStateRequest, Id, MintRequest,
    PaymentMethod, RestoreRequest, SwapRequest,
};

/// A single unary mint RPC and its parameters, tagged by method name.
///
/// Serializes as `{"method": "<name>", "params": <params>}` (params omitted for
/// the no-argument reads).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum MintRpcRequest {
    /// [NUT-01] `get_mint_keys`
    GetMintKeys,
    /// [NUT-01] `get_mint_keyset`
    GetMintKeyset {
        /// Keyset id
        keyset_id: Id,
    },
    /// [NUT-02] `get_mint_keysets`
    GetMintKeysets,
    /// [NUT-04/23/25] `post_mint_quote`
    PostMintQuote(MintQuoteRequest),
    /// `get_mint_quote_status`
    GetMintQuoteStatus {
        /// Payment method
        method: PaymentMethod,
        /// Quote id
        quote_id: String,
    },
    /// [NUT-04] `post_mint`
    PostMint {
        /// Payment method
        method: PaymentMethod,
        /// Mint request
        request: MintRequest<String>,
    },
    /// [NUT-29] `post_batch_check_mint_quote_status`
    PostBatchCheckMintQuoteStatus {
        /// Payment method
        method: PaymentMethod,
        /// Batch request
        request: BatchCheckMintQuoteRequest<String>,
    },
    /// [NUT-29] `post_batch_mint`
    PostBatchMint {
        /// Payment method
        method: PaymentMethod,
        /// Batch request
        request: BatchMintRequest<String>,
    },
    /// [NUT-05] `post_melt_quote`
    PostMeltQuote(MeltQuoteRequest),
    /// `get_melt_quote_status`
    GetMeltQuoteStatus {
        /// Payment method
        method: PaymentMethod,
        /// Quote id
        quote_id: String,
    },
    /// [NUT-05/08] `post_melt`
    PostMelt {
        /// Payment method
        method: PaymentMethod,
        /// Melt request
        request: MeltRequest<String>,
    },
    /// [NUT-03/06] `post_swap`
    PostSwap(SwapRequest),
    /// [NUT-06] `get_mint_info`
    GetMintInfo,
    /// [NUT-07] `post_check_state`
    PostCheckState(CheckStateRequest),
    /// [NUT-13] `post_restore`
    PostRestore(RestoreRequest),
}

impl MintRpcRequest {
    /// The auth endpoint a transport must gate this call with (`None` for the
    /// public reads: keys, keysets, and info). A transport calls
    /// `mint.verify_auth(token, &endpoint)` before [`dispatch`].
    pub fn protected_endpoint(&self) -> Option<ProtectedEndpoint> {
        use MintRpcRequest::*;
        let (method, path) = match self {
            GetMintKeys | GetMintKeyset { .. } | GetMintKeysets | GetMintInfo => return None,
            PostSwap(_) => (Method::Post, RoutePath::Swap),
            PostCheckState(_) => (Method::Post, RoutePath::Checkstate),
            PostRestore(_) => (Method::Post, RoutePath::Restore),
            PostMintQuote(r) => (Method::Post, RoutePath::MintQuote(r.method().to_string())),
            GetMintQuoteStatus { method, .. } => {
                (Method::Get, RoutePath::MintQuote(method.to_string()))
            }
            PostMint { method, .. } => (Method::Post, RoutePath::Mint(method.to_string())),
            PostBatchCheckMintQuoteStatus { method, .. } => {
                (Method::Post, RoutePath::MintQuote(method.to_string()))
            }
            PostBatchMint { method, .. } => (Method::Post, RoutePath::Mint(method.to_string())),
            PostMeltQuote(r) => (Method::Post, RoutePath::MeltQuote(r.method().to_string())),
            GetMeltQuoteStatus { method, .. } => {
                (Method::Get, RoutePath::MeltQuote(method.to_string()))
            }
            PostMelt { method, .. } => (Method::Post, RoutePath::Melt(method.to_string())),
        };
        Some(ProtectedEndpoint::new(method, path))
    }
}

/// The reply to a [`MintRpcRequest`]: the method's response as JSON, or a
/// NUT-00 error. Serializes as `{"ok": <value>}` or `{"err": <ErrorResponse>}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MintRpcResponse {
    /// Successful response payload.
    Ok(serde_json::Value),
    /// Error response.
    Err(ErrorResponse),
}

impl MintRpcResponse {
    /// Decode the response into the concrete type the caller's method expects,
    /// or map the error back to [`Error`].
    pub fn decode<T: DeserializeOwned>(self) -> Result<T, Error> {
        match self {
            MintRpcResponse::Ok(value) => {
                serde_json::from_value(value).map_err(|e| Error::Custom(format!("decode: {e}")))
            }
            MintRpcResponse::Err(err) => Err(err.into()),
        }
    }
}

/// Serialize a `MintServer` result into a [`MintRpcResponse`].
#[cfg(feature = "mint")]
fn wrap<T: Serialize>(result: Result<T, Error>) -> MintRpcResponse {
    match result {
        Ok(value) => match serde_json::to_value(value) {
            Ok(value) => MintRpcResponse::Ok(value),
            Err(e) => MintRpcResponse::Err(Error::Custom(format!("encode: {e}")).into()),
        },
        Err(e) => MintRpcResponse::Err(e.into()),
    }
}

/// Dispatch a decoded request to the matching [`MintServer`](crate::mint::MintServer)
/// method.
///
/// Performs no authentication: a transport gates the call with
/// [`MintRpcRequest::protected_endpoint`] and `Mint::verify_auth` first.
#[cfg(feature = "mint")]
pub async fn dispatch<S>(server: &S, request: MintRpcRequest) -> MintRpcResponse
where
    S: crate::mint::MintServer + ?Sized,
{
    use MintRpcRequest::*;
    match request {
        GetMintKeys => wrap(server.get_mint_keys().await),
        GetMintKeyset { keyset_id } => wrap(server.get_mint_keyset(keyset_id).await),
        GetMintKeysets => wrap(server.get_mint_keysets().await),
        PostMintQuote(request) => wrap(server.post_mint_quote(request).await),
        GetMintQuoteStatus { method, quote_id } => {
            wrap(server.get_mint_quote_status(method, &quote_id).await)
        }
        PostMint { method, request } => wrap(server.post_mint(&method, request).await),
        PostBatchCheckMintQuoteStatus { method, request } => wrap(
            server
                .post_batch_check_mint_quote_status(&method, request)
                .await,
        ),
        PostBatchMint { method, request } => wrap(server.post_batch_mint(&method, request).await),
        PostMeltQuote(request) => wrap(server.post_melt_quote(request).await),
        GetMeltQuoteStatus { method, quote_id } => {
            wrap(server.get_melt_quote_status(method, &quote_id).await)
        }
        PostMelt { method, request } => wrap(server.post_melt(&method, request).await),
        PostSwap(request) => wrap(server.post_swap(request).await),
        GetMintInfo => wrap(server.get_mint_info().await),
        PostCheckState(request) => wrap(server.post_check_state(request).await),
        PostRestore(request) => wrap(server.post_restore(request).await),
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use cdk_common::{MintQuoteBolt11Request, MintQuoteRequest};

    use super::*;
    use crate::nuts::CurrencyUnit;
    use crate::Amount;

    fn mint_req() -> MintRequest<String> {
        MintRequest {
            quote: String::new(),
            outputs: vec![],
            signature: None,
        }
    }

    /// The auth-gating contract every transport relies on: each RPC must map to
    /// the same `(Method, RoutePath)` the HTTP router protects, and the public
    /// reads must stay unprotected. A wrong entry here is a silent auth bypass,
    /// so pin the whole table.
    #[test]
    fn protected_endpoint_mapping() {
        use MintRpcRequest::*;

        let m = PaymentMethod::BOLT11.to_string();
        let ep = |method, path| Some(ProtectedEndpoint::new(method, path));

        // Public reads: no auth.
        assert_eq!(GetMintKeys.protected_endpoint(), None);
        assert_eq!(GetMintKeysets.protected_endpoint(), None);
        assert_eq!(GetMintInfo.protected_endpoint(), None);
        assert_eq!(
            GetMintKeyset {
                keyset_id: Id::from_str("00deadbeef123456").unwrap(),
            }
            .protected_endpoint(),
            None
        );

        // Protected writes and status reads, keyed by method where relevant.
        assert_eq!(
            PostSwap(SwapRequest::new(vec![], vec![])).protected_endpoint(),
            ep(Method::Post, RoutePath::Swap)
        );
        assert_eq!(
            PostCheckState(CheckStateRequest { ys: vec![] }).protected_endpoint(),
            ep(Method::Post, RoutePath::Checkstate)
        );
        assert_eq!(
            PostRestore(RestoreRequest { outputs: vec![] }).protected_endpoint(),
            ep(Method::Post, RoutePath::Restore)
        );

        let mint_quote = MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(1),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        });
        assert_eq!(
            PostMintQuote(mint_quote).protected_endpoint(),
            ep(Method::Post, RoutePath::MintQuote(m.clone()))
        );
        assert_eq!(
            GetMintQuoteStatus {
                method: PaymentMethod::BOLT11,
                quote_id: String::new(),
            }
            .protected_endpoint(),
            ep(Method::Get, RoutePath::MintQuote(m.clone()))
        );
        assert_eq!(
            PostMint {
                method: PaymentMethod::BOLT11,
                request: mint_req(),
            }
            .protected_endpoint(),
            ep(Method::Post, RoutePath::Mint(m.clone()))
        );
        assert_eq!(
            PostBatchCheckMintQuoteStatus {
                method: PaymentMethod::BOLT11,
                request: BatchCheckMintQuoteRequest { quotes: vec![] },
            }
            .protected_endpoint(),
            ep(Method::Post, RoutePath::MintQuote(m.clone()))
        );
        assert_eq!(
            PostBatchMint {
                method: PaymentMethod::BOLT11,
                request: BatchMintRequest {
                    quotes: vec![],
                    quote_amounts: None,
                    outputs: vec![],
                    signatures: None,
                },
            }
            .protected_endpoint(),
            ep(Method::Post, RoutePath::Mint(m.clone()))
        );

        // `PostMeltQuote` reads its method from the payload the same way
        // `PostMintQuote` does; the melt route paths below cover the mapping.
        assert_eq!(
            GetMeltQuoteStatus {
                method: PaymentMethod::BOLT11,
                quote_id: String::new(),
            }
            .protected_endpoint(),
            ep(Method::Get, RoutePath::MeltQuote(m.clone()))
        );
        assert_eq!(
            PostMelt {
                method: PaymentMethod::BOLT11,
                request: MeltRequest::new(String::new(), vec![], None),
            }
            .protected_endpoint(),
            ep(Method::Post, RoutePath::Melt(m))
        );
    }
}
