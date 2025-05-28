//! CDK MINTD
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use axum::Router;
use bip39::Mnemonic;
use cdk::cdk_database::{self, MintAuthDatabase};
use cdk::mint::{MintBuilder, MintMeltLimits};
// Feature-gated imports
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
use cdk::nuts::{
    AuthRequired, ContactInfo, MintVersion, PaymentMethod, ProtectedEndpoint, RoutePath,
};
use cdk::types::QuoteTTL;
use cdk_axum::cache::HttpCache;
#[cfg(feature = "management-rpc")]
use cdk_mint_rpc::MintRPCServer;
use cdk_mintd::cli::CLIArgs;
use cdk_mintd::config::{self, DatabaseEngine, LnBackend};
use cdk_mintd::env_vars::ENV_WORK_DIR;
use cdk_mintd::setup::LnBackendSetup;
#[cfg(feature = "redb")]
use cdk_redb::mint::MintRedbAuthDatabase;
#[cfg(feature = "redb")]
use cdk_redb::MintRedbDatabase;
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let default_filter = "debug";

    let sqlx_filter = "sqlx=warn";
    let hyper_filter = "hyper=warn";
    let h2_filter = "h2=warn";
    let tower_http = "tower_http=warn";

    let env_filter = EnvFilter::new(format!(
        "{default_filter},{sqlx_filter},{hyper_filter},{h2_filter},{tower_http}"
    ));

    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let args = CLIArgs::parse();

    let work_dir = if let Some(work_dir) = args.work_dir {
        tracing::info!("Using work dir from cmd arg");
        work_dir
    } else if let Ok(env_work_dir) = env::var(ENV_WORK_DIR) {
        tracing::info!("Using work dir from env var");
        env_work_dir.into()
    } else {
        work_dir()?
    };

    tracing::info!("Using work dir: {}", work_dir.display());

    // get config file name from args
    let config_file_arg = match args.config {
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
    let settings = settings.from_env()?;

    let mut mint_builder = match settings.database.engine {
        DatabaseEngine::Sqlite => {
            let sql_db_path = work_dir.join("cdk-mintd.sqlite");
            #[cfg(not(feature = "sqlcipher"))]
            let sqlite_db = MintSqliteDatabase::new(&sql_db_path).await?;
            #[cfg(feature = "sqlcipher")]
            let sqlite_db = MintSqliteDatabase::new(&sql_db_path, args.password).await?;

            let db = Arc::new(sqlite_db);
            MintBuilder::new()
                .with_localstore(db.clone())
                .with_keystore(db)
        }
        #[cfg(feature = "redb")]
        DatabaseEngine::Redb => {
            let redb_path = work_dir.join("cdk-mintd.redb");
            let db = Arc::new(MintRedbDatabase::new(&redb_path)?);
            MintBuilder::new()
                .with_localstore(db.clone())
                .with_keystore(db)
        }
    };

    let mut contact_info: Option<Vec<ContactInfo>> = None;

    if let Some(nostr_contact) = &settings.mint_info.contact_nostr_public_key {
        let nostr_contact = ContactInfo::new("nostr".to_string(), nostr_contact.to_string());

        contact_info = match contact_info {
            Some(mut vec) => {
                vec.push(nostr_contact);
                Some(vec)
            }
            None => Some(vec![nostr_contact]),
        };
    }

    if let Some(email_contact) = &settings.mint_info.contact_email {
        let email_contact = ContactInfo::new("email".to_string(), email_contact.to_string());

        contact_info = match contact_info {
            Some(mut vec) => {
                vec.push(email_contact);
                Some(vec)
            }
            None => Some(vec![email_contact]),
        };
    }

    let mint_version = MintVersion::new(
        "cdk-mintd".to_string(),
        CARGO_PKG_VERSION.unwrap_or("Unknown").to_string(),
    );

    let mut ln_routers = vec![];

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
                .setup(&mut ln_routers, &settings, CurrencyUnit::Msat)
                .await?;
            let cln = Arc::new(cln);

            mint_builder = mint_builder
                .add_ln_backend(
                    CurrencyUnit::Sat,
                    PaymentMethod::Bolt11,
                    mint_melt_limits,
                    cln.clone(),
                )
                .await?;

            if let Some(input_fee) = settings.info.input_fee_ppk {
                mint_builder = mint_builder.set_unit_fee(&CurrencyUnit::Sat, input_fee)?;
            }

            let nut17_supported = SupportedMethods::default_bolt11(CurrencyUnit::Sat);

            mint_builder = mint_builder.add_supported_websockets(nut17_supported);
        }
        #[cfg(feature = "lnbits")]
        LnBackend::LNbits => {
            let lnbits_settings = settings.clone().lnbits.expect("Checked on config load");
            let lnbits = lnbits_settings
                .setup(&mut ln_routers, &settings, CurrencyUnit::Sat)
                .await?;

            mint_builder = mint_builder
                .add_ln_backend(
                    CurrencyUnit::Sat,
                    PaymentMethod::Bolt11,
                    mint_melt_limits,
                    Arc::new(lnbits),
                )
                .await?;
            if let Some(input_fee) = settings.info.input_fee_ppk {
                mint_builder = mint_builder.set_unit_fee(&CurrencyUnit::Sat, input_fee)?;
            }

            let nut17_supported = SupportedMethods::default_bolt11(CurrencyUnit::Sat);

            mint_builder = mint_builder.add_supported_websockets(nut17_supported);
        }
        #[cfg(feature = "lnd")]
        LnBackend::Lnd => {
            let lnd_settings = settings.clone().lnd.expect("Checked at config load");
            let lnd = lnd_settings
                .setup(&mut ln_routers, &settings, CurrencyUnit::Msat)
                .await?;

            mint_builder = mint_builder
                .add_ln_backend(
                    CurrencyUnit::Sat,
                    PaymentMethod::Bolt11,
                    mint_melt_limits,
                    Arc::new(lnd),
                )
                .await?;
            if let Some(input_fee) = settings.info.input_fee_ppk {
                mint_builder = mint_builder.set_unit_fee(&CurrencyUnit::Sat, input_fee)?;
            }

            let nut17_supported = SupportedMethods::default_bolt11(CurrencyUnit::Sat);

            mint_builder = mint_builder.add_supported_websockets(nut17_supported);
        }
        #[cfg(feature = "fakewallet")]
        LnBackend::FakeWallet => {
            let fake_wallet = settings.clone().fake_wallet.expect("Fake wallet defined");
            tracing::info!("Using fake wallet: {:?}", fake_wallet);

            for unit in fake_wallet.clone().supported_units {
                let fake = fake_wallet
                    .setup(&mut ln_routers, &settings, CurrencyUnit::Sat)
                    .await
                    .expect("hhh");

                let fake = Arc::new(fake);

                mint_builder = mint_builder
                    .add_ln_backend(
                        unit.clone(),
                        PaymentMethod::Bolt11,
                        mint_melt_limits,
                        fake.clone(),
                    )
                    .await?;
                if let Some(input_fee) = settings.info.input_fee_ppk {
                    mint_builder = mint_builder.set_unit_fee(&unit, input_fee)?;
                }

                let nut17_supported = SupportedMethods::default_bolt11(unit);

                mint_builder = mint_builder.add_supported_websockets(nut17_supported);
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

            tracing::info!("{:?}", grpc_processor);

            for unit in grpc_processor.clone().supported_units {
                tracing::debug!("Adding unit: {:?}", unit);

                let processor = grpc_processor
                    .setup(&mut ln_routers, &settings, unit.clone())
                    .await?;

                mint_builder = mint_builder
                    .add_ln_backend(
                        unit.clone(),
                        PaymentMethod::Bolt11,
                        mint_melt_limits,
                        Arc::new(processor),
                    )
                    .await?;
                if let Some(input_fee) = settings.info.input_fee_ppk {
                    mint_builder = mint_builder.set_unit_fee(&unit, input_fee)?;
                }

                let nut17_supported = SupportedMethods::default_bolt11(unit);
                mint_builder = mint_builder.add_supported_websockets(nut17_supported);
            }
        }
        LnBackend::None => {
            tracing::error!(
                "Payment backend was not set or feature disabled. {:?}",
                settings.ln.ln_backend
            );
            bail!("Ln backend must be")
        }
    };

    if let Some(long_description) = &settings.mint_info.description_long {
        mint_builder = mint_builder.with_long_description(long_description.to_string());
    }

    if let Some(contact_info) = contact_info {
        for info in contact_info {
            mint_builder = mint_builder.add_contact_info(info);
        }
    }

    if let Some(pubkey) = settings.mint_info.pubkey {
        mint_builder = mint_builder.with_pubkey(pubkey);
    }

    if let Some(icon_url) = &settings.mint_info.icon_url {
        mint_builder = mint_builder.with_icon_url(icon_url.to_string());
    }

    if let Some(motd) = settings.mint_info.motd {
        mint_builder = mint_builder.with_motd(motd);
    }

    if let Some(tos_url) = &settings.mint_info.tos_url {
        mint_builder = mint_builder.with_tos_url(tos_url.to_string());
    }

    mint_builder = mint_builder
        .with_name(settings.mint_info.name)
        .with_version(mint_version)
        .with_description(settings.mint_info.description);

    mint_builder = if let Some(signatory_url) = settings.info.signatory_url {
        tracing::info!(
            "Connecting to remote signatory to {} with certs {:?}",
            signatory_url,
            settings.info.signatory_certs
        );
        mint_builder.with_signatory(Arc::new(
            cdk_signatory::SignatoryRpcClient::new(signatory_url, settings.info.signatory_certs)
                .await?,
        ))
    } else if let Some(mnemonic) = settings
        .info
        .mnemonic
        .map(|s| Mnemonic::from_str(&s))
        .transpose()?
    {
        mint_builder.with_seed(mnemonic.to_seed_normalized("").to_vec())
    } else {
        bail!("No seed nor remote signatory set");
    };

    let cached_endpoints = vec![
        CachedEndpoint::new(NUT19Method::Post, NUT19Path::MintBolt11),
        CachedEndpoint::new(NUT19Method::Post, NUT19Path::MeltBolt11),
        CachedEndpoint::new(NUT19Method::Post, NUT19Path::Swap),
    ];

    let cache: HttpCache = settings.info.http_cache.into();

    mint_builder = mint_builder.add_cache(Some(cache.ttl.as_secs()), cached_endpoints);

    // Add auth to mint
    if let Some(auth_settings) = settings.auth {
        tracing::info!("Auth settings are defined. {:?}", auth_settings);
        let auth_localstore: Arc<dyn MintAuthDatabase<Err = cdk_database::Error> + Send + Sync> =
            match settings.database.engine {
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
            ProtectedEndpoint::new(cdk::nuts::Method::Post, RoutePath::MintBlindAuth);

        mint_builder = mint_builder.set_clear_auth_settings(
            auth_settings.openid_discovery,
            auth_settings.openid_client_id,
        );

        let mut protected_endpoints = HashMap::new();

        protected_endpoints.insert(mint_blind_auth_endpoint, AuthRequired::Clear);

        let mut blind_auth_endpoints = vec![];
        let mut unprotected_endpoints = vec![];

        {
            let mint_quote_protected_endpoint = ProtectedEndpoint::new(
                cdk::nuts::Method::Post,
                cdk::nuts::RoutePath::MintQuoteBolt11,
            );
            let mint_protected_endpoint =
                ProtectedEndpoint::new(cdk::nuts::Method::Post, cdk::nuts::RoutePath::MintBolt11);
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
            let melt_quote_protected_endpoint = ProtectedEndpoint::new(
                cdk::nuts::Method::Post,
                cdk::nuts::RoutePath::MeltQuoteBolt11,
            );
            let melt_protected_endpoint =
                ProtectedEndpoint::new(cdk::nuts::Method::Post, cdk::nuts::RoutePath::MeltBolt11);

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
            let swap_protected_endpoint =
                ProtectedEndpoint::new(cdk::nuts::Method::Post, cdk::nuts::RoutePath::Swap);

            if auth_settings.enabled_swap {
                protected_endpoints.insert(swap_protected_endpoint, AuthRequired::Blind);
                blind_auth_endpoints.push(swap_protected_endpoint);
            } else {
                unprotected_endpoints.push(swap_protected_endpoint);
            }
        }

        {
            let check_mint_protected_endpoint = ProtectedEndpoint::new(
                cdk::nuts::Method::Get,
                cdk::nuts::RoutePath::MintQuoteBolt11,
            );

            if auth_settings.enabled_check_mint_quote {
                protected_endpoints.insert(check_mint_protected_endpoint, AuthRequired::Blind);
                blind_auth_endpoints.push(check_mint_protected_endpoint);
            } else {
                unprotected_endpoints.push(check_mint_protected_endpoint);
            }
        }

        {
            let check_melt_protected_endpoint = ProtectedEndpoint::new(
                cdk::nuts::Method::Get,
                cdk::nuts::RoutePath::MeltQuoteBolt11,
            );

            if auth_settings.enabled_check_melt_quote {
                protected_endpoints.insert(check_melt_protected_endpoint, AuthRequired::Blind);
                blind_auth_endpoints.push(check_melt_protected_endpoint);
            } else {
                unprotected_endpoints.push(check_melt_protected_endpoint);
            }
        }

        {
            let restore_protected_endpoint =
                ProtectedEndpoint::new(cdk::nuts::Method::Post, cdk::nuts::RoutePath::Restore);

            if auth_settings.enabled_restore {
                protected_endpoints.insert(restore_protected_endpoint, AuthRequired::Blind);
                blind_auth_endpoints.push(restore_protected_endpoint);
            } else {
                unprotected_endpoints.push(restore_protected_endpoint);
            }
        }

        {
            let state_protected_endpoint =
                ProtectedEndpoint::new(cdk::nuts::Method::Post, cdk::nuts::RoutePath::Checkstate);

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

    let listen_addr = settings.info.listen_host;
    let listen_port = settings.info.listen_port;

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

    #[cfg(feature = "management-rpc")]
    let mut rpc_server: Option<cdk_mint_rpc::MintRPCServer> = None;

    #[cfg(feature = "management-rpc")]
    {
        if let Some(rpc_settings) = settings.mint_management_rpc {
            if rpc_settings.enabled {
                let addr = rpc_settings.address.unwrap_or("127.0.0.1".to_string());
                let port = rpc_settings.port.unwrap_or(8086);
                let mut mint_rpc = MintRPCServer::new(&addr, port, mint.clone())?;

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
            mint.set_mint_info(mint_builder.mint_info).await?;
            mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000)).await?;
        } else {
            if mint.localstore.get_quote_ttl().await.is_err() {
                mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000)).await?;
            }
            tracing::info!("Mint info already set, not using config file settings.");
        }
    } else {
        tracing::warn!("RPC not enabled, using mint info from config.");
        mint.set_mint_info(mint_builder.mint_info).await?;
        mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000)).await?;
    }

    let socket_addr = SocketAddr::from_str(&format!("{listen_addr}:{listen_port}"))?;

    let listener = tokio::net::TcpListener::bind(socket_addr).await?;

    tracing::debug!("listening on {}", listener.local_addr().unwrap());

    let axum_result = axum::serve(listener, mint_service).with_graceful_shutdown(shutdown_signal());

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

    shutdown.notify_waiters();

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
