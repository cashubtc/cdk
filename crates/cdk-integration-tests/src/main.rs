use std::env;

use anyhow::Result;
use cdk::cdk_database::mint_memory::MintMemoryDatabase;
use cdk_integration_tests::init_regtest::{
    fund_ln, get_temp_dir, init_bitcoin_client, init_bitcoind, init_cln, init_cln_client, init_lnd,
    init_lnd_client, open_channel, start_cln_mint,
};
use cdk_redb::MintRedbDatabase;
use cdk_sqlite::MintSqliteDatabase;
use ln_regtest_rs::ln_client::LightningClient;

#[tokio::main]
async fn main() -> Result<()> {
    let mut bitcoind = init_bitcoind();
    bitcoind.start_bitcoind()?;

    let bitcoin_client = init_bitcoin_client()?;
    bitcoin_client.create_wallet().ok();
    bitcoin_client.load_wallet()?;

    let new_add = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&new_add, 200).unwrap();

    let mut clnd = init_cln();
    clnd.start_clnd()?;

    let cln_client = init_cln_client().await?;

    cln_client.wait_chain_sync().await.unwrap();

    let mut lnd = init_lnd().await;
    lnd.start_lnd().unwrap();

    let lnd_client = init_lnd_client().await.unwrap();

    lnd_client.wait_chain_sync().await.unwrap();

    fund_ln(&bitcoin_client, &cln_client, &lnd_client)
        .await
        .unwrap();

    open_channel(&bitcoin_client, &cln_client, &lnd_client)
        .await
        .unwrap();

    let addr = "127.0.0.1";
    let port = 8085;

    let mint_db_kind = env::var("MINT_DATABASE")?;

    match mint_db_kind.as_str() {
        "MEMORY" => {
            start_cln_mint(addr, port, MintMemoryDatabase::default()).await?;
        }
        "SQLITE" => {
            let sqlite_db = MintSqliteDatabase::new(&get_temp_dir().join("mint")).await?;
            sqlite_db.migrate().await;
            start_cln_mint(addr, port, sqlite_db).await?;
        }
        "REDB" => {
            let redb_db = MintRedbDatabase::new(&get_temp_dir().join("mint")).unwrap();
            start_cln_mint(addr, port, redb_db).await?;
        }
        _ => panic!("Unknown mint db type: {}", mint_db_kind),
    };

    Ok(())
}
