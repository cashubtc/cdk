use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::{env, fs};

use anyhow::{anyhow, bail, Result};
use bip39::Mnemonic;
use cashu::nut00::KnownMethod;
use cdk::amount::SplitTarget;
use cdk::cdk_database::{self, WalletDatabase};
use cdk::mint::{MintBuilder, MintMeltLimits};
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use cdk::types::{FeeReserve, QuoteTTL};
use cdk::wallet::{Wallet, WalletBuilder};
use cdk::{Amount, Mint, StreamExt};
use cdk_fake_wallet::FakeWallet;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

pub use crate::direct_connection::DirectMintConnection;

pub fn setup_tracing() {
    let default_filter = "debug";

    let h2_filter = "h2=warn";
    let hyper_filter = "hyper=warn";
    let tower_filter = "tower=warn";
    let tokio_postgres = "tokio_postgres=warn";

    let env_filter = EnvFilter::new(format!(
        "{default_filter},{h2_filter},{hyper_filter},{tower_filter},{tokio_postgres}"
    ));

    // Ok if successful, Err if already initialized
    // Allows us to setup tracing at the start of several parallel tests
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .try_init();
}

pub async fn create_and_start_test_mint() -> Result<Mint> {
    create_mint_with_limits(None).await
}

pub async fn create_mint_with_fee(fee_ppk: u64) -> Result<Mint> {
    build_and_start_mint(env_localstore().await?, None, Some(fee_ppk)).await
}

pub async fn create_mint_with_limits(limits: Option<(usize, usize)>) -> Result<Mint> {
    build_and_start_mint(env_localstore().await?, limits, None).await
}

/// Build and start an in-memory test mint without reading `CDK_TEST_DB_TYPE`,
/// so callers do not need to set (and mutate) that process-global variable.
pub async fn create_and_start_in_memory_test_mint() -> Result<Mint> {
    build_and_start_mint(
        Arc::new(cdk_sqlite::mint::memory::empty().await?),
        None,
        None,
    )
    .await
}

/// Select the mint store from `CDK_TEST_DB_TYPE` (`memory` or a temp SQLite file).
async fn env_localstore() -> Result<Arc<cdk_sqlite::MintSqliteDatabase>> {
    let db_type = env::var("CDK_TEST_DB_TYPE").expect("Database type set");

    let localstore = match db_type.to_lowercase().as_str() {
        "memory" => Arc::new(cdk_sqlite::mint::memory::empty().await?),
        _ => {
            // Create a temporary directory for SQLite database
            let temp_dir = create_temp_dir("cdk-test-sqlite-mint")?;
            let path = temp_dir.join("mint.db").to_str().unwrap().to_string();
            Arc::new(
                cdk_sqlite::MintSqliteDatabase::new(path.as_str())
                    .await
                    .expect("Could not create sqlite db"),
            )
        }
    };

    Ok(localstore)
}

/// Assemble and start a test mint (bolt11 + `paypal` fake backends, standard
/// metadata) over the given store. `limits` overrides the input/output caps
/// (default 2000/2000); `fee` sets a per-unit fee when provided.
async fn build_and_start_mint(
    localstore: Arc<cdk_sqlite::MintSqliteDatabase>,
    limits: Option<(usize, usize)>,
    fee: Option<u64>,
) -> Result<Mint> {
    let mut mint_builder = MintBuilder::new(localstore.clone());

    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 0.02,
    };

    let ln_fake_backend = FakeWallet::new(
        fee_reserve.clone(),
        HashMap::default(),
        HashSet::default(),
        2,
        CurrencyUnit::Sat,
    );

    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 10_000),
            Arc::new(ln_fake_backend),
        )
        .await?;

    let custom_fake_backend = FakeWallet::new(
        fee_reserve.clone(),
        HashMap::default(),
        HashSet::default(),
        2,
        CurrencyUnit::Sat,
    )
    .with_custom_payment_methods(HashMap::from([("paypal".to_string(), "{}".to_string())]));

    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Custom("paypal".to_string()),
            MintMeltLimits::new(1, 10_000),
            Arc::new(custom_fake_backend),
        )
        .await?;

    if let Some(fee_ppk) = fee {
        mint_builder.set_unit_fee(&CurrencyUnit::Sat, fee_ppk)?;
    }

    let (max_inputs, max_outputs) = limits.unwrap_or((2000, 2000));

    let mnemonic = Mnemonic::generate(12)?;

    mint_builder = mint_builder
        .with_name("pure test mint".to_string())
        .with_description("pure test mint".to_string())
        .with_urls(vec!["https://aaa".to_string()])
        .with_limits(max_inputs, max_outputs)
        .with_batch_minting(Some(100), Some(vec!["bolt11".to_string()]));

    let quote_ttl = QuoteTTL::new(10000, 10000);

    let mint = mint_builder
        .build_with_seed(localstore.clone(), &mnemonic.to_seed_normalized(""))
        .await?;

    mint.set_quote_ttl(quote_ttl).await?;

    mint.start().await?;

    Ok(mint)
}

pub async fn create_test_wallet_for_mint(mint: Mint) -> Result<Wallet> {
    let seed = Mnemonic::generate(12)?.to_seed_normalized("");
    create_test_wallet_for_mint_with_seed(mint, seed).await
}

/// Create a test wallet connected directly to a mint with a specific seed
///
/// Useful for restore tests where two wallets must share the same seed.
pub async fn create_test_wallet_for_mint_with_seed(mint: Mint, seed: [u8; 64]) -> Result<Wallet> {
    let connector = DirectMintConnection::new(mint.clone());

    let mint_info = mint.mint_info().await?;
    let mint_url = mint_info
        .urls
        .as_ref()
        .ok_or(anyhow!("Test mint URLs list is unset"))?
        .first()
        .ok_or(anyhow!("Test mint has empty URLs list"))?;

    let unit = CurrencyUnit::Sat;

    // Read environment variable to determine database type
    let db_type = env::var("CDK_TEST_DB_TYPE").expect("Database type set");

    let localstore: Arc<dyn WalletDatabase<cdk_database::Error> + Send + Sync> =
        match db_type.to_lowercase().as_str() {
            "sqlite" => {
                // Create a temporary directory for SQLite database
                let temp_dir = create_temp_dir("cdk-test-sqlite-wallet")?;
                let path = temp_dir.join("wallet.db").to_str().unwrap().to_string();
                let database = cdk_sqlite::WalletSqliteDatabase::new(path.as_str())
                    .await
                    .expect("Could not create sqlite db");
                Arc::new(database)
            }
            "redb" => {
                // Create a temporary directory for ReDB database
                let temp_dir = create_temp_dir("cdk-test-redb-wallet")?;
                let path = temp_dir.join("wallet.redb");
                let database = cdk_redb::WalletRedbDatabase::new(&path)
                    .expect("Could not create redb mint database");
                Arc::new(database)
            }
            "memory" => {
                let database = cdk_sqlite::wallet::memory::empty().await?;
                Arc::new(database)
            }
            _ => {
                bail!("Db type not set")
            }
        };

    let wallet = WalletBuilder::new()
        .mint_url(mint_url.parse().unwrap())
        .unit(unit)
        .localstore(localstore)
        .seed(seed)
        .client(connector)
        .build()?;

    Ok(wallet)
}

/// Creates a mint quote for the given amount and checks its state in a loop. Returns when
/// amount is minted.
/// Creates a temporary directory with a unique name based on the prefix
fn create_temp_dir(prefix: &str) -> Result<PathBuf> {
    let temp_dir = env::temp_dir();
    let unique_dir = temp_dir.join(format!("{}-{}", prefix, Uuid::new_v4()));
    fs::create_dir_all(&unique_dir)?;
    Ok(unique_dir)
}

pub async fn fund_wallet(
    wallet: Wallet,
    amount: u64,
    split_target: Option<SplitTarget>,
) -> Result<Amount> {
    let desired_amount = Amount::from(amount);
    let quote = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(desired_amount), None, None)
        .await?;

    Ok(wallet
        .proof_stream(quote, split_target.unwrap_or_default(), None)
        .next()
        .await
        .expect("proofs")?
        .total_amount()?)
}
