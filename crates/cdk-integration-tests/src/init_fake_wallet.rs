use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use cdk::cdk_database::{self, MintDatabase};
use cdk::cdk_lightning::MintLightning;
use cdk::mint::FeeReserve;
use cdk::nuts::CurrencyUnit;
use cdk::types::LnKey;
use cdk_fake_wallet::FakeWallet;
use tokio::sync::Notify;
use tower_http::cors::CorsLayer;
use tracing_subscriber::EnvFilter;

use crate::init_regtest::create_mint;

pub async fn start_fake_mint<D>(addr: &str, port: u16, database: D) -> Result<()>
where
    D: MintDatabase<Err = cdk_database::Error> + Send + Sync + 'static,
{
    let default_filter = "debug";

    let sqlx_filter = "sqlx=warn";
    let hyper_filter = "hyper=warn";

    let env_filter = EnvFilter::new(format!(
        "{},{},{}",
        default_filter, sqlx_filter, hyper_filter
    ));

    // Parse input
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let mut ln_backends: HashMap<
        LnKey,
        Arc<dyn MintLightning<Err = cdk::cdk_lightning::Error> + Sync + Send>,
    > = HashMap::new();

    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let fake_wallet = FakeWallet::new(fee_reserve, HashMap::default(), HashSet::default(), 0);

    ln_backends.insert(
        LnKey::new(CurrencyUnit::Sat, cdk::nuts::PaymentMethod::Bolt11),
        Arc::new(fake_wallet),
    );

    let mint = create_mint(database, ln_backends.clone()).await?;
    let mint_arc = Arc::new(mint);

    let v1_service = cdk_axum::create_mint_router(Arc::clone(&mint_arc))
        .await
        .unwrap();

    let mint_service = Router::new()
        .merge(v1_service)
        .layer(CorsLayer::permissive());

    let mint = Arc::clone(&mint_arc);

    let shutdown = Arc::new(Notify::new());

    tokio::spawn({
        let shutdown = Arc::clone(&shutdown);
        async move { mint.wait_for_paid_invoices(shutdown).await }
    });

    println!("Staring Axum server");
    axum::Server::bind(&format!("{}:{}", addr, port).as_str().parse().unwrap())
        .serve(mint_service.into_make_service())
        .await?;

    Ok(())
}
