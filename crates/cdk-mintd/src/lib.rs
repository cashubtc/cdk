#![allow(missing_docs)]
//! Cdk mintd lib

// std
use std::collections::{HashMap, HashSet};
use std::env::{self};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

// external crates
use anyhow::{anyhow, bail, Context, Result};
use axum::extract::DefaultBodyLimit;
use axum::Router;
use bip39::Mnemonic;
use cdk::cdk_database::{self, KVStore, MintDatabase, MintKeysDatabase};
use cdk::mint::{Mint, MintBuilder, MintMeltLimits};
use cdk::nuts::nut00::KnownMethod;
#[cfg(any(
    feature = "cln",
    feature = "lnbits",
    feature = "lnd",
    feature = "ldk-node",
    feature = "fakewallet",
    feature = "bdk",
    feature = "grpc-processor"
))]
use cdk::nuts::nut17::SupportedMethods;
use cdk::nuts::nut19::{CachedEndpoint, Method as NUT19Method, Path as NUT19Path};
use cdk::nuts::{
    AuthRequired, ContactInfo, Method, MintVersion, PaymentMethod, ProtectedEndpoint, RoutePath,
};
use cdk_axum::cache::HttpCache;
use cdk_common::common::QuoteTTL;
use cdk_common::database::DynMintDatabase;
// internal crate modules
#[cfg(feature = "prometheus")]
use cdk_common::payment::MetricsMintPayment;
use cdk_common::payment::MintPayment;
#[cfg(feature = "postgres")]
use cdk_postgres::{MintPgAuthDatabase, MintPgDatabase, PgConfig};
#[cfg(feature = "sqlite")]
use cdk_sqlite::mint::MintSqliteAuthDatabase;
#[cfg(feature = "sqlite")]
use cdk_sqlite::MintSqliteDatabase;
use cli::CLIArgs;
use config::{AuthType, DatabaseEngine, LnBackend};
use env_vars::ENV_WORK_DIR;
use setup::LnBackendSetup;
use tower::ServiceBuilder;
use tower_http::compression::CompressionLayer;
use tower_http::decompression::RequestDecompressionLayer;
use tower_http::trace::TraceLayer;
use tracing_appender::{non_blocking, rolling};
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::EnvFilter;

pub mod cli;
pub mod config;
pub mod env_vars;
pub mod setup;

#[cfg(test)]
pub(crate) mod test_utils {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    pub(crate) fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn unique_temp_path(name: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "{name}_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }
}

const CARGO_PKG_VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");
const DEFAULT_BATCH_MINT_SIZE: u64 = 100;
const REQUEST_BODY_LIMIT_BYTES: usize = 1_048_576;

fn extract_supported_payment_methods(mint_info: &cdk::nuts::MintInfo) -> Vec<String> {
    let mut seen = HashSet::new();
    mint_info
        .nuts
        .nut04
        .methods
        .iter()
        .map(|method| method.method.to_string())
        .filter(|method| seen.insert(method.clone()))
        .collect()
}

#[cfg(feature = "cln")]
fn expand_path(path: &str) -> Option<PathBuf> {
    if path == "~" {
        return home::home_dir();
    }

    if let Some(remainder) = path.strip_prefix("~/") {
        return home::home_dir().map(|home_dir| home_dir.join(remainder));
    }

    Some(PathBuf::from(path))
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
    Arc<dyn KVStore<Err = cdk_database::Error> + Send + Sync>,
)> {
    tracing::info!("Initializing database...");
    let (localstore, keystore, kv) = setup_database(settings, work_dir, db_password).await?;
    tracing::info!("Database initialized successfully");
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
    let tower_filter = "tower=warn";
    let tower_http = "tower_http=warn";
    let rustls = "rustls=warn";
    let tungstenite = "tungstenite=warn";
    let tokio_postgres = "tokio_postgres=warn";

    let env_filter = EnvFilter::new(format!(
        "{default_filter},{hyper_filter},{h2_filter},{tower_filter},{tower_http},{rustls},{tungstenite},{tokio_postgres}"
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
                .with_ansi(false)
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
                .with_ansi(false)
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
                .with_ansi(false)
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
    let settings = load_settings_from_sources(work_dir, config_path)?;
    validate_settings(&settings)?;

    Ok(settings)
}

fn load_settings_from_sources(
    work_dir: &Path,
    config_path: Option<PathBuf>,
) -> Result<config::Settings> {
    // get config file name from args
    let config_file_arg = match config_path {
        Some(c) => c,
        None => work_dir.join("config.toml"),
    };

    let mut settings = if config_file_arg.exists() {
        config::Settings::try_new(Some(config_file_arg.clone()))
            .with_context(|| format!("Failed to read config file {}", config_file_arg.display()))?
    } else {
        tracing::info!("Config file does not exist. Attempting to read env vars");
        config::Settings::default()
    };
    // This check for any settings defined in ENV VARs
    // ENV VARS will take **priority** over those in the config
    settings.from_env()
}

fn validate_settings(settings: &config::Settings) -> Result<()> {
    validate_listen_config(settings)?;
    validate_signing_config(settings)?;
    validate_lightning_config(settings)?;
    validate_onchain_config(settings)?;
    validate_database_config(settings)?;
    validate_auth_config(settings)?;
    validate_management_rpc_config(settings)?;
    validate_prometheus_config(settings)?;

    Ok(())
}

fn validate_database_config(settings: &config::Settings) -> Result<()> {
    if settings.database.engine == DatabaseEngine::Postgres {
        let pg_config = settings.database.postgres.as_ref().ok_or_else(|| {
            anyhow!("PostgreSQL configuration is required when using PostgreSQL engine")
        })?;

        if pg_config.url.is_empty() {
            bail!("PostgreSQL URL is required. Set it in config file [database.postgres] section or via CDK_MINTD_POSTGRES_URL/CDK_MINTD_DATABASE_URL environment variable");
        }
    }

    Ok(())
}

fn validate_listen_config(settings: &config::Settings) -> Result<()> {
    format!(
        "{}:{}",
        settings.info.listen_host, settings.info.listen_port
    )
    .parse::<SocketAddr>()
    .map_err(|err| {
        anyhow!(
            "Invalid mint listen address [info].listen_host/[info].listen_port ({}:{}): {err}",
            settings.info.listen_host,
            settings.info.listen_port
        )
    })?;

    Ok(())
}

fn validate_signing_config(settings: &config::Settings) -> Result<()> {
    const MIN_SEED_BYTES: usize = 32;

    if let Some(signatory) = settings.enabled_signatory() {
        if signatory.tls_dir.is_none() && !signatory.allow_insecure {
            bail!(
                "gRPC signatory TLS is not configured. Set [signatory].tls_dir or \
                 [signatory].allow_insecure = true to connect without TLS"
            );
        }

        return Ok(());
    }

    let seed = settings.info.seed.as_ref();
    let mnemonic = settings
        .info
        .mnemonic
        .as_ref()
        .filter(|value| !value.is_empty());

    if let Some(seed) = seed {
        if seed.is_empty() {
            bail!("Seed in [info].seed/CDK_MINTD_SEED must not be empty");
        }
        if seed.len() < MIN_SEED_BYTES {
            bail!(
                "Seed in [info].seed/CDK_MINTD_SEED is too short ({} bytes); require at least {MIN_SEED_BYTES} bytes",
                seed.len()
            );
        }
        return Ok(());
    }

    if let Some(mnemonic) = mnemonic {
        Mnemonic::from_str(mnemonic).map_err(|err| {
            anyhow!("Invalid mnemonic in [info].mnemonic/CDK_MINTD_MNEMONIC: {err}")
        })?;
        return Ok(());
    }

    bail!("No signing source configured. Set one of [info].mnemonic/CDK_MINTD_MNEMONIC, [info].seed/CDK_MINTD_SEED, or [signatory] with enabled = true");
}

fn validate_lightning_config(settings: &config::Settings) -> Result<()> {
    // No emptiness check here: `from_env` already guarantees at least one
    // payment backend (Lightning *or* on-chain), so requiring `[[ln]]` here
    // would wrongly reject valid on-chain-only configs. An empty `ln` simply
    // skips the loop below.
    for ln in &settings.ln {
        if ln.min_mint > ln.max_mint {
            bail!("Lightning min_mint cannot be greater than max_mint");
        }
        if ln.min_melt > ln.max_melt {
            bail!("Lightning min_melt cannot be greater than max_melt");
        }

        match ln.ln_backend {
            LnBackend::None => {}
            #[cfg(feature = "cln")]
            LnBackend::Cln => {
                let default_cln;
                let cln = match settings.cln.as_ref() {
                    Some(c) => c,
                    None => {
                        default_cln = config::Cln::default();
                        &default_cln
                    }
                };
                if cln.rpc_path.as_os_str().is_empty() {
                    bail!("CLN rpc_path must be set via [cln].rpc_path or CDK_MINTD_CLN_RPC_PATH");
                }
            }
            #[cfg(feature = "lnbits")]
            LnBackend::LNbits => {
                let default_lnbits;
                let lnbits = match settings.lnbits.as_ref() {
                    Some(l) => l,
                    None => {
                        default_lnbits = config::LNbits::default();
                        &default_lnbits
                    }
                };
                if lnbits.admin_api_key.is_empty() {
                    bail!("LNbits admin_api_key must be set via [lnbits].admin_api_key or CDK_MINTD_LNBITS_ADMIN_API_KEY");
                }
                if lnbits.invoice_api_key.is_empty() {
                    bail!("LNbits invoice_api_key must be set via [lnbits].invoice_api_key or CDK_MINTD_LNBITS_INVOICE_API_KEY");
                }
                if lnbits.lnbits_api.is_empty() {
                    bail!(
                        "LNbits lnbits_api must be set via [lnbits].lnbits_api or CDK_MINTD_LNBITS_API"
                    );
                }
            }
            #[cfg(feature = "lnd")]
            LnBackend::Lnd => {
                let default_lnd;
                let lnd = match settings.lnd.as_ref() {
                    Some(l) => l,
                    None => {
                        default_lnd = config::Lnd::default();
                        &default_lnd
                    }
                };
                if lnd.address.is_empty() {
                    bail!("LND address must be set via [lnd].address or CDK_MINTD_LND_ADDRESS");
                }
                if lnd.cert_file.as_os_str().is_empty() {
                    bail!(
                        "LND cert_file must be set via [lnd].cert_file or CDK_MINTD_LND_CERT_FILE"
                    );
                }
                if lnd.macaroon_file.as_os_str().is_empty() {
                    bail!("LND macaroon_file must be set via [lnd].macaroon_file or CDK_MINTD_LND_MACAROON_FILE");
                }
            }
            #[cfg(feature = "fakewallet")]
            LnBackend::FakeWallet => {
                let default_fake_wallet;
                let fake_wallet = match settings.fake_wallet.as_ref() {
                    Some(f) => f,
                    None => {
                        default_fake_wallet = config::FakeWallet::default();
                        &default_fake_wallet
                    }
                };
                if fake_wallet.supported_units.is_empty() {
                    bail!("Fake wallet supported_units must contain at least one unit via [fake_wallet].supported_units or CDK_MINTD_FAKE_WALLET_SUPPORTED_UNITS");
                }
                if fake_wallet.min_delay_time > fake_wallet.max_delay_time {
                    bail!("Fake wallet min_delay_time cannot be greater than max_delay_time");
                }
            }
            #[cfg(feature = "grpc-processor")]
            LnBackend::GrpcProcessor => {
                let default_grpc_processor;
                let grpc_processor = match settings.grpc_processor.as_ref() {
                    Some(g) => g,
                    None => {
                        default_grpc_processor = config::GrpcProcessor::default();
                        &default_grpc_processor
                    }
                };
                if grpc_processor.supported_units.is_empty() {
                    bail!("gRPC payment processor supported_units must contain at least one unit via [grpc_processor].supported_units or CDK_MINTD_GRPC_PAYMENT_PROCESSOR_SUPPORTED_UNITS");
                }
                if grpc_processor.address.is_empty() {
                    bail!("gRPC payment processor address must be set via [grpc_processor].address or CDK_MINTD_GRPC_PAYMENT_PROCESSOR_ADDRESS");
                }
            }
            #[cfg(feature = "ldk-node")]
            // LDK node has no required-field validation; defaults are usable.
            LnBackend::LdkNode => {}
        }
    }

    Ok(())
}

fn validate_onchain_config(settings: &config::Settings) -> Result<()> {
    let Some(onchain) = settings.onchain.as_ref() else {
        return Ok(());
    };

    if onchain.min_mint > onchain.max_mint {
        bail!("On-chain min_mint cannot be greater than max_mint");
    }
    if onchain.min_melt > onchain.max_melt {
        bail!("On-chain min_melt cannot be greater than max_melt");
    }

    Ok(())
}

fn validate_auth_config(settings: &config::Settings) -> Result<()> {
    let Some(auth) = settings.auth.as_ref() else {
        return Ok(());
    };

    if auth.openid_discovery.is_empty() {
        bail!("Auth openid_discovery must be set via [auth].openid_discovery or CDK_MINTD_AUTH_OPENID_DISCOVERY");
    }
    if auth.openid_client_id.is_empty() {
        bail!("Auth openid_client_id must be set via [auth].openid_client_id or CDK_MINTD_AUTH_OPENID_CLIENT_ID");
    }

    if settings.database.engine == DatabaseEngine::Postgres {
        let auth_db_config = settings.auth_database.as_ref().ok_or_else(|| {
            anyhow!("Auth database configuration is required when using PostgreSQL with authentication. Set [auth_database] section or CDK_MINTD_AUTH_POSTGRES_URL")
        })?;
        let auth_pg_config = auth_db_config.postgres.as_ref().ok_or_else(|| {
            anyhow!("PostgreSQL auth database configuration is required when using PostgreSQL with authentication. Set [auth_database.postgres] section or CDK_MINTD_AUTH_POSTGRES_URL")
        })?;
        if auth_pg_config.url.is_empty() {
            bail!("Auth database PostgreSQL URL is required. Set [auth_database.postgres].url or CDK_MINTD_AUTH_POSTGRES_URL");
        }
    }

    Ok(())
}

fn validate_management_rpc_config(settings: &config::Settings) -> Result<()> {
    #[cfg(not(feature = "management-rpc"))]
    let _ = settings;

    #[cfg(feature = "management-rpc")]
    if let Some(rpc_settings) = settings.mint_management_rpc.as_ref() {
        if rpc_settings.enabled {
            let address = rpc_settings.address.as_deref().unwrap_or("127.0.0.1");
            let port = rpc_settings.port.unwrap_or(8086);
            format!("{address}:{port}")
                .parse::<SocketAddr>()
                .map_err(|err| {
                    anyhow!(
                        "Invalid mint management RPC address [mint_management_rpc].address/[mint_management_rpc].port ({address}:{port}): {err}"
                    )
                })?;
        }
    }

    Ok(())
}

fn validate_prometheus_config(settings: &config::Settings) -> Result<()> {
    #[cfg(not(feature = "prometheus"))]
    let _ = settings;

    #[cfg(feature = "prometheus")]
    if let Some(prometheus_settings) = settings.prometheus.as_ref() {
        if prometheus_settings.enabled {
            let address = prometheus_settings
                .address
                .as_deref()
                .unwrap_or("127.0.0.1");
            let port = prometheus_settings.port.unwrap_or(9000);
            format!("{address}:{port}")
                .parse::<SocketAddr>()
                .map_err(|err| {
                    anyhow!(
                        "Invalid Prometheus address [prometheus].address/[prometheus].port ({address}:{port}): {err}"
                    )
                })?;
        }
    }

    Ok(())
}

/// Loads settings from command line arguments, environment variables, and optional seed file.
pub fn load_settings_from_args(work_dir: &Path, args: &CLIArgs) -> Result<config::Settings> {
    let mut settings = load_settings_from_sources(work_dir, args.config.clone())?;

    if let Some(seed_file) = args.seed_file.as_deref() {
        apply_seed_file(&mut settings, seed_file)?;
    }

    validate_settings(&settings)?;

    Ok(settings)
}

/// Overrides the configured mint and active payment backend mnemonic with a seed file.
pub fn apply_seed_file(settings: &mut config::Settings, seed_file: &Path) -> Result<()> {
    let mnemonic = std::fs::read_to_string(seed_file)
        .with_context(|| format!("Failed to read seed file {}", seed_file.display()))?;
    let mnemonic = mnemonic.trim();

    if mnemonic.is_empty() {
        bail!("Seed file {} is empty", seed_file.display());
    }

    Mnemonic::parse(mnemonic)
        .with_context(|| format!("Invalid seed phrase in seed file {}", seed_file.display()))?;

    settings.info.seed = None;
    settings.info.mnemonic = Some(mnemonic.to_owned());

    #[cfg(feature = "bdk")]
    if settings
        .onchain
        .as_ref()
        .is_some_and(|onchain| onchain.onchain_backend == config::OnchainBackend::Bdk)
    {
        let mut bdk = settings.bdk.clone().unwrap_or_default();
        bdk.mnemonic = Some(mnemonic.to_owned());
        settings.bdk = Some(bdk);
    }

    #[cfg(feature = "ldk-node")]
    if settings
        .ln
        .iter()
        .any(|ln| ln.ln_backend == LnBackend::LdkNode)
    {
        let mut ldk_node = settings.ldk_node.clone().unwrap_or_default();
        ldk_node.ldk_node_mnemonic = Some(mnemonic.to_owned());
        settings.ldk_node = Some(ldk_node);
    }

    Ok(())
}

async fn setup_database(
    settings: &config::Settings,
    _work_dir: &Path,
    _db_password: Option<String>,
) -> Result<(
    DynMintDatabase,
    Arc<dyn MintKeysDatabase<Err = cdk_database::Error> + Send + Sync>,
    Arc<dyn KVStore<Err = cdk_database::Error> + Send + Sync>,
)> {
    tracing::info!("Using database engine: {:?}", settings.database.engine);
    match settings.database.engine {
        #[cfg(feature = "sqlite")]
        DatabaseEngine::Sqlite => {
            let db = setup_sqlite_database(_work_dir, _db_password).await?;
            let localstore: Arc<dyn MintDatabase<cdk_database::Error> + Send + Sync> = db.clone();
            let kv: Arc<dyn KVStore<Err = cdk_database::Error> + Send + Sync> = db.clone();
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
            let db_config = PgConfig::new(
                pg_config.url.as_str(),
                pg_config.tls_mode.as_deref(),
                pg_config.max_connections,
                pg_config.connection_timeout_seconds,
            );
            #[cfg(feature = "postgres")]
            let pg_db = Arc::new(MintPgDatabase::new(db_config).await?);
            tracing::info!("PostgreSQL database connection established");
            #[cfg(feature = "postgres")]
            let localstore: Arc<dyn MintDatabase<cdk_database::Error> + Send + Sync> =
                pg_db.clone();
            #[cfg(feature = "postgres")]
            let kv: Arc<dyn KVStore<Err = cdk_database::Error> + Send + Sync> = pg_db.clone();
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
    tracing::info!("SQLite database path: {}", sql_db_path.display());

    #[cfg(not(feature = "sqlcipher"))]
    let db = MintSqliteDatabase::new(&sql_db_path).await?;
    #[cfg(feature = "sqlcipher")]
    let db = {
        // Get password from command line arguments for sqlcipher
        let password = _password
            .ok_or_else(|| anyhow!("Password required when sqlcipher feature is enabled"))?;
        tracing::info!("Using SQLCipher encryption for SQLite database");
        MintSqliteDatabase::new((sql_db_path, password)).await?
    };

    tracing::info!("SQLite database initialized successfully");
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
    kv_store: Option<Arc<dyn KVStore<Err = cdk::cdk_database::Error> + Send + Sync>>,
) -> Result<MintBuilder> {
    settings
        .validate_backend_pairing()
        .map_err(anyhow::Error::msg)?;

    // Configure basic mint information
    let mint_builder = configure_basic_info(settings, mint_builder);

    // Check that fake wallet is not used on mainnet
    #[cfg(feature = "fakewallet")]
    if settings
        .ln
        .iter()
        .any(|ln| ln.ln_backend == LnBackend::FakeWallet)
    {
        if let Some(_onchain) = &settings.onchain {
            #[cfg(feature = "bdk")]
            if _onchain.onchain_backend == config::OnchainBackend::Bdk {
                if let Some(bdk) = &settings.bdk {
                    if let Some(network) = &bdk.network {
                        let network = network.to_lowercase();
                        if network == "mainnet" || network == "bitcoin" {
                            bail!("Fake wallet cannot be used for Lightning when On-chain is configured for Mainnet");
                        }
                    }
                }
            }
        }
    }

    // Configure lightning backend
    let mint_builder = configure_lightning_backend(
        settings,
        mint_builder,
        runtime.clone(),
        work_dir,
        kv_store.clone(),
    )
    .await?;

    // Configure onchain backend
    let mint_builder =
        configure_onchain_backend(settings, mint_builder, runtime, work_dir, kv_store).await?;

    // Extract configured payment methods from mint_builder
    let mint_info = mint_builder.current_mint_info();
    let payment_methods = extract_supported_payment_methods(&mint_info);

    // Enable batch minting by default for all supported methods
    let mint_builder = mint_builder
        .with_batch_minting(Some(DEFAULT_BATCH_MINT_SIZE), Some(payment_methods.clone()));

    // Configure caching with payment methods
    let mint_builder = configure_cache(settings, mint_builder, &payment_methods).await?;

    // Configure transaction limits
    let mint_builder =
        mint_builder.with_limits(settings.limits.max_inputs, settings.limits.max_outputs);

    // Verify at least one payment processor is configured
    if mint_builder
        .current_mint_info()
        .nuts
        .nut04
        .methods
        .is_empty()
    {
        bail!("At least one payment backend (Lightning or On-chain) must be configured");
    }

    Ok(mint_builder)
}

/// Configures basic mint information (name, contact info, descriptions, etc.)
fn configure_basic_info(settings: &config::Settings, mint_builder: MintBuilder) -> MintBuilder {
    // Add contact information
    let mut contacts = Vec::new();
    if let Some(nostr_key) = &settings.mint_info.contact_nostr_public_key {
        if !nostr_key.is_empty() {
            contacts.push(ContactInfo::new("nostr".to_string(), nostr_key.to_string()));
        }
    }
    if let Some(email) = &settings.mint_info.contact_email {
        if !email.is_empty() {
            contacts.push(ContactInfo::new("email".to_string(), email.to_string()));
        }
    }

    // Add version information
    let mint_version = MintVersion::new(
        "cdk-mintd".to_string(),
        CARGO_PKG_VERSION.unwrap_or("Unknown").to_string(),
    );

    // Configure mint builder with basic info
    let mut builder = mint_builder.with_version(mint_version);

    // Only set name if it's not empty
    if !settings.mint_info.name.is_empty() {
        builder = builder.with_name(settings.mint_info.name.clone());
    }

    // Only set description if it's not empty
    if !settings.mint_info.description.is_empty() {
        builder = builder.with_description(settings.mint_info.description.clone());
    }

    // Add optional information
    if let Some(long_description) = &settings.mint_info.description_long {
        if !long_description.is_empty() {
            builder = builder.with_long_description(long_description.to_string());
        }
    }

    for contact in contacts {
        builder = builder.with_contact_info(contact);
    }

    if let Some(pubkey) = settings.mint_info.pubkey {
        builder = builder.with_pubkey(pubkey);
    }

    if let Some(icon_url) = &settings.mint_info.icon_url {
        if !icon_url.is_empty() {
            builder = builder.with_icon_url(icon_url.to_string());
        }
    }

    if let Some(motd) = &settings.mint_info.motd {
        if !motd.is_empty() {
            builder = builder.with_motd(motd.to_string());
        }
    }

    if let Some(tos_url) = &settings.mint_info.tos_url {
        if !tos_url.is_empty() {
            builder = builder.with_tos_url(tos_url.to_string());
        }
    }

    builder = builder.with_keyset_v2(settings.info.use_keyset_v2);

    builder = builder.with_keyset_rotation_interval(
        settings
            .info
            .keyset_rotation_interval_seconds
            .map(std::time::Duration::from_secs),
    );

    builder
}
/// Configures Lightning Network backend based on the specified backend type
async fn configure_lightning_backend(
    settings: &config::Settings,
    mut mint_builder: MintBuilder,
    _runtime: Option<std::sync::Arc<tokio::runtime::Runtime>>,
    work_dir: &Path,
    _kv_store: Option<Arc<dyn KVStore<Err = cdk::cdk_database::Error> + Send + Sync>>,
) -> Result<MintBuilder> {
    if settings.ln.is_empty() {
        tracing::info!("No Lightning backend configured");
        return Ok(mint_builder);
    }

    #[cfg(feature = "fakewallet")]
    let mut configure_fake_wallet_keyset_rotations = false;

    for ln_entry in &settings.ln {
        let mint_melt_limits = MintMeltLimits {
            mint_min: ln_entry.min_mint,
            mint_max: ln_entry.max_mint,
            melt_min: ln_entry.min_melt,
            melt_max: ln_entry.max_melt,
        };

        tracing::debug!(
            "Ln backend: {:?} (unit: {:?})",
            ln_entry.ln_backend,
            ln_entry.unit
        );

        match ln_entry.ln_backend {
            #[cfg(feature = "cln")]
            LnBackend::Cln => {
                let cln_settings = settings.cln.clone().ok_or_else(|| {
                    anyhow!("CLN backend selected but [cln] config section is missing")
                })?;
                let cln = cln_settings
                    .setup(
                        settings,
                        cdk::nuts::CurrencyUnit::Msat,
                        None,
                        work_dir,
                        _kv_store.clone(),
                    )
                    .await?;
                #[cfg(feature = "prometheus")]
                let cln = MetricsMintPayment::new(cln);

                mint_builder = configure_backend_for_unit(
                    settings,
                    mint_builder,
                    ln_entry.unit.clone(),
                    mint_melt_limits,
                    Arc::new(cln),
                )
                .await?;
            }
            #[cfg(feature = "lnbits")]
            LnBackend::LNbits => {
                let lnbits_settings = settings.lnbits.clone().ok_or_else(|| {
                    anyhow!("LNbits backend selected but [lnbits] config section is missing")
                })?;
                let lnbits = lnbits_settings
                    .setup(settings, ln_entry.unit.clone(), None, work_dir, None)
                    .await?;
                #[cfg(feature = "prometheus")]
                let lnbits = MetricsMintPayment::new(lnbits);

                mint_builder = configure_backend_for_unit(
                    settings,
                    mint_builder,
                    ln_entry.unit.clone(),
                    mint_melt_limits,
                    Arc::new(lnbits),
                )
                .await?;
            }
            #[cfg(feature = "lnd")]
            LnBackend::Lnd => {
                let lnd_settings = settings.lnd.clone().ok_or_else(|| {
                    anyhow!("LND backend selected but [lnd] config section is missing")
                })?;
                let lnd = lnd_settings
                    .setup(
                        settings,
                        cdk::nuts::CurrencyUnit::Msat,
                        None,
                        work_dir,
                        _kv_store.clone(),
                    )
                    .await?;
                #[cfg(feature = "prometheus")]
                let lnd = MetricsMintPayment::new(lnd);

                mint_builder = configure_backend_for_unit(
                    settings,
                    mint_builder,
                    ln_entry.unit.clone(),
                    mint_melt_limits,
                    Arc::new(lnd),
                )
                .await?;
            }
            #[cfg(feature = "fakewallet")]
            LnBackend::FakeWallet => {
                let fake_wallet = settings.fake_wallet.clone().ok_or_else(|| {
                    anyhow!(
                        "Fake wallet backend selected but [fake_wallet] config section is missing"
                    )
                })?;
                tracing::info!("Using fake wallet: {:?}", fake_wallet);

                let fake = fake_wallet
                    .setup(
                        settings,
                        ln_entry.unit.clone(),
                        None,
                        work_dir,
                        _kv_store.clone(),
                    )
                    .await?;
                #[cfg(feature = "prometheus")]
                let fake = MetricsMintPayment::new(fake);

                mint_builder = configure_backend_for_unit(
                    settings,
                    mint_builder,
                    ln_entry.unit.clone(),
                    mint_melt_limits,
                    Arc::new(fake),
                )
                .await?;

                configure_fake_wallet_keyset_rotations = true;
            }
            #[cfg(feature = "grpc-processor")]
            LnBackend::GrpcProcessor => {
                let grpc_processor = settings.grpc_processor.clone().ok_or_else(|| {
                    anyhow!(
                        "gRPC payment processor backend selected but [grpc_processor] config section is missing"
                    )
                })?;

                tracing::info!(
                    "Attempting to start with gRPC payment processor at {}:{}.",
                    grpc_processor.address,
                    grpc_processor.port
                );

                let processor = grpc_processor
                    .setup(settings, ln_entry.unit.clone(), None, work_dir, None)
                    .await?;
                #[cfg(feature = "prometheus")]
                let processor = MetricsMintPayment::new(processor);

                mint_builder = configure_backend_for_unit(
                    settings,
                    mint_builder,
                    ln_entry.unit.clone(),
                    mint_melt_limits,
                    Arc::new(processor),
                )
                .await?;
            }
            #[cfg(feature = "ldk-node")]
            LnBackend::LdkNode => {
                let ldk_node_settings = settings.ldk_node.clone().ok_or_else(|| {
                    anyhow!("LDK Node backend selected but [ldk_node] config section is missing")
                })?;
                tracing::info!("Using LDK Node backend: {:?}", ldk_node_settings);

                let ldk_node = ldk_node_settings
                    .setup(
                        settings,
                        ln_entry.unit.clone(),
                        _runtime.clone(),
                        work_dir,
                        None,
                    )
                    .await?;

                mint_builder = configure_backend_for_unit(
                    settings,
                    mint_builder,
                    ln_entry.unit.clone(),
                    mint_melt_limits,
                    Arc::new(ldk_node),
                )
                .await?;
            }
            LnBackend::None => {
                tracing::info!(
                    "No Lightning backend configured for unit {:?}",
                    ln_entry.unit
                );
            }
        };
    }

    #[cfg(feature = "fakewallet")]
    if configure_fake_wallet_keyset_rotations {
        let fake_wallet = settings.fake_wallet.as_ref().ok_or_else(|| {
            anyhow!("Fake wallet backend selected but [fake_wallet] config section is missing")
        })?;
        mint_builder = configure_fake_wallet_keyset_rotations_once(mint_builder, fake_wallet);
    }

    Ok(mint_builder)
}

#[cfg(feature = "fakewallet")]
fn configure_fake_wallet_keyset_rotations_once(
    mut mint_builder: MintBuilder,
    fake_wallet: &config::FakeWallet,
) -> MintBuilder {
    for rotation_cfg in &fake_wallet.keyset_rotations {
        use cdk::mint::KeysetRotation;

        let amounts = cdk::mint::UnitConfig::default().amounts;
        let final_expiry = if rotation_cfg.expired {
            Some(cdk::util::unix_time().saturating_sub(3600))
        } else {
            None
        };

        mint_builder = mint_builder.with_keyset_rotation(KeysetRotation {
            unit: rotation_cfg.unit.clone(),
            amounts,
            input_fee_ppk: rotation_cfg.input_fee_ppk,
            use_keyset_v2: rotation_cfg.version == "v2",
            final_expiry,
        });
    }

    mint_builder
}

/// Configures Onchain backend based on the specified backend type
async fn configure_onchain_backend(
    settings: &config::Settings,
    #[cfg_attr(not(feature = "bdk"), allow(unused_mut))] mut mint_builder: MintBuilder,
    _runtime: Option<std::sync::Arc<tokio::runtime::Runtime>>,
    _work_dir: &Path,
    _kv_store: Option<Arc<dyn KVStore<Err = cdk::cdk_database::Error> + Send + Sync>>,
) -> Result<MintBuilder> {
    use config::OnchainBackend;
    #[cfg(feature = "bdk")]
    use setup::OnchainBackendSetup;

    if let Some(onchain_settings) = &settings.onchain {
        match onchain_settings.onchain_backend {
            #[cfg(feature = "bdk")]
            OnchainBackend::Bdk => {
                let mint_melt_limits = MintMeltLimits {
                    mint_min: onchain_settings.min_mint,
                    mint_max: onchain_settings.max_mint,
                    melt_min: onchain_settings.min_melt,
                    melt_max: onchain_settings.max_melt,
                };

                let bdk_settings = settings.bdk.clone().ok_or_else(|| {
                    anyhow!("BDK onchain backend selected but [bdk] config section is missing")
                })?;
                let bdk = bdk_settings
                    .setup(
                        settings,
                        cdk::nuts::CurrencyUnit::Sat,
                        None,
                        _work_dir,
                        _kv_store,
                    )
                    .await?;
                let bdk = Arc::new(bdk);

                mint_builder = configure_backend_for_unit(
                    settings,
                    mint_builder,
                    cdk::nuts::CurrencyUnit::Sat,
                    mint_melt_limits,
                    bdk,
                )
                .await?;
            }
            OnchainBackend::None => {}
            #[cfg(feature = "fakewallet")]
            OnchainBackend::FakeWallet => {
                let has_lightning_backend = settings
                    .ln
                    .iter()
                    .any(|ln| ln.ln_backend != LnBackend::None);
                let has_real_ln_backend = settings
                    .ln
                    .iter()
                    .any(|ln| !matches!(ln.ln_backend, LnBackend::None | LnBackend::FakeWallet));

                if !has_lightning_backend {
                    let mint_melt_limits = MintMeltLimits {
                        mint_min: onchain_settings.min_mint,
                        mint_max: onchain_settings.max_mint,
                        melt_min: onchain_settings.min_melt,
                        melt_max: onchain_settings.max_melt,
                    };
                    let fake_wallet = settings
                        .fake_wallet
                        .clone()
                        .ok_or_else(|| anyhow!("Fake wallet config section is missing"))?;

                    for unit in fake_wallet.clone().supported_units {
                        let fake = fake_wallet
                            .setup(settings, unit.clone(), None, _work_dir, _kv_store.clone())
                            .await?;
                        #[cfg(feature = "prometheus")]
                        let fake = MetricsMintPayment::new(fake);

                        mint_builder = configure_backend_for_methods(
                            settings,
                            mint_builder,
                            unit,
                            mint_melt_limits,
                            Arc::new(fake),
                            vec![PaymentMethod::Known(KnownMethod::Onchain)],
                        )
                        .await?;
                    }
                } else if has_real_ln_backend {
                    bail!(
                        "onchain_backend = \"fakewallet\" cannot be combined with a real Lightning backend"
                    );
                }
            }
        }
    }

    Ok(mint_builder)
}

/// Helper function to configure a mint builder with a lightning backend for a specific currency unit
async fn configure_backend_for_unit(
    settings: &config::Settings,
    mint_builder: MintBuilder,
    unit: cdk::nuts::CurrencyUnit,
    mint_melt_limits: MintMeltLimits,
    backend: Arc<dyn MintPayment<Err = cdk_common::payment::Error> + Send + Sync>,
) -> Result<MintBuilder> {
    let payment_settings = backend.get_settings().await?;
    validate_backend_unit(&unit, &payment_settings.unit)?;

    let mut methods = Vec::new();

    // Add bolt11 if supported by payment processor
    if payment_settings.bolt11.is_some() {
        methods.push(PaymentMethod::Known(KnownMethod::Bolt11));
    }

    // Add bolt12 if supported by payment processor
    if payment_settings.bolt12.is_some() {
        methods.push(PaymentMethod::Known(KnownMethod::Bolt12));
    }

    // Add onchain if supported by payment processor
    if payment_settings.onchain.is_some() {
        methods.push(PaymentMethod::Known(KnownMethod::Onchain));
    }

    // Add custom methods from payment settings
    for method_name in payment_settings.custom.keys() {
        methods.push(PaymentMethod::from(method_name.as_str()));
    }

    configure_backend_for_methods(
        settings,
        mint_builder,
        unit,
        mint_melt_limits,
        backend,
        methods,
    )
    .await
}

async fn configure_backend_for_methods(
    settings: &config::Settings,
    mut mint_builder: MintBuilder,
    unit: cdk::nuts::CurrencyUnit,
    mint_melt_limits: MintMeltLimits,
    backend: Arc<dyn MintPayment<Err = cdk_common::payment::Error> + Send + Sync>,
    methods: Vec<PaymentMethod>,
) -> Result<MintBuilder> {
    // Add all supported payment methods to the mint builder
    for method in &methods {
        mint_builder
            .add_payment_processor(
                unit.clone(),
                method.clone(),
                mint_melt_limits,
                backend.clone(),
            )
            .await?;
    }

    // Configure NUT17 (WebSocket support) for all payment methods
    for method in &methods {
        let method_str = method.to_string();
        let nut17_supported = match method_str.as_str() {
            "bolt11" => SupportedMethods::default_bolt11(unit.clone()),
            "bolt12" => SupportedMethods::default_bolt12(unit.clone()),
            _ => SupportedMethods::default_custom(method.clone(), unit.clone()),
        };
        mint_builder = mint_builder.with_supported_websockets(nut17_supported);
    }

    if let Some(input_fee) = settings.info.input_fee_ppk {
        mint_builder.set_unit_fee(&unit, input_fee)?;
    }

    Ok(mint_builder)
}

fn validate_backend_unit(
    configured_unit: &cdk::nuts::CurrencyUnit,
    backend_unit: &str,
) -> Result<()> {
    let backend_unit = cdk::nuts::CurrencyUnit::from_str(backend_unit)
        .with_context(|| format!("Payment backend returned invalid unit `{backend_unit}`"))?;

    if units_are_compatible(&backend_unit, configured_unit) {
        return Ok(());
    }

    bail!(
        "Payment backend reports unit {} but config registers unit {}; only matching units or sat/msat conversions are supported",
        backend_unit,
        configured_unit
    )
}

fn units_are_compatible(
    backend_unit: &cdk::nuts::CurrencyUnit,
    configured_unit: &cdk::nuts::CurrencyUnit,
) -> bool {
    backend_unit == configured_unit
        || matches!(
            (backend_unit, configured_unit),
            (cdk::nuts::CurrencyUnit::Sat, cdk::nuts::CurrencyUnit::Msat)
                | (cdk::nuts::CurrencyUnit::Msat, cdk::nuts::CurrencyUnit::Sat)
        )
}

/// Configures cache settings with support for custom payment methods
async fn configure_cache(
    settings: &config::Settings,
    mint_builder: MintBuilder,
    payment_methods: &[String],
) -> Result<MintBuilder> {
    let mut cached_endpoints = vec![
        // Always include swap endpoint
        CachedEndpoint::new(NUT19Method::Post, NUT19Path::Swap),
    ];

    // Add cache endpoints for each configured payment method
    for method in payment_methods {
        // All payment methods (including bolt11, bolt12) use custom paths now
        cached_endpoints.push(CachedEndpoint::new(
            NUT19Method::Post,
            NUT19Path::custom_mint(method),
        ));
        cached_endpoints.push(CachedEndpoint::new(
            NUT19Method::Post,
            NUT19Path::custom_melt(method),
        ));
    }

    let cache: HttpCache = HttpCache::from_config(settings.info.http_cache.clone()).await?;
    Ok(mint_builder.with_cache(Some(cache.ttl.as_secs()), cached_endpoints))
}

async fn setup_authentication(
    settings: &config::Settings,
    _work_dir: &Path,
    mut mint_builder: MintBuilder,
    _password: Option<String>,
) -> Result<(
    MintBuilder,
    Option<cdk_common::database::DynMintAuthDatabase>,
)> {
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
                        let password = _password.clone().ok_or_else(|| {
                            anyhow!("Password required when sqlcipher feature is enabled")
                        })?;
                        MintSqliteAuthDatabase::new((sql_db_path, password)).await?
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

                    let auth_db_config = PgConfig::new(
                        auth_pg_config.url.as_str(),
                        auth_pg_config.tls_mode.as_deref(),
                        auth_pg_config.max_connections,
                        auth_pg_config.connection_timeout_seconds,
                    );
                    Arc::new(MintPgAuthDatabase::new(auth_db_config).await?)
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

        protected_endpoints.insert(mint_blind_auth_endpoint.clone(), AuthRequired::Clear);

        clear_auth_endpoints.push(mint_blind_auth_endpoint);

        // Helper function to add endpoint based on auth type
        let mut add_endpoint = |endpoint: ProtectedEndpoint, auth_type: &AuthType| {
            match auth_type {
                AuthType::Blind => {
                    protected_endpoints.insert(endpoint.clone(), AuthRequired::Blind);
                    blind_auth_endpoints.push(endpoint);
                }
                AuthType::Clear => {
                    protected_endpoints.insert(endpoint.clone(), AuthRequired::Clear);
                    clear_auth_endpoints.push(endpoint);
                }
                AuthType::None => {
                    unprotected_endpoints.push(endpoint);
                }
            };
        };

        // Payment method endpoints (bolt11, bolt12, custom) will be added dynamically
        // after the mint is built and we can query the payment processors for their
        // supported methods. See the start_services_with_shutdown function where we
        // add auth endpoints for all configured payment methods.

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

        // Custom protected_endpoints will be added dynamically after the mint is built
        // and we can query the payment processors for their supported methods.
        // For now, we don't add any custom endpoints here - they'll be added in the
        // start_services_with_shutdown function after we have access to the mint instance.

        mint_builder = mint_builder.with_auth(
            auth_localstore.clone(),
            auth_settings.openid_discovery,
            auth_settings.openid_client_id,
            clear_auth_endpoints,
        );
        mint_builder =
            mint_builder.with_blind_auth(auth_settings.mint_max_bat, blind_auth_endpoints);

        let mut tx = auth_localstore.begin_transaction().await?;

        if !unprotected_endpoints.is_empty() {
            tx.remove_protected_endpoints(unprotected_endpoints).await?;
        }
        if !protected_endpoints.is_empty() {
            tx.add_protected_endpoints(protected_endpoints).await?;
        }
        tx.commit().await?;

        Ok((mint_builder, Some(auth_localstore)))
    } else {
        Ok((mint_builder, None))
    }
}

/// Build mints with the configured the signing method (remote signatory or local seed)
async fn build_mint(
    settings: &config::Settings,
    keystore: Arc<dyn MintKeysDatabase<Err = cdk_database::Error> + Send + Sync>,
    mint_builder: MintBuilder,
) -> Result<Mint> {
    if let Some(signatory) = settings.enabled_signatory() {
        let tls_dir = signatory.tls_dir.clone();

        if tls_dir.is_none() {
            if !signatory.allow_insecure {
                bail!(
                    "gRPC signatory TLS is not configured. Set [signatory].tls_dir or \
                     [signatory].allow_insecure = true to connect without TLS"
                );
            }

            tracing::warn!(
                "No gRPC signatory TLS directory configured; connecting without TLS because \
                 allow_insecure is true"
            );
        }

        tracing::info!(
            "Connecting to remote signatory to {}:{} with TLS directory {:?}",
            signatory.address,
            signatory.port,
            tls_dir.clone()
        );

        Ok(mint_builder
            .build_with_signatory(Arc::new(
                cdk_signatory::SignatoryRpcClient::new(&signatory.address, signatory.port, tls_dir)
                    .await?,
            ))
            .await?)
    } else if let Some(seed) = settings.info.seed.clone().filter(|seed| !seed.is_empty()) {
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
    _work_dir: &Path,
    mint_builder_info: cdk::nuts::MintInfo,
    shutdown_signal: impl std::future::Future<Output = ()> + Send + 'static,
    routers: Vec<Router>,
    auth_localstore: Option<cdk_common::database::DynMintAuthDatabase>,
) -> Result<()> {
    let listen_addr = settings.info.listen_host.clone();
    let listen_port = settings.info.listen_port;
    let cache: HttpCache = HttpCache::from_config(settings.info.http_cache.clone()).await?;

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

                let tls_dir = rpc_settings.tls_dir.unwrap_or(_work_dir.join("tls"));

                let tls_dir = if tls_dir.exists() {
                    Some(tls_dir)
                } else if rpc_settings.allow_insecure {
                    tracing::warn!(
                        "TLS directory does not exist: {}. Starting RPC server in INSECURE mode without TLS encryption because allow_insecure is true",
                        tls_dir.display()
                    );
                    None
                } else {
                    bail!(
                        "Management RPC TLS directory does not exist: {}. Set \
                         [mint_management_rpc].tls_dir or \
                         [mint_management_rpc].allow_insecure = true to start without \
                         TLS",
                        tls_dir.display()
                    );
                };

                mint_rpc.start(tls_dir).await?;

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

    // Get custom payment methods from payment processors
    let mut custom_methods = mint.get_custom_payment_methods().await?;

    // Add bolt11 if it's supported by any payment processor
    let bolt11_method = PaymentMethod::Known(KnownMethod::Bolt11);
    let bolt11_supported =
        nut04_methods.contains(&&bolt11_method) || nut05_methods.contains(&&bolt11_method);
    // Add bolt12 if it's supported by any payment processor
    let bolt12_method = PaymentMethod::Known(KnownMethod::Bolt12);
    let bolt12_supported =
        nut04_methods.contains(&&bolt12_method) || nut05_methods.contains(&&bolt12_method);

    // Add onchain if it's supported by any payment processor
    let onchain_method = PaymentMethod::Known(KnownMethod::Onchain);
    let onchain_supported =
        nut04_methods.contains(&&onchain_method) || nut05_methods.contains(&&onchain_method);

    if bolt11_supported
        && !custom_methods.contains(&PaymentMethod::Known(KnownMethod::Bolt11).to_string())
    {
        custom_methods.push(PaymentMethod::Known(KnownMethod::Bolt11).to_string());
    }
    if bolt12_supported
        && !custom_methods.contains(&PaymentMethod::Known(KnownMethod::Bolt12).to_string())
    {
        custom_methods.push(PaymentMethod::Known(KnownMethod::Bolt12).to_string());
    }
    if onchain_supported
        && !custom_methods.contains(&PaymentMethod::Known(KnownMethod::Onchain).to_string())
    {
        custom_methods.push(PaymentMethod::Known(KnownMethod::Onchain).to_string());
    }

    tracing::info!("Payment methods: {:?}", custom_methods);

    // Configure auth for custom payment methods if auth is enabled
    if let (Some(ref auth_settings), Some(auth_db)) = (&settings.auth, &auth_localstore) {
        if auth_settings.auth_enabled {
            use std::collections::HashMap;

            use cdk::nuts::nut21::{Method, ProtectedEndpoint, RoutePath};
            use cdk::nuts::AuthRequired;

            use crate::config::AuthType;

            // First, remove all existing payment-method-related endpoints from the database
            // to ensure old payment methods don't persist when configuration changes
            let existing_endpoints = auth_db.get_auth_for_endpoints().await?;
            let payment_method_endpoints_to_remove: Vec<ProtectedEndpoint> = existing_endpoints
                .keys()
                .filter(|endpoint| {
                    matches!(
                        endpoint.path,
                        RoutePath::MintQuote(_)
                            | RoutePath::Mint(_)
                            | RoutePath::MeltQuote(_)
                            | RoutePath::Melt(_)
                    )
                })
                .cloned()
                .collect();

            if !payment_method_endpoints_to_remove.is_empty() {
                tracing::debug!(
                    "Removing {} old payment method endpoints from database",
                    payment_method_endpoints_to_remove.len()
                );
                let mut tx = auth_db.begin_transaction().await?;
                tx.remove_protected_endpoints(payment_method_endpoints_to_remove)
                    .await?;
                tx.commit().await?;
            }

            // Now add endpoints for current payment methods
            if !custom_methods.is_empty() {
                let mut protected_endpoints = HashMap::new();

                for method_name in &custom_methods {
                    tracing::debug!("Adding auth endpoints for payment method: {}", method_name);

                    // Determine auth type based on settings
                    let mint_quote_auth = match auth_settings.get_mint_quote {
                        AuthType::Clear => Some(AuthRequired::Clear),
                        AuthType::Blind => Some(AuthRequired::Blind),
                        AuthType::None => None,
                    };

                    let check_mint_quote_auth = match auth_settings.check_mint_quote {
                        AuthType::Clear => Some(AuthRequired::Clear),
                        AuthType::Blind => Some(AuthRequired::Blind),
                        AuthType::None => None,
                    };

                    let mint_auth = match auth_settings.mint {
                        AuthType::Clear => Some(AuthRequired::Clear),
                        AuthType::Blind => Some(AuthRequired::Blind),
                        AuthType::None => None,
                    };

                    let melt_quote_auth = match auth_settings.get_melt_quote {
                        AuthType::Clear => Some(AuthRequired::Clear),
                        AuthType::Blind => Some(AuthRequired::Blind),
                        AuthType::None => None,
                    };

                    let check_melt_quote_auth = match auth_settings.check_melt_quote {
                        AuthType::Clear => Some(AuthRequired::Clear),
                        AuthType::Blind => Some(AuthRequired::Blind),
                        AuthType::None => None,
                    };

                    let melt_auth = match auth_settings.melt {
                        AuthType::Clear => Some(AuthRequired::Clear),
                        AuthType::Blind => Some(AuthRequired::Blind),
                        AuthType::None => None,
                    };

                    // Create endpoints for each payment method operation
                    if let Some(auth) = mint_quote_auth {
                        protected_endpoints.insert(
                            ProtectedEndpoint::new(
                                Method::Post,
                                RoutePath::MintQuote(method_name.clone()),
                            ),
                            auth,
                        );
                    }
                    if let Some(auth) = check_mint_quote_auth {
                        protected_endpoints.insert(
                            ProtectedEndpoint::new(
                                Method::Get,
                                RoutePath::MintQuote(method_name.clone()),
                            ),
                            auth,
                        );
                    }
                    if let Some(auth) = mint_auth {
                        protected_endpoints.insert(
                            ProtectedEndpoint::new(
                                Method::Post,
                                RoutePath::Mint(method_name.clone()),
                            ),
                            auth,
                        );
                    }
                    if let Some(auth) = melt_quote_auth {
                        protected_endpoints.insert(
                            ProtectedEndpoint::new(
                                Method::Post,
                                RoutePath::MeltQuote(method_name.clone()),
                            ),
                            auth,
                        );
                    }
                    if let Some(auth) = check_melt_quote_auth {
                        protected_endpoints.insert(
                            ProtectedEndpoint::new(
                                Method::Get,
                                RoutePath::MeltQuote(method_name.clone()),
                            ),
                            auth,
                        );
                    }
                    if let Some(auth) = melt_auth {
                        protected_endpoints.insert(
                            ProtectedEndpoint::new(
                                Method::Post,
                                RoutePath::Melt(method_name.clone()),
                            ),
                            auth,
                        );
                    }
                }

                // Add all custom endpoints in one transaction
                if !protected_endpoints.is_empty() {
                    let mut tx = auth_db.begin_transaction().await?;
                    tx.add_protected_endpoints(protected_endpoints).await?;
                    tx.commit().await?;
                }
            }
        }
    }

    let v1_service = cdk_axum::create_mint_router_with_custom_cache(
        Arc::clone(&mint),
        cache,
        custom_methods,
        settings.info.enable_info_page.unwrap_or(true),
    )
    .await?;

    let mut mint_service = Router::new()
        .merge(v1_service)
        .layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT_BYTES))
        .layer(
            ServiceBuilder::new()
                .layer(RequestDecompressionLayer::new())
                .layer(CompressionLayer::new()),
        )
        .layer(TraceLayer::new_for_http());

    for router in routers {
        mint_service = mint_service.merge(router);
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

                let address = format!("{addr}:{port}")
                    .parse()
                    .with_context(|| format!("Invalid Prometheus address {addr}:{port}"))?;

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

    mint.start().await?;

    let socket_addr = SocketAddr::from_str(&format!("{listen_addr}:{listen_port}"))?;

    let listener = tokio::net::TcpListener::bind(socket_addr).await?;

    tracing::info!("listening on {}", listener.local_addr()?);

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
    let (mint_builder, auth_localstore) =
        setup_authentication(settings, work_dir, mint_builder, db_password).await?;

    let config_mint_info = mint_builder.current_mint_info();

    let mint = build_mint(settings, keystore, mint_builder).await?;

    tracing::debug!("Mint built from builder.");

    let mint = Arc::new(mint);

    start_services_with_shutdown(
        mint.clone(),
        settings,
        work_dir,
        config_mint_info,
        shutdown_signal,
        routers,
        auth_localstore,
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::fs;

    use cdk::nuts::{CurrencyUnit, MintMethodSettings, PaymentMethod};

    use super::*;

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn temp_seed_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cdk_mintd_{name}_{}", std::process::id()))
    }

    #[test]
    fn apply_seed_file_sets_mint_mnemonic_from_trimmed_file_contents() {
        let seed_file = temp_seed_file("seed_file_sets_seed");
        fs::write(&seed_file, format!("  {TEST_MNEMONIC}\n")).expect("seed file should be written");
        let mut settings = config::Settings {
            info: config::Info {
                seed: Some("raw seed from config".to_string()),
                mnemonic: Some("mnemonic from config".to_string()),
                ..Default::default()
            },
            signatory: Some(config::Signatory {
                enabled: true,
                address: "127.0.0.1".to_string(),
                port: 15060,
                tls_dir: Some("/tmp/certs".into()),
                allow_insecure: false,
            }),
            ..Default::default()
        };

        apply_seed_file(&mut settings, &seed_file).expect("seed file should be applied");

        assert_eq!(settings.info.seed, None);
        assert_eq!(settings.info.mnemonic, Some(TEST_MNEMONIC.to_string()));
        assert_eq!(
            settings
                .signatory
                .as_ref()
                .map(|signatory| signatory.address.clone()),
            Some("127.0.0.1".to_string())
        );
        assert_eq!(
            settings.signatory.as_ref().map(|signatory| signatory.port),
            Some(15060)
        );
        assert_eq!(
            settings
                .signatory
                .as_ref()
                .and_then(|signatory| signatory.tls_dir.clone()),
            Some("/tmp/certs".into())
        );

        let _ = fs::remove_file(&seed_file);
    }

    #[cfg(feature = "bdk")]
    #[test]
    fn apply_seed_file_sets_active_bdk_mnemonic() {
        use crate::config::{Bdk, Onchain, OnchainBackend};

        let seed_file = temp_seed_file("seed_file_sets_bdk_seed");
        fs::write(&seed_file, TEST_MNEMONIC).expect("seed file should be written");
        let mut settings = config::Settings {
            onchain: Some(Onchain {
                onchain_backend: OnchainBackend::Bdk,
                ..Default::default()
            }),
            bdk: Some(Bdk {
                mnemonic: Some("old bdk mnemonic".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        apply_seed_file(&mut settings, &seed_file).expect("seed file should be applied");

        assert_eq!(
            settings
                .bdk
                .expect("bdk settings should be present")
                .mnemonic,
            Some(TEST_MNEMONIC.to_string())
        );

        let _ = fs::remove_file(&seed_file);
    }

    #[cfg(feature = "ldk-node")]
    #[test]
    fn apply_seed_file_sets_active_ldk_node_mnemonic() {
        use crate::config::{LdkNode, Ln, LnBackend};

        let seed_file = temp_seed_file("seed_file_sets_ldk_seed");
        fs::write(&seed_file, TEST_MNEMONIC).expect("seed file should be written");
        let mut settings = config::Settings {
            ln: vec![Ln {
                ln_backend: LnBackend::LdkNode,
                ..Default::default()
            }],
            ldk_node: Some(LdkNode {
                ldk_node_mnemonic: Some("old ldk mnemonic".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        apply_seed_file(&mut settings, &seed_file).expect("seed file should be applied");

        assert_eq!(
            settings
                .ldk_node
                .expect("ldk node settings should be present")
                .ldk_node_mnemonic,
            Some(TEST_MNEMONIC.to_string())
        );

        let _ = fs::remove_file(&seed_file);
    }

    #[test]
    fn apply_seed_file_rejects_empty_seed_file() {
        let seed_file = temp_seed_file("empty_seed_file");
        fs::write(&seed_file, "\n\t ").expect("seed file should be written");
        let mut settings = config::Settings::default();

        let err = apply_seed_file(&mut settings, &seed_file)
            .expect_err("empty seed file should be rejected");

        assert!(err.to_string().contains("is empty"));
        assert_eq!(settings.info.seed, None);

        let _ = fs::remove_file(&seed_file);
    }

    #[test]
    fn apply_seed_file_rejects_invalid_seed_phrase() {
        let seed_file = temp_seed_file("invalid_seed_file");
        fs::write(&seed_file, "not a valid seed phrase").expect("seed file should be written");
        let mut settings = config::Settings::default();

        let err = apply_seed_file(&mut settings, &seed_file)
            .expect_err("invalid seed phrase should be rejected");

        assert!(err.to_string().contains("Invalid seed phrase"));
        assert_eq!(settings.info.mnemonic, None);

        let _ = fs::remove_file(&seed_file);
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn load_settings_from_args_applies_seed_file_before_validation() {
        let _env_lock = crate::test_utils::env_lock();
        clear_mintd_env();

        let temp_dir = crate::test_utils::unique_temp_path("seed_file_only_signing");
        fs::create_dir_all(&temp_dir).expect("temp directory should be created");
        let config_path = temp_dir.join("config.toml");
        fs::write(
            &config_path,
            r#"
[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
"#,
        )
        .expect("config file should be written");
        let seed_file = temp_dir.join("seed.txt");
        fs::write(&seed_file, TEST_MNEMONIC).expect("seed file should be written");

        let args = CLIArgs {
            work_dir: None,
            #[cfg(feature = "sqlcipher")]
            password: "test-password".to_string(),
            config: Some(config_path),
            seed_file: Some(seed_file),
            enable_logging: false,
        };

        let settings = load_settings_from_args(&temp_dir, &args)
            .expect("seed-file-only signing should pass validation");

        assert_eq!(settings.info.mnemonic.as_deref(), Some(TEST_MNEMONIC));
        let _ = fs::remove_dir_all(&temp_dir);
        clear_mintd_env();
    }

    #[cfg(all(feature = "fakewallet", feature = "sqlite"))]
    #[tokio::test]
    async fn fakewallet_dispatcher_uses_ln_entry_unit() {
        use cdk::mint::MintBuilder;
        use cdk_sqlite::mint::memory;

        use crate::config::{FakeWallet, Ln, LnBackend};

        let settings = config::Settings {
            ln: vec![Ln {
                ln_backend: LnBackend::FakeWallet,
                unit: CurrencyUnit::Eur,
                ..Default::default()
            }],
            fake_wallet: Some(FakeWallet::default()),
            ..Default::default()
        };

        let localstore = Arc::new(memory::empty().await.unwrap());
        let builder = MintBuilder::new(localstore);
        let builder =
            configure_lightning_backend(&settings, builder, None, &std::env::temp_dir(), None)
                .await
                .expect("dispatcher should succeed");

        let mint_info = builder.current_mint_info();
        let units: Vec<_> = mint_info
            .nuts
            .nut04
            .methods
            .iter()
            .map(|m| m.unit.clone())
            .collect();
        assert!(
            units.contains(&CurrencyUnit::Eur),
            "expected Eur, got {units:?}"
        );
        assert!(
            !units.contains(&CurrencyUnit::Sat),
            "Sat would only appear if supported_units leaked through; got {units:?}"
        );
    }

    #[test]
    fn backend_unit_validation_allows_matching_units() {
        validate_backend_unit(&CurrencyUnit::Eur, "EUR").expect("matching units should pass");
    }

    #[test]
    fn backend_unit_validation_allows_sat_msat_pair() {
        validate_backend_unit(&CurrencyUnit::Sat, "MSAT")
            .expect("sat/msat compatible units should pass");
        validate_backend_unit(&CurrencyUnit::Msat, "SAT")
            .expect("msat/sat compatible units should pass");
    }

    #[test]
    fn backend_unit_validation_rejects_unsupported_conversion() {
        let err = validate_backend_unit(&CurrencyUnit::Eur, "SAT")
            .expect_err("sat backend should not advertise eur");

        assert!(
            err.to_string().contains("only matching units"),
            "error should explain the supported conversions: {err}"
        );
    }

    #[cfg(feature = "cln")]
    #[test]
    fn expand_path_expands_bare_tilde_without_panic() {
        let expanded = expand_path("~");

        assert_eq!(expanded, home::home_dir());
    }

    #[cfg(feature = "cln")]
    #[test]
    fn expand_path_keeps_named_tilde_paths_literal() {
        let expanded = expand_path("~foo").expect("path should be returned");

        assert_eq!(expanded, PathBuf::from("~foo"));
    }

    #[cfg(all(feature = "fakewallet", feature = "sqlite"))]
    #[tokio::test]
    async fn duplicate_ln_unit_method_pair_is_rejected() {
        use cdk::mint::MintBuilder;
        use cdk_sqlite::mint::memory;

        use crate::config::{FakeWallet, Ln, LnBackend};

        let settings = config::Settings {
            ln: vec![
                Ln {
                    ln_backend: LnBackend::FakeWallet,
                    unit: CurrencyUnit::Sat,
                    ..Default::default()
                },
                Ln {
                    ln_backend: LnBackend::FakeWallet,
                    unit: CurrencyUnit::Sat,
                    ..Default::default()
                },
            ],
            fake_wallet: Some(FakeWallet::default()),
            ..Default::default()
        };

        let localstore = Arc::new(memory::empty().await.unwrap());
        let builder = MintBuilder::new(localstore);
        let err =
            configure_lightning_backend(&settings, builder, None, &std::env::temp_dir(), None)
                .await
                .expect_err("duplicate unit/method pair should be rejected");

        assert!(err.to_string().contains("Duplicate payment processor"));
    }

    #[cfg(all(feature = "fakewallet", feature = "sqlite"))]
    #[tokio::test]
    async fn empty_ln_vec_returns_unchanged_builder() {
        use cdk::mint::MintBuilder;
        use cdk_sqlite::mint::memory;

        let settings = config::Settings {
            ln: vec![],
            ..Default::default()
        };

        let localstore = Arc::new(memory::empty().await.unwrap());
        let builder = MintBuilder::new(localstore);
        let builder =
            configure_lightning_backend(&settings, builder, None, &std::env::temp_dir(), None)
                .await
                .expect("empty ln should succeed");

        let mint_info = builder.current_mint_info();
        assert!(
            mint_info.nuts.nut04.methods.is_empty(),
            "no backends should be registered"
        );
    }

    #[cfg(all(feature = "fakewallet", feature = "sqlite"))]
    #[tokio::test]
    async fn ln_backend_none_logs_and_continues() {
        use cdk::mint::MintBuilder;
        use cdk_sqlite::mint::memory;

        use crate::config::{Ln, LnBackend};

        let settings = config::Settings {
            ln: vec![Ln {
                ln_backend: LnBackend::None,
                unit: CurrencyUnit::Sat,
                ..Default::default()
            }],
            ..Default::default()
        };

        let localstore = Arc::new(memory::empty().await.unwrap());
        let builder = MintBuilder::new(localstore);
        let builder =
            configure_lightning_backend(&settings, builder, None, &std::env::temp_dir(), None)
                .await
                .expect("LnBackend::None should succeed");

        let mint_info = builder.current_mint_info();
        assert!(
            mint_info.nuts.nut04.methods.is_empty(),
            "LnBackend::None should not register any methods"
        );
    }

    #[cfg(all(feature = "fakewallet", feature = "sqlite"))]
    #[tokio::test]
    async fn onchain_backend_none_returns_unchanged() {
        use cdk::mint::MintBuilder;
        use cdk_sqlite::mint::memory;

        use crate::config::{Onchain, OnchainBackend};

        let settings = config::Settings {
            onchain: Some(Onchain {
                onchain_backend: OnchainBackend::None,
                ..Default::default()
            }),
            ..Default::default()
        };

        let localstore = Arc::new(memory::empty().await.unwrap());
        let builder = MintBuilder::new(localstore);
        let builder =
            configure_onchain_backend(&settings, builder, None, &std::env::temp_dir(), None)
                .await
                .expect("OnchainBackend::None should succeed");

        let mint_info = builder.current_mint_info();
        assert!(
            mint_info.nuts.nut04.methods.is_empty(),
            "OnchainBackend::None should not register any methods"
        );
    }

    #[cfg(all(feature = "fakewallet", feature = "sqlite"))]
    #[tokio::test]
    async fn fakewallet_onchain_no_lightning_configures_onchain_methods() {
        use cdk::mint::MintBuilder;
        use cdk_sqlite::mint::memory;

        use crate::config::{FakeWallet, Ln, LnBackend, Onchain, OnchainBackend};

        let settings = config::Settings {
            ln: vec![Ln {
                ln_backend: LnBackend::None,
                ..Default::default()
            }],
            onchain: Some(Onchain {
                onchain_backend: OnchainBackend::FakeWallet,
                ..Default::default()
            }),
            fake_wallet: Some(FakeWallet::default()),
            ..Default::default()
        };

        let localstore = Arc::new(memory::empty().await.unwrap());
        let builder = MintBuilder::new(localstore);
        let builder =
            configure_onchain_backend(&settings, builder, None, &std::env::temp_dir(), None)
                .await
                .expect("fakewallet onchain should succeed");

        let mint_info = builder.current_mint_info();
        let methods: Vec<_> = mint_info
            .nuts
            .nut04
            .methods
            .iter()
            .map(|m| m.method.clone())
            .collect();
        assert!(
            methods.contains(&PaymentMethod::Known(KnownMethod::Onchain)),
            "expected onchain method, got {methods:?}"
        );
    }

    #[cfg(all(feature = "fakewallet", feature = "cln", feature = "sqlite"))]
    #[tokio::test]
    async fn fakewallet_onchain_with_real_ln_bails() {
        use cdk::mint::MintBuilder;
        use cdk_sqlite::mint::memory;

        use crate::config::{Ln, LnBackend, Onchain, OnchainBackend};

        let settings = config::Settings {
            ln: vec![Ln {
                ln_backend: LnBackend::Cln,
                unit: CurrencyUnit::Sat,
                ..Default::default()
            }],
            onchain: Some(Onchain {
                onchain_backend: OnchainBackend::FakeWallet,
                ..Default::default()
            }),
            ..Default::default()
        };

        let localstore = Arc::new(memory::empty().await.unwrap());
        let builder = MintBuilder::new(localstore);
        let err = configure_onchain_backend(&settings, builder, None, &std::env::temp_dir(), None)
            .await
            .expect_err("fakewallet onchain with real LN should bail");

        assert!(
            err.to_string().contains("fakewallet"),
            "error should mention fakewallet: {err}"
        );
    }

    #[cfg(all(feature = "fakewallet", feature = "sqlite"))]
    #[tokio::test]
    async fn configure_mint_builder_no_backends_bails() {
        use cdk::mint::MintBuilder;
        use cdk_sqlite::mint::memory;

        use crate::config::{Ln, LnBackend};

        let settings = config::Settings {
            ln: vec![Ln {
                ln_backend: LnBackend::None,
                ..Default::default()
            }],
            ..Default::default()
        };

        let localstore = Arc::new(memory::empty().await.unwrap());
        let builder = MintBuilder::new(localstore);
        let err = configure_mint_builder(&settings, builder, None, &std::env::temp_dir(), None)
            .await
            .expect_err("no payment backends should bail");

        assert!(
            err.to_string().contains("At least one payment backend"),
            "error should mention missing backends: {err}"
        );
    }

    #[cfg(all(feature = "fakewallet", feature = "sqlite", feature = "bdk"))]
    #[tokio::test]
    async fn configure_mint_builder_fake_wallet_with_bdk_onchain_bails() {
        use cdk::mint::MintBuilder;
        use cdk_sqlite::mint::memory;

        use crate::config::{Bdk, FakeWallet, Ln, LnBackend, Onchain, OnchainBackend};

        let settings = config::Settings {
            ln: vec![Ln {
                ln_backend: LnBackend::FakeWallet,
                ..Default::default()
            }],
            onchain: Some(Onchain {
                onchain_backend: OnchainBackend::Bdk,
                ..Default::default()
            }),
            fake_wallet: Some(FakeWallet::default()),
            bdk: Some(Bdk {
                network: Some("mainnet".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let localstore = Arc::new(memory::empty().await.unwrap());
        let builder = MintBuilder::new(localstore);
        let err = configure_mint_builder(&settings, builder, None, &std::env::temp_dir(), None)
            .await
            .expect_err("fake wallet with BDK onchain should bail");

        assert!(
            err.to_string().contains("fakewallet") && err.to_string().contains("bdk"),
            "error should mention backend pairing validation: {err}"
        );
    }

    #[cfg(all(feature = "fakewallet", feature = "sqlite"))]
    #[tokio::test]
    async fn configure_backend_for_methods_registers_websockets_and_fee() {
        use cdk::mint::MintBuilder;
        use cdk_sqlite::mint::memory;

        use crate::config::{FakeWallet, Ln, LnBackend};

        let settings = config::Settings {
            ln: vec![Ln {
                ln_backend: LnBackend::FakeWallet,
                unit: CurrencyUnit::Sat,
                ..Default::default()
            }],
            fake_wallet: Some(FakeWallet::default()),
            info: config::Info {
                input_fee_ppk: Some(100),
                ..Default::default()
            },
            ..Default::default()
        };

        let localstore = Arc::new(memory::empty().await.unwrap());
        let builder = MintBuilder::new(localstore);

        let fake_wallet = settings.fake_wallet.clone().expect("fake wallet config");
        let fake = fake_wallet
            .setup(
                &settings,
                CurrencyUnit::Sat,
                None,
                &std::env::temp_dir(),
                None,
            )
            .await
            .expect("fake wallet setup");

        let mint_melt_limits = cdk::mint::MintMeltLimits {
            mint_min: 1.into(),
            mint_max: 500_000.into(),
            melt_min: 1.into(),
            melt_max: 500_000.into(),
        };

        let builder = configure_backend_for_methods(
            &settings,
            builder,
            CurrencyUnit::Sat,
            mint_melt_limits,
            Arc::new(fake),
            vec![PaymentMethod::Known(KnownMethod::Bolt11)],
        )
        .await
        .expect("configure_backend_for_methods should succeed");

        let mint_info = builder.current_mint_info();
        assert!(
            !mint_info.nuts.nut04.methods.is_empty(),
            "bolt11 method should be registered"
        );
        assert!(
            !mint_info.nuts.nut17.supported.is_empty(),
            "websocket support should be configured"
        );
    }

    #[cfg(all(feature = "fakewallet", feature = "sqlite"))]
    #[tokio::test]
    async fn fakewallet_onchain_with_fake_ln_does_not_duplicate() {
        use cdk::mint::MintBuilder;
        use cdk_sqlite::mint::memory;

        use crate::config::{FakeWallet, Ln, LnBackend, Onchain, OnchainBackend};

        let settings = config::Settings {
            ln: vec![Ln {
                ln_backend: LnBackend::FakeWallet,
                unit: CurrencyUnit::Sat,
                ..Default::default()
            }],
            onchain: Some(Onchain {
                onchain_backend: OnchainBackend::FakeWallet,
                ..Default::default()
            }),
            fake_wallet: Some(FakeWallet::default()),
            ..Default::default()
        };

        let localstore = Arc::new(memory::empty().await.unwrap());
        let builder = MintBuilder::new(localstore);
        let builder =
            configure_onchain_backend(&settings, builder, None, &std::env::temp_dir(), None)
                .await
                .expect("fakewallet onchain with fake LN should succeed without duplicating");

        let mint_info = builder.current_mint_info();
        assert!(
            mint_info.nuts.nut04.methods.is_empty(),
            "when has_lightning_backend is true and no real LN, fakewallet onchain should skip; got {:?}",
            mint_info.nuts.nut04.methods
        );
    }

    #[cfg(all(feature = "fakewallet", feature = "sqlite"))]
    #[tokio::test]
    async fn fakewallet_onchain_missing_fake_wallet_config_bails() {
        use cdk::mint::MintBuilder;
        use cdk_sqlite::mint::memory;

        use crate::config::{Ln, LnBackend, Onchain, OnchainBackend};

        let settings = config::Settings {
            ln: vec![Ln {
                ln_backend: LnBackend::None,
                ..Default::default()
            }],
            onchain: Some(Onchain {
                onchain_backend: OnchainBackend::FakeWallet,
                ..Default::default()
            }),
            fake_wallet: None,
            ..Default::default()
        };

        let localstore = Arc::new(memory::empty().await.unwrap());
        let builder = MintBuilder::new(localstore);
        let err = configure_onchain_backend(&settings, builder, None, &std::env::temp_dir(), None)
            .await
            .expect_err("missing fake_wallet config should bail");

        assert!(
            err.to_string().contains("Fake wallet config"),
            "error should mention missing config: {err}"
        );
    }

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

    #[test]
    fn test_extract_supported_payment_methods_unique_ordered() {
        let mut mint_info = cdk::nuts::MintInfo::default();
        mint_info.nuts.nut04.methods = vec![
            MintMethodSettings {
                method: PaymentMethod::Known(KnownMethod::Bolt11),
                unit: CurrencyUnit::Sat,
                method_name: None,
                min_amount: None,
                max_amount: None,
                options: None,
            },
            MintMethodSettings {
                method: PaymentMethod::Known(KnownMethod::Bolt12),
                unit: CurrencyUnit::Sat,
                method_name: None,
                min_amount: None,
                max_amount: None,
                options: None,
            },
            MintMethodSettings {
                method: PaymentMethod::Known(KnownMethod::Bolt11),
                unit: CurrencyUnit::Msat,
                method_name: None,
                min_amount: None,
                max_amount: None,
                options: None,
            },
            MintMethodSettings {
                method: PaymentMethod::Custom("paypal".to_string()),
                unit: CurrencyUnit::Usd,
                method_name: None,
                min_amount: None,
                max_amount: None,
                options: None,
            },
            MintMethodSettings {
                method: PaymentMethod::Custom("paypal".to_string()),
                unit: CurrencyUnit::Eur,
                method_name: None,
                min_amount: None,
                max_amount: None,
                options: None,
            },
        ];

        let methods = extract_supported_payment_methods(&mint_info);

        assert_eq!(methods, vec!["bolt11", "bolt12", "paypal"]);
    }

    fn clear_mintd_env() {
        for var in [
            "CDK_MINTD_DATABASE",
            "CDK_MINTD_DATABASE_URL",
            "CDK_MINTD_POSTGRES_URL",
            "CDK_MINTD_POSTGRES_TLS_MODE",
            "CDK_MINTD_POSTGRES_MAX_CONNECTIONS",
            "CDK_MINTD_POSTGRES_CONNECTION_TIMEOUT_SECONDS",
            "CDK_MINTD_SEED",
            "CDK_MINTD_MNEMONIC",
            "CDK_MINTD_SIGNATORY_ENABLED",
            "CDK_MINTD_SIGNATORY_ADDRESS",
            "CDK_MINTD_SIGNATORY_PORT",
            "CDK_MINTD_SIGNATORY_TLS_DIR",
            "CDK_MINTD_SIGNATORY_ALLOW_INSECURE",
            "CDK_MINTD_LISTEN_HOST",
            "CDK_MINTD_LISTEN_PORT",
            "CDK_MINTD_LN_BACKEND",
            "CDK_MINTD_LN_MIN_MINT",
            "CDK_MINTD_LN_MAX_MINT",
            "CDK_MINTD_LN_MIN_MELT",
            "CDK_MINTD_LN_MAX_MELT",
            "CDK_MINTD_AUTH_ENABLED",
            "CDK_MINTD_AUTH_OPENID_DISCOVERY",
            "CDK_MINTD_AUTH_OPENID_CLIENT_ID",
            "CDK_MINTD_AUTH_POSTGRES_URL",
            "CDK_MINTD_AUTH_POSTGRES_TLS_MODE",
            "CDK_MINTD_AUTH_POSTGRES_MAX_CONNECTIONS",
            "CDK_MINTD_AUTH_POSTGRES_CONNECTION_TIMEOUT_SECONDS",
            "CDK_MINTD_CLN_RPC_PATH",
            "CDK_MINTD_LNBITS_ADMIN_API_KEY",
            "CDK_MINTD_LNBITS_INVOICE_API_KEY",
            "CDK_MINTD_LNBITS_API",
            "CDK_MINTD_LND_ADDRESS",
            "CDK_MINTD_LND_CERT_FILE",
            "CDK_MINTD_LND_MACAROON_FILE",
            "CDK_MINTD_FAKE_WALLET_SUPPORTED_UNITS",
            "CDK_MINTD_FAKE_WALLET_FEE_PERCENT",
            "CDK_MINTD_FAKE_WALLET_RESERVE_FEE_MIN",
            "CDK_MINTD_FAKE_WALLET_MIN_DELAY",
            "CDK_MINTD_FAKE_WALLET_MAX_DELAY",
            "CDK_MINTD_GRPC_PAYMENT_PROCESSOR_SUPPORTED_UNITS",
            "CDK_MINTD_GRPC_PAYMENT_PROCESSOR_ADDRESS",
            "CDK_MINTD_GRPC_PAYMENT_PROCESSOR_PORT",
            "CDK_MINTD_PROMETHEUS_ENABLED",
            "CDK_MINTD_PROMETHEUS_ADDRESS",
            "CDK_MINTD_PROMETHEUS_PORT",
            "CDK_MINTD_MINT_MANAGEMENT_ENABLED",
            "CDK_MINTD_MANAGEMENT_ADDRESS",
            "CDK_MINTD_MANAGEMENT_PORT",
        ] {
            std::env::remove_var(var);
        }
    }

    fn load_settings_from_toml(name: &str, config_content: &str) -> Result<config::Settings> {
        use std::fs;

        let temp_dir = crate::test_utils::unique_temp_path(name);
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        let config_path = temp_dir.join("config.toml");
        fs::write(&config_path, config_content).expect("Failed to write config file");

        let result = load_settings(&temp_dir, Some(config_path));

        let _ = fs::remove_dir_all(&temp_dir);

        result
    }

    fn assert_load_settings_error(config_content: &str, expected: &str) {
        let _env_lock = crate::test_utils::env_lock();
        clear_mintd_env();
        let err = load_settings_from_toml("cdk_mintd_invalid_config", config_content)
            .expect_err("Settings should fail validation");
        assert!(
            err.to_string().contains(expected),
            "expected error containing `{expected}`, got `{err}`"
        );
    }

    #[cfg(all(feature = "prometheus", feature = "fakewallet"))]
    #[test]
    fn test_load_settings_merges_partial_postgres_toml_with_env() {
        use std::{env, fs};

        let _env_lock = crate::test_utils::env_lock();
        clear_mintd_env();
        env::remove_var(crate::env_vars::DATABASE_URL_ENV_VAR);
        env::remove_var(crate::env_vars::ENV_POSTGRES_URL);
        env::remove_var(crate::env_vars::ENV_PROMETHEUS_ENABLED);
        env::remove_var(crate::env_vars::ENV_PROMETHEUS_ADDRESS);
        env::remove_var(crate::env_vars::ENV_PROMETHEUS_PORT);

        let postgres_url = "postgresql://user:password@localhost:5432/cdk_mint";
        env::set_var(crate::env_vars::ENV_POSTGRES_URL, postgres_url);

        let temp_dir = crate::test_utils::unique_temp_path("cdk_mintd_partial_config");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        let config_path = temp_dir.join("config.toml");

        let config_content = r#"
[info]
mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"

[database]
engine = "postgres"

[database.postgres]
tls_mode = "require"
max_connections = 30
connection_timeout_seconds = 15

[ln]
ln_backend = "fakewallet"

[prometheus]
enabled = true
address = "0.0.0.0"
port = 9090
"#;
        fs::write(&config_path, config_content).expect("Failed to write config file");

        let settings =
            load_settings(&temp_dir, Some(config_path)).expect("Failed to load settings");

        let postgres = settings
            .database
            .postgres
            .as_ref()
            .expect("Postgres config should be present");
        assert_eq!(postgres.url, postgres_url);
        assert_eq!(postgres.tls_mode.as_deref(), Some("require"));

        let prometheus = settings
            .prometheus
            .as_ref()
            .expect("Prometheus config should be loaded from TOML");
        assert!(prometheus.enabled);
        assert_eq!(prometheus.address.as_deref(), Some("0.0.0.0"));
        assert_eq!(prometheus.port, Some(9090));

        env::remove_var(crate::env_vars::ENV_POSTGRES_URL);
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_reports_missing_postgres_url_after_merge() {
        use std::{env, fs};

        let _env_lock = crate::test_utils::env_lock();
        clear_mintd_env();
        env::remove_var(crate::env_vars::DATABASE_URL_ENV_VAR);
        env::remove_var(crate::env_vars::ENV_POSTGRES_URL);

        let temp_dir = crate::test_utils::unique_temp_path("cdk_mintd_invalid_config");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        let config_path = temp_dir.join("config.toml");

        let config_content = r#"
[info]
mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"

[database]
engine = "postgres"

[database.postgres]
tls_mode = "require"

[ln]
ln_backend = "fakewallet"
"#;
        fs::write(&config_path, config_content).expect("Failed to write config file");

        let err = load_settings(&temp_dir, Some(config_path))
            .expect_err("Settings should fail validation without a Postgres URL");
        assert!(err.to_string().contains("PostgreSQL URL is required"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_reports_short_seed() {
        assert_load_settings_error(
            r#"
[info]
seed = "tooshort"

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
"#,
            "Seed in [info].seed/CDK_MINTD_SEED is too short",
        );
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_reports_missing_signing_source() {
        assert_load_settings_error(
            r#"
[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
"#,
            "No signing source configured",
        );
    }

    #[test]
    fn test_load_settings_reports_missing_ln_backend() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"
"#
            ),
            "At least one payment backend",
        );
    }

    #[cfg(feature = "cln")]
    #[test]
    fn test_load_settings_reports_missing_cln_rpc_path() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "cln"
"#
            ),
            "CLN rpc_path must be set",
        );
    }

    #[cfg(feature = "lnbits")]
    #[test]
    fn test_load_settings_reports_missing_lnbits_credentials() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "lnbits"
"#
            ),
            "LNbits admin_api_key must be set",
        );
    }

    #[cfg(feature = "lnd")]
    #[test]
    fn test_load_settings_reports_missing_lnd_address() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "lnd"
"#
            ),
            "LND address must be set",
        );
    }

    #[cfg(feature = "grpc-processor")]
    #[test]
    fn test_load_settings_reports_missing_grpc_supported_units() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "grpcprocessor"

[grpc_processor]
addr = "http://127.0.0.1"
"#
            ),
            "gRPC payment processor supported_units must contain at least one unit",
        );
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_reports_invalid_fakewallet_delay_range() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"

[fake_wallet]
min_delay_time = 10
max_delay_time = 1
"#
            ),
            "Fake wallet min_delay_time cannot be greater than max_delay_time",
        );
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_reports_missing_auth_openid_config() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"

[auth]
auth_enabled = true
"#
            ),
            "Auth openid_discovery must be set",
        );
    }

    #[test]
    fn test_load_settings_reports_toml_parse_errors() {
        assert_load_settings_error(
            r#"
[info
mnemonic = "not valid toml"
"#,
            "Failed to read config file",
        );
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_reports_invalid_ln_limit_range() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
min_mint = 10
max_mint = 1
"#
            ),
            "Lightning min_mint cannot be greater than max_mint",
        );
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_merges_partial_onchain_config_with_defaults() {
        let _env_lock = crate::test_utils::env_lock();
        clear_mintd_env();

        let settings = load_settings_from_toml(
            "cdk_mintd_partial_onchain_config",
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[onchain]
onchain_backend = "fakewallet"
"#
            ),
        )
        .expect("partial on-chain config should use defaults");

        let onchain = settings.onchain.expect("on-chain config should be present");
        assert_eq!(onchain.min_mint, 1.into());
        assert_eq!(onchain.max_mint, 500_000.into());
        assert_eq!(onchain.min_melt, 1.into());
        assert_eq!(onchain.max_melt, 500_000.into());
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_reports_invalid_onchain_mint_limit_range() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[onchain]
onchain_backend = "fakewallet"
min_mint = 10
max_mint = 1
"#
            ),
            "On-chain min_mint cannot be greater than max_mint",
        );
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_reports_invalid_onchain_melt_limit_range() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[onchain]
onchain_backend = "fakewallet"
min_melt = 10
max_melt = 1
"#
            ),
            "On-chain min_melt cannot be greater than max_melt",
        );
    }

    #[cfg(all(feature = "prometheus", feature = "fakewallet"))]
    #[test]
    fn test_load_settings_reports_invalid_prometheus_address() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"

[prometheus]
enabled = true
address = "localhost"
port = 9090
"#
            ),
            "Invalid Prometheus address",
        );
    }

    #[cfg(all(feature = "management-rpc", feature = "fakewallet"))]
    #[test]
    fn test_load_settings_reports_invalid_management_rpc_address() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"

[mint_management_rpc]
enabled = true
address = "localhost"
port = 8086
"#
            ),
            "Invalid mint management RPC address",
        );
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_valid_config() {
        let _env_lock = crate::test_utils::env_lock();
        clear_mintd_env();
        load_settings_from_toml(
            "cdk_mintd_valid",
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
"#
            ),
        )
        .expect("valid config should load without error");
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_valid_config_with_insecure_signatory() {
        let _env_lock = crate::test_utils::env_lock();
        clear_mintd_env();
        load_settings_from_toml(
            "cdk_mintd_valid_signatory",
            r#"
[signatory]
enabled = true
allow_insecure = true

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
"#,
        )
        .expect("valid config with an insecure signatory should load without error");
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_rejects_signatory_without_tls() {
        assert_load_settings_error(
            r#"
[signatory]
enabled = true

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
"#,
            "gRPC signatory TLS is not configured",
        );
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_rejects_empty_seed_before_mnemonic() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
seed = ""
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
"#
            ),
            "Seed in [info].seed/CDK_MINTD_SEED must not be empty",
        );
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_reports_invalid_mnemonic() {
        assert_load_settings_error(
            r#"
[info]
mnemonic = "not a valid mnemonic phrase at all"

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
"#,
            "Invalid mnemonic",
        );
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_reports_invalid_listen_address() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"
listen_host = "999.999.999.999"

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
"#
            ),
            "Invalid mint listen address",
        );
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_reports_missing_auth_openid_client_id() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"

[auth]
auth_enabled = true
openid_discovery = "https://issuer.example.com/.well-known/openid-configuration"
"#
            ),
            "Auth openid_client_id must be set",
        );
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_reports_invalid_melt_limit_range() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
min_melt = 10
max_melt = 1
"#
            ),
            "Lightning min_melt cannot be greater than max_melt",
        );
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_reports_missing_fakewallet_supported_units() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"

[fake_wallet]
supported_units = []
"#
            ),
            "Fake wallet supported_units must contain at least one unit",
        );
    }

    #[cfg(feature = "lnd")]
    #[test]
    fn test_load_settings_reports_missing_lnd_cert_file() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "lnd"

[lnd]
address = "127.0.0.1:10009"
"#
            ),
            "LND cert_file must be set",
        );
    }

    #[cfg(feature = "lnd")]
    #[test]
    fn test_load_settings_reports_missing_lnd_macaroon_file() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "lnd"

[lnd]
address = "127.0.0.1:10009"
cert_file = "/path/to/tls.cert"
"#
            ),
            "LND macaroon_file must be set",
        );
    }

    #[cfg(feature = "lnbits")]
    #[test]
    fn test_load_settings_reports_missing_lnbits_invoice_api_key() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "lnbits"

[lnbits]
admin_api_key = "admin123"
"#
            ),
            "LNbits invoice_api_key must be set",
        );
    }

    #[cfg(feature = "lnbits")]
    #[test]
    fn test_load_settings_reports_missing_lnbits_api_url() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "lnbits"

[lnbits]
admin_api_key = "admin123"
invoice_api_key = "inv123"
"#
            ),
            "LNbits lnbits_api must be set",
        );
    }

    #[cfg(feature = "grpc-processor")]
    #[test]
    fn test_load_settings_reports_missing_grpc_processor_address() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"

[ln]
ln_backend = "grpcprocessor"

[grpc_processor]
supported_units = ["sat"]
address = ""
"#
            ),
            "gRPC payment processor address must be set",
        );
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_load_settings_reports_missing_auth_postgres_url() {
        assert_load_settings_error(
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "postgres"

[database.postgres]
url = "postgresql://user:password@localhost:5432/cdk_mint"

[ln]
ln_backend = "fakewallet"

[auth]
auth_enabled = true
openid_discovery = "https://issuer.example.com/.well-known/openid-configuration"
openid_client_id = "mintd"
"#
            ),
            "Auth database PostgreSQL URL is required",
        );
    }

    fn load_settings_with_env(
        name: &str,
        config_content: &str,
        setup_env: impl FnOnce(),
    ) -> Result<config::Settings> {
        use std::fs;

        let _env_lock = crate::test_utils::env_lock();
        clear_mintd_env();

        let temp_dir = crate::test_utils::unique_temp_path(name);
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");
        let config_path = temp_dir.join("config.toml");
        fs::write(&config_path, config_content).expect("Failed to write config file");

        setup_env();

        let result = load_settings(&temp_dir, Some(config_path));
        let _ = fs::remove_dir_all(&temp_dir);
        clear_mintd_env();
        result
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_env_var_provides_mnemonic_when_toml_has_none() {
        let settings = load_settings_with_env(
            "cdk_mintd_env_mnemonic",
            r#"
[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
"#,
            || std::env::set_var("CDK_MINTD_MNEMONIC", TEST_MNEMONIC),
        )
        .expect("valid config with env mnemonic should load");

        let mnemonic = settings
            .info
            .mnemonic
            .expect("mnemonic should be set from env");
        assert_eq!(mnemonic, TEST_MNEMONIC);
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_env_var_provides_seed_when_toml_has_none() {
        let seed = "a".repeat(32);
        let settings = load_settings_with_env(
            "cdk_mintd_env_seed",
            r#"
[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
"#,
            || std::env::set_var("CDK_MINTD_SEED", &seed),
        )
        .expect("valid config with env seed should load");

        let loaded_seed = settings.info.seed.expect("seed should be set from env");
        assert_eq!(loaded_seed, seed);
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_env_var_provides_ln_backend_when_toml_has_none() {
        let settings = load_settings_with_env(
            "cdk_mintd_env_ln_only",
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"

[database]
engine = "sqlite"
"#
            ),
            || {
                std::env::set_var("CDK_MINTD_LN_BACKEND", "fakewallet");
                std::env::set_var("CDK_MINTD_LN_MIN_MINT", "10");
            },
        )
        .expect("env-only LN config should load");

        assert_eq!(settings.ln.len(), 1);
        assert_eq!(settings.ln[0].ln_backend, config::LnBackend::FakeWallet);
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_env_var_overrides_toml_listen_host() {
        let settings = load_settings_with_env(
            "cdk_mintd_env_override_listen",
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"
listen_host = "127.0.0.1"

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
"#
            ),
            || std::env::set_var("CDK_MINTD_LISTEN_HOST", "0.0.0.0"),
        )
        .expect("config with env override should load");

        assert_eq!(settings.info.listen_host, "0.0.0.0");
    }

    #[cfg(feature = "fakewallet")]
    #[test]
    fn test_env_var_overrides_toml_listen_port() {
        let settings = load_settings_with_env(
            "cdk_mintd_env_override_port",
            &format!(
                r#"
[info]
mnemonic = "{TEST_MNEMONIC}"
listen_port = 8080

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
"#
            ),
            || std::env::set_var("CDK_MINTD_LISTEN_PORT", "9090"),
        )
        .expect("config with env port override should load");

        assert_eq!(settings.info.listen_port, 9090);
    }
}
