use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use cdk::mint::FeeReserve;
use cdk_cln::Cln as CdkCln;
use cdk_lnd::Lnd as CdkLnd;
use ln_regtest_rs::bitcoin_client::BitcoinClient;
use ln_regtest_rs::bitcoind::Bitcoind;
use ln_regtest_rs::cln::Clnd;
use ln_regtest_rs::ln_client::{ClnClient, LightningClient, LndClient};
use ln_regtest_rs::lnd::Lnd;
use tokio::sync::oneshot::Sender;
use tokio::sync::Notify;

pub const BITCOIND_ADDR: &str = "127.0.0.1:18443";
pub const ZMQ_RAW_BLOCK: &str = "tcp://127.0.0.1:28332";
pub const ZMQ_RAW_TX: &str = "tcp://127.0.0.1:28333";
pub const BITCOIN_RPC_USER: &str = "testuser";
pub const BITCOIN_RPC_PASS: &str = "testpass";

const BITCOIN_DIR: &str = "bitcoin";

pub const LND_ADDR: &str = "0.0.0.0:18449";
pub const LND_RPC_ADDR: &str = "localhost:10009";

pub const LND_TWO_ADDR: &str = "0.0.0.0:18410";
pub const LND_TWO_RPC_ADDR: &str = "localhost:10010";

pub const CLN_ADDR: &str = "127.0.0.1:19846";
pub const CLN_TWO_ADDR: &str = "127.0.0.1:19847";

pub fn get_mint_addr() -> String {
    env::var("cdk_itests_mint_addr").expect("Temp dir set")
}

pub fn get_mint_port(which: &str) -> u16 {
    let dir = env::var(format!("cdk_itests_mint_port_{}", which)).expect("Temp dir set");
    dir.parse().unwrap()
}

pub fn get_mint_url(which: &str) -> String {
    format!("http://{}:{}", get_mint_addr(), get_mint_port(which))
}

pub fn get_mint_ws_url(which: &str) -> String {
    format!("ws://{}:{}/v1/ws", get_mint_addr(), get_mint_port(which))
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
    let dir = get_temp_dir().join("cln").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn get_lnd_dir(name: &str) -> PathBuf {
    let dir = get_temp_dir().join("lnd").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn get_lnd_cert_file_path(lnd_dir: &Path) -> PathBuf {
    lnd_dir.join("tls.cert")
}

pub fn get_lnd_macaroon_path(lnd_dir: &Path) -> PathBuf {
    lnd_dir.join("data/chain/bitcoin/regtest/admin.macaroon")
}

pub async fn init_lnd(lnd_dir: PathBuf, lnd_addr: &str, lnd_rpc_addr: &str) -> Lnd {
    Lnd::new(
        get_bitcoin_dir(),
        lnd_dir,
        lnd_addr.parse().unwrap(),
        lnd_rpc_addr.to_string(),
        BITCOIN_RPC_USER.to_string(),
        BITCOIN_RPC_PASS.to_string(),
        ZMQ_RAW_BLOCK.to_string(),
        ZMQ_RAW_TX.to_string(),
    )
}

pub fn generate_block(bitcoin_client: &BitcoinClient) -> Result<()> {
    let mine_to_address = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&mine_to_address, 10)?;

    Ok(())
}

pub async fn create_cln_backend(cln_client: &ClnClient) -> Result<CdkCln> {
    let rpc_path = cln_client.rpc_path.clone();

    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    Ok(CdkCln::new(rpc_path, fee_reserve).await?)
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
    )
    .await?)
}

pub async fn fund_ln<C>(bitcoin_client: &BitcoinClient, ln_client: &C) -> Result<()>
where
    C: LightningClient,
{
    let ln_address = ln_client.get_new_onchain_address().await?;

    bitcoin_client.send_to_address(&ln_address, 5_000_000)?;

    ln_client.wait_chain_sync().await?;

    let mine_to_address = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&mine_to_address, 10)?;

    ln_client.wait_chain_sync().await?;

    Ok(())
}

pub async fn open_channel<C1, C2>(cln_client: &C1, lnd_client: &C2) -> Result<()>
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

    cln_client.wait_chain_sync().await?;
    lnd_client.wait_chain_sync().await?;

    lnd_client
        .open_channel(1_500_000, &cln_pubkey.to_string(), Some(750_000))
        .await
        .unwrap();

    Ok(())
}

pub async fn start_regtest_end(sender: Sender<()>, notify: Arc<Notify>) -> anyhow::Result<()> {
    let mut bitcoind = init_bitcoind();
    bitcoind.start_bitcoind()?;

    let bitcoin_client = init_bitcoin_client()?;
    bitcoin_client.create_wallet().ok();
    bitcoin_client.load_wallet()?;

    let new_add = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&new_add, 200).unwrap();

    let cln_one_dir = get_cln_dir("one");
    let mut clnd = Clnd::new(
        get_bitcoin_dir(),
        cln_one_dir.clone(),
        CLN_ADDR.into(),
        BITCOIN_RPC_USER.to_string(),
        BITCOIN_RPC_PASS.to_string(),
    );
    clnd.start_clnd()?;

    let cln_client = ClnClient::new(cln_one_dir.clone(), None).await?;

    cln_client.wait_chain_sync().await.unwrap();

    fund_ln(&bitcoin_client, &cln_client).await.unwrap();

    // Create second cln
    let cln_two_dir = get_cln_dir("two");
    let mut clnd_two = Clnd::new(
        get_bitcoin_dir(),
        cln_two_dir.clone(),
        CLN_TWO_ADDR.into(),
        BITCOIN_RPC_USER.to_string(),
        BITCOIN_RPC_PASS.to_string(),
    );
    clnd_two.start_clnd()?;

    let cln_two_client = ClnClient::new(cln_two_dir.clone(), None).await?;

    cln_two_client.wait_chain_sync().await.unwrap();

    fund_ln(&bitcoin_client, &cln_two_client).await.unwrap();

    let lnd_dir = get_lnd_dir("one");
    println!("{}", lnd_dir.display());

    let mut lnd = init_lnd(lnd_dir.clone(), LND_ADDR, LND_RPC_ADDR).await;
    lnd.start_lnd().unwrap();
    tracing::info!("Started lnd node");

    let lnd_client = LndClient::new(
        format!("https://{}", LND_RPC_ADDR),
        get_lnd_cert_file_path(&lnd_dir),
        get_lnd_macaroon_path(&lnd_dir),
    )
    .await?;

    lnd_client.wait_chain_sync().await.unwrap();

    fund_ln(&bitcoin_client, &lnd_client).await.unwrap();

    // create second lnd node
    let lnd_two_dir = get_lnd_dir("two");
    let mut lnd_two = init_lnd(lnd_two_dir.clone(), LND_TWO_ADDR, LND_TWO_RPC_ADDR).await;
    lnd_two.start_lnd().unwrap();
    tracing::info!("Started second lnd node");

    let lnd_two_client = LndClient::new(
        format!("https://{}", LND_TWO_RPC_ADDR),
        get_lnd_cert_file_path(&lnd_two_dir),
        get_lnd_macaroon_path(&lnd_two_dir),
    )
    .await?;

    lnd_two_client.wait_chain_sync().await.unwrap();

    fund_ln(&bitcoin_client, &lnd_two_client).await.unwrap();

    // Open channels concurrently
    // Open channels
    {
        open_channel(&cln_client, &lnd_client).await.unwrap();
        tracing::info!("Opened channel between cln and lnd one");
        generate_block(&bitcoin_client)?;
        // open_channel(&bitcoin_client, &cln_client, &cln_two_client)
        //     .await
        //     .unwrap();
        // tracing::info!("Opened channel between cln and cln two");

        open_channel(&lnd_client, &lnd_two_client).await.unwrap();
        tracing::info!("Opened channel between lnd and lnd two");
        generate_block(&bitcoin_client)?;

        // open_channel(&cln_client, &lnd_two_client).await.unwrap();
        // tracing::info!("Opened channel between cln and lnd two");
        open_channel(&cln_two_client, &lnd_client).await.unwrap();
        tracing::info!("Opened channel between cln two and lnd");
        generate_block(&bitcoin_client)?;

        open_channel(&cln_client, &lnd_two_client).await.unwrap();
        tracing::info!("Opened channel between cln and lnd two");
        generate_block(&bitcoin_client)?;

        cln_client.wait_channels_active().await?;
        cln_two_client.wait_channels_active().await?;
        lnd_client.wait_channels_active().await?;
        lnd_two_client.wait_channels_active().await?;
    }

    // Send notification that regtest set up is complete
    sender.send(()).expect("Could not send oneshot");

    // Wait until we are told to shutdown
    // If we return the bitcoind, lnd, and cln will be dropped and shutdown
    notify.notified().await;

    Ok(())
}
