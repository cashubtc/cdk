//! Cdk mintd lib

// std
#[cfg(feature = "auth")]
use std::collections::HashMap;
use std::env::{self};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

// external crates
use anyhow::{anyhow, bail, Result};
use axum::Router;
use bip39::Mnemonic;
use cdk::cdk_database::{self, MintDatabase, MintKVStore, MintKeysDatabase};
use cdk::mint::{Mint, MintBuilder, MintMeltLimits};
#[cfg(any(
    feature = "cln",
    feature = "lnbits",
    feature = "lnd",
    feature = "ldk-node",
    feature = "fakewallet",
    feature = "grpc-processor"
))]
use cdk::nuts::nut17::SupportedMethods;
use cdk::nuts::nut19::{CachedEndpoint, Method as NUT19Method, Path as NUT19Path};
#[cfg(any(
    feature = "cln",
    feature = "lnbits",
    feature = "lnd",
    feature = "ldk-node",
    feature = "fakewallet"
))]
use cdk::nuts::CurrencyUnit;
#[cfg(feature = "auth")]
use cdk::nuts::{AuthRequired, Method, ProtectedEndpoint, RoutePath};
use cdk::nuts::{ContactInfo, MintVersion, PaymentMethod};
use cdk_axum::cache::HttpCache;
use cdk_common::common::QuoteTTL;
use cdk_common::database::DynMintDatabase;
// internal crate modules
#[cfg(feature = "prometheus")]
use cdk_common::payment::MetricsMintPayment;
use cdk_common::payment::MintPayment;
#[cfg(all(feature = "auth", feature = "postgres"))]
use cdk_postgres::MintPgAuthDatabase;
#[cfg(feature = "postgres")]
use cdk_postgres::MintPgDatabase;
#[cfg(all(feature = "auth", feature = "sqlite"))]
use cdk_sqlite::mint::MintSqliteAuthDatabase;
#[cfg(feature = "sqlite")]
use cdk_sqlite::MintSqliteDatabase;
use cli::CLIArgs;
#[cfg(feature = "auth")]
use config::AuthType;
use config::{DatabaseEngine, LnBackend};
use env_vars::ENV_WORK_DIR;
use setup::LnBackendSetup;
use tower::ServiceBuilder;
use tower_http::compression::CompressionLayer;
use tower_http::decompression::RequestDecompressionLayer;
use tower_http::trace::TraceLayer;
use tracing_appender::{non_blocking, rolling};
use tracing_subscriber::fmt::writer::MakeWriterExt;
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
    DynMintDatabase,
    Arc<dyn MintKeysDatabase<Err = cdk_database::Error> + Send + Sync>,
    Arc<dyn MintKVStore<Err = cdk_database::Error> + Send + Sync>,
)> {
    let (localstore, keystore, kv) = setup_database(settings, work_dir, db_password).await?;
    Ok((localstore, keystore, kv))
}

/// Sets up and initializes a tracing subscriber with custom log filtering.
/// Logs can be configured to output to stdout only, file only, or both.
/// Returns a guard that must be kept alive and properly dropped on shutdown.
pub fn setup_tracing(
    work_dir: &Path,
    logging_config: &config::LoggingConfig,
) -> Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    let default_filter = "debug";
    let hyper_filter = "hyper=warn,rustls=warn,reqwest=warn";
    let h2_filter = "h2=warn";
    let tower_http = "tower_http=warn";
    let rustls = "rustls=warn";

    let env_filter = EnvFilter::new(format!(
        "{default_filter},{hyper_filter},{h2_filter},{tower_http},{rustls}"
    ));

    use config::LoggingOutput;
    match logging_config.output {
        LoggingOutput::Stderr => {
            // Console output only (stderr)
            let console_level = logging_config
                .console_level
                .as_deref()
                .unwrap_or("info")
                .parse::<tracing::Level>()
                .unwrap_or(tracing::Level::INFO);

            let stderr = std::io::stderr.with_max_level(console_level);

            tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .with_writer(stderr)
                .init();

            tracing::info!("Logging initialized: console only ({}+)", console_level);
            Ok(None)
        }
        LoggingOutput::File => {
            // File output only
            let file_level = logging_config
                .file_level
                .as_deref()
                .unwrap_or("debug")
                .parse::<tracing::Level>()
                .unwrap_or(tracing::Level::DEBUG);

            // Create logs directory in work_dir if it doesn't exist
            let logs_dir = work_dir.join("logs");
            std::fs::create_dir_all(&logs_dir)?;

            // Set up file appender with daily rotation
            let file_appender = rolling::daily(&logs_dir, "cdk-mintd.log");
            let (non_blocking_appender, guard) = non_blocking(file_appender);

            let file_writer = non_blocking_appender.with_max_level(file_level);

            tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .with_writer(file_writer)
                .init();

            tracing::info!(
                "Logging initialized: file only at {}/cdk-mintd.log ({}+)",
                logs_dir.display(),
                file_level
            );
            Ok(Some(guard))
        }
        LoggingOutput::Both => {
            // Both console and file output (stderr + file)
            let console_level = logging_config
                .console_level
                .as_deref()
                .unwrap_or("info")
                .parse::<tracing::Level>()
                .unwrap_or(tracing::Level::INFO);
            let file_level = logging_config
                .file_level
                .as_deref()
                .unwrap_or("debug")
                .parse::<tracing::Level>()
                .unwrap_or(tracing::Level::DEBUG);

            // Create logs directory in work_dir if it doesn't exist
            let logs_dir = work_dir.join("logs");
            std::fs::create_dir_all(&logs_dir)?;

            // Set up file appender with daily rotation
            let file_appender = rolling::daily(&logs_dir, "cdk-mintd.log");
            let (non_blocking_appender, guard) = non_blocking(file_appender);

            // Combine console output (stderr) and file output
            let stderr = std::io::stderr.with_max_level(console_level);
            let file_writer = non_blocking_appender.with_max_level(file_level);

            tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .with_writer(stderr.and(file_writer))
                .init();

            tracing::info!(
                "Logging initialized: console ({}+) and file at {}/cdk-mintd.log ({}+)",
                console_level,
                logs_dir.display(),
                file_level
            );
            Ok(Some(guard))
        }
    }
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
    _work_dir: &Path,
    _db_password: Option<String>,
) -> Result<(
    DynMintDatabase,
    Arc<dyn MintKeysDatabase<Err = cdk_database::Error> + Send + Sync>,
    Arc<dyn MintKVStore<Err = cdk_database::Error> + Send + Sync>,
)> {
    match settings.database.engine {
        #[cfg(feature = "sqlite")]
        DatabaseEngine::Sqlite => {
            let db = setup_sqlite_database(_work_dir, _db_password).await?;
            let localstore: Arc<dyn MintDatabase<cdk_database::Error> + Send + Sync> = db.clone();
            let kv: Arc<dyn MintKVStore<Err = cdk_database::Error> + Send + Sync> = db.clone();
            let keystore: Arc<dyn MintKeysDatabase<Err = cdk_database::Error> + Send + Sync> = db;
            Ok((localstore, keystore, kv))
        }
        #[cfg(feature = "postgres")]
        DatabaseEngine::Postgres => {
            // Get the PostgreSQL configuration, ensuring it exists
            let pg_config = settings.database.postgres.as_ref().ok_or_else(|| {
                anyhow!("PostgreSQL configuration is required when using PostgreSQL engine")
            })?;

            if pg_config.url.is_empty() {
                bail!("PostgreSQL URL is required. Set it in config file [database.postgres] section or via CDK_MINTD_POSTGRES_URL/CDK_MINTD_DATABASE_URL environment variable");
            }

            #[cfg(feature = "postgres")]
            let pg_db = Arc::new(MintPgDatabase::new(pg_config.url.as_str()).await?);
            #[cfg(feature = "postgres")]
            let localstore: Arc<dyn MintDatabase<cdk_database::Error> + Send + Sync> =
                pg_db.clone();
            #[cfg(feature = "postgres")]
            let kv: Arc<dyn MintKVStore<Err = cdk_database::Error> + Send + Sync> = pg_db.clone();
            #[cfg(feature = "postgres")]
            let keystore: Arc<
                dyn MintKeysDatabase<Err = cdk_database::Error> + Send + Sync,
            > = pg_db;
            #[cfg(feature = "postgres")]
            return Ok((localstore, keystore, kv));

            #[cfg(not(feature = "postgres"))]
            bail!("PostgreSQL support not compiled in. Enable the 'postgres' feature to use PostgreSQL database.")
        }
        #[cfg(not(feature = "sqlite"))]
        DatabaseEngine::Sqlite => {
            bail!("SQLite support not compiled in. Enable the 'sqlite' feature to use SQLite database.")
        }
        #[cfg(not(feature = "postgres"))]
        DatabaseEngine::Postgres => {
            bail!("PostgreSQL support not compiled in. Enable the 'postgres' feature to use PostgreSQL database.")
        }
    }
}

#[cfg(feature = "sqlite")]
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
    runtime: Option<std::sync::Arc<tokio::runtime::Runtime>>,
    work_dir: &Path,
    kv_store: Option<Arc<dyn MintKVStore<Err = cdk::cdk_database::Error> + Send + Sync>>,
) -> Result<MintBuilder> {
    // Configure basic mint information
    let mint_builder = configure_basic_info(settings, mint_builder);

    // Configure lightning backend
    let mint_builder =
        configure_lightning_backend(settings, mint_builder, runtime, work_dir, kv_store).await?;

    // Configure caching
    let mint_builder = configure_cache(settings, mint_builder);

    Ok(mint_builder)
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
    _runtime: Option<std::sync::Arc<tokio::runtime::Runtime>>,
    work_dir: &Path,
    _kv_store: Option<Arc<dyn MintKVStore<Err = cdk::cdk_database::Error> + Send + Sync>>,
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
                .setup(settings, CurrencyUnit::Msat, None, work_dir, _kv_store)
                .await?;
            #[cfg(feature = "prometheus")]
            let cln = MetricsMintPayment::new(cln);

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
                .setup(settings, CurrencyUnit::Sat, None, work_dir, None)
                .await?;
            #[cfg(feature = "prometheus")]
            let lnbits = MetricsMintPayment::new(lnbits);

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
                .setup(settings, CurrencyUnit::Msat, None, work_dir, _kv_store)
                .await?;
            #[cfg(feature = "prometheus")]
            let lnd = MetricsMintPayment::new(lnd);

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
                    .setup(settings, unit.clone(), None, work_dir, _kv_store.clone())
                    .await?;
                #[cfg(feature = "prometheus")]
                let fake = MetricsMintPayment::new(fake);

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
                    .setup(settings, unit.clone(), None, work_dir, None)
                    .await?;
                #[cfg(feature = "prometheus")]
                let processor = MetricsMintPayment::new(processor);

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
        #[cfg(feature = "ldk-node")]
        LnBackend::LdkNode => {
            let ldk_node_settings = settings.clone().ldk_node.expect("Checked at config load");
            tracing::info!("Using LDK Node backend: {:?}", ldk_node_settings);

            let ldk_node = ldk_node_settings
                .setup(settings, CurrencyUnit::Sat, _runtime, work_dir, None)
                .await?;

            mint_builder = configure_backend_for_unit(
                settings,
                mint_builder,
                CurrencyUnit::Sat,
                mint_melt_limits,
                Arc::new(ldk_node),
            )
            .await?;
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
    backend: Arc<dyn MintPayment<Err = cdk_common::payment::Error> + Send + Sync>,
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

            let nut17_supported = SupportedMethods::default_bolt12(unit.clone());
            mint_builder = mint_builder.with_supported_websockets(nut17_supported);
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

    #[cfg(any(
        feature = "cln",
        feature = "lnbits",
        feature = "lnd",
        feature = "fakewallet",
        feature = "grpc-processor",
        feature = "ldk-node"
    ))]
    {
        let nut17_supported = SupportedMethods::default_bolt11(unit);
        mint_builder = mint_builder.with_supported_websockets(nut17_supported);
    }

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
    _work_dir: &Path,
    mut mint_builder: MintBuilder,
    _password: Option<String>,
) -> Result<MintBuilder> {
    if let Some(auth_settings) = settings.auth.clone() {
        use cdk_common::database::DynMintAuthDatabase;

        tracing::info!("Auth settings are defined. {:?}", auth_settings);
        let auth_localstore: DynMintAuthDatabase = match settings.database.engine {
            #[cfg(feature = "sqlite")]
            DatabaseEngine::Sqlite => {
                #[cfg(feature = "sqlite")]
                {
                    let sql_db_path = _work_dir.join("cdk-mintd-auth.sqlite");
                    #[cfg(not(feature = "sqlcipher"))]
                    let sqlite_db = MintSqliteAuthDatabase::new(&sql_db_path).await?;
                    #[cfg(feature = "sqlcipher")]
                    let sqlite_db = {
                        // Get password from command line arguments for sqlcipher
                        MintSqliteAuthDatabase::new((sql_db_path, _password.unwrap())).await?
                    };

                    Arc::new(sqlite_db)
                }
                #[cfg(not(feature = "sqlite"))]
                {
                    bail!("SQLite support not compiled in. Enable the 'sqlite' feature to use SQLite database.")
                }
            }
            #[cfg(feature = "postgres")]
            DatabaseEngine::Postgres => {
                #[cfg(feature = "postgres")]
                {
                    // Require dedicated auth database configuration - no fallback to main database
                    let auth_db_config = settings.auth_database.as_ref().ok_or_else(|| {
                        anyhow!("Auth database configuration is required when using PostgreSQL with authentication. Set [auth_database] section in config file or CDK_MINTD_AUTH_POSTGRES_URL environment variable")
                    })?;

                    let auth_pg_config = auth_db_config.postgres.as_ref().ok_or_else(|| {
                        anyhow!("PostgreSQL auth database configuration is required when using PostgreSQL with authentication. Set [auth_database.postgres] section in config file or CDK_MINTD_AUTH_POSTGRES_URL environment variable")
                    })?;

                    if auth_pg_config.url.is_empty() {
                        bail!("Auth database PostgreSQL URL is required and cannot be empty. Set it in config file [auth_database.postgres] section or via CDK_MINTD_AUTH_POSTGRES_URL environment variable");
                    }

                    Arc::new(MintPgAuthDatabase::new(auth_pg_config.url.as_str()).await?)
                }
                #[cfg(not(feature = "postgres"))]
                {
                    bail!("PostgreSQL support not compiled in. Enable the 'postgres' feature to use PostgreSQL database.")
                }
            }
            #[cfg(not(feature = "sqlite"))]
            DatabaseEngine::Sqlite => {
                bail!("SQLite support not compiled in. Enable the 'sqlite' feature to use SQLite database.")
            }
            #[cfg(not(feature = "postgres"))]
            DatabaseEngine::Postgres => {
                bail!("PostgreSQL support not compiled in. Enable the 'postgres' feature to use PostgreSQL database.")
            }
        };

        let mut protected_endpoints = HashMap::new();
        let mut blind_auth_endpoints = vec![];
        let mut clear_auth_endpoints = vec![];
        let mut unprotected_endpoints = vec![];

        let mint_blind_auth_endpoint =
            ProtectedEndpoint::new(Method::Post, RoutePath::MintBlindAuth);

        protected_endpoints.insert(mint_blind_auth_endpoint, AuthRequired::Clear);

        clear_auth_endpoints.push(mint_blind_auth_endpoint);

        // Helper function to add endpoint based on auth type
        let mut add_endpoint = |endpoint: ProtectedEndpoint, auth_type: &AuthType| {
            match auth_type {
                AuthType::Blind => {
                    protected_endpoints.insert(endpoint, AuthRequired::Blind);
                    blind_auth_endpoints.push(endpoint);
                }
                AuthType::Clear => {
                    protected_endpoints.insert(endpoint, AuthRequired::Clear);
                    clear_auth_endpoints.push(endpoint);
                }
                AuthType::None => {
                    unprotected_endpoints.push(endpoint);
                }
            };
        };

        // Get mint quote endpoint
        {
            let mint_quote_protected_endpoint =
                ProtectedEndpoint::new(cdk::nuts::Method::Post, RoutePath::MintQuoteBolt11);
            add_endpoint(mint_quote_protected_endpoint, &auth_settings.get_mint_quote);
        }

        // Check mint quote endpoint
        {
            let check_mint_protected_endpoint =
                ProtectedEndpoint::new(Method::Get, RoutePath::MintQuoteBolt11);
            add_endpoint(
                check_mint_protected_endpoint,
                &auth_settings.check_mint_quote,
            );
        }

        // Mint endpoint
        {
            let mint_protected_endpoint =
                ProtectedEndpoint::new(cdk::nuts::Method::Post, RoutePath::MintBolt11);
            add_endpoint(mint_protected_endpoint, &auth_settings.mint);
        }

        // Get melt quote endpoint
        {
            let melt_quote_protected_endpoint = ProtectedEndpoint::new(
                cdk::nuts::Method::Post,
                cdk::nuts::RoutePath::MeltQuoteBolt11,
            );
            add_endpoint(melt_quote_protected_endpoint, &auth_settings.get_melt_quote);
        }

        // Check melt quote endpoint
        {
            let check_melt_protected_endpoint =
                ProtectedEndpoint::new(Method::Get, RoutePath::MeltQuoteBolt11);
            add_endpoint(
                check_melt_protected_endpoint,
                &auth_settings.check_melt_quote,
            );
        }

        // Melt endpoint
        {
            let melt_protected_endpoint =
                ProtectedEndpoint::new(Method::Post, RoutePath::MeltBolt11);
            add_endpoint(melt_protected_endpoint, &auth_settings.melt);
        }

        // Swap endpoint
        {
            let swap_protected_endpoint = ProtectedEndpoint::new(Method::Post, RoutePath::Swap);
            add_endpoint(swap_protected_endpoint, &auth_settings.swap);
        }

        // Restore endpoint
        {
            let restore_protected_endpoint =
                ProtectedEndpoint::new(Method::Post, RoutePath::Restore);
            add_endpoint(restore_protected_endpoint, &auth_settings.restore);
        }

        // Check proof state endpoint
        {
            let state_protected_endpoint =
                ProtectedEndpoint::new(Method::Post, RoutePath::Checkstate);
            add_endpoint(state_protected_endpoint, &auth_settings.check_proof_state);
        }

        // Ws endpoint
        {
            let ws_protected_endpoint = ProtectedEndpoint::new(Method::Get, RoutePath::Ws);
            add_endpoint(ws_protected_endpoint, &auth_settings.websocket_auth);
        }

        mint_builder = mint_builder.with_auth(
            auth_localstore.clone(),
            auth_settings.openid_discovery,
            auth_settings.openid_client_id,
            clear_auth_endpoints,
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
    } else if let Some(seed) = settings.info.seed.clone() {
        let seed_bytes: Vec<u8> = seed.into();
        Ok(mint_builder.build_with_seed(keystore, &seed_bytes).await?)
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
    work_dir: &Path,
    mint_builder_info: cdk::nuts::MintInfo,
    shutdown_signal: impl std::future::Future<Output = ()> + Send + 'static,
    routers: Vec<Router>,
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

    // Determine the desired QuoteTTL from config/env or fall back to defaults
    let desired_quote_ttl: QuoteTTL = settings.info.quote_ttl.unwrap_or_default();

    if rpc_enabled {
        if mint.mint_info().await.is_err() {
            tracing::info!("Mint info not set on mint, setting.");
            // First boot with RPC enabled: seed from config
            mint.set_mint_info(mint_builder_info).await?;
            mint.set_quote_ttl(desired_quote_ttl).await?;
        } else {
            // If QuoteTTL has never been persisted, seed it now from config
            if !mint.quote_ttl_is_persisted().await? {
                mint.set_quote_ttl(desired_quote_ttl).await?;
            }
            // Add/refresh version information without altering stored mint_info fields
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
        // RPC disabled: config is source of truth on every boot
        tracing::info!("RPC not enabled, using mint info and quote TTL from config.");
        let mut mint_builder_info = mint_builder_info;

        if let Ok(mint_info) = mint.mint_info().await {
            if mint_builder_info.pubkey.is_none() {
                mint_builder_info.pubkey = mint_info.pubkey;
            }
        }

        mint.set_mint_info(mint_builder_info).await?;
        mint.set_quote_ttl(desired_quote_ttl).await?;
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

    for router in routers {
        mint_service = mint_service.merge(router);
    }

    #[cfg(feature = "swagger")]
    {
        if settings.info.enable_swagger_ui.unwrap_or(false) {
            mint_service = mint_service.merge(
                utoipa_swagger_ui::SwaggerUi::new("/swagger-ui")
                    .url("/api-docs/openapi.json", cdk_axum::ApiDoc::openapi()),
            );
        }
    }
    // Create a broadcast channel to share shutdown signal between services
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    // Start Prometheus server if enabled
    #[cfg(feature = "prometheus")]
    let prometheus_handle = {
        if let Some(prometheus_settings) = &settings.prometheus {
            if prometheus_settings.enabled {
                let addr = prometheus_settings
                    .address
                    .clone()
                    .unwrap_or("127.0.0.1".to_string());
                let port = prometheus_settings.port.unwrap_or(9000);

                let address = format!("{}:{}", addr, port)
                    .parse()
                    .expect("Invalid prometheus address");

                let server = cdk_prometheus::PrometheusBuilder::new()
                    .bind_address(address)
                    .build_with_cdk_metrics()?;

                let mut shutdown_rx = shutdown_tx.subscribe();
                let prometheus_shutdown = async move {
                    let _ = shutdown_rx.recv().await;
                };

                Some(tokio::spawn(async move {
                    if let Err(e) = server.start(prometheus_shutdown).await {
                        tracing::error!("Failed to start prometheus server: {}", e);
                    }
                }))
            } else {
                None
            }
        } else {
            None
        }
    };

    #[cfg(not(feature = "prometheus"))]
    let prometheus_handle: Option<tokio::task::JoinHandle<()>> = None;

    mint.start().await?;

    let socket_addr = SocketAddr::from_str(&format!("{listen_addr}:{listen_port}"))?;

    let listener = tokio::net::TcpListener::bind(socket_addr).await?;

    tracing::info!("listening on {}", listener.local_addr().unwrap());

    // Create a task to wait for the shutdown signal and broadcast it
    let shutdown_broadcast_task = {
        let shutdown_tx = shutdown_tx.clone();
        tokio::spawn(async move {
            shutdown_signal.await;
            tracing::info!("Shutdown signal received, broadcasting to all services");
            let _ = shutdown_tx.send(());
        })
    };

    // Create shutdown future for axum server
    let mut axum_shutdown_rx = shutdown_tx.subscribe();
    let axum_shutdown = async move {
        let _ = axum_shutdown_rx.recv().await;
    };

    // Wait for axum server to complete with custom shutdown signal
    let axum_result = axum::serve(listener, mint_service).with_graceful_shutdown(axum_shutdown);

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

    // Wait for the shutdown broadcast task to complete
    let _ = shutdown_broadcast_task.await;

    // Wait for prometheus server to shutdown if it was started
    #[cfg(feature = "prometheus")]
    if let Some(handle) = prometheus_handle {
        if let Err(e) = handle.await {
            tracing::warn!("Prometheus server task failed: {}", e);
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

/// The main entry point for the application when used as a library
pub async fn run_mintd(
    work_dir: &Path,
    settings: &config::Settings,
    db_password: Option<String>,
    enable_logging: bool,
    runtime: Option<std::sync::Arc<tokio::runtime::Runtime>>,
    routers: Vec<Router>,
) -> Result<()> {
    let _guard = if enable_logging {
        setup_tracing(work_dir, &settings.info.logging)?
    } else {
        None
    };

    let result = run_mintd_with_shutdown(
        work_dir,
        settings,
        shutdown_signal(),
        db_password,
        runtime,
        routers,
    )
    .await;

    // Explicitly drop the guard to ensure proper cleanup
    if let Some(guard) = _guard {
        tracing::info!("Shutting down logging worker thread");
        drop(guard);
        // Give the worker thread a moment to flush any remaining logs
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    tracing::info!("Mintd shutdown");

    result
}

/// Run mintd with a custom shutdown signal
pub async fn run_mintd_with_shutdown(
    work_dir: &Path,
    settings: &config::Settings,
    shutdown_signal: impl std::future::Future<Output = ()> + Send + 'static,
    db_password: Option<String>,
    runtime: Option<std::sync::Arc<tokio::runtime::Runtime>>,
    routers: Vec<Router>,
) -> Result<()> {
    let (localstore, keystore, kv) = initial_setup(work_dir, settings, db_password.clone()).await?;

    let mint_builder = MintBuilder::new(localstore);

    // If RPC is enabled and DB contains mint_info already, initialize the builder from DB.
    // This ensures subsequent builder modifications (like version injection) can respect stored values.
    let maybe_mint_builder = {
        #[cfg(feature = "management-rpc")]
        {
            if let Some(rpc_settings) = settings.mint_management_rpc.clone() {
                if rpc_settings.enabled {
                    // Best-effort: pull DB state into builder if present
                    let mut tmp = mint_builder;
                    if let Err(e) = tmp.init_from_db_if_present().await {
                        tracing::warn!("Failed to init builder from DB: {}", e);
                    }
                    tmp
                } else {
                    mint_builder
                }
            } else {
                mint_builder
            }
        }
        #[cfg(not(feature = "management-rpc"))]
        {
            mint_builder
        }
    };

    let mint_builder =
        configure_mint_builder(settings, maybe_mint_builder, runtime, work_dir, Some(kv)).await?;
    #[cfg(feature = "auth")]
    let mint_builder = setup_authentication(settings, work_dir, mint_builder, db_password).await?;

    let config_mint_info = mint_builder.current_mint_info();

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
        work_dir,
        config_mint_info,
        shutdown_signal,
        routers,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_postgres_auth_url_validation() {
        // Test that the auth database config requires explicit configuration

        // Test empty URL
        let auth_config = config::PostgresAuthConfig {
            url: "".to_string(),
            ..Default::default()
        };
        assert!(auth_config.url.is_empty());

        // Test non-empty URL
        let auth_config = config::PostgresAuthConfig {
            url: "postgresql://user:password@localhost:5432/auth_db".to_string(),
            ..Default::default()
        };
        assert!(!auth_config.url.is_empty());
    }
}
