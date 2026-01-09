//! Axum server for Mint

#![doc = include_str!("../README.md")]

use std::sync::Arc;

use anyhow::Result;
#[cfg(feature = "auth")]
use auth::create_auth_router;
use axum::middleware::from_fn;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use cache::HttpCache;
use cdk::mint::Mint;
use router_handlers::*;

mod metrics;

#[cfg(feature = "auth")]
mod auth;
pub mod cache;
mod custom_handlers;
mod custom_router;
mod router_handlers;
mod ws;

#[cfg(feature = "swagger")]
mod swagger_imports {
    pub use cdk::amount::Amount;
    pub use cdk::error::{ErrorCode, ErrorResponse};
    pub use cdk::nuts::nut00::{
        BlindSignature, BlindedMessage, CurrencyUnit, PaymentMethod, Proof, Witness,
    };
    pub use cdk::nuts::nut01::{Keys, KeysResponse, PublicKey, SecretKey};
    pub use cdk::nuts::nut02::{KeySet, KeySetInfo, KeysetResponse};
    pub use cdk::nuts::nut03::{SwapRequest, SwapResponse};
    pub use cdk::nuts::nut04::{MintMethodSettings, MintRequest, MintResponse};
    pub use cdk::nuts::nut05::{MeltMethodSettings, MeltRequest};
    pub use cdk::nuts::nut06::{ContactInfo, MintInfo, MintVersion, Nuts, SupportedSettings};
    pub use cdk::nuts::nut07::{CheckStateRequest, CheckStateResponse, ProofState, State};
    pub use cdk::nuts::nut09::{RestoreRequest, RestoreResponse};
    pub use cdk::nuts::nut11::P2PKWitness;
    pub use cdk::nuts::nut12::{BlindSignatureDleq, ProofDleq};
    pub use cdk::nuts::nut14::HTLCWitness;
    pub use cdk::nuts::nut15::{Mpp, MppMethodSettings};
    pub use cdk::nuts::nut23::{
        MeltQuoteBolt11Request, MeltQuoteBolt11Response, MintQuoteBolt11Request,
        MintQuoteBolt11Response,
    };
    #[cfg(feature = "auth")]
    pub use cdk::nuts::MintAuthRequest;
    pub use cdk::nuts::{nut04, nut05, nut15, MeltQuoteState, MintQuoteState};
}

#[cfg(feature = "swagger")]
use swagger_imports::*;

/// CDK Mint State
#[derive(Clone)]
pub struct MintState {
    mint: Arc<Mint>,
    cache: Arc<cache::HttpCache>,
}

#[cfg(feature = "swagger")]
macro_rules! define_api_doc {
    (
        schemas: [$($schema:ty),* $(,)?]
        $(, auth_schemas: [$($auth_schema:ty),* $(,)?])?
        $(, paths: [$($path:path),* $(,)?])?
        $(, auth_paths: [$($auth_path:path),* $(,)?])?
    ) => {
        #[derive(utoipa::OpenApi)]
        #[openapi(
            components(schemas(
                $($schema,)*
                $($($auth_schema,)*)?
            )),
            info(description = "Cashu CDK mint APIs", title = "cdk-mintd"),
            paths(
                get_keys,
                get_keyset_pubkeys,
                get_keysets,
                get_mint_info,
                post_swap,
                post_check,
                post_restore
                $(,$($path,)*)?
                $(,$($auth_path,)*)?
            )
        )]
        /// Swagger api docs
        pub struct ApiDoc;
    };
}

// Configuration without auth feature
#[cfg(all(feature = "swagger", not(feature = "auth")))]
define_api_doc! {
    schemas: [
        Amount,
        BlindedMessage,
        BlindSignature,
        BlindSignatureDleq,
        CheckStateRequest,
        CheckStateResponse,
        ContactInfo,
        CurrencyUnit,
        ErrorCode,
        ErrorResponse,
        HTLCWitness,
        Keys,
        KeysResponse,
        KeysetResponse,
        KeySet,
        KeySetInfo,
        MeltRequest<String>,
        MeltQuoteBolt11Request,
        MeltQuoteBolt11Response<String>,
        MeltQuoteState,
        MeltMethodSettings,
        MintRequest<String>,
        MintResponse,
        MintInfo,
        MintQuoteBolt11Request,
        MintQuoteBolt11Response<String>,
        MintQuoteState,
        MintMethodSettings,
        MintVersion,
        Mpp,
        MppMethodSettings,
        Nuts,
        P2PKWitness,
        PaymentMethod,
        Proof,
        ProofDleq,
        ProofState,
        PublicKey,
        RestoreRequest,
        RestoreResponse,
        SecretKey,
        State,
        SupportedSettings,
        SwapRequest,
        SwapResponse,
        Witness,
        nut04::Settings,
        nut05::Settings,
        nut15::Settings
    ]
}

// Configuration with auth feature
#[cfg(all(feature = "swagger", feature = "auth"))]
define_api_doc! {
    schemas: [
        Amount,
        BlindedMessage,
        BlindSignature,
        BlindSignatureDleq,
        CheckStateRequest,
        CheckStateResponse,
        ContactInfo,
        CurrencyUnit,
        ErrorCode,
        ErrorResponse,
        HTLCWitness,
        Keys,
        KeysResponse,
        KeysetResponse,
        KeySet,
        KeySetInfo,
        MeltRequest<String>,
        MeltQuoteBolt11Request,
        MeltQuoteBolt11Response<String>,
        MeltQuoteState,
        MeltMethodSettings,
        MintRequest<String>,
        MintResponse,
        MintInfo,
        MintQuoteBolt11Request,
        MintQuoteBolt11Response<String>,
        MintQuoteState,
        MintMethodSettings,
        MintVersion,
        Mpp,
        MppMethodSettings,
        Nuts,
        P2PKWitness,
        PaymentMethod,
        Proof,
        ProofDleq,
        ProofState,
        PublicKey,
        RestoreRequest,
        RestoreResponse,
        SecretKey,
        State,
        SupportedSettings,
        SwapRequest,
        SwapResponse,
        Witness,
        nut04::Settings,
        nut05::Settings,
        nut15::Settings
    ],
    auth_schemas: [MintAuthRequest],
    auth_paths: [
        crate::auth::get_auth_keysets,
        crate::auth::get_blind_auth_keys,
        crate::auth::post_mint_auth
    ]
}

/// Create mint [`Router`] with required endpoints for cashu mint with the default cache
///
/// The `custom_methods` parameter should include all custom payment methods supported
/// by the payment processor, including "bolt11" and "bolt12" if they are supported.
pub async fn create_mint_router(mint: Arc<Mint>, custom_methods: Vec<String>) -> Result<Router> {
    create_mint_router_with_custom_cache(mint, Default::default(), custom_methods).await
}

async fn cors_middleware(
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    #[cfg(feature = "auth")]
    let allowed_headers = "Content-Type, Clear-auth, Blind-auth";
    #[cfg(not(feature = "auth"))]
    let allowed_headers = "Content-Type";

    // Handle preflight requests
    if req.method() == axum::http::Method::OPTIONS {
        let mut response = Response::new("".into());
        response.headers_mut().insert(
            "Access-Control-Allow-Origin",
            "*".parse().expect("Valid header value"),
        );
        response.headers_mut().insert(
            "Access-Control-Allow-Methods",
            "GET, POST, OPTIONS".parse().expect("Valid header value"),
        );
        response.headers_mut().insert(
            "Access-Control-Allow-Headers",
            allowed_headers.parse().expect("Valid header value"),
        );
        return response;
    }

    // Call the next handler
    let mut response = next.run(req).await;

    response.headers_mut().insert(
        "Access-Control-Allow-Origin",
        "*".parse().expect("Valid header value"),
    );
    response.headers_mut().insert(
        "Access-Control-Allow-Methods",
        "GET, POST, OPTIONS".parse().expect("Valid header value"),
    );
    response.headers_mut().insert(
        "Access-Control-Allow-Headers",
        allowed_headers.parse().expect("Valid header value"),
    );

    response
}

/// Create mint [`Router`] with required endpoints for cashu mint with a custom
/// backend for cache
///
/// The `custom_methods` parameter should include all custom payment methods supported
/// by the payment processor, including "bolt11" and "bolt12" if they are supported.
pub async fn create_mint_router_with_custom_cache(
    mint: Arc<Mint>,
    cache: HttpCache,
    custom_methods: Vec<String>,
) -> Result<Router> {
    let state = MintState {
        mint,
        cache: Arc::new(cache),
    };

    let v1_router = Router::new()
        .route("/keys", get(get_keys))
        .route("/keysets", get(get_keysets))
        .route("/keys/{keyset_id}", get(get_keyset_pubkeys))
        .route("/swap", post(cache_post_swap))
        .route("/ws", get(ws_handler))
        .route("/checkstate", post(post_check))
        .route("/info", get(get_mint_info))
        .route("/restore", post(post_restore));

    let mint_router = Router::new().nest("/v1", v1_router);

    #[cfg(feature = "auth")]
    let mint_router = {
        let auth_router = create_auth_router(state.clone());
        mint_router.nest("/v1", auth_router)
    };

    // Create and merge custom payment method routers
    // This now includes bolt11 and bolt12 if they are in custom_methods
    let mint_router = if !custom_methods.is_empty() {
        // Validate custom method names
        custom_router::validate_custom_method_names(&custom_methods)
            .map_err(|e| anyhow::anyhow!("Invalid custom method names: {}", e))?;

        tracing::info!(
            "Creating routes for {} payment methods: {:?}",
            custom_methods.len(),
            custom_methods
        );

        let custom_router = custom_router::create_custom_routers(state.clone(), custom_methods);
        mint_router.nest("/v1", custom_router)
    } else {
        mint_router
    };

    #[cfg(feature = "prometheus")]
    let mint_router = mint_router.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        metrics::global_metrics_middleware,
    ));
    let mint_router = mint_router
        .layer(from_fn(cors_middleware))
        .with_state(state);

    Ok(mint_router)
}
