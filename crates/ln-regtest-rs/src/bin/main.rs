use std::fs;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::Result;
use ln_regtest_rs::bitcoin_client::BitcoinClient;
use ln_regtest_rs::bitcoind::Bitcoind;
use ln_regtest_rs::cln::Clnd;
use ln_regtest_rs::ln_client::{ClnClient, LightningClient, LndClient};
use ln_regtest_rs::lnd::Lnd;
use tempfile::tempdir;
use tracing_subscriber::EnvFilter;

fn create_wallet(bitcoind: &mut BitcoinClient) -> Result<()> {
    bitcoind.create_wallet().ok();
    bitcoind.load_wallet()?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let default_filter = "debug";

    let h2_filter = "h2=warn";
    let hyper_filter = "hyper=warn";

    let env_filter = EnvFilter::new(format!("{},{},{}", default_filter, h2_filter, hyper_filter));

    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let temp_dir = tempdir()?;

    let btc_dir = temp_dir.path().join("btc_one");
    fs::create_dir_all(&btc_dir)?;

    let bitcoind_addr = PathBuf::from_str("127.0.0.1:18443")?;

    let zmq_raw_block = "tcp://127.0.0.1:28332";
    let zmq_raw_tx = "tcp://127.0.0.1:28333";

    let btc_rpc_user = "testuser".to_string();
    let btc_rpc_password = "testpassword".to_string();
    let mut bitcoind = Bitcoind::new(
        btc_dir.clone(),
        bitcoind_addr.clone(),
        btc_rpc_user.clone(),
        btc_rpc_password.clone(),
        zmq_raw_block.to_string(),
        zmq_raw_tx.to_string(),
    );

    println!("Starting bitcoind");
    bitcoind.start_bitcoind()?;
    println!("Started bitcoind");

    println!("Creating mining client");
    let mut bitcoin_client_mining = BitcoinClient::new(
        "Minting_wallet".to_string(),
        bitcoind_addr.clone(),
        None,
        Some(btc_rpc_user.clone()),
        Some(btc_rpc_password.clone()),
    )?;

    println!("Creating spending client");
    let mut bitcoin_client_spending = BitcoinClient::new(
        "spending_wallet".to_string(),
        bitcoind_addr,
        None,
        Some(btc_rpc_user.clone()),
        Some(btc_rpc_password.clone()),
    )?;

    println!("Creating mining wallet");
    if let Err(err) = create_wallet(&mut bitcoin_client_mining) {
        println!("{}", err);
        println!("Could not create or load wallet");
    }

    println!("Creating spending wallet");
    if let Err(err) = create_wallet(&mut bitcoin_client_spending) {
        println!("{}", err);
        println!("Could not create or load wallet");
    }

    // Init block
    let mine_to_address = bitcoin_client_mining.get_new_address()?;
    bitcoin_client_mining.generate_blocks(&mine_to_address, 200)?;

    let spending_address = bitcoin_client_spending.get_new_address()?;
    bitcoin_client_mining.send_to_address(&spending_address, 10_000_000)?;
    bitcoin_client_mining.generate_blocks(&mine_to_address, 10)?;

    println!("Minting balance: {}", bitcoin_client_mining.get_balance()?);
    println!(
        "Spening balance: {}",
        bitcoin_client_spending.get_balance()?
    );

    // Start CLN
    let cln_one_addr = PathBuf::from_str("127.0.0.1:19846")?;

    let cln_one_dir = temp_dir.path().join("cln_one");
    fs::create_dir_all(&cln_one_dir)?;

    let mut clnd = Clnd::new(
        btc_dir.clone(),
        cln_one_dir.clone(),
        cln_one_addr.clone(),
        btc_rpc_user.clone(),
        btc_rpc_password.clone(),
    );
    // Start CLN One
    clnd.start_clnd().map_err(|err| {
        bitcoind.stop_bitcoind().ok();
        err
    })?;
    tracing::info!("CLN Started");

    let cln_client = ClnClient::new(cln_one_dir, None).await?;

    cln_client.wait_chain_sync().await?;
    tracing::info!("Cln client completed chain sync");

    // Fund CLN one
    let cln_one_address = cln_client.get_new_onchain_address().await.unwrap();
    println!("CLN Address: {}", cln_one_address);

    bitcoin_client_spending.send_to_address(&cln_one_address, 3_000_000)?;
    // CLN doesn't seem to see the funds unless 100 blocks are generated
    bitcoin_client_mining.generate_blocks(&mine_to_address, 100)?;
    cln_client.wait_chain_sync().await?;

    let bal = cln_client.balance().await?;

    println!("{:?}", bal);

    let lnd_dir = temp_dir.path().join("lnd_data_dir");

    let lnd_addr = "0.0.0.0:18449".to_string();

    let lnd_rpc_listen = "127.0.0.1:10009".to_string();

    let mut lnd = Lnd::new(
        btc_dir,
        lnd_dir.clone(),
        lnd_addr.into(),
        lnd_rpc_listen,
        btc_rpc_user,
        btc_rpc_password,
        zmq_raw_block.to_string(),
        zmq_raw_tx.to_string(),
    );

    lnd.start_lnd()?;
    tracing::info!("LND Started");

    let cert_file = lnd_dir.join("tls.cert");
    let macaroon_file = lnd_dir.join("data/chain/bitcoin/regtest/admin.macaroon");

    let lnd_addr = "https://127.0.0.1:10009".to_string();

    let lnd_client = LndClient::new(lnd_addr, cert_file, macaroon_file).await?;
    tracing::info!("LND Client created");

    lnd_client.wait_chain_sync().await?;
    tracing::info!("LND Client completed chain sync");

    // Fund LND
    let lnd_address = lnd_client.get_new_onchain_address().await?;
    bitcoin_client_spending.send_to_address(&lnd_address, 3_000_000)?;
    bitcoin_client_mining.generate_blocks(&mine_to_address, 10)?;

    lnd_client.wait_chain_sync().await?;
    cln_client.wait_chain_sync().await?;

    // Get lnd info

    let lnd_info = lnd_client.get_info().await?;

    let lnd_pubkey = lnd_info.identity_pubkey;

    let cln_info = cln_client.get_connect_info().await?;

    let cln_pubkey = cln_info.pubkey;
    let cln_address = cln_info.address;
    let cln_port = cln_info.port;

    lnd_client
        .connect_peer(cln_pubkey.clone(), cln_address, cln_port)
        .await
        .unwrap();

    lnd_client
        .open_channel(1_500_000, &cln_pubkey, Some(500_000))
        .await
        .unwrap();

    bitcoin_client_mining.generate_blocks(&mine_to_address, 10)?;

    lnd_client.wait_chain_sync().await?;
    cln_client.wait_chain_sync().await?;

    lnd_client.wait_channels_active().await?;

    let lnd_balance = lnd_client.balance().await?;

    println!("{:?}", lnd_balance);

    let bolt11 = cln_client.create_invoice(Some(1000)).await?;

    let preimage = lnd_client.pay_invoice(bolt11).await?;

    let lnd_bolt11 = lnd_client.create_invoice(Some(1000)).await?;

    let cln_preimage = cln_client.pay_invoice(lnd_bolt11).await?;

    println!("preimage: {}", preimage);
    println!("cln preimage: {}", cln_preimage);

    cln_client
        .open_channel(1_500_000, &lnd_pubkey, None)
        .await?;
    bitcoin_client_mining.generate_blocks(&mine_to_address, 10)?;

    cln_client.wait_channels_active().await?;

    Ok(())
}
