//! CDK Mint Server

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use axum::Router;
use bip39::Mnemonic;
use cdk::cdk_database::{self, MintDatabase};
use cdk::cdk_lightning::MintLightning;
use cdk::mint::{FeeReserve, Mint};
use cdk::nuts::{
    nut04, nut05, ContactInfo, CurrencyUnit, MeltMethodSettings, MintInfo, MintMethodSettings,
    MintVersion, MppMethodSettings, Nuts, PaymentMethod,
};
use cdk::{cdk_lightning, Amount};
use cdk_axum::LnKey;
use cdk_cln::Cln;
use cdk_redb::MintRedbDatabase;
use cdk_sqlite::MintSqliteDatabase;
use clap::Parser;
use cli::CLIArgs;
use config::{DatabaseEngine, LnBackend};
use futures::StreamExt;
use tower_http::cors::CorsLayer;

mod cli;
mod config;

const CARGO_PKG_VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");
const DEFAULT_QUOTE_TTL_SECS: u64 = 1800;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

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

    let mut contact_info: Option<Vec<ContactInfo>> = None;

    if let Some(nostr_contact) = settings.mint_info.contact_nostr_public_key {
        let nostr_contact = ContactInfo::new("nostr".to_string(), nostr_contact);

        contact_info = match contact_info {
            Some(mut vec) => {
                vec.push(nostr_contact);
                Some(vec)
            }
            None => Some(vec![nostr_contact]),
        };
    }

    if let Some(email_contact) = settings.mint_info.contact_email {
        let email_contact = ContactInfo::new("email".to_string(), email_contact);

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

    let relative_ln_fee = settings.ln.fee_percent;

    let absolute_ln_fee_reserve = settings.ln.reserve_fee_min;

    let fee_reserve = FeeReserve {
        min_fee_reserve: absolute_ln_fee_reserve,
        percent_fee_reserve: relative_ln_fee,
    };
    let ln: Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync> =
        match settings.ln.ln_backend {
            LnBackend::Cln => {
                let cln_socket = expand_path(
                    settings
                        .ln
                        .cln_path
                        .clone()
                        .ok_or(anyhow!("cln socket not defined"))?
                        .to_str()
                        .ok_or(anyhow!("cln socket not defined"))?,
                )
                .ok_or(anyhow!("cln socket not defined"))?;

                Arc::new(Cln::new(cln_socket, fee_reserve, 1000, 1000000, 1000, 100000).await?)
            }
        };

    let mut ln_backends = HashMap::new();

    ln_backends.insert(
        LnKey::new(CurrencyUnit::Sat, PaymentMethod::Bolt11),
        Arc::clone(&ln),
    );

    let (nut04_settings, nut05_settings, mpp_settings): (
        nut04::Settings,
        nut05::Settings,
        Vec<MppMethodSettings>,
    ) = ln_backends.iter().fold(
        (
            nut04::Settings::new(vec![], false),
            nut05::Settings::new(vec![], false),
            Vec::new(),
        ),
        |(mut nut_04, mut nut_05, mut mpp), (key, ln)| {
            let settings = ln.get_settings();

            let m = MppMethodSettings {
                method: key.method.clone(),
                unit: key.unit,
                mpp: settings.mpp,
            };

            let n4 = MintMethodSettings {
                method: key.method.clone(),
                unit: key.unit,
                min_amount: Some(Amount::from(settings.min_mint_amount)),
                max_amount: Some(Amount::from(settings.max_mint_amount)),
            };

            let n5 = MeltMethodSettings {
                method: key.method.clone(),
                unit: key.unit,
                min_amount: Some(Amount::from(settings.min_melt_amount)),
                max_amount: Some(Amount::from(settings.max_melt_amount)),
            };

            nut_04.methods.push(n4);
            nut_05.methods.push(n5);
            mpp.push(m);

            (nut_04, nut_05, mpp)
        },
    );

    let nuts = Nuts::new()
        .nut04(nut04_settings)
        .nut05(nut05_settings)
        .nut15(mpp_settings);

    let mut mint_info = MintInfo::new()
        .name(settings.mint_info.name)
        .version(mint_version)
        .description(settings.mint_info.description)
        .nuts(nuts);

    if let Some(long_description) = &settings.mint_info.description_long {
        mint_info = mint_info.long_description(long_description);
    }

    if let Some(contact_info) = contact_info {
        mint_info = mint_info.contact_info(contact_info);
    }

    if let Some(pubkey) = settings.mint_info.pubkey {
        mint_info = mint_info.pubkey(pubkey);
    }

    if let Some(motd) = settings.mint_info.motd {
        mint_info = mint_info.motd(motd);
    }

    let mnemonic = Mnemonic::from_str(&settings.info.mnemonic)?;

    let mint = Mint::new(
        &settings.info.url,
        &mnemonic.to_seed_normalized(""),
        mint_info,
        localstore,
        absolute_ln_fee_reserve,
        relative_ln_fee,
    )
    .await?;

    let mint = Arc::new(mint);

    // Check the status of any mint quotes that are pending
    // In the event that the mint server is down but the ln node is not
    // it is possible that a mint quote was paid but the mint has not been updated
    // this will check and update the mint state of those quotes
    check_pending_quotes(Arc::clone(&mint), Arc::clone(&ln)).await?;

    let mint_url = settings.info.url;
    let listen_addr = settings.info.listen_host;
    let listen_port = settings.info.listen_port;
    let quote_ttl = settings
        .info
        .seconds_quote_is_valid_for
        .unwrap_or(DEFAULT_QUOTE_TTL_SECS);

    let v1_service =
        cdk_axum::create_mint_router(&mint_url, Arc::clone(&mint), ln_backends, quote_ttl).await?;

    let mint_service = Router::new()
        .nest("/", v1_service)
        .layer(CorsLayer::permissive());

    // Spawn task to wait for invoces to be paid and update mint quotes
    tokio::spawn(async move {
        loop {
            match ln.wait_any_invoice().await {
                Ok(mut stream) => {
                    while let Some(request_lookup_id) = stream.next().await {
                        if let Err(err) =
                            handle_paid_invoice(Arc::clone(&mint), &request_lookup_id).await
                        {
                            tracing::warn!("{:?}", err);
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!("Could not get invoice stream: {}", err);
                }
            }
        }
    });

    let listener =
        tokio::net::TcpListener::bind(format!("{}:{}", listen_addr, listen_port)).await?;

    axum::serve(listener, mint_service).await?;

    Ok(())
}

/// Update mint quote when called for a paid invoice
async fn handle_paid_invoice(mint: Arc<Mint>, request_lookup_id: &str) -> Result<()> {
    tracing::debug!("Invoice with lookup id paid: {}", request_lookup_id);
    if let Ok(Some(mint_quote)) = mint
        .localstore
        .get_mint_quote_by_request_lookup_id(request_lookup_id)
        .await
    {
        tracing::debug!(
            "Quote {} paid by lookup id {}",
            mint_quote.id,
            request_lookup_id
        );
        mint.localstore
            .update_mint_quote_state(&mint_quote.id, cdk::nuts::MintQuoteState::Paid)
            .await?;
    }
    Ok(())
}

/// Used on mint start up to check status of all pending mint quotes
async fn check_pending_quotes(
    mint: Arc<Mint>,
    ln: Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>,
) -> Result<()> {
    let pending_quotes = mint.get_pending_mint_quotes().await?;

    for quote in pending_quotes {
        let lookup_id = quote.request_lookup_id;
        let state = ln.check_invoice_status(&lookup_id).await?;

        if state != quote.state {
            mint.localstore
                .update_mint_quote_state(&quote.id, state)
                .await?;
        }
    }

    Ok(())
}

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

fn work_dir() -> Result<PathBuf> {
    let home_dir = home::home_dir().ok_or(anyhow!("Unknown home dir"))?;

    Ok(home_dir.join(".cdk-mintd"))
}
