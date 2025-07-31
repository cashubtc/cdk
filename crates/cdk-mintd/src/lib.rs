//! Cdk mintd lib

// std
#[cfg(feature = "auth")]
use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

// external crates
use anyhow::{anyhow, bail, Result};
use axum::Router;
use bip39::Mnemonic;
// internal crate modules
use cdk::cdk_database::{self, MintDatabase, MintKeysDatabase};
use cdk::cdk_payment;
use cdk::cdk_payment::MintPayment;
use cdk::mint::{Mint, MintBuilder, MintMeltLimits};
#[cfg(any(
    feature = "cln",
    feature = "lnbits",
    feature = "lnd",
    feature = "fakewallet",
    feature = "grpc-processor"
))]
use cdk::nuts::nut17::SupportedMethods;
use cdk::nuts::nut19::{CachedEndpoint, Method as NUT19Method, Path as NUT19Path};
#[cfg(any(
    feature = "cln",
    feature = "lnbits",
    feature = "lnd",
    feature = "fakewallet"
))]
use cdk::nuts::CurrencyUnit;
#[cfg(feature = "auth")]
use cdk::nuts::{AuthRequired, Method, ProtectedEndpoint, RoutePath};
use cdk::nuts::{ContactInfo, MintVersion, PaymentMethod};
use cdk::types::QuoteTTL;
use cdk_axum::cache::HttpCache;
#[cfg(feature = "auth")]
use cdk_sqlite::mint::MintSqliteAuthDatabase;
use cdk_sqlite::MintSqliteDatabase;
use cli::CLIArgs;
use config::{DatabaseEngine, LnBackend};
use env_vars::ENV_WORK_DIR;
use setup::LnBackendSetup;
use tower::ServiceBuilder;
use tower_http::compression::CompressionLayer;
use tower_http::decompression::RequestDecompressionLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;
#[cfg(feature = "swagger")]
use utoipa::OpenApi;

pub mod cli;
pub mod config;
pub mod env_vars;
pub mod setup;

const CARGO_PKG_VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");

#[cfg(feature = "cln")]
fn expand_path(path: &str) -> Option<PathBuf> {
    if path.starts_with('~') {
        if let Some(home_dir) = home::home_dir().as_mut() {
            let remainder = &path[2..];
            home_dir.push(remainder);
            let expanded_path = home_dir;
            Some(expanded_path.clone())
        } else {
            None
        }
    } else {
        Some(PathBuf::from(path))
    }
}

/// Performs the initial setup for the application, including configuring tracing,
/// parsing CLI arguments, setting up the working directory, loading settings,
/// and initializing the database connection.
async fn initial_setup(
    work_dir: &Path,
    settings: &config::Settings,
    db_password: Option<String>,
) -> Result<(
    Arc<dyn MintDatabase<cdk_database::Error> + Send + Sync>,
    Arc<dyn MintKeysDatabase<Err = cdk_database::Error> + Send + Sync>,
)> {
    let (localstore, keystore) = setup_database(settings, work_dir, db_password).await?;
    Ok((localstore, keystore))
}

/// Sets up and initializes a tracing subscriber with custom log filtering.
pub fn setup_tracing() {
    let default_filter = "debug";
    let hyper_filter = "hyper=warn";
    let h2_filter = "h2=warn";
    let tower_http = "tower_http=warn";

    let env_filter = EnvFilter::new(format!(
        "{default_filter},{hyper_filter},{h2_filter},{tower_http}"
    ));

    tracing_subscriber::fmt().with_env_filter(env_filter).init();
}

/// Retrieves the work directory based on command-line arguments, environment variables, or system defaults.
pub async fn get_work_directory(args: &CLIArgs) -> Result<PathBuf> {
    let work_dir = if let Some(work_dir) = &args.work_dir {
        tracing::info!("Using work dir from cmd arg");
        work_dir.clone()
    } else if let Ok(env_work_dir) = env::var(ENV_WORK_DIR) {
        tracing::info!("Using work dir from env var");
        env_work_dir.into()
    } else {
        work_dir()?
    };
    tracing::info!("Using work dir: {}", work_dir.display());
    Ok(work_dir)
}

/// Loads the application settings based on a configuration file and environment variables.
pub fn load_settings(work_dir: &Path, config_path: Option<PathBuf>) -> Result<config::Settings> {
    // get config file name from args
    let config_file_arg = match config_path {
        Some(c) => c,
        None => work_dir.join("config.toml"),
    };

    let mut settings = if config_file_arg.exists() {
        config::Settings::new(Some(config_file_arg))
    } else {
        tracing::info!("Config file does not exist. Attempting to read env vars");
        config::Settings::default()
    };

    // This check for any settings defined in ENV VARs
    // ENV VARS will take **priority** over those in the config
    settings.from_env()
}

async fn setup_database(
    settings: &config::Settings,
    work_dir: &Path,
    db_password: Option<String>,
) -> Result<(
    Arc<dyn MintDatabase<cdk_database::Error> + Send + Sync>,
    Arc<dyn MintKeysDatabase<Err = cdk_database::Error> + Send + Sync>,
)> {
    match settings.database.engine {
        DatabaseEngine::Sqlite => {
            let db = setup_sqlite_database(work_dir, db_password).await?;
            let localstore: Arc<dyn MintDatabase<cdk_database::Error> + Send + Sync> = db.clone();
            let keystore: Arc<dyn MintKeysDatabase<Err = cdk_database::Error> + Send + Sync> = db;
            Ok((localstore, keystore))
        }
    }
}

async fn setup_sqlite_database(
    work_dir: &Path,
    _password: Option<String>,
) -> Result<Arc<MintSqliteDatabase>> {
    let sql_db_path = work_dir.join("cdk-mintd.sqlite");

    #[cfg(not(feature = "sqlcipher"))]
    let db = MintSqliteDatabase::new(&sql_db_path).await?;
    #[cfg(feature = "sqlcipher")]
    let db = {
        // Get password from command line arguments for sqlcipher
        MintSqliteDatabase::new((sql_db_path, _password.unwrap())).await?
    };

    Ok(Arc::new(db))
}

/**
 * Configures a `MintBuilder` instance with provided settings and initializes
 * routers for Lightning Network backends.
 */
async fn configure_mint_builder(
    settings: &config::Settings,
    mint_builder: MintBuilder,
) -> Result<(MintBuilder, Vec<Router>)> {
    let mut ln_routers = vec![];

    // Configure basic mint information
    let mint_builder = configure_basic_info(settings, mint_builder);

    // Configure lightning backend
    let mint_builder = configure_lightning_backend(settings, mint_builder, &mut ln_routers).await?;

    // Configure caching
    let mint_builder = configure_cache(settings, mint_builder);

    Ok((mint_builder, ln_routers))
}

/// Configures basic mint information (name, contact info, descriptions, etc.)
fn configure_basic_info(settings: &config::Settings, mint_builder: MintBuilder) -> MintBuilder {
    // Add contact information
    let mut contacts = Vec::new();
    if let Some(nostr_key) = &settings.mint_info.contact_nostr_public_key {
        contacts.push(ContactInfo::new("nostr".to_string(), nostr_key.to_string()));
    }
    if let Some(email) = &settings.mint_info.contact_email {
        contacts.push(ContactInfo::new("email".to_string(), email.to_string()));
    }

    // Add version information
    let mint_version = MintVersion::new(
        "cdk-mintd".to_string(),
        CARGO_PKG_VERSION.unwrap_or("Unknown").to_string(),
    );

    // Configure mint builder with basic info
    let mut builder = mint_builder
        .with_name(settings.mint_info.name.clone())
        .with_version(mint_version)
        .with_description(settings.mint_info.description.clone());

    // Add optional information
    if let Some(long_description) = &settings.mint_info.description_long {
        builder = builder.with_long_description(long_description.to_string());
    }

    for contact in contacts {
        builder = builder.with_contact_info(contact);
    }

    if let Some(pubkey) = settings.mint_info.pubkey {
        builder = builder.with_pubkey(pubkey);
    }

    if let Some(icon_url) = &settings.mint_info.icon_url {
        builder = builder.with_icon_url(icon_url.to_string());
    }

    if let Some(motd) = &settings.mint_info.motd {
        builder = builder.with_motd(motd.to_string());
    }

    if let Some(tos_url) = &settings.mint_info.tos_url {
        builder = builder.with_tos_url(tos_url.to_string());
    }

    builder
}
/// Configures Lightning Network backend based on the specified backend type
async fn configure_lightning_backend(
    settings: &config::Settings,
    mut mint_builder: MintBuilder,
    ln_routers: &mut Vec<Router>,
) -> Result<MintBuilder> {
    let mint_melt_limits = MintMeltLimits {
        mint_min: settings.ln.min_mint,
        mint_max: settings.ln.max_mint,
        melt_min: settings.ln.min_melt,
        melt_max: settings.ln.max_melt,
    };

    tracing::debug!("Ln backend: {:?}", settings.ln.ln_backend);

    match settings.ln.ln_backend {
        #[cfg(feature = "cln")]
        LnBackend::Cln => {
            let cln_settings = settings
                .cln
                .clone()
                .expect("Config checked at load that cln is some");
            let cln = cln_settings
                .setup(ln_routers, settings, CurrencyUnit::Msat)
                .await?;

            mint_builder = configure_backend_for_unit(
                settings,
                mint_builder,
                CurrencyUnit::Sat,
                mint_melt_limits,
                Arc::new(cln),
            )
            .await?;
        }
        #[cfg(feature = "lnbits")]
        LnBackend::LNbits => {
            let lnbits_settings = settings.clone().lnbits.expect("Checked on config load");
            let lnbits = lnbits_settings
                .setup(ln_routers, settings, CurrencyUnit::Sat)
                .await?;

            mint_builder = configure_backend_for_unit(
                settings,
                mint_builder,
                CurrencyUnit::Sat,
                mint_melt_limits,
                Arc::new(lnbits),
            )
            .await?;
        }
        #[cfg(feature = "lnd")]
        LnBackend::Lnd => {
            let lnd_settings = settings.clone().lnd.expect("Checked at config load");
            let lnd = lnd_settings
                .setup(ln_routers, settings, CurrencyUnit::Msat)
                .await?;

            mint_builder = configure_backend_for_unit(
                settings,
                mint_builder,
                CurrencyUnit::Sat,
                mint_melt_limits,
                Arc::new(lnd),
            )
            .await?;
        }
        #[cfg(feature = "fakewallet")]
        LnBackend::FakeWallet => {
            let fake_wallet = settings.clone().fake_wallet.expect("Fake wallet defined");
            tracing::info!("Using fake wallet: {:?}", fake_wallet);

            for unit in fake_wallet.clone().supported_units {
                let fake = fake_wallet
                    .setup(ln_routers, settings, CurrencyUnit::Sat)
                    .await?;

                mint_builder = configure_backend_for_unit(
                    settings,
                    mint_builder,
                    unit.clone(),
                    mint_melt_limits,
                    Arc::new(fake),
                )
                .await?;
            }
        }
        #[cfg(feature = "grpc-processor")]
        LnBackend::GrpcProcessor => {
            let grpc_processor = settings
                .clone()
                .grpc_processor
                .expect("grpc processor config defined");

            tracing::info!(
                "Attempting to start with gRPC payment processor at {}:{}.",
                grpc_processor.addr,
                grpc_processor.port
            );

            for unit in grpc_processor.clone().supported_units {
                tracing::debug!("Adding unit: {:?}", unit);
                let processor = grpc_processor
                    .setup(ln_routers, settings, unit.clone())
                    .await?;

                mint_builder = configure_backend_for_unit(
                    settings,
                    mint_builder,
                    unit.clone(),
                    mint_melt_limits,
                    Arc::new(processor),
                )
                .await?;
            }
        }
        LnBackend::None => {
            tracing::error!(
                "Payment backend was not set or feature disabled. {:?}",
                settings.ln.ln_backend
            );
            bail!("Lightning backend must be configured");
        }
    };

    Ok(mint_builder)
}

/// Helper function to configure a mint builder with a lightning backend for a specific currency unit
async fn configure_backend_for_unit(
    settings: &config::Settings,
    mut mint_builder: MintBuilder,
    unit: cdk::nuts::CurrencyUnit,
    mint_melt_limits: MintMeltLimits,
    backend: Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>,
) -> Result<MintBuilder> {
    let payment_settings = backend.get_settings().await?;

    if let Some(bolt12) = payment_settings.get("bolt12") {
        if bolt12.as_bool().unwrap_or_default() {
            mint_builder
                .add_payment_processor(
                    unit.clone(),
                    PaymentMethod::Bolt12,
                    mint_melt_limits,
                    Arc::clone(&backend),
                )
                .await?;
        }
    }

    mint_builder
        .add_payment_processor(
            unit.clone(),
            PaymentMethod::Bolt11,
            mint_melt_limits,
            backend,
        )
        .await?;

    if let Some(input_fee) = settings.info.input_fee_ppk {
        mint_builder.set_unit_fee(&unit, input_fee)?;
    }

    let nut17_supported = SupportedMethods::default_bolt11(unit);
    mint_builder = mint_builder.with_supported_websockets(nut17_supported);

    Ok(mint_builder)
}

/// Configures cache settings
fn configure_cache(settings: &config::Settings, mint_builder: MintBuilder) -> MintBuilder {
    let cached_endpoints = vec![
        CachedEndpoint::new(NUT19Method::Post, NUT19Path::MintBolt11),
        CachedEndpoint::new(NUT19Method::Post, NUT19Path::MeltBolt11),
        CachedEndpoint::new(NUT19Method::Post, NUT19Path::Swap),
    ];

    let cache: HttpCache = settings.info.http_cache.clone().into();
    mint_builder.with_cache(Some(cache.ttl.as_secs()), cached_endpoints)
}

#[cfg(feature = "auth")]
async fn setup_authentication(
    settings: &config::Settings,
    work_dir: &Path,
    mut mint_builder: MintBuilder,
    _password: Option<String>,
) -> Result<MintBuilder> {
    if let Some(auth_settings) = settings.auth.clone() {
        tracing::info!("Auth settings are defined. {:?}", auth_settings);
        let auth_localstore: Arc<
            dyn cdk_database::MintAuthDatabase<Err = cdk_database::Error> + Send + Sync,
        > = match settings.database.engine {
            DatabaseEngine::Sqlite => {
                let sql_db_path = work_dir.join("cdk-mintd-auth.sqlite");
                #[cfg(not(feature = "sqlcipher"))]
                let sqlite_db = MintSqliteAuthDatabase::new(&sql_db_path).await?;
                #[cfg(feature = "sqlcipher")]
                let sqlite_db = {
                    // Get password from command line arguments for sqlcipher
                    MintSqliteAuthDatabase::new((sql_db_path, _password.unwrap())).await?
                };

                Arc::new(sqlite_db)
            }
        };

        let mint_blind_auth_endpoint =
            ProtectedEndpoint::new(Method::Post, RoutePath::MintBlindAuth);

        let mut protected_endpoints = HashMap::new();

        protected_endpoints.insert(mint_blind_auth_endpoint, AuthRequired::Clear);

        let mut blind_auth_endpoints = vec![];
        let mut unprotected_endpoints = vec![];

        {
            let mint_quote_protected_endpoint =
                ProtectedEndpoint::new(Method::Post, RoutePath::MintQuoteBolt11);
            let mint_protected_endpoint =
                ProtectedEndpoint::new(Method::Post, RoutePath::MintBolt11);
            if auth_settings.enabled_mint {
                protected_endpoints.insert(mint_quote_protected_endpoint, AuthRequired::Blind);

                protected_endpoints.insert(mint_protected_endpoint, AuthRequired::Blind);

                blind_auth_endpoints.push(mint_quote_protected_endpoint);
                blind_auth_endpoints.push(mint_protected_endpoint);
            } else {
                unprotected_endpoints.push(mint_protected_endpoint);
                unprotected_endpoints.push(mint_quote_protected_endpoint);
            }
        }

        {
            let melt_quote_protected_endpoint =
                ProtectedEndpoint::new(Method::Post, RoutePath::MeltQuoteBolt11);
            let melt_protected_endpoint =
                ProtectedEndpoint::new(Method::Post, RoutePath::MeltBolt11);

            if auth_settings.enabled_melt {
                protected_endpoints.insert(melt_quote_protected_endpoint, AuthRequired::Blind);
                protected_endpoints.insert(melt_protected_endpoint, AuthRequired::Blind);

                blind_auth_endpoints.push(melt_quote_protected_endpoint);
                blind_auth_endpoints.push(melt_protected_endpoint);
            } else {
                unprotected_endpoints.push(melt_quote_protected_endpoint);
                unprotected_endpoints.push(melt_protected_endpoint);
            }
        }

        {
            let swap_protected_endpoint = ProtectedEndpoint::new(Method::Post, RoutePath::Swap);

            if auth_settings.enabled_swap {
                protected_endpoints.insert(swap_protected_endpoint, AuthRequired::Blind);
                blind_auth_endpoints.push(swap_protected_endpoint);
            } else {
                unprotected_endpoints.push(swap_protected_endpoint);
            }
        }

        {
            let check_mint_protected_endpoint =
                ProtectedEndpoint::new(Method::Get, RoutePath::MintQuoteBolt11);

            if auth_settings.enabled_check_mint_quote {
                protected_endpoints.insert(check_mint_protected_endpoint, AuthRequired::Blind);
                blind_auth_endpoints.push(check_mint_protected_endpoint);
            } else {
                unprotected_endpoints.push(check_mint_protected_endpoint);
            }
        }

        {
            let check_melt_protected_endpoint =
                ProtectedEndpoint::new(Method::Get, RoutePath::MeltQuoteBolt11);

            if auth_settings.enabled_check_melt_quote {
                protected_endpoints.insert(check_melt_protected_endpoint, AuthRequired::Blind);
                blind_auth_endpoints.push(check_melt_protected_endpoint);
            } else {
                unprotected_endpoints.push(check_melt_protected_endpoint);
            }
        }

        {
            let restore_protected_endpoint =
                ProtectedEndpoint::new(Method::Post, RoutePath::Restore);

            if auth_settings.enabled_restore {
                protected_endpoints.insert(restore_protected_endpoint, AuthRequired::Blind);
                blind_auth_endpoints.push(restore_protected_endpoint);
            } else {
                unprotected_endpoints.push(restore_protected_endpoint);
            }
        }

        {
            let state_protected_endpoint =
                ProtectedEndpoint::new(Method::Post, RoutePath::Checkstate);

            if auth_settings.enabled_check_proof_state {
                protected_endpoints.insert(state_protected_endpoint, AuthRequired::Blind);
                blind_auth_endpoints.push(state_protected_endpoint);
            } else {
                unprotected_endpoints.push(state_protected_endpoint);
            }
        }

        mint_builder = mint_builder.with_auth(
            auth_localstore.clone(),
            auth_settings.openid_discovery,
            auth_settings.openid_client_id,
            vec![mint_blind_auth_endpoint],
        );
        mint_builder =
            mint_builder.with_blind_auth(auth_settings.mint_max_bat, blind_auth_endpoints);

        let mut tx = auth_localstore.begin_transaction().await?;

        tx.remove_protected_endpoints(unprotected_endpoints).await?;
        tx.add_protected_endpoints(protected_endpoints).await?;
        tx.commit().await?;
    }
    Ok(mint_builder)
}

/// Build mints with the configured the signing method (remote signatory or local seed)
async fn build_mint(
    settings: &config::Settings,
    keystore: Arc<dyn MintKeysDatabase<Err = cdk_database::Error> + Send + Sync>,
    mint_builder: MintBuilder,
) -> Result<Mint> {
    if let Some(signatory_url) = settings.info.signatory_url.clone() {
        tracing::info!(
            "Connecting to remote signatory to {} with certs {:?}",
            signatory_url,
            settings.info.signatory_certs.clone()
        );

        Ok(mint_builder
            .build_with_signatory(Arc::new(
                cdk_signatory::SignatoryRpcClient::new(
                    signatory_url,
                    settings.info.signatory_certs.clone(),
                )
                .await?,
            ))
            .await?)
    } else if let Some(mnemonic) = settings
        .info
        .mnemonic
        .clone()
        .map(|s| Mnemonic::from_str(&s))
        .transpose()?
    {
        Ok(mint_builder
            .build_with_seed(keystore, &mnemonic.to_seed_normalized(""))
            .await?)
    } else {
        bail!("No seed nor remote signatory set");
    }
}

async fn start_services_with_shutdown(
    mint: Arc<cdk::mint::Mint>,
    settings: &config::Settings,
    ln_routers: Vec<Router>,
    work_dir: &Path,
    mint_builder_info: cdk::nuts::MintInfo,
    shutdown_signal: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let listen_addr = settings.info.listen_host.clone();
    let listen_port = settings.info.listen_port;
    let cache: HttpCache = settings.info.http_cache.clone().into();

    #[cfg(feature = "management-rpc")]
    let mut rpc_enabled = false;
    #[cfg(not(feature = "management-rpc"))]
    let rpc_enabled = false;

    #[cfg(feature = "management-rpc")]
    let mut rpc_server: Option<cdk_mint_rpc::MintRPCServer> = None;

    #[cfg(feature = "management-rpc")]
    {
        if let Some(rpc_settings) = settings.mint_management_rpc.clone() {
            if rpc_settings.enabled {
                let addr = rpc_settings.address.unwrap_or("127.0.0.1".to_string());
                let port = rpc_settings.port.unwrap_or(8086);
                let mut mint_rpc = cdk_mint_rpc::MintRPCServer::new(&addr, port, mint.clone())?;

                let tls_dir = rpc_settings.tls_dir_path.unwrap_or(work_dir.join("tls"));

                if !tls_dir.exists() {
                    tracing::error!("TLS directory does not exist: {}", tls_dir.display());
                    bail!("Cannot start RPC server: TLS directory does not exist");
                }

                mint_rpc.start(Some(tls_dir)).await?;

                rpc_server = Some(mint_rpc);

                rpc_enabled = true;
            }
        }
    }

    if rpc_enabled {
        if mint.mint_info().await.is_err() {
            tracing::info!("Mint info not set on mint, setting.");
            mint.set_mint_info(mint_builder_info).await?;
            mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000)).await?;
        } else {
            if mint.localstore().get_quote_ttl().await.is_err() {
                mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000)).await?;
            }
            // Add version information
            let mint_version = MintVersion::new(
                "cdk-mintd".to_string(),
                CARGO_PKG_VERSION.unwrap_or("Unknown").to_string(),
            );
            let mut stored_mint_info = mint.mint_info().await?;
            stored_mint_info.version = Some(mint_version);
            mint.set_mint_info(stored_mint_info).await?;

            tracing::info!("Mint info already set, not using config file settings.");
        }
    } else {
        tracing::warn!("RPC not enabled, using mint info from config.");
        mint.set_mint_info(mint_builder_info).await?;
        mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000)).await?;
    }

    let mint_info = mint.mint_info().await?;
    let nut04_methods = mint_info.nuts.nut04.supported_methods();
    let nut05_methods = mint_info.nuts.nut05.supported_methods();

    let bolt12_supported = nut04_methods.contains(&&PaymentMethod::Bolt12)
        || nut05_methods.contains(&&PaymentMethod::Bolt12);

    let v1_service =
        cdk_axum::create_mint_router_with_custom_cache(Arc::clone(&mint), cache, bolt12_supported)
            .await?;

    let mut mint_service = Router::new()
        .merge(v1_service)
        .layer(
            ServiceBuilder::new()
                .layer(RequestDecompressionLayer::new())
                .layer(CompressionLayer::new()),
        )
        .layer(TraceLayer::new_for_http());

    #[cfg(feature = "swagger")]
    {
        if settings.info.enable_swagger_ui.unwrap_or(false) {
            mint_service = mint_service.merge(
                utoipa_swagger_ui::SwaggerUi::new("/swagger-ui")
                    .url("/api-docs/openapi.json", cdk_axum::ApiDoc::openapi()),
            );
        }
    }

    for router in ln_routers {
        mint_service = mint_service.merge(router);
    }

    mint.start().await?;

    let socket_addr = SocketAddr::from_str(&format!("{listen_addr}:{listen_port}"))?;

    let listener = tokio::net::TcpListener::bind(socket_addr).await?;

    tracing::debug!("listening on {}", listener.local_addr().unwrap());

    // Wait for axum server to complete with custom shutdown signal
    let axum_result = axum::serve(listener, mint_service).with_graceful_shutdown(shutdown_signal);

    match axum_result.await {
        Ok(_) => {
            tracing::info!("Axum server stopped with okay status");
        }
        Err(err) => {
            tracing::warn!("Axum server stopped with error");
            tracing::error!("{}", err);
            bail!("Axum exited with error")
        }
    }

    mint.stop().await?;

    #[cfg(feature = "management-rpc")]
    {
        if let Some(rpc_server) = rpc_server {
            rpc_server.stop().await?;
        }
    }

    Ok(())
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C handler");
    tracing::info!("Shutdown signal received");
}

fn work_dir() -> Result<PathBuf> {
    let home_dir = home::home_dir().ok_or(anyhow!("Unknown home dir"))?;
    let dir = home_dir.join(".cdk-mintd");

    std::fs::create_dir_all(&dir)?;

    Ok(dir)
}

/// The main entry point for the application when used as a library.
///
/// This asynchronous function performs the following steps:
/// 1. Executes the initial setup, including loading configurations and initializing the database.
/// 2. Configures a `MintBuilder` instance with the local store and keystore based on the database.
/// 3. Applies additional custom configurations and authentication setup for the `MintBuilder`.
/// 4. Constructs a `Mint` instance from the configured `MintBuilder`.
/// 5. Checks and resolves the status of any pending mint and melt quotes.
pub async fn run_mintd(
    work_dir: &Path,
    settings: &config::Settings,
    db_password: Option<String>,
) -> Result<()> {
    run_mintd_with_shutdown(work_dir, settings, shutdown_signal(), db_password).await
}

/// Run mintd with a custom shutdown signal
pub async fn run_mintd_with_shutdown(
    work_dir: &Path,
    settings: &config::Settings,
    shutdown_signal: impl std::future::Future<Output = ()> + Send + 'static,
    db_password: Option<String>,
) -> Result<()> {
    let (localstore, keystore) = initial_setup(work_dir, settings, db_password.clone()).await?;

    let mint_builder = MintBuilder::new(localstore);

    let (mint_builder, ln_routers) = configure_mint_builder(settings, mint_builder).await?;
    #[cfg(feature = "auth")]
    let mint_builder = setup_authentication(settings, work_dir, mint_builder, db_password).await?;

    let mint = build_mint(settings, keystore, mint_builder).await?;

    tracing::debug!("Mint built from builder.");

    let mint = Arc::new(mint);

    // Checks the status of all pending melt quotes
    // Pending melt quotes where the payment has gone through inputs are burnt
    // Pending melt quotes where the payment has **failed** inputs are reset to unspent
    mint.check_pending_melt_quotes().await?;

    start_services_with_shutdown(
        mint.clone(),
        settings,
        ln_routers,
        work_dir,
        mint.mint_info().await?,
        shutdown_signal,
    )
    .await?;

    Ok(())
}
