//! Axum server for Mint

#![doc = include_str!("../README.md")]

use std::sync::Arc;

use anyhow::Result;
use auth::create_auth_router;
use axum::middleware::from_fn;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use cache::HttpCache;
use cdk::mint::Mint;
use router_handlers::*;

mod metrics;

mod auth;
pub mod cache;
mod custom_handlers;
mod custom_router;
mod router_handlers;
mod ws;

/// CDK Mint State
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct MintState {
    mint: Arc<Mint>,
    cache: Arc<cache::HttpCache>,
}

/// Create mint [`Router`] with required endpoints for cashu mint with the default cache
///
/// The `custom_methods` parameter should include all custom payment methods supported
/// by the payment processor, including "bolt11" and "bolt12" if they are supported.
pub async fn create_mint_router(mint: Arc<Mint>, custom_methods: Vec<String>) -> Result<Router> {
    create_mint_router_with_custom_cache(mint, Default::default(), custom_methods, false).await
}

async fn cors_middleware(
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    let allowed_headers = "*";

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
#[allow(unused_mut, unused_variables)]
pub async fn create_mint_router_with_custom_cache(
    mint: Arc<Mint>,
    cache: HttpCache,
    custom_methods: Vec<String>,
    enable_info_page: bool,
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

    let mut mint_router = Router::new().nest("/v1", v1_router);

    #[cfg(feature = "info-page")]
    if enable_info_page {
        mint_router = mint_router.route("/", get(get_index));
    }

    // Proof-of-reserves attestation bundle, served from a file written by the payment processor.
    mint_router = mint_router.route("/audit/latest.json", get(get_audit_latest));

    // Mint icon (PNG), served from a file.
    mint_router = mint_router.route("/icon.png", get(get_mint_icon));

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
