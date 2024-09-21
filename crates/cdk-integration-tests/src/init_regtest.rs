use std::{collections::HashMap, env, path::PathBuf, sync::Arc};

use anyhow::Result;
use axum::Router;
use bip39::Mnemonic;
use cdk::{
    cdk_database::{self, MintDatabase},
    cdk_lightning::MintLightning,
    mint::{FeeReserve, Mint},
    nuts::{CurrencyUnit, MeltMethodSettings, MintInfo, MintMethodSettings},
};
use cdk_axum::LnKey;
use cdk_cln::Cln as CdkCln;
use futures::StreamExt;
use ln_regtest_rs::{
    bitcoin_client::BitcoinClient, bitcoind::Bitcoind, cln::Clnd, cln_client::ClnClient, lnd::Lnd,
    lnd_client::LndClient,
};
use tower_http::cors::CorsLayer;
use tracing_subscriber::EnvFilter;

const BITCOIND_ADDR: &str = "127.0.0.1:18443";
const ZMQ_RAW_BLOCK: &str = "tcp://127.0.0.1:28332";
const ZMQ_RAW_TX: &str = "tcp://127.0.0.1:28333";
const BITCOIN_RPC_USER: &str = "testuser";
const BITCOIN_RPC_PASS: &str = "testpass";
const CLN_ADDR: &str = "127.0.0.1:19846";
const LND_ADDR: &str = "0.0.0.0:18444";
const LND_RPC_ADDR: &str = "https://127.0.0.1:10009";

const BITCOIN_DIR: &str = "bitcoin";
const CLN_DIR: &str = "cln";
const LND_DIR: &str = "lnd";

pub fn get_mint_addr() -> String {
    env::var("cdk_itests_mint_addr").expect("Temp dir set")
}

pub fn get_mint_port() -> u16 {
    let dir = env::var("cdk_itests_mint_port").expect("Temp dir set");
    dir.parse().unwrap()
}

pub fn get_mint_url() -> String {
    format!("http://{}:{}", get_mint_addr(), get_mint_port())
}

pub fn get_temp_dir() -> PathBuf {
    let dir = env::var("cdk_itests").expect("Temp dir set");
    std::fs::create_dir_all(&dir).unwrap();
    dir.parse().expect("Valid path buf")
}

pub fn get_bitcoin_dir() -> PathBuf {
    let dir = get_temp_dir().join(BITCOIN_DIR);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn init_bitcoind() -> Bitcoind {
    Bitcoind::new(
        get_bitcoin_dir(),
        BITCOIND_ADDR.parse().unwrap(),
        BITCOIN_RPC_USER.to_string(),
        BITCOIN_RPC_PASS.to_string(),
        ZMQ_RAW_BLOCK.to_string(),
        ZMQ_RAW_TX.to_string(),
    )
}

pub fn init_bitcoin_client() -> Result<BitcoinClient> {
    BitcoinClient::new(
        "wallet".to_string(),
        BITCOIND_ADDR.into(),
        None,
        Some(BITCOIN_RPC_USER.to_string()),
        Some(BITCOIN_RPC_PASS.to_string()),
    )
}

pub fn get_cln_dir() -> PathBuf {
    let dir = get_temp_dir().join(CLN_DIR);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn init_cln() -> Clnd {
    Clnd::new(
        get_bitcoin_dir(),
        get_cln_dir(),
        CLN_ADDR.to_string().parse().unwrap(),
        BITCOIN_RPC_USER.to_string(),
        BITCOIN_RPC_PASS.to_string(),
    )
}

pub async fn init_cln_client() -> Result<ClnClient> {
    ClnClient::new(get_cln_dir(), None).await
}

pub fn get_lnd_dir() -> PathBuf {
    let dir = get_temp_dir().join(LND_DIR);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub async fn init_lnd() -> Lnd {
    Lnd::new(
        get_bitcoin_dir(),
        get_lnd_dir(),
        LND_ADDR.parse().unwrap(),
        BITCOIN_RPC_USER.to_string(),
        BITCOIN_RPC_PASS.to_string(),
        ZMQ_RAW_BLOCK.to_string(),
        ZMQ_RAW_TX.to_string(),
    )
}

pub async fn init_lnd_client() -> Result<LndClient> {
    let lnd_dir = get_lnd_dir();
    let cert_file = lnd_dir.join("tls.cert");
    let macaroon_file = lnd_dir.join("data/chain/bitcoin/regtest/admin.macaroon");
    LndClient::new(LND_RPC_ADDR.parse().unwrap(), cert_file, macaroon_file).await
}

pub async fn create_cln_backend(cln_client: &ClnClient) -> Result<CdkCln> {
    let rpc_path = cln_client.rpc_path.clone();

    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    Ok(CdkCln::new(
        rpc_path,
        fee_reserve,
        MintMethodSettings::default(),
        MeltMethodSettings::default(),
    )
    .await?)
}

pub async fn create_mint<D>(database: D) -> Result<Mint>
where
    D: MintDatabase<Err = cdk_database::Error> + Send + Sync + 'static,
{
    let nuts = cdk::nuts::Nuts::new()
        .nut07(true)
        .nut08(true)
        .nut09(true)
        .nut10(true)
        .nut11(true)
        .nut12(true)
        .nut14(true);

    let mint_info = MintInfo::new().nuts(nuts);

    let mnemonic = Mnemonic::generate(12)?;

    let mut supported_units: HashMap<CurrencyUnit, (u64, u8)> = HashMap::new();
    supported_units.insert(CurrencyUnit::Sat, (0, 32));

    let mint = Mint::new(
        &get_mint_url(),
        &mnemonic.to_seed_normalized(""),
        mint_info,
        Arc::new(database),
        supported_units,
    )
    .await?;

    Ok(mint)
}

pub async fn start_cln_mint<D>(addr: &str, port: u16, database: D) -> Result<()>
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
    let cln_client = init_cln_client().await?;

    let cln_backend = create_cln_backend(&cln_client).await?;

    let mut ln_backends: HashMap<
        LnKey,
        Arc<dyn MintLightning<Err = cdk::cdk_lightning::Error> + Sync + Send>,
    > = HashMap::new();

    ln_backends.insert(
        LnKey::new(CurrencyUnit::Sat, cdk::nuts::PaymentMethod::Bolt11),
        Arc::new(cln_backend),
    );

    let quote_ttl = 100000;

    let mint_arc = Arc::new(mint);

    let v1_service = cdk_axum::create_mint_router(
        &get_mint_url(),
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

/// Update mint quote when called for a paid invoice
async fn handle_paid_invoice(mint: Arc<Mint>, request_lookup_id: &str) -> Result<()> {
    println!("Invoice with lookup id paid: {}", request_lookup_id);
    if let Ok(Some(mint_quote)) = mint
        .localstore
        .get_mint_quote_by_request_lookup_id(request_lookup_id)
        .await
    {
        println!(
            "Quote {} paid by lookup id {}",
            mint_quote.id, request_lookup_id
        );
        mint.localstore
            .update_mint_quote_state(&mint_quote.id, cdk::nuts::MintQuoteState::Paid)
            .await?;
    }
    Ok(())
}

pub async fn fund_ln(
    bitcoin_client: &BitcoinClient,
    cln_client: &ClnClient,
    lnd_client: &LndClient,
) -> Result<()> {
    let lnd_address = lnd_client.get_new_address().await?;

    bitcoin_client.send_to_address(&lnd_address, 2_000_000)?;

    let cln_address = cln_client.get_new_address().await?;
    bitcoin_client.send_to_address(&cln_address, 2_000_000)?;

    let mining_address = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&mining_address, 200)?;

    cln_client.wait_chain_sync().await?;
    lnd_client.wait_chain_sync().await?;

    Ok(())
}

pub async fn open_channel(
    bitcoin_client: &BitcoinClient,
    cln_client: &ClnClient,
    lnd_client: &LndClient,
) -> Result<()> {
    let cln_info = cln_client.get_info().await?;

    let cln_pubkey = cln_info.id;
    let cln_address = "127.0.0.1";
    let cln_port = 19846;

    lnd_client
        .connect(cln_pubkey.to_string(), cln_address.to_string(), cln_port)
        .await
        .unwrap();

    lnd_client
        .open_channel(1_500_000, &cln_pubkey.to_string(), Some(750_000))
        .await
        .unwrap();

    let mine_to_address = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&mine_to_address, 10)?;

    cln_client.wait_chain_sync().await?;
    lnd_client.wait_chain_sync().await?;

    cln_client.wait_channels_active().await?;
    lnd_client.wait_channels_active().await?;

    Ok(())
}
