//! Axum server for Mint

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::sync::Arc;

use anyhow::Result;
use axum::routing::{get, post};
use axum::Router;
use cdk::mint::Mint;
use moka::future::Cache;
use router_handlers::*;
use std::time::Duration;

mod router_handlers;

/// CDK Mint State
#[derive(Clone)]
pub struct MintState {
    mint: Arc<Mint>,
    cache: Cache<String, String>,
}

/// Create mint [`Router`] with required endpoints for cashu mint
pub async fn create_mint_router(
    mint: Arc<Mint>,
    cache_ttl: u64,
    cache_tti: u64,
    include_bolt12: bool,
) -> Result<Router> {
    let state = MintState {
        mint,
        cache: Cache::builder()
            .max_capacity(10_000)
            .time_to_live(Duration::from_secs(cache_ttl))
            .time_to_idle(Duration::from_secs(cache_tti))
            .build(),
    };

    let mut v1_router = Router::new()
        .route("/keys", get(get_keys))
        .route("/keysets", get(get_keysets))
        .route("/keys/:keyset_id", get(get_keyset_pubkeys))
        .route("/swap", post(cache_post_swap))
        .route("/mint/quote/bolt11", post(get_mint_bolt11_quote))
        .route(
            "/mint/quote/bolt11/:quote_id",
            get(get_check_mint_bolt11_quote),
        )
        .route("/mint/bolt11", post(cache_post_mint_bolt11))
        .route("/melt/quote/bolt11", post(get_melt_bolt11_quote))
        .route(
            "/melt/quote/bolt11/:quote_id",
            get(get_check_melt_bolt11_quote),
        )
        .route("/melt/bolt11", post(cache_post_melt_bolt11))
        .route("/checkstate", post(post_check))
        .route("/info", get(get_mint_info))
        .route("/restore", post(post_restore));

    // Conditionally create and merge bolt12_router
    if include_bolt12 {
        let bolt12_router = create_bolt12_router(state.clone());
        //v1_router = bolt12_router.merge(v1_router);
        v1_router = v1_router.merge(bolt12_router);
    }

    // Nest the combined router under "/v1"
    let mint_router = Router::new().nest("/v1", v1_router).with_state(state);

    Ok(mint_router)
}

fn create_bolt12_router(state: MintState) -> Router<MintState> {
    Router::new()
        .route("/melt/quote/bolt12", post(get_melt_bolt12_quote))
        .route(
            "/melt/quote/bolt12/:quote_id",
            get(get_check_melt_bolt11_quote),
        )
        .route("/melt/bolt12", post(post_melt_bolt12))
        .route("/mint/quote/bolt12", post(get_mint_bolt12_quote))
        .route(
            "/mint/quote/bolt12/:quote_id",
            get(get_check_mint_bolt11_quote),
        )
        .route("/mint/bolt12", post(post_mint_bolt12))
        .with_state(state)
}
