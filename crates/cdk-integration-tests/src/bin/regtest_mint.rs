use std::env;

use anyhow::Result;
use cdk::cdk_database::mint_memory::MintMemoryDatabase;
use cdk_integration_tests::init_regtest::{
    create_cln_backend, create_lnd_backend, create_mint, fund_ln, generate_block, get_bitcoin_dir,
    get_cln_dir, get_lnd_cert_file_path, get_lnd_dir, get_lnd_macaroon_path, get_temp_dir,
    init_bitcoin_client, init_bitcoind, init_lnd, open_channel, BITCOIN_RPC_PASS, BITCOIN_RPC_USER,
    LND_ADDR, LND_RPC_ADDR, LND_TWO_ADDR, LND_TWO_RPC_ADDR,
};
use cdk_redb::MintRedbDatabase;
use cdk_sqlite::MintSqliteDatabase;
use ln_regtest_rs::cln::Clnd;
use ln_regtest_rs::ln_client::{ClnClient, LightningClient, LndClient};
use tracing_subscriber::EnvFilter;

const CLN_ADDR: &str = "127.0.0.1:19846";
const CLN_TWO_ADDR: &str = "127.0.0.1:19847";

#[tokio::main]
async fn main() -> Result<()> {
    let default_filter = "debug";

    let sqlx_filter = "sqlx=warn";
    let hyper_filter = "hyper=warn";
    let h2_filter = "h2=warn";
    let rustls_filter = "rustls=warn";

    let env_filter = EnvFilter::new(format!(
        "{},{},{},{},{}",
        default_filter, sqlx_filter, hyper_filter, h2_filter, rustls_filter
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

    let mint_addr = "127.0.0.1";
    let cln_mint_port = 8085;

    let mint_db_kind = env::var("MINT_DATABASE")?;

    let lnd_mint_db_path = get_temp_dir().join("lnd_mint");
    let cln_mint_db_path = get_temp_dir().join("cln_mint");

    let cln_backend = create_cln_backend(&cln_client).await?;
    let lnd_mint_port = 8087;

    let lnd_backend = create_lnd_backend(&lnd_two_client).await?;

    match mint_db_kind.as_str() {
        "MEMORY" => {
            tokio::spawn(async move {
                create_mint(
                    mint_addr,
                    cln_mint_port,
                    MintMemoryDatabase::default(),
                    cln_backend,
                )
                .await
                .expect("Could not start cln mint");
            });

            create_mint(
                mint_addr,
                lnd_mint_port,
                MintMemoryDatabase::default(),
                lnd_backend,
            )
            .await?;
        }
        "SQLITE" => {
            tokio::spawn(async move {
                let sqlite_db = MintSqliteDatabase::new(&cln_mint_db_path)
                    .await
                    .expect("Could not create mint db");
                sqlite_db.migrate().await;
                create_mint(mint_addr, cln_mint_port, sqlite_db, cln_backend)
                    .await
                    .expect("Could not start cln mint");
            });

            let sqlite_db = MintSqliteDatabase::new(&lnd_mint_db_path).await?;
            sqlite_db.migrate().await;
            create_mint(mint_addr, lnd_mint_port, sqlite_db, lnd_backend).await?;
        }
        "REDB" => {
            tokio::spawn(async move {
                let redb_db = MintRedbDatabase::new(&cln_mint_db_path).unwrap();
                create_mint(mint_addr, cln_mint_port, redb_db, cln_backend)
                    .await
                    .expect("Could not start cln mint");
            });

            let redb_db = MintRedbDatabase::new(&lnd_mint_db_path).unwrap();

            create_mint(mint_addr, lnd_mint_port, redb_db, lnd_backend).await?;
        }
        _ => panic!("Unknown mint db type: {}", mint_db_kind),
    };

    Ok(())
}
