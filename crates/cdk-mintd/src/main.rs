//! CDK MINTD
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

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
use cdk::mint::{MintBuilder, MintMeltLimits};
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
use cdk_mintd::cli::CLIArgs;
use cdk_mintd::config::{self, DatabaseEngine, LnBackend};
use cdk_mintd::env_vars::ENV_WORK_DIR;
use cdk_mintd::setup::LnBackendSetup;
#[cfg(feature = "redb")]
use cdk_redb::mint::MintRedbAuthDatabase;
#[cfg(feature = "redb")]
use cdk_redb::MintRedbDatabase;
#[cfg(feature = "auth")]
use cdk_sqlite::mint::MintSqliteAuthDatabase;
use cdk_sqlite::MintSqliteDatabase;
use clap::Parser;
use tokio::sync::Notify;
use tower::ServiceBuilder;
use tower_http::compression::CompressionLayer;
use tower_http::decompression::RequestDecompressionLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;
#[cfg(feature = "swagger")]
use utoipa::OpenApi;

const CARGO_PKG_VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");

// Ensure at least one lightning backend is enabled at compile time
#[cfg(not(any(
    feature = "cln",
    feature = "lnbits",
    feature = "lnd",
    feature = "fakewallet",
    feature = "grpc-processor"
)))]
compile_error!(
    "At least one lightning backend feature must be enabled: cln, lnbits, lnd, fakewallet, or grpc-processor"
);

/// The main entry point for the application.
///
/// This asynchronous function performs the following steps:
/// 1. Executes the initial setup, including loading configurations and initializing the database.
/// 2. Configures a `MintBuilder` instance with the local store and keystore based on the database.
/// 3. Applies additional custom configurations and authentication setup for the `MintBuilder`.
/// 4. Constructs a `Mint` instance from the configured `MintBuilder`.
/// 5. Checks and resolves the status of any pending mint and melt quotes.
#[tokio::main]
async fn main() -> Result<()> {
    let (work_dir, settings, db) = initial_setup().await?;
    let mint_builder = MintBuilder::new()
        .with_localstore(db.clone())
        .with_keystore(db.clone());

    let (mint_builder, ln_routers) = configure_mint_builder(&settings, mint_builder).await?;
    #[cfg(feature = "auth")]
    let mint_builder = setup_authentication(&settings, &work_dir, mint_builder).await?;
    let mint_builder_info = mint_builder.mint_info.clone();

    let mint = mint_builder.build().await?;

    tracing::debug!("Mint built from builder.");

    let mint = Arc::new(mint);

    // Check the status of any mint quotes that are pending
    // In the event that the mint server is down but the ln node is not
    // it is possible that a mint quote was paid but the mint has not been updated
    // this will check and update the mint state of those quotes
    mint.check_pending_mint_quotes().await?;

    // Checks the status of all pending melt quotes
    // Pending melt quotes where the payment has gone through inputs are burnt
    // Pending melt quotes where the payment has **failed** inputs are reset to unspent
    mint.check_pending_melt_quotes().await?;

    #[cfg(feature = "management-rpc")]
    let (shutdown, rpc_server_option) = start_services(
        mint.clone(),
        &settings,
        ln_routers,
        &work_dir,
        mint_builder_info,
    )
    .await?;
    #[cfg(not(feature = "management-rpc"))]
    let (shutdown, _rpc_server_option) = start_services(
        mint.clone(),
        &settings,
        ln_routers,
        &work_dir,
        mint_builder_info,
    )
    .await?;

    // Wait for shutdown signal
    shutdown.notified().await;

    // Stop RPC server if it was started
    #[cfg(feature = "management-rpc")]
    {
        if let Some(rpc_server) = rpc_server_option {
            if let Some(server) = rpc_server.downcast_ref::<cdk_mint_rpc::MintRPCServer>() {
                server.stop().await?;
            }
        }
    }

    Ok(())
}

trait MintCombinedDatabase:
    MintDatabase<cdk_database::Error> + MintKeysDatabase<Err = cdk_database::Error>
{
}

// Implement the combined trait
impl<T> MintCombinedDatabase for T where
    T: MintDatabase<cdk_database::Error> + MintKeysDatabase<Err = cdk_database::Error>
{
}

/// Performs the initial setup for the application, including configuring tracing,
/// parsing CLI arguments, setting up the working directory, loading settings,
/// and initializing the database connection.
async fn initial_setup() -> Result<(
    PathBuf,
    config::Settings,
    Arc<dyn MintCombinedDatabase + Send + Sync>,
)> {
    setup_tracing();
    let args = CLIArgs::parse();
    let work_dir = get_work_directory(&args).await?;

    let settings = load_settings(&work_dir, args.config)?;
    let db = setup_database(&settings, &work_dir).await?;
    Ok((work_dir, settings, db.clone()))
}

/// Sets up and initializes a tracing subscriber with custom log filtering.
fn setup_tracing() {
    let default_filter = "debug";
    let sqlx_filter = "sqlx=warn";
    let hyper_filter = "hyper=warn";
    let h2_filter = "h2=warn";
    let tower_http = "tower_http=warn";

    let env_filter = EnvFilter::new(format!(
        "{default_filter},{sqlx_filter},{hyper_filter},{h2_filter},{tower_http}"
    ));

    tracing_subscriber::fmt().with_env_filter(env_filter).init();
}

async fn get_work_directory(args: &CLIArgs) -> Result<PathBuf> {
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

fn load_settings(work_dir: &Path, config_path: Option<PathBuf>) -> Result<config::Settings> {
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
) -> Result<Arc<dyn MintCombinedDatabase + Send + Sync>> {
    match settings.database.engine {
        DatabaseEngine::Sqlite => Ok(setup_sqlite_database(work_dir).await?),
        #[cfg(feature = "redb")]
        DatabaseEngine::Redb => Ok(setup_redb_database(work_dir).await?),
    }
}

#[cfg(feature = "redb")]
async fn setup_redb_database(
    work_dir: &Path,
) -> Result<Arc<dyn MintCombinedDatabase + Send + Sync>> {
    let redb_path = work_dir.join("cdk-mintd.redb");
    Ok(Arc::new(MintRedbDatabase::new(&redb_path)?))
}

async fn setup_sqlite_database(
    work_dir: &Path,
) -> Result<Arc<dyn MintCombinedDatabase + Send + Sync>> {
    let sql_db_path = work_dir.join("cdk-mintd.sqlite");
    #[cfg(not(feature = "sqlcipher"))]
    let db = MintSqliteDatabase::new(&sql_db_path).await?;
    #[cfg(feature = "sqlcipher")]
    let db = MintSqliteDatabase::new(&sql_db_path, args.password).await?;
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

    // Configure signatory or seed
    let mint_builder = configure_signing_method(settings, mint_builder).await?;

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
        builder = builder.add_contact_info(contact);
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
    mint_builder = mint_builder
        .add_ln_backend(
            unit.clone(),
            PaymentMethod::Bolt11,
            mint_melt_limits,
            backend,
        )
        .await?;

    if let Some(input_fee) = settings.info.input_fee_ppk {
        mint_builder = mint_builder.set_unit_fee(&unit, input_fee)?;
    }

    let nut17_supported = SupportedMethods::default_bolt11(unit);
    mint_builder = mint_builder.add_supported_websockets(nut17_supported);

    Ok(mint_builder)
}

/// Configures the signing method (remote signatory or local seed)
async fn configure_signing_method(
    settings: &config::Settings,
    mint_builder: MintBuilder,
) -> Result<MintBuilder> {
    if let Some(signatory_url) = settings.info.signatory_url.clone() {
        tracing::info!(
            "Connecting to remote signatory to {} with certs {:?}",
            signatory_url,
            settings.info.signatory_certs.clone()
        );

        Ok(mint_builder.with_signatory(Arc::new(
            cdk_signatory::SignatoryRpcClient::new(
                signatory_url,
                settings.info.signatory_certs.clone(),
            )
            .await?,
        )))
    } else if let Some(mnemonic) = settings
        .info
        .mnemonic
        .clone()
        .map(|s| Mnemonic::from_str(&s))
        .transpose()?
    {
        Ok(mint_builder.with_seed(mnemonic.to_seed_normalized("").to_vec()))
    } else {
        bail!("No seed nor remote signatory set");
    }
}

/// Configures cache settings
fn configure_cache(settings: &config::Settings, mint_builder: MintBuilder) -> MintBuilder {
    let cached_endpoints = vec![
        CachedEndpoint::new(NUT19Method::Post, NUT19Path::MintBolt11),
        CachedEndpoint::new(NUT19Method::Post, NUT19Path::MeltBolt11),
        CachedEndpoint::new(NUT19Method::Post, NUT19Path::Swap),
    ];

    let cache: HttpCache = settings.info.http_cache.clone().into();
    mint_builder.add_cache(Some(cache.ttl.as_secs()), cached_endpoints)
}

#[cfg(feature = "auth")]
async fn setup_authentication(
    settings: &config::Settings,
    work_dir: &Path,
    mut mint_builder: MintBuilder,
) -> Result<MintBuilder> {
    if let Some(auth_settings) = settings.auth.clone() {
        tracing::info!("Auth settings are defined. {:?}", auth_settings);
        let auth_localstore: Arc<
            dyn cdk_database::MintAuthDatabase<Err = cdk_database::Error> + Send + Sync,
        > = match settings.database.engine {
            DatabaseEngine::Sqlite => {
                let sql_db_path = work_dir.join("cdk-mintd-auth.sqlite");
                let sqlite_db = MintSqliteAuthDatabase::new(&sql_db_path).await?;

                sqlite_db.migrate().await;

                Arc::new(sqlite_db)
            }
            #[cfg(feature = "redb")]
            DatabaseEngine::Redb => {
                let redb_path = work_dir.join("cdk-mintd-auth.redb");
                Arc::new(MintRedbAuthDatabase::new(&redb_path)?)
            }
        };

        mint_builder = mint_builder.with_auth_localstore(auth_localstore.clone());

        let mint_blind_auth_endpoint =
            ProtectedEndpoint::new(Method::Post, RoutePath::MintBlindAuth);

        mint_builder = mint_builder.set_clear_auth_settings(
            auth_settings.openid_discovery,
            auth_settings.openid_client_id,
        );

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

        mint_builder = mint_builder.set_blind_auth_settings(auth_settings.mint_max_bat);

        auth_localstore
            .remove_protected_endpoints(unprotected_endpoints)
            .await?;
        auth_localstore
            .add_protected_endpoints(protected_endpoints)
            .await?;
    }
    Ok(mint_builder)
}

async fn start_services(
    mint: Arc<cdk::mint::Mint>,
    settings: &config::Settings,
    ln_routers: Vec<Router>,
    _work_dir: &Path,
    mint_builder_info: cdk::nuts::MintInfo,
) -> Result<(Arc<Notify>, Option<Box<dyn std::any::Any + Send + Sync>>)> {
    let listen_addr = settings.info.listen_host.clone();
    let listen_port = settings.info.listen_port;
    let cache: HttpCache = settings.info.http_cache.clone().into();

    let v1_service =
        cdk_axum::create_mint_router_with_custom_cache(Arc::clone(&mint), cache).await?;

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
                    .url("/api-docs/openapi.json", cdk_axum::ApiDocV1::openapi()),
            );
        }
    }

    for router in ln_routers {
        mint_service = mint_service.merge(router);
    }

    let shutdown = Arc::new(Notify::new());
    let mint_clone = Arc::clone(&mint);
    tokio::spawn({
        let shutdown = Arc::clone(&shutdown);
        async move { mint_clone.wait_for_paid_invoices(shutdown).await }
    });

    #[cfg(feature = "management-rpc")]
    let mut rpc_enabled = false;
    #[cfg(not(feature = "management-rpc"))]
    let rpc_enabled = false;

    // Initialize rpc_server_option based on feature flag
    #[cfg(feature = "management-rpc")]
    let mut rpc_server_option: Option<Box<dyn std::any::Any + Send + Sync>> = None;
    #[cfg(not(feature = "management-rpc"))]
    let rpc_server_option: Option<Box<dyn std::any::Any + Send + Sync>> = None;

    #[cfg(feature = "management-rpc")]
    {
        if let Some(rpc_settings) = settings.mint_management_rpc.clone() {
            if rpc_settings.enabled {
                let addr = rpc_settings.address.unwrap_or("127.0.0.1".to_string());
                let port = rpc_settings.port.unwrap_or(8086);
                let mut mint_rpc = cdk_mint_rpc::MintRPCServer::new(&addr, port, mint.clone())?;

                let tls_dir = rpc_settings.tls_dir_path.unwrap_or(_work_dir.join("tls"));

                if !tls_dir.exists() {
                    tracing::error!("TLS directory does not exist: {}", tls_dir.display());
                    bail!("Cannot start RPC server: TLS directory does not exist");
                }

                mint_rpc.start(Some(tls_dir)).await?;

                // Properly box MintRPCServer
                rpc_server_option = Some(Box::new(mint_rpc));

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
            if mint.localstore.get_quote_ttl().await.is_err() {
                mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000)).await?;
            }
            tracing::info!("Mint info already set, not using config file settings.");
        }
    } else {
        tracing::warn!("RPC not enabled, using mint info from config.");
        mint.set_mint_info(mint_builder_info).await?;
        mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000)).await?;
    }

    let socket_addr = SocketAddr::from_str(&format!("{listen_addr}:{listen_port}"))?;

    let listener = tokio::net::TcpListener::bind(socket_addr).await?;

    tracing::debug!("listening on {}", listener.local_addr().unwrap());

    let axum_server = axum::serve(listener, mint_service).with_graceful_shutdown(shutdown_signal());

    tokio::spawn(async move {
        if let Err(err) = axum_server.await {
            tracing::error!("Axum server error: {}", err);
        } else {
            tracing::info!("Axum server stopped gracefully.");
        }
    });

    Ok((shutdown, rpc_server_option))
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
