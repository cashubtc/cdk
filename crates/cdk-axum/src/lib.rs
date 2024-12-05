//! Axum server for Mint

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::routing::{get, post};
use axum::Router;
use cdk::mint::Mint;
use moka::future::Cache;
use router_handlers::*;

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
    pub use cdk::nuts::nut02::{Id, KeySet, KeySetInfo, KeySetVersion, KeysetResponse};
    pub use cdk::nuts::nut03::{SwapRequest, SwapResponse};
    pub use cdk::nuts::nut04::{
        MintBolt11Request, MintBolt11Response, MintMethodSettings, MintQuoteBolt11Request,
        MintQuoteBolt11Response,
    };
    pub use cdk::nuts::nut05::{
        MeltBolt11Request, MeltMethodSettings, MeltQuoteBolt11Request, MeltQuoteBolt11Response,
    };
    pub use cdk::nuts::nut06::{ContactInfo, MintInfo, MintVersion, Nuts, SupportedSettings};
    pub use cdk::nuts::nut07::{CheckStateRequest, CheckStateResponse, ProofState, State};
    pub use cdk::nuts::nut09::{RestoreRequest, RestoreResponse};
    pub use cdk::nuts::nut11::P2PKWitness;
    pub use cdk::nuts::nut12::{BlindSignatureDleq, ProofDleq};
    pub use cdk::nuts::nut14::HTLCWitness;
    pub use cdk::nuts::nut15::{Mpp, MppMethodSettings};
    pub use cdk::nuts::{nut04, nut05, nut15, MeltQuoteState, MintQuoteState};
}

#[cfg(feature = "swagger")]
use swagger_imports::*;
#[cfg(feature = "swagger")]
use uuid::Uuid;

/// CDK Mint State
#[derive(Clone)]
pub struct MintState {
    mint: Arc<Mint>,
    cache: Cache<String, String>,
}

#[cfg(feature = "swagger")]
#[derive(utoipa::OpenApi)]
#[openapi(
    components(schemas(
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
        Id,
        Keys,
        KeysResponse,
        KeysetResponse,
        KeySet,
        KeySetInfo,
        KeySetVersion,
        MeltBolt11Request<Uuid>,
        MeltQuoteBolt11Request,
        MeltQuoteBolt11Response<Uuid>,
        MeltQuoteState,
        MeltMethodSettings,
        MintBolt11Request<Uuid>,
        MintBolt11Response,
        MintInfo,
        MintQuoteBolt11Request,
        MintQuoteBolt11Response<Uuid>,
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
    )),
    info(description = "Cashu CDK mint APIs", title = "cdk-mintd",),
    paths(
        get_keys,
        get_keyset_pubkeys,
        get_keysets,
        get_mint_info,
        post_mint_bolt11_quote,
        get_check_mint_bolt11_quote,
        post_mint_bolt11,
        post_melt_bolt11_quote,
        get_check_melt_bolt11_quote,
        post_melt_bolt11,
        post_swap,
        post_check,
        post_restore
    )
)]
/// OpenAPI spec for the mint's v1 APIs
pub struct ApiDocV1;

/// Create mint [`Router`] with required endpoints for cashu mint
pub async fn create_mint_router(mint: Arc<Mint>, cache_ttl: u64, cache_tti: u64) -> Result<Router> {
    let state = MintState {
        mint,
        cache: Cache::builder()
            .max_capacity(10_000)
            .time_to_live(Duration::from_secs(cache_ttl))
            .time_to_idle(Duration::from_secs(cache_tti))
            .build(),
    };

    let v1_router = Router::new()
        .route("/keys", get(get_keys))
        .route("/keysets", get(get_keysets))
        .route("/keys/:keyset_id", get(get_keyset_pubkeys))
        .route("/swap", post(cache_post_swap))
        .route("/mint/quote/bolt11", post(post_mint_bolt11_quote))
        .route(
            "/mint/quote/bolt11/:quote_id",
            get(get_check_mint_bolt11_quote),
        )
        .route("/mint/bolt11", post(cache_post_mint_bolt11))
        .route("/melt/quote/bolt11", post(post_melt_bolt11_quote))
        .route("/ws", get(ws_handler))
        .route(
            "/melt/quote/bolt11/:quote_id",
            get(get_check_melt_bolt11_quote),
        )
        .route("/melt/bolt11", post(cache_post_melt_bolt11))
        .route("/checkstate", post(post_check))
        .route("/info", get(get_mint_info))
        .route("/restore", post(post_restore));

    let mint_router = Router::new().nest("/v1", v1_router).with_state(state);

    Ok(mint_router)
}
