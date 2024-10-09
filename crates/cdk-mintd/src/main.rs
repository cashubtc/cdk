//! CDK Mint Server

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use axum::Router;
use bip39::Mnemonic;
use cdk::cdk_database::{self, MintDatabase};
use cdk::cdk_lightning;
use cdk::cdk_lightning::MintLightning;
use cdk::mint::{MeltQuote, Mint};
use cdk::nuts::{ContactInfo, CurrencyUnit, MeltQuoteState, MintVersion, PaymentMethod};
use cdk::types::LnKey;
use cdk_mintd::mint::{MintBuilder, MintMeltLimits};
use cdk_mintd::setup::LnBackendSetup;
use cdk_redb::MintRedbDatabase;
use cdk_sqlite::MintSqliteDatabase;
use clap::Parser;
use tokio::sync::Notify;
use tower_http::cors::CorsLayer;
use tracing_subscriber::EnvFilter;

use cdk_mintd::cli::CLIArgs;
use cdk_mintd::config::{self, DatabaseEngine, LnBackend};

const CARGO_PKG_VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");
const DEFAULT_QUOTE_TTL_SECS: u64 = 1800;
const DEFAULT_CACHE_TTL_SECS: u64 = 1800;
const DEFAULT_CACHE_TTI_SECS: u64 = 1800;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let default_filter = "debug";

    let sqlx_filter = "sqlx=warn";
    let hyper_filter = "hyper=warn";

    let env_filter = EnvFilter::new(format!(
        "{},{},{}",
        default_filter, sqlx_filter, hyper_filter
    ));

    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let args = CLIArgs::parse();

    let work_dir = match args.work_dir {
        Some(w) => w,
        None => work_dir()?,
    };

    // get config file name from args
    let config_file_arg = match args.config {
        Some(c) => c,
        None => work_dir.join("config.toml"),
    };

    let mut mint_builder = MintBuilder::new();

    let settings = config::Settings::new(&Some(config_file_arg));

    let localstore: Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync> =
        match settings.database.engine {
            DatabaseEngine::Sqlite => {
                let sql_db_path = work_dir.join("cdk-mintd.sqlite");
                let sqlite_db = MintSqliteDatabase::new(&sql_db_path).await?;

                sqlite_db.migrate().await;

                Arc::new(sqlite_db)
            }
            DatabaseEngine::Redb => {
                let redb_path = work_dir.join("cdk-mintd.redb");
                Arc::new(MintRedbDatabase::new(&redb_path)?)
            }
        };

    mint_builder = mint_builder.with_localstore(localstore);

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

    let mut ln_backends: HashMap<
        LnKey,
        Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>,
    > = HashMap::new();
    let mut ln_routers = vec![];

    let mint_melt_limits = MintMeltLimits {
        mint_min: settings.ln.min_mint,
        mint_max: settings.ln.max_mint,
        melt_min: settings.ln.min_melt,
        melt_max: settings.ln.max_melt,
    };

    match settings.ln.ln_backend {
        LnBackend::Cln => {
            let cln_settings = settings
                .cln
                .clone()
                .expect("Config checked at load that cln is some");

            let cln = cln_settings
                .setup(&mut ln_routers, &settings, CurrencyUnit::Msat)
                .await?;
            let cln = Arc::new(cln);
            let ln_key = LnKey {
                unit: CurrencyUnit::Sat,
                method: PaymentMethod::Bolt11,
            };
            ln_backends.insert(ln_key, cln.clone());

            mint_builder = mint_builder.add_ln_backend(
                CurrencyUnit::Sat,
                PaymentMethod::Bolt11,
                mint_melt_limits,
                cln,
            )
        }
        LnBackend::Strike => {
            let strike_settings = settings.clone().strike.expect("Checked on config load");

            for unit in strike_settings
                .clone()
                .supported_units
                .unwrap_or(vec![CurrencyUnit::Sat])
            {
                let strike = strike_settings
                    .setup(&mut ln_routers, &settings, unit)
                    .await?;

                mint_builder = mint_builder.add_ln_backend(
                    unit,
                    PaymentMethod::Bolt11,
                    mint_melt_limits,
                    Arc::new(strike),
                );
            }
        }
        LnBackend::LNbits => {
            let lnbits_settings = settings.clone().lnbits.expect("Checked on config load");
            let lnbits = lnbits_settings
                .setup(&mut ln_routers, &settings, CurrencyUnit::Sat)
                .await?;

            mint_builder = mint_builder.add_ln_backend(
                CurrencyUnit::Sat,
                PaymentMethod::Bolt11,
                mint_melt_limits,
                Arc::new(lnbits),
            );
        }
        LnBackend::Phoenixd => {
            let phd_settings = settings.clone().phoenixd.expect("Checked at config load");
            let phd = phd_settings
                .setup(&mut ln_routers, &settings, CurrencyUnit::Sat)
                .await?;

            mint_builder = mint_builder.add_ln_backend(
                CurrencyUnit::Sat,
                PaymentMethod::Bolt11,
                mint_melt_limits,
                Arc::new(phd),
            );
        }
        LnBackend::Lnd => {
            let lnd_settings = settings.clone().lnd.expect("Checked at config load");
            let lnd = lnd_settings
                .setup(&mut ln_routers, &settings, CurrencyUnit::Msat)
                .await?;

            mint_builder = mint_builder.add_ln_backend(
                CurrencyUnit::Sat,
                PaymentMethod::Bolt11,
                mint_melt_limits,
                Arc::new(lnd),
            );
        }
        LnBackend::FakeWallet => {
            let fake_wallet = settings.clone().fake_wallet.expect("Fake wallet defined");

            for unit in fake_wallet.clone().supported_units {
                let fake = fake_wallet
                    .setup(&mut ln_routers, &settings, CurrencyUnit::Sat)
                    .await?;

                let fake = Arc::new(fake);

                mint_builder = mint_builder.add_ln_backend(
                    unit,
                    PaymentMethod::Bolt11,
                    mint_melt_limits,
                    fake.clone(),
                );

                mint_builder = mint_builder.add_ln_backend(
                    unit,
                    PaymentMethod::Bolt12,
                    mint_melt_limits,
                    fake.clone(),
                );
            }
        }
    };

    let support_bolt12_mint = ln_backends.iter().any(|(_k, ln)| {
        let settings = ln.get_settings();
        settings.bolt12_mint
    });

    let support_bolt12_melt = ln_backends.iter().any(|(_k, ln)| {
        let settings = ln.get_settings();
        settings.bolt12_melt
    });

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

    let mnemonic = Mnemonic::from_str(&settings.info.mnemonic)?;

    mint_builder = mint_builder
        .with_name(settings.mint_info.name)
        .with_mint_url(settings.info.url)
        .with_version(mint_version)
        .with_description(settings.mint_info.description)
        .with_quote_ttl(10000, 10000)
        .with_seed(mnemonic.to_seed_normalized("").to_vec());

    let mint = mint_builder.build().await?;

    let mint = Arc::new(mint);

    // Check the status of any mint quotes that are pending
    // In the event that the mint server is down but the ln node is not
    // it is possible that a mint quote was paid but the mint has not been updated
    // this will check and update the mint state of those quotes
    for ln in ln_backends.values() {
        check_pending_mint_quotes(Arc::clone(&mint), Arc::clone(ln)).await?;
    }

    // Checks the status of all pending melt quotes
    // Pending melt quotes where the payment has gone through inputs are burnt
    // Pending melt quotes where the paynment has **failed** inputs are reset to unspent
    check_pending_melt_quotes(Arc::clone(&mint), &ln_backends).await?;

    let listen_addr = settings.info.listen_host;
    let listen_port = settings.info.listen_port;
    let _quote_ttl = settings
        .info
        .seconds_quote_is_valid_for
        .unwrap_or(DEFAULT_QUOTE_TTL_SECS);
    let cache_ttl = settings
        .info
        .seconds_to_cache_requests_for
        .unwrap_or(DEFAULT_CACHE_TTL_SECS);
    let cache_tti = settings
        .info
        .seconds_to_extend_cache_by
        .unwrap_or(DEFAULT_CACHE_TTI_SECS);

    let include_bolt12 = support_bolt12_mint || support_bolt12_melt;

    let v1_service =
        cdk_axum::create_mint_router(Arc::clone(&mint), cache_ttl, cache_tti, include_bolt12)
            .await?;

    let mut mint_service = Router::new()
        .merge(v1_service)
        .layer(CorsLayer::permissive());

    for router in ln_routers {
        mint_service = mint_service.merge(router);
    }

    let shutdown = Arc::new(Notify::new());

    tokio::spawn({
        let shutdown = Arc::clone(&shutdown);
        async move { mint.wait_for_paid_invoices(shutdown).await }
    });

    let axum_result = axum::Server::bind(
        &format!("{}:{}", listen_addr, listen_port)
            .as_str()
            .parse()?,
    )
    .serve(mint_service.into_make_service())
    .await;

    shutdown.notify_waiters();

    match axum_result {
        Ok(_) => {
            tracing::info!("Axum server stopped with okay status");
        }
        Err(err) => {
            tracing::warn!("Axum server stopped with error");
            tracing::error!("{}", err);

            bail!("Axum exited with error")
        }
    }

    Ok(())
}

/// Used on mint start up to check status of all pending mint quotes
async fn check_pending_mint_quotes(
    mint: Arc<Mint>,
    ln: Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>,
) -> Result<()> {
    let mut pending_quotes = mint.get_pending_mint_quotes().await?;
    tracing::info!("There are {} pending mint quotes.", pending_quotes.len());
    let mut unpaid_quotes = mint.get_unpaid_mint_quotes().await?;
    tracing::info!("There are {} unpaid mint quotes.", unpaid_quotes.len());

    unpaid_quotes.append(&mut pending_quotes);

    for quote in unpaid_quotes {
        tracing::debug!("Checking status of mint quote: {}", quote.id);
        let lookup_id = quote.request_lookup_id;
        match ln.check_incoming_invoice_status(&lookup_id).await {
            Ok(state) => {
                if state != quote.state {
                    tracing::trace!("Mint quote status changed: {}", quote.id);
                    mint.localstore
                        .update_mint_quote_state(&quote.id, state)
                        .await?;
                }
            }

            Err(err) => {
                tracing::warn!("Could not check state of pending invoice: {}", lookup_id);
                tracing::error!("{}", err);
            }
        }
    }

    Ok(())
}

async fn check_pending_melt_quotes(
    mint: Arc<Mint>,
    ln_backends: &HashMap<LnKey, Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>>,
) -> Result<()> {
    let melt_quotes = mint.localstore.get_melt_quotes().await?;
    let pending_quotes: Vec<MeltQuote> = melt_quotes
        .into_iter()
        .filter(|q| q.state == MeltQuoteState::Pending || q.state == MeltQuoteState::Unknown)
        .collect();
    tracing::info!("There are {} pending melt quotes.", pending_quotes.len());

    for pending_quote in pending_quotes {
        tracing::debug!("Checking status for melt quote {}.", pending_quote.id);
        let melt_request_ln_key = mint.localstore.get_melt_request(&pending_quote.id).await?;

        let (melt_request, ln_key) = match melt_request_ln_key {
            None => (
                None,
                LnKey {
                    unit: pending_quote.unit,
                    method: PaymentMethod::Bolt11,
                },
            ),
            Some((melt_request, ln_key)) => (Some(melt_request), ln_key),
        };

        let ln_backend = match ln_backends.get(&ln_key) {
            Some(ln_backend) => ln_backend,
            None => {
                tracing::warn!("No backend for ln key: {:?}", ln_key);
                continue;
            }
        };

        let pay_invoice_response = ln_backend
            .check_outgoing_payment(&pending_quote.request_lookup_id)
            .await?;

        match melt_request {
            Some(melt_request) => {
                match pay_invoice_response.status {
                    MeltQuoteState::Paid => {
                        if let Err(err) = mint
                            .process_melt_request(&melt_request, pay_invoice_response.total_spent)
                            .await
                        {
                            tracing::error!(
                                "Could not process melt request for pending quote: {}",
                                melt_request.quote
                            );
                            tracing::error!("{}", err);
                        }
                    }
                    MeltQuoteState::Unpaid | MeltQuoteState::Unknown | MeltQuoteState::Failed => {
                        // Payment has not been made we want to unset
                        tracing::info!("Lightning payment for quote {} failed.", pending_quote.id);
                        if let Err(err) = mint.process_unpaid_melt(&melt_request).await {
                            tracing::error!("Could not reset melt quote state: {}", err);
                        }
                    }
                    MeltQuoteState::Pending => {
                        tracing::warn!(
                            "LN payment pending, proofs are stuck as pending for quote: {}",
                            melt_request.quote
                        );
                        // Quote is still pending we do not want to do anything
                        // continue to check next quote
                    }
                }
            }
            None => {
                tracing::warn!(
                    "There is no stored melt request for pending melt quote: {}",
                    pending_quote.id
                );

                mint.localstore
                    .update_melt_quote_state(&pending_quote.id, pay_invoice_response.status)
                    .await?;
            }
        };
    }
    Ok(())
}

fn work_dir() -> Result<PathBuf> {
    let home_dir = home::home_dir().ok_or(anyhow!("Unknown home dir"))?;

    Ok(home_dir.join(".cdk-mintd"))
}
