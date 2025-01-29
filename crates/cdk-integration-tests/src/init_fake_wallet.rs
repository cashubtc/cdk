use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use bip39::Mnemonic;
use cdk::cdk_database::{self, MintDatabase};
use cdk::mint::{FeeReserve, MintBuilder, MintMeltLimits};
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use cdk_fake_wallet::FakeWallet;
use tracing_subscriber::EnvFilter;

use crate::init_mint::start_mint;

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

    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let fake_wallet = FakeWallet::new(fee_reserve, HashMap::default(), HashSet::default(), 0);

    let mut mint_builder = MintBuilder::new();

    mint_builder = mint_builder.with_localstore(Arc::new(database));

    mint_builder = mint_builder.add_ln_backend(
        CurrencyUnit::Sat,
        PaymentMethod::Bolt11,
        MintMeltLimits::new(1, 5_000),
        Arc::new(fake_wallet),
    );

    let mnemonic = Mnemonic::generate(12)?;

    mint_builder = mint_builder
        .with_name("fake test mint".to_string())
        .with_description("fake test mint".to_string())
        .with_quote_ttl(10000, 10000)
        .with_seed(mnemonic.to_seed_normalized("").to_vec());

    let mint = mint_builder.build().await?;

    start_mint(addr, port, mint).await?;

    Ok(())
}
