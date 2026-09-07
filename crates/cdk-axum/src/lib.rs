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
#[cfg(feature = "federation-bdk")]
use cdk_bdk::FederatedBdkLocalConfig;
use router_handlers::*;

#[cfg(feature = "federation")]
use self::federation::bootstrap::*;
#[cfg(feature = "federation")]
use self::federation::mint_state::*;
#[cfg(feature = "federation")]
use self::federation::state::*;

mod metrics;

mod auth;
pub mod cache;
mod custom_handlers;
mod custom_router;
#[cfg(feature = "federation")]
mod federation;
#[cfg(all(feature = "federation", feature = "iroh"))]
pub use self::federation::federation_transport_with_iroh_node;
#[cfg(feature = "federation")]
pub use self::federation::services::{FederationServices, RunningFederationServices};
#[cfg(feature = "federation")]
pub use self::federation::{
    create_federation_setup_router, default_federation_transport, FederationDkgAutoStartConfig,
    FederationDkgDriverRunConfig, FederationDkgDriverRunOutcome, FederationDkgDriverTickOutcome,
    FederationDkgSecretShareTransportPolicy, FederationPeerSyncConfig,
    FederationPeerSyncConfigError, FederationSetupCoordinatorService,
    FederationSetupCoordinatorServiceConfig, FederationSetupHttpClient, FederationSetupHttpError,
    FederationSetupHttpErrorCode, FederationTransport,
};
mod router_handlers;
mod ws;

/// CDK Mint State
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct MintState {
    mint: Arc<Mint>,
    cache: Arc<cache::HttpCache>,
    #[cfg(feature = "federation")]
    federation: federation::FederationNodeRuntime,
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
    create_mint_router_with_custom_cache_inner(
        mint,
        cache,
        custom_methods,
        enable_info_page,
        #[cfg(feature = "federation")]
        FederationRouterConfig::default(),
        #[cfg(feature = "federation")]
        FederationRouterStartup::Automatic,
    )
    .await
    .map(|built| built.router)
}

/// Member-local federation services attached to a mint router.
#[cfg(feature = "federation")]
#[derive(Debug)]
pub struct FederationRouterConfig {
    /// Peer synchronization, admission, and retry policy.
    pub peer_sync: FederationPeerSyncConfig,
    /// Optional dealer-free DKG driver.
    pub dkg: Option<FederationDkgAutoStartConfig>,
    /// Optional watch-only Bitcoin wallet.
    #[cfg(feature = "federation-bdk")]
    pub bdk: Option<FederatedBdkLocalConfig>,
    /// Optional Bark custody runtime.
    #[cfg(feature = "federation-bark")]
    pub bark: Option<cdk_bark::FederatedBarkLocalConfig>,
    /// Shared outbound member transport.
    pub transport: FederationTransport,
}

#[cfg(feature = "federation")]
impl Default for FederationRouterConfig {
    fn default() -> Self {
        Self {
            peer_sync: FederationPeerSyncConfig::default(),
            dkg: None,
            #[cfg(feature = "federation-bdk")]
            bdk: None,
            #[cfg(feature = "federation-bark")]
            bark: None,
            transport: default_federation_transport(),
        }
    }
}

#[cfg(feature = "federation")]
enum FederationRouterStartup {
    Automatic,
    Prepared,
    Setup,
    #[cfg(test)]
    LocalFinality,
}

/// Create federation routes with services owned by the router's lifetime.
#[cfg(feature = "federation")]
pub async fn create_federation_mint_router(
    mint: Arc<Mint>,
    cache: HttpCache,
    custom_methods: Vec<String>,
    enable_info_page: bool,
    config: FederationRouterConfig,
) -> Result<Router> {
    create_mint_router_with_custom_cache_inner(
        mint,
        cache,
        custom_methods,
        enable_info_page,
        config,
        FederationRouterStartup::Automatic,
    )
    .await
    .map(|built| built.router)
}

/// Create the isolated setup runtime exposing only authenticated DKG routes.
#[cfg(feature = "federation")]
pub async fn create_federation_dkg_setup_router(
    mint: Arc<Mint>,
    federation_peer_sync_config: FederationPeerSyncConfig,
    federation_dkg_config: FederationDkgAutoStartConfig,
) -> Result<Router> {
    create_federation_dkg_setup_router_with_transport(
        mint,
        federation_peer_sync_config,
        federation_dkg_config,
        default_federation_transport(),
    )
    .await
}

/// Create the isolated setup DKG runtime with an injected federation transport.
#[cfg(feature = "federation")]
pub async fn create_federation_dkg_setup_router_with_transport(
    mint: Arc<Mint>,
    peer_sync: FederationPeerSyncConfig,
    dkg: FederationDkgAutoStartConfig,
    transport: FederationTransport,
) -> Result<Router> {
    create_mint_router_with_custom_cache_inner(
        mint,
        HttpCache::default(),
        Vec::new(),
        false,
        FederationRouterConfig {
            peer_sync,
            dkg: Some(dkg),
            transport,
            #[cfg(feature = "federation-bdk")]
            bdk: None,
            #[cfg(feature = "federation-bark")]
            bark: None,
        },
        FederationRouterStartup::Setup,
    )
    .await
    .map(|built| built.router)
}

/// Prepare federation routes and workers for explicit daemon lifecycle ownership.
#[cfg(feature = "federation")]
pub async fn prepare_federation_router(
    mint: Arc<Mint>,
    cache: HttpCache,
    custom_methods: Vec<String>,
    enable_info_page: bool,
    config: FederationRouterConfig,
) -> Result<(Router, Option<FederationServices>)> {
    let router = create_mint_router_with_custom_cache_inner(
        mint,
        cache,
        custom_methods,
        enable_info_page,
        config,
        FederationRouterStartup::Prepared,
    )
    .await?;
    Ok((router.router, router.services))
}

/// Build member routes without cross-member synchronization for isolated tests.
#[cfg(all(test, feature = "federation"))]
pub(crate) async fn create_mint_router_without_federation_peer_sync(
    mint: Arc<Mint>,
    custom_methods: Vec<String>,
) -> Result<Router> {
    create_mint_router_with_custom_cache_inner(
        mint,
        HttpCache::default(),
        custom_methods,
        false,
        FederationRouterConfig::default(),
        FederationRouterStartup::LocalFinality,
    )
    .await
    .map(|built| built.router)
}

struct BuiltMintRouter {
    router: Router,
    #[cfg(feature = "federation")]
    services: Option<FederationServices>,
}

#[allow(unused_mut, unused_variables)]
async fn create_mint_router_with_custom_cache_inner(
    mint: Arc<Mint>,
    cache: HttpCache,
    custom_methods: Vec<String>,
    enable_info_page: bool,
    #[cfg(feature = "federation")] federation_config: FederationRouterConfig,
    #[cfg(feature = "federation")] startup: FederationRouterStartup,
) -> Result<BuiltMintRouter> {
    #[cfg(feature = "federation")]
    let FederationRouterConfig {
        peer_sync: federation_peer_sync_config,
        dkg: federation_dkg_config,
        #[cfg(feature = "federation-bdk")]
            bdk: federation_bdk_config,
        #[cfg(feature = "federation-bark")]
            bark: federation_bark_config,
        transport: federation_transport,
    } = federation_config;
    #[cfg(feature = "federation")]
    federation_peer_sync_config.validate()?;
    #[cfg(feature = "federation")]
    let spawn_federation_peer_sync = matches!(
        startup,
        FederationRouterStartup::Automatic | FederationRouterStartup::Prepared
    );
    #[cfg(feature = "federation")]
    let federation_setup_only = matches!(startup, FederationRouterStartup::Setup);
    #[cfg(all(test, feature = "federation"))]
    let federation_local_finality_test = matches!(startup, FederationRouterStartup::LocalFinality);
    #[cfg(all(feature = "federation", feature = "prometheus"))]
    let federation_startup_started_at = std::time::Instant::now();
    #[cfg(feature = "federation")]
    let federation_runtime_state = create_federation_runtime_state_with_wallet(
        &mint,
        federation_peer_sync_config,
        federation_transport.clone(),
        #[cfg(feature = "federation-bdk")]
        federation_bdk_config,
        #[cfg(feature = "federation-bark")]
        federation_bark_config,
    )
    .await?;
    #[cfg(feature = "federation")]
    let federation_member = federation_runtime_state.member;
    #[cfg(feature = "federation-bdk")]
    let federation_onchain_wallets = federation_runtime_state.onchain_wallets;
    #[cfg(feature = "federation")]
    let federation_frost_signing = federation_runtime_state.frost_signing;
    #[cfg(feature = "federation-bark")]
    let federation_bark = federation_runtime_state.bark;
    #[cfg(feature = "federation-bdk")]
    let federation_startup_bdk_degraded_reason = federation_runtime_state.bdk_degraded_reason;
    #[cfg(feature = "federation")]
    let federation_sync = federation_member
        .as_ref()
        .map(|_| Arc::new(tokio::sync::Mutex::new(FederationSyncState::for_aleph_bft())));
    let mut state = MintState {
        mint: Arc::clone(&mint),
        cache: Arc::new(cache),
        #[cfg(feature = "federation")]
        federation: federation::FederationNodeRuntime {
            transport: federation_transport,
            membership: federation_member.map_or(
                federation::FederationMembership::Standalone,
                federation::FederationMembership::configured,
            ),
            tasks: crate::federation::services::FederationTaskGroup::default(),
            payment_observer: federation::FederationPaymentObserver::default(),
            #[cfg(feature = "federation-bdk")]
            transaction_broadcaster: federation::FederationTransactionBroadcaster::default(),
            #[cfg(feature = "federation-bdk")]
            wallet_liveness: federation::FederationWalletLivenessWorker::default(),
            components: federation::runtime_components::FederationRuntimeComponents::new(
                federation_sync,
                None,
                None,
                #[cfg(feature = "federation-bdk")]
                federation_onchain_wallets,
                federation_frost_signing,
                #[cfg(feature = "federation-bark")]
                federation_bark,
            ),
            aleph_bft_network_inbound: Arc::new(tokio::sync::RwLock::new(None)),
            aleph_bft_live_session: Arc::new(tokio::sync::RwLock::new(None)),
            aleph_bft_halt_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            #[cfg(test)]
            local_finality_test: federation_local_finality_test,
            #[cfg(test)]
            manual_finality_test: false,
        },
    };
    #[cfg(feature = "federation-bdk")]
    state
        .federation
        .components
        .set_startup_bdk_degraded_reason(federation_startup_bdk_degraded_reason)
        .await;
    #[cfg(all(test, feature = "federation"))]
    if state.federation.local_finality_test {
        state.federation.set_sync_handle(
            state
                .federation
                .consensus()
                .map(|_| Arc::new(tokio::sync::Mutex::new(FederationSyncState::default()))),
        );
    }
    #[cfg(feature = "federation")]
    let (federation_dkg_config, spawn_federation_dkg_driver) = match federation_dkg_config {
        Some(config) => {
            let runtime_config = state.mint.federation_config().ok_or_else(|| {
                anyhow::anyhow!(
                    "federation DKG auto-start requires a mint federation runtime config"
                )
            })?;
            let public_config = runtime_config.public_config();
            if !public_config.keysets.is_empty() || public_config.setup_transcript.is_some() {
                tracing::info!(
                    member_id = %runtime_config.member_id(),
                    ceremony_id = %config.ceremony_id,
                    "Federation DKG auto-start skipped because finalized key material is already active"
                );
                (None, false)
            } else {
                config.validate_for_public_config(public_config)?;
                (Some(config), true)
            }
        }
        None => (None, false),
    };
    #[cfg(feature = "federation")]
    {
        let dkg_member_admission = match federation_dkg_config.as_ref() {
            Some(config) => state.mint.federation_config().map(|runtime_config| {
                federation_dkg_member_admission(
                    runtime_config.public_config(),
                    config.driver.member_request_burst,
                    config.driver.member_request_refill_interval,
                )
            }),
            None => None,
        };
        state.federation.set_dkg_components(
            federation_dkg_config.clone().map(Arc::new),
            dkg_member_admission,
        );
    }
    #[cfg(feature = "federation")]
    if state.federation.consensus().is_some() {
        state
            .federation
            .recover_scheduled_checkpoint_candidate_at_startup()
            .await?;
        state
            .federation
            .recover_recovery_checkpoint_candidate_at_startup()
            .await?;
    }
    #[cfg(feature = "federation")]
    if spawn_federation_peer_sync {
        state.restore_federation_startup_readiness().await;
    }
    #[cfg(all(feature = "federation", feature = "prometheus"))]
    if let Some(config) = state.mint.federation_config() {
        cdk_prometheus::METRICS.record_federation_checkpoint_duration(
            &config.member_id().to_string(),
            cdk_prometheus::FederationCheckpointOperation::StartupReadiness,
            federation_startup_started_at.elapsed().as_secs_f64(),
        );
    }
    #[cfg(feature = "federation")]
    let expose_federation_routes =
        state.federation.consensus().is_some() || federation_dkg_config.is_some();
    #[cfg(feature = "federation")]
    let mut federation_services = state.mint.federation_config().map(|_| {
        FederationServices::new(
            state.clone(),
            spawn_federation_peer_sync.then_some(federation_peer_sync_config),
            if spawn_federation_dkg_driver {
                federation_dkg_config
            } else {
                None
            },
        )
    });
    #[cfg(feature = "federation")]
    if federation_setup_only {
        let router = Router::new()
            .nest("/federation/v1", federation::create_federation_dkg_router())
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                federation_invalid_operation_alert_middleware,
            ))
            .layer(from_fn(cors_middleware))
            .with_state(state);
        let router = match federation_services.take() {
            Some(services) => router.layer(axum::Extension(Arc::new(services.start().await?))),
            None => router,
        };
        return Ok(BuiltMintRouter {
            router,
            services: None,
        });
    }

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
    if expose_federation_routes {
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

    #[cfg(feature = "federation")]
    let mint_router = mint_router.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        federation_invalid_operation_alert_middleware,
    ));

    #[cfg(feature = "prometheus")]
    let mint_router = mint_router.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        metrics::global_metrics_middleware,
    ));
    let mut mint_router = mint_router
        .layer(from_fn(cors_middleware))
        .with_state(state);

    #[cfg(feature = "federation")]
    if !matches!(startup, FederationRouterStartup::Prepared) {
        if let Some(services) = federation_services.take() {
            mint_router = mint_router.layer(axum::Extension(Arc::new(services.start().await?)));
        }
    }
    Ok(BuiltMintRouter {
        router: mint_router,
        #[cfg(feature = "federation")]
        services: federation_services,
    })
}
