use std::env;

use anyhow::Result;
use cdk::cdk_database::mint_memory::MintMemoryDatabase;
use cdk_integration_tests::init_regtest::{
    fund_ln, get_bitcoin_dir, get_cln_dir, get_temp_dir, init_bitcoin_client, init_bitcoind,
    init_lnd, init_lnd_client, open_channel, start_cln_mint, BITCOIN_RPC_PASS, BITCOIN_RPC_USER,
};
use cdk_redb::MintRedbDatabase;
use cdk_sqlite::MintSqliteDatabase;
use ln_regtest_rs::cln::Clnd;
use ln_regtest_rs::ln_client::{ClnClient, LightningClient};
use tracing_subscriber::EnvFilter;

const CLN_ADDR: &str = "127.0.0.1:19846";
const CLN_TWO_ADDR: &str = "127.0.0.1:19847";

#[tokio::main]
async fn main() -> Result<()> {
    let default_filter = "debug";

    let sqlx_filter = "sqlx=warn";
    let hyper_filter = "hyper=warn";
    let h2_filter = "h2=warn";

    let env_filter = EnvFilter::new(format!(
        "{},{},{},{}",
        default_filter, sqlx_filter, hyper_filter, h2_filter
    ));

    tracing_subscriber::fmt().with_env_filter(env_filter).init();

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

    cln_client.wait_chain_sync().await.unwrap();

    fund_ln(&bitcoin_client, &cln_two_client).await.unwrap();

    let mut lnd = init_lnd().await;
    lnd.start_lnd().unwrap();

    let lnd_client = init_lnd_client().await.unwrap();

    lnd_client.wait_chain_sync().await.unwrap();

    fund_ln(&bitcoin_client, &lnd_client).await.unwrap();

    open_channel(&bitcoin_client, &cln_client, &lnd_client)
        .await
        .unwrap();

    let addr = "127.0.0.1";
    let port = 8085;

    let mint_db_kind = env::var("MINT_DATABASE")?;

    let temp_dir_path = get_temp_dir();
    let db_path = get_temp_dir().join("mint");
    let cln_path = temp_dir_path.join("one");

    match mint_db_kind.as_str() {
        "MEMORY" => {
            start_cln_mint(addr, port, MintMemoryDatabase::default(), cln_path).await?;
        }
        "SQLITE" => {
            let sqlite_db = MintSqliteDatabase::new(&db_path).await?;
            sqlite_db.migrate().await;
            start_cln_mint(addr, port, sqlite_db, cln_path).await?;
        }
        "REDB" => {
            let redb_db = MintRedbDatabase::new(&db_path).unwrap();
            start_cln_mint(addr, port, redb_db, cln_path).await?;
        }
        _ => panic!("Unknown mint db type: {}", mint_db_kind),
    };

    Ok(())
}
