use std::{collections::HashMap, path::PathBuf, sync::Arc};

use anyhow::Result;
use axum::Router;
use bip39::Mnemonic;
use cdk::{
    cdk_database::{self, MintDatabase},
    cdk_lightning::MintLightning,
    mint::{FeeReserve, Mint},
    nuts::{CurrencyUnit, MeltMethodSettings, MintInfo, MintMethodSettings, PaymentMethod},
    types::{LnKey, QuoteTTL},
};
use cdk_cln::Cln as CdkCln;
use cdk_lnd::Lnd as CdkLnd;
use ln_regtest_rs::{
    bitcoin_client::BitcoinClient, bitcoind::Bitcoind, cln::Clnd, cln_client::ClnClient, lnd::Lnd,
    lnd_client::LndClient,
};
use tokio::sync::Notify;
use tower_http::cors::CorsLayer;
use tracing_subscriber::EnvFilter;

pub fn init_bitcoind(
    data_dir: PathBuf,
    addr: PathBuf,
    rpc_user: String,
    rpc_password: String,
    zmq_raw_block: String,
    zmq_raw_tx: String,
) -> Bitcoind {
    Bitcoind::new(
        data_dir,
        addr,
        rpc_user,
        rpc_password,
        zmq_raw_block,
        zmq_raw_tx,
    )
}

pub fn init_bitcoin_client(
    addr: PathBuf,
    bitcoin_rpc_user: Option<String>,
    bitcoin_rpc_password: Option<String>,
) -> Result<BitcoinClient> {
    BitcoinClient::new(
        "wallet".to_string(),
        addr,
        None,
        bitcoin_rpc_user,
        bitcoin_rpc_password,
    )
}

pub fn init_cln(
    bitcoin_dir: PathBuf,
    cln_dir: PathBuf,
    addr: PathBuf,
    bitcoin_rpc_user: String,
    bitcoin_rpc_password: String,
) -> Clnd {
    std::fs::create_dir_all(&cln_dir).unwrap();
    Clnd::new(
        bitcoin_dir,
        cln_dir,
        addr,
        bitcoin_rpc_user,
        bitcoin_rpc_password,
    )
}

pub async fn init_cln_client(cln_dir: PathBuf, rpc_path: Option<PathBuf>) -> Result<ClnClient> {
    ClnClient::new(cln_dir, rpc_path).await
}

#[allow(clippy::too_many_arguments)]
pub async fn init_lnd(
    bitcoin_dir: PathBuf,
    lnd_dir: PathBuf,
    addr: String,
    rpc_listen_addr: String,
    bitcoin_rpc_user: String,
    bitcoin_rpc_password: String,
    zmq_raw_block: String,
    zmq_raw_tx: String,
) -> Lnd {
    std::fs::create_dir_all(&lnd_dir).unwrap();

    Lnd::new(
        bitcoin_dir,
        lnd_dir,
        addr.into(),
        rpc_listen_addr,
        bitcoin_rpc_user,
        bitcoin_rpc_password,
        zmq_raw_block,
        zmq_raw_tx,
    )
}

pub async fn init_lnd_client(lnd_dir: PathBuf, lnd_rpc_addr: String) -> Result<LndClient> {
    let cert_file = lnd_dir.join("tls.cert");
    let macaroon_file = lnd_dir.join("data/chain/bitcoin/regtest/admin.macaroon");
    LndClient::new(lnd_rpc_addr, cert_file, macaroon_file).await
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

pub async fn create_lnd_backend(lnd_client: &LndClient) -> Result<CdkLnd> {
    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    Ok(CdkLnd::new(
        lnd_client.address.clone(),
        lnd_client.cert_file.clone(),
        lnd_client.macaroon_file.clone(),
        fee_reserve,
        MintMethodSettings::default(),
        MeltMethodSettings::default(),
    )
    .await?)
}

pub async fn create_mint<D>(
    mint_url: &str,
    database: D,
    ln_backends: HashMap<
        LnKey,
        Arc<dyn MintLightning<Err = cdk::cdk_lightning::Error> + Sync + Send>,
    >,
) -> Result<Mint>
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

    let quote_ttl = QuoteTTL::new(10000, 10000);

    let mint = Mint::new(
        mint_url,
        &mnemonic.to_seed_normalized(""),
        mint_info,
        quote_ttl,
        Arc::new(database),
        ln_backends,
        supported_units,
    )
    .await?;

    Ok(mint)
}

pub async fn start_mint<D>(
    addr: &str,
    port: u16,
    database: D,
    ln_backend: Arc<dyn MintLightning<Err = cdk::cdk_lightning::Error> + Sync + Send>,
) -> Result<()>
where
    D: MintDatabase<Err = cdk_database::Error> + Send + Sync + 'static,
{
    let mint_url = format!("http://{}:{}", addr, port);

    let ln_key = LnKey {
        unit: ln_backend.get_settings().unit,
        method: PaymentMethod::Bolt11,
    };

    let mut ln_backends = HashMap::new();

    ln_backends.insert(ln_key, ln_backend);

    let mint = create_mint(&mint_url, database, ln_backends).await?;

    let default_filter = "debug";

    let sqlx_filter = "sqlx=warn";
    let hyper_filter = "hyper=warn";

    let env_filter = EnvFilter::new(format!(
        "{},{},{}",
        default_filter, sqlx_filter, hyper_filter
    ));

    // Parse input
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

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

pub async fn fund_lnd(bitcoin_client: &BitcoinClient, lnd_client: &LndClient) -> Result<()> {
    let lnd_address = lnd_client.get_new_address().await?;

    bitcoin_client.send_to_address(&lnd_address, 4_000_000)?;

    let mining_address = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&mining_address, 200)?;

    lnd_client.wait_chain_sync().await?;

    Ok(())
}

pub async fn fund_cln(bitcoin_client: &BitcoinClient, cln_client: &ClnClient) -> Result<()> {
    let cln_address = cln_client.get_new_address().await?;
    bitcoin_client.send_to_address(&cln_address, 2_000_000)?;

    let mining_address = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&mining_address, 200)?;

    cln_client.wait_chain_sync().await?;

    Ok(())
}

pub async fn open_channel(
    bitcoin_client: &BitcoinClient,
    cln_client: &ClnClient,
    lnd_client: &LndClient,
) -> Result<()> {
    let cln_info = cln_client.get_info().await?;

    let cln_pubkey = cln_info.id;

    let cln_port = cln_info.binding.unwrap().first().unwrap().port.unwrap();

    lnd_client
        .connect(cln_pubkey.to_string(), "127.0.0.1".to_string(), cln_port)
        .await
        .unwrap();

    cln_client.wait_chain_sync().await?;
    lnd_client.wait_chain_sync().await?;

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

pub async fn open_channel_lnd_lnd(
    bitcoin_client: &BitcoinClient,
    lnd_client: &LndClient,
    lnd_two_client: &LndClient,
) -> Result<()> {
    lnd_client.wait_chain_sync().await?;
    lnd_two_client.wait_chain_sync().await?;

    let lnd_one_connect_info = lnd_client.get_connect_info().await?;
    lnd_two_client
        .connect(
            lnd_one_connect_info.pubkey.clone(),
            lnd_one_connect_info.address,
            lnd_one_connect_info.port,
        )
        .await
        .unwrap();

    lnd_two_client.wait_chain_sync().await?;

    lnd_two_client
        .open_channel(1_500_000, &lnd_one_connect_info.pubkey, Some(750_000))
        .await
        .unwrap();

    let mine_to_address = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&mine_to_address, 10)?;

    lnd_client.wait_chain_sync().await?;
    lnd_two_client.wait_chain_sync().await?;

    lnd_client.wait_channels_active().await?;
    lnd_two_client.wait_channels_active().await?;

    Ok(())
}
