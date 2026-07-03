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
#[cfg(feature = "federation")]
use cdk::federation::{
    FederationJournalAppendOutcome, FederationJournalEntry, FederationOperationEnvelope,
    FederationRestSequenceTracker, FederationSubmitOperationResponse, InMemoryFederationConsensus,
    MemberId,
};
use cdk::mint::Mint;
use router_handlers::*;

mod metrics;

mod auth;
pub mod cache;
mod custom_handlers;
mod custom_router;
#[cfg(feature = "federation")]
mod federation;
mod router_handlers;
mod ws;

#[cfg(feature = "federation")]
type FederationConsensusHandle = Arc<tokio::sync::Mutex<InMemoryFederationConsensus>>;
#[cfg(feature = "federation")]
type FederationSequenceHandle = Arc<tokio::sync::Mutex<FederationRestSequenceTracker>>;

/// CDK Mint State
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct MintState {
    mint: Arc<Mint>,
    cache: Arc<cache::HttpCache>,
    #[cfg(feature = "federation")]
    federation_consensus: Option<FederationConsensusHandle>,
    #[cfg(feature = "federation")]
    federation_sequences: Option<FederationSequenceHandle>,
}

#[cfg(feature = "federation")]
impl MintState {
    pub(crate) async fn submit_federation_envelope(
        &self,
        envelope: FederationOperationEnvelope,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        self.submit_federation_envelope_with_sequence(envelope, None)
            .await
    }

    pub(crate) async fn submit_federation_envelope_with_sequence(
        &self,
        envelope: FederationOperationEnvelope,
        sequence: Option<(MemberId, u64)>,
    ) -> Result<FederationSubmitOperationResponse, cdk::Error> {
        let consensus = self.federation_consensus.as_ref().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its consensus handle".to_string())
        })?;
        let mut consensus = consensus.lock().await;
        let mut next_consensus = consensus.clone();
        let response = next_consensus.submit_envelope(envelope)?;

        self.persist_federation_append_outcome(&response.outcome, sequence)
            .await?;
        *consensus = next_consensus;
        if let Some((sender, sequence)) = sequence {
            self.record_federation_sequence(sender, sequence).await;
        }

        Ok(response)
    }

    pub(crate) async fn apply_federation_catch_up_response(
        &self,
        response: &cdk::federation::FederationJournalCatchUpResponse,
    ) -> Result<Vec<FederationJournalEntry>, cdk::Error> {
        let consensus = self.federation_consensus.as_ref().ok_or_else(|| {
            cdk::Error::Custom("federation mint is missing its consensus handle".to_string())
        })?;
        let mut consensus = consensus.lock().await;
        let mut next_consensus = consensus.clone();
        let applied = next_consensus.apply_catch_up_response(response)?;

        self.persist_federation_journal_entries(&applied, None)
            .await?;
        *consensus = next_consensus;

        Ok(applied)
    }

    pub(crate) async fn persist_federation_sequence(
        &self,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        self.persist_federation_journal_entries(&[], Some((sender, sequence)))
            .await?;
        self.record_federation_sequence(sender, sequence).await;
        Ok(())
    }

    async fn persist_federation_append_outcome(
        &self,
        outcome: &FederationJournalAppendOutcome,
        sequence: Option<(MemberId, u64)>,
    ) -> Result<(), cdk::Error> {
        match outcome {
            FederationJournalAppendOutcome::Appended(entry) => {
                self.persist_federation_journal_entries(std::slice::from_ref(entry), sequence)
                    .await
            }
            FederationJournalAppendOutcome::Duplicate(_) => {
                self.persist_federation_journal_entries(&[], sequence).await
            }
        }
    }

    async fn persist_federation_journal_entries(
        &self,
        entries: &[FederationJournalEntry],
        sequence: Option<(MemberId, u64)>,
    ) -> Result<(), cdk::Error> {
        let db = self.mint.localstore();
        let mut tx = db.begin_transaction().await?;
        for entry in entries {
            tx.add_federation_journal_entry(entry).await?;
        }
        if let Some((sender, sequence)) = sequence {
            tx.set_federation_member_sequence(sender, sequence).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn record_federation_sequence(&self, sender: MemberId, sequence: u64) {
        if let Some(sequences) = &self.federation_sequences {
            sequences.lock().await.record_sequence(sender, sequence);
        }
    }
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
    #[cfg(feature = "federation")]
    let (federation_consensus, federation_sequences) =
        create_federation_runtime_state(&mint).await?;
    let state = MintState {
        mint,
        cache: Arc::new(cache),
        #[cfg(feature = "federation")]
        federation_consensus,
        #[cfg(feature = "federation")]
        federation_sequences,
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

    #[cfg(feature = "federation")]
    if state.federation_consensus.is_some() {
        mint_router = mint_router.nest("/federation/v1", federation::create_federation_router());
    }

    #[cfg(feature = "info-page")]
    if enable_info_page {
        mint_router = mint_router.route("/", get(get_index));
    }

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

#[cfg(feature = "federation")]
async fn create_federation_runtime_state(
    mint: &Arc<Mint>,
) -> Result<(
    Option<FederationConsensusHandle>,
    Option<FederationSequenceHandle>,
)> {
    let Some(config) = mint.federation_config() else {
        return Ok((None, None));
    };

    let db = mint.localstore();
    let entries = db.get_federation_journal_entries().await?;
    let sequences = db.get_federation_member_sequences().await?;
    let consensus = InMemoryFederationConsensus::from_entries(
        config.federation_id(),
        config.member_id(),
        entries,
    )?;
    let sequences = FederationRestSequenceTracker::from_sequences(sequences);

    Ok((
        Some(Arc::new(tokio::sync::Mutex::new(consensus))),
        Some(Arc::new(tokio::sync::Mutex::new(sequences))),
    ))
}
