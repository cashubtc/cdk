use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use bip39::Mnemonic;
use cdk::cdk_database::{self, MintDatabase};
use cdk::mint::{FeeReserve, MintBuilder, MintMeltLimits};
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use cdk_cln::Cln as CdkCln;
use ln_regtest_rs::bitcoin_client::BitcoinClient;
use ln_regtest_rs::bitcoind::Bitcoind;
use ln_regtest_rs::ln_client::{ClnClient, LightningClient, LndClient};
use ln_regtest_rs::lnd::Lnd;

use crate::init_mint::start_mint;

pub const BITCOIND_ADDR: &str = "127.0.0.1:18443";
pub const ZMQ_RAW_BLOCK: &str = "tcp://127.0.0.1:28332";
pub const ZMQ_RAW_TX: &str = "tcp://127.0.0.1:28333";
pub const BITCOIN_RPC_USER: &str = "testuser";
pub const BITCOIN_RPC_PASS: &str = "testpass";
const LND_ADDR: &str = "0.0.0.0:18449";
const LND_RPC_ADDR: &str = "localhost:10009";

const BITCOIN_DIR: &str = "bitcoin";
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

pub fn get_mint_ws_url() -> String {
    format!("ws://{}:{}/v1/ws", get_mint_addr(), get_mint_port())
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

pub fn get_cln_dir(name: &str) -> PathBuf {
    let dir = get_temp_dir().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
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
        LND_RPC_ADDR.to_string(),
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
    LndClient::new(
        format!("https://{}", LND_RPC_ADDR).parse().unwrap(),
        cert_file,
        macaroon_file,
    )
    .await
}

pub async fn create_cln_backend(cln_client: &ClnClient) -> Result<CdkCln> {
    let rpc_path = cln_client.rpc_path.clone();

    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    Ok(CdkCln::new(rpc_path, fee_reserve).await?)
}

pub async fn start_cln_mint<D>(addr: &str, port: u16, database: D, dir: PathBuf) -> Result<()>
where
    D: MintDatabase<Err = cdk_database::Error> + Send + Sync + 'static,
{
    let cln_client = ClnClient::new(dir.clone(), None).await?;

    let cln_backend = create_cln_backend(&cln_client).await?;

    let mut mint_builder = MintBuilder::new();

    mint_builder = mint_builder.with_localstore(Arc::new(database));

    mint_builder = mint_builder.add_ln_backend(
        CurrencyUnit::Sat,
        PaymentMethod::Bolt11,
        MintMeltLimits::new(1, 5_000),
        Arc::new(cln_backend),
    );

    let mnemonic = Mnemonic::generate(12)?;

    mint_builder = mint_builder
        .with_name("regtest mint".to_string())
        .with_mint_url(format!("http://{addr}:{port}"))
        .with_description("regtest mint".to_string())
        .with_quote_ttl(10000, 10000)
        .with_seed(mnemonic.to_seed_normalized("").to_vec());

    let mint = mint_builder.build().await?;

    start_mint(addr, port, mint).await?;

    Ok(())
}

pub async fn fund_ln<C>(bitcoin_client: &BitcoinClient, ln_client: &C) -> Result<()>
where
    C: LightningClient,
{
    let ln_address = ln_client.get_new_onchain_address().await?;

    bitcoin_client.send_to_address(&ln_address, 2_000_000)?;

    ln_client.wait_chain_sync().await?;

    let mine_to_address = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&mine_to_address, 10)?;

    ln_client.wait_chain_sync().await?;

    Ok(())
}

pub async fn open_channel<C1, C2>(
    bitcoin_client: &BitcoinClient,
    cln_client: &C1,
    lnd_client: &C2,
) -> Result<()>
where
    C1: LightningClient,
    C2: LightningClient,
{
    let cln_info = cln_client.get_connect_info().await?;

    let cln_pubkey = cln_info.pubkey;
    let cln_address = cln_info.address;
    let cln_port = cln_info.port;

    lnd_client
        .connect_peer(cln_pubkey.to_string(), cln_address.to_string(), cln_port)
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
