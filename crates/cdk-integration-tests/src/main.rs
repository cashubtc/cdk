use std::{env, sync::Arc};

use anyhow::Result;
use cdk::cdk_database::mint_memory::MintMemoryDatabase;
use cdk_integration_tests::{
    get_bitcoin_dir, get_temp_dir,
    init_regtest::{
        create_cln_backend, create_lnd_backend, fund_cln, fund_lnd, init_bitcoin_client,
        init_bitcoind, init_cln, init_cln_client, init_lnd, init_lnd_client, open_channel,
        open_channel_lnd_lnd, start_mint,
    },
    LNNode,
};
use cdk_redb::MintRedbDatabase;
use cdk_sqlite::MintSqliteDatabase;
use tracing_subscriber::EnvFilter;

const BITCOIND_ADDR: &str = "127.0.0.1:18443";
const ZMQ_RAW_BLOCK: &str = "tcp://127.0.0.1:28332";
const ZMQ_RAW_TX: &str = "tcp://127.0.0.1:28333";
const BITCOIN_RPC_USER: &str = "testuser";
const BITCOIN_RPC_PASS: &str = "testpass";

#[tokio::main]
async fn main() -> Result<()> {
    let default_filter = "debug";

    let sqlx_filter = "sqlx=warn";
    let hyper_filter = "hyper=warn";

    let env_filter = EnvFilter::new(format!(
        "{},{},{},h2=warn",
        default_filter, sqlx_filter, hyper_filter
    ));

    // Parse input
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let mut bitcoind = init_bitcoind(
        get_bitcoin_dir(),
        BITCOIND_ADDR.into(),
        BITCOIN_RPC_USER.to_string(),
        BITCOIN_RPC_PASS.to_string(),
        ZMQ_RAW_BLOCK.to_string(),
        ZMQ_RAW_TX.to_string(),
    );

    bitcoind.start_bitcoind()?;

    let bitcoin_client = init_bitcoin_client(
        BITCOIND_ADDR.into(),
        Some(BITCOIN_RPC_USER.to_string()),
        Some(BITCOIN_RPC_PASS.to_string()),
    )?;
    bitcoin_client.create_wallet().ok();
    bitcoin_client.load_wallet()?;

    let new_add = bitcoin_client.get_new_address()?;
    bitcoin_client.generate_blocks(&new_add, 200).unwrap();
    let mut clnd_mint_one = init_cln(
        get_bitcoin_dir(),
        LNNode::CLNMintOne.data_dir(),
        LNNode::CLNMintOne.address().into(),
        BITCOIN_RPC_USER.to_string(),
        BITCOIN_RPC_PASS.to_string(),
    );

    clnd_mint_one.start_clnd().unwrap();
    let cln_client_mint_one = init_cln_client(LNNode::CLNMintOne.data_dir(), None)
        .await
        .unwrap();

    let mut clnd_wallet_one = init_cln(
        get_bitcoin_dir(),
        LNNode::CLNWalletOne.data_dir(),
        LNNode::CLNWalletOne.address().into(),
        BITCOIN_RPC_USER.to_string(),
        BITCOIN_RPC_PASS.to_string(),
    );

    clnd_wallet_one.start_clnd().unwrap();
    let cln_client_wallet_one = init_cln_client(LNNode::CLNWalletOne.data_dir(), None)
        .await
        .unwrap();

    let mut lnd = init_lnd(
        get_bitcoin_dir(),
        LNNode::LNDWalletOne.data_dir(),
        LNNode::LNDWalletOne.address(),
        LNNode::LNDWalletOne.rpc_listen_addr(),
        BITCOIN_RPC_USER.to_string(),
        BITCOIN_RPC_PASS.to_string(),
        ZMQ_RAW_BLOCK.to_string(),
        ZMQ_RAW_TX.to_string(),
    )
    .await;
    lnd.start_lnd().unwrap();

    let lnd_client_wallet = init_lnd_client(
        LNNode::LNDWalletOne.data_dir(),
        format!("https://{}", LNNode::LNDWalletOne.rpc_listen_addr()),
    )
    .await
    .unwrap();

    fund_lnd(&bitcoin_client, &lnd_client_wallet).await.unwrap();

    let mut mint_lnd = init_lnd(
        get_bitcoin_dir(),
        LNNode::LNDMintOne.data_dir(),
        LNNode::LNDMintOne.address(),
        LNNode::LNDMintOne.rpc_listen_addr(),
        BITCOIN_RPC_USER.to_string(),
        BITCOIN_RPC_PASS.to_string(),
        ZMQ_RAW_BLOCK.to_string(),
        ZMQ_RAW_TX.to_string(),
    )
    .await;
    mint_lnd.start_lnd().unwrap();

    let mint_lnd_client = init_lnd_client(
        LNNode::LNDMintOne.data_dir(),
        format!("https://{}", LNNode::LNDMintOne.rpc_listen_addr()),
    )
    .await
    .unwrap();

    fund_lnd(&bitcoin_client, &mint_lnd_client).await.unwrap();

    fund_cln(&bitcoin_client, &cln_client_mint_one)
        .await
        .unwrap();
    open_channel(&bitcoin_client, &cln_client_mint_one, &lnd_client_wallet)
        .await
        .unwrap();

    open_channel(&bitcoin_client, &cln_client_wallet_one, &lnd_client_wallet)
        .await
        .unwrap();

    open_channel(&bitcoin_client, &cln_client_mint_one, &mint_lnd_client)
        .await
        .unwrap();

    open_channel_lnd_lnd(&bitcoin_client, &lnd_client_wallet, &mint_lnd_client)
        .await
        .unwrap();

    let addr = "127.0.0.1";
    let cln_one_port = 8085;
    let lnd_one_port = 8086;

    let cln_backend = create_cln_backend(&cln_client_mint_one).await?;
    let lnd_backend = create_lnd_backend(&mint_lnd_client).await?;

    let mint_db_kind = env::var("MINT_DATABASE")?;

    match mint_db_kind.as_str() {
        "MEMORY" => {
            tokio::spawn(async move {
                start_mint(
                    addr,
                    cln_one_port,
                    MintMemoryDatabase::default(),
                    Arc::new(cln_backend),
                )
                .await
                .unwrap();
            });
            start_mint(
                addr,
                lnd_one_port,
                MintMemoryDatabase::default(),
                Arc::new(lnd_backend),
            )
            .await?;
        }
        "SQLITE" => {
            tokio::spawn(async move {
                let sqlite_db = MintSqliteDatabase::new(&get_temp_dir().join("mint"))
                    .await
                    .unwrap();
                sqlite_db.migrate().await;
                start_mint(addr, cln_one_port, sqlite_db, Arc::new(cln_backend))
                    .await
                    .unwrap();
            });

            let sqlite_db = MintSqliteDatabase::new(&get_temp_dir().join("lnd_one_mint")).await?;
            sqlite_db.migrate().await;
            start_mint(addr, lnd_one_port, sqlite_db, Arc::new(lnd_backend)).await?;
        }
        "REDB" => {
            tokio::spawn(async move {
                let redb_db = MintRedbDatabase::new(&get_temp_dir().join("mint")).unwrap();
                start_mint(addr, cln_one_port, redb_db, Arc::new(cln_backend))
                    .await
                    .unwrap();
            });

            let redb_db = MintRedbDatabase::new(&get_temp_dir().join("lnd_mint"))?;
            start_mint(addr, lnd_one_port, redb_db, Arc::new(lnd_backend)).await?;
        }
        _ => panic!("Unknown mint db type: {}", mint_db_kind),
    };

    Ok(())
}
