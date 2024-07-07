//! Axum server for Mint

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use axum::routing::{get, post};
use axum::Router;
use cdk::cdk_lightning::{self, MintLightning};
use cdk::mint::Mint;
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use router_handlers::*;

mod router_handlers;

/// Create mint [`Router`] with required endpoints for cashu mint
pub async fn create_mint_router(
    mint_url: &str,
    mint: Arc<Mint>,
    ln: HashMap<LnKey, Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>>,
    quote_ttl: u64,
) -> Result<Router> {
    let state = MintState {
        ln,
        mint,
        mint_url: mint_url.to_string(),
        quote_ttl,
    };

    let v1_router = Router::new()
        .route("/keys", get(get_keys))
        .route("/keysets", get(get_keysets))
        .route("/keys/:keyset_id", get(get_keyset_pubkeys))
        .route("/swap", post(post_swap))
        .route("/mint/quote/bolt11", post(get_mint_bolt11_quote))
        .route(
            "/mint/quote/bolt11/:quote_id",
            get(get_check_mint_bolt11_quote),
        )
        .route("/mint/bolt11", post(post_mint_bolt11))
        .route("/melt/quote/bolt11", post(get_melt_bolt11_quote))
        .route(
            "/melt/quote/bolt11/:quote_id",
            get(get_check_melt_bolt11_quote),
        )
        .route("/melt/bolt11", post(post_melt_bolt11))
        .route("/checkstate", post(post_check))
        .route("/info", get(get_mint_info))
        .route("/restore", post(post_restore));

    let mint_router = Router::new().nest("/v1", v1_router).with_state(state);

    Ok(mint_router)
}

#[derive(Clone)]
struct MintState {
    ln: HashMap<LnKey, Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>>,
    mint: Arc<Mint>,
    mint_url: String,
    quote_ttl: u64,
}

/// Key used in hashmap of ln backends to identify what unit and payment method it is for
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct LnKey {
    /// Unit of Payment backend
    pub unit: CurrencyUnit,
    /// Method of payment backend
    pub method: PaymentMethod,
}

impl LnKey {
    /// Create new [`LnKey`]
    pub fn new(unit: CurrencyUnit, method: PaymentMethod) -> Self {
        Self { unit, method }
    }
}
