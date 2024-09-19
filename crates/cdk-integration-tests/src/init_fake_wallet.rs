use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::Result;
use axum::Router;
use cdk::{
    cdk_database::{self, MintDatabase},
    cdk_lightning::MintLightning,
    mint::FeeReserve,
    nuts::{CurrencyUnit, MeltMethodSettings, MintMethodSettings},
    types::LnKey,
};
use cdk_fake_wallet::FakeWallet;
use futures::StreamExt;
use tower_http::cors::CorsLayer;
use tracing_subscriber::EnvFilter;

use crate::{handle_paid_invoice, init_regtest::create_mint};

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

    let mint = create_mint(database).await?;

    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let fake_wallet = FakeWallet::new(
        fee_reserve,
        MintMethodSettings::default(),
        MeltMethodSettings::default(),
        HashMap::default(),
        HashSet::default(),
        0,
    );

    let mut ln_backends: HashMap<
        LnKey,
        Arc<dyn MintLightning<Err = cdk::cdk_lightning::Error> + Sync + Send>,
    > = HashMap::new();

    ln_backends.insert(
        LnKey::new(CurrencyUnit::Sat, cdk::nuts::PaymentMethod::Bolt11),
        Arc::new(fake_wallet),
    );

    let quote_ttl = 100000;

    let mint_arc = Arc::new(mint);

    let v1_service = cdk_axum::create_mint_router(
        &format!("http://{}:{}", addr, port),
        Arc::clone(&mint_arc),
        ln_backends.clone(),
        quote_ttl,
    )
    .await
    .unwrap();

    let mint_service = Router::new()
        .merge(v1_service)
        .layer(CorsLayer::permissive());

    let mint = Arc::clone(&mint_arc);

    for wallet in ln_backends.values() {
        let wallet_clone = Arc::clone(wallet);
        let mint = Arc::clone(&mint);
        tokio::spawn(async move {
            match wallet_clone.wait_any_invoice().await {
                Ok(mut stream) => {
                    while let Some(request_lookup_id) = stream.next().await {
                        if let Err(err) =
                            handle_paid_invoice(Arc::clone(&mint), &request_lookup_id).await
                        {
                            // nosemgrep: direct-panic
                            panic!("{:?}", err);
                        }
                    }
                }
                Err(err) => {
                    // nosemgrep: direct-panic
                    panic!("Could not get invoice stream: {}", err);
                }
            }
        });
    }
    println!("Staring Axum server");
    axum::Server::bind(&format!("{}:{}", addr, port).as_str().parse().unwrap())
        .serve(mint_service.into_make_service())
        .await?;

    Ok(())
}
