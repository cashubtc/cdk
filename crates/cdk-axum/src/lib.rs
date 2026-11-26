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
#[cfg(feature = "federation")]
use cdk_federation::{FederationNode, FederationNodeMode, PreparedFederationNode};
use router_handlers::*;

#[cfg(feature = "federation")]
use self::federation::federation_invalid_operation_alert_middleware;
#[cfg(all(test, feature = "federation"))]
use self::federation::{bootstrap::*, state::*};

mod metrics;

mod auth;
pub mod cache;
mod custom_handlers;
mod custom_router;
#[cfg(feature = "federation")]
mod federation;
#[cfg(feature = "federation")]
pub use self::federation::create_federation_setup_router;
#[cfg(all(feature = "federation", feature = "iroh"))]
pub use cdk_federation::federation_transport_with_iroh_node;
#[cfg(feature = "federation")]
pub use cdk_federation::services::{FederationServices, RunningFederationServices};
#[cfg(feature = "federation")]
pub use cdk_federation::setup::{
    FederationSetupCoordinatorService, FederationSetupCoordinatorServiceConfig,
    FederationSetupHttpClient, FederationSetupHttpError, FederationSetupHttpErrorCode,
};
#[cfg(feature = "federation")]
pub use cdk_federation::{
    default_federation_transport, FederationDkgAutoStartConfig, FederationDkgDriverRunConfig,
    FederationDkgDriverRunOutcome, FederationDkgDriverTickOutcome,
    FederationDkgSecretShareTransportPolicy, FederationNodeConfig, FederationPeerSyncConfig,
    FederationPeerSyncConfigError, FederationTransport,
};
mod router_handlers;
mod ws;

/// CDK Mint State
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct MintState {
    mint: Arc<Mint>,
    cache: Arc<cache::HttpCache>,
    /// The federation member this mint serves, in member mode.
    ///
    /// `MintState` dereferences to it, so handlers reach the node's runtime
    /// and methods directly.
    #[cfg(feature = "federation")]
    node: cdk_federation::FederationNode,
}

#[cfg(feature = "federation")]
impl std::ops::Deref for MintState {
    type Target = cdk_federation::FederationNode;

    fn deref(&self) -> &Self::Target {
        &self.node
    }
}

#[cfg(feature = "federation")]
impl std::ops::DerefMut for MintState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.node
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
pub type FederationRouterConfig = FederationNodeConfig;

/// When the router's federation services start, and what the node runs.
#[cfg(feature = "federation")]
enum FederationRouterStartup {
    /// A member whose services start with the router.
    Automatic,
    /// A member whose services the caller starts.
    Prepared,
    /// A setup-only DKG node whose services start with the router.
    Setup,
    /// A member without peer synchronization, finalizing locally.
    #[cfg(test)]
    LocalFinality,
}

#[cfg(feature = "federation")]
impl FederationRouterStartup {
    fn node_mode(&self) -> FederationNodeMode {
        match self {
            Self::Automatic | Self::Prepared => FederationNodeMode::Member,
            Self::Setup => FederationNodeMode::SetupDkg,
            #[cfg(test)]
            Self::LocalFinality => FederationNodeMode::LocalFinality,
        }
    }
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
    let PreparedFederationNode {
        node,
        services: mut federation_services,
        serves_peer_api,
    } = FederationNode::prepare(Arc::clone(&mint), federation_config, startup.node_mode()).await?;
    let state = MintState {
        mint,
        cache: Arc::new(cache),
        #[cfg(feature = "federation")]
        node,
    };
    #[cfg(feature = "federation")]
    if matches!(startup, FederationRouterStartup::Setup) {
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
    if serves_peer_api {
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
