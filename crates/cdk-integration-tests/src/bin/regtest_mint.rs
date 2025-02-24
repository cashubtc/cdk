use std::env;
use std::sync::Arc;

use anyhow::Result;
use cdk::cdk_database::mint_memory::MintMemoryDatabase;
use cdk_integration_tests::init_regtest::{
    create_cln_backend, create_lnd_backend, create_mint, get_cln_dir, get_lnd_cert_file_path,
    get_lnd_dir, get_lnd_macaroon_path, get_temp_dir, LND_TWO_RPC_ADDR,
};
use cdk_redb::MintRedbDatabase;
use cdk_sqlite::MintSqliteDatabase;
use ln_regtest_rs::ln_client::{ClnClient, LndClient};
use tokio::sync::Notify;
use tracing_subscriber::EnvFilter;

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

    let mint_addr = "127.0.0.1";
    let cln_mint_port = 8085;

    let mint_db_kind = env::var("MINT_DATABASE")?;

    let lnd_mint_db_path = get_temp_dir().join("lnd_mint");
    let cln_mint_db_path = get_temp_dir().join("cln_mint");

    let shutdown_regtest = Arc::new(Notify::new());

    let cln_one_dir = get_cln_dir("one");
    let cln_client = ClnClient::new(cln_one_dir.clone(), None).await?;
    let cln_backend = create_cln_backend(&cln_client).await?;
    let lnd_mint_port = 8087;

    let lnd_two_dir = get_lnd_dir("two");
    let lnd_two_client = LndClient::new(
        format!("https://{}", LND_TWO_RPC_ADDR),
        get_lnd_cert_file_path(&lnd_two_dir),
        get_lnd_macaroon_path(&lnd_two_dir),
    )
    .await?;
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
                let cln_sqlite_db = MintSqliteDatabase::new(&cln_mint_db_path)
                    .await
                    .expect("Could not create CLN mint db");
                cln_sqlite_db.migrate().await;
                create_mint(mint_addr, cln_mint_port, cln_sqlite_db, cln_backend)
                    .await
                    .expect("Could not start cln mint");
            });

            let lnd_sqlite_db = MintSqliteDatabase::new(&lnd_mint_db_path).await?;
            lnd_sqlite_db.migrate().await;
            create_mint(mint_addr, lnd_mint_port, lnd_sqlite_db, lnd_backend).await?;
        }
        "REDB" => {
            tokio::spawn(async move {
                let cln_redb_db = MintRedbDatabase::new(&cln_mint_db_path).unwrap();
                create_mint(mint_addr, cln_mint_port, cln_redb_db, cln_backend)
                    .await
                    .expect("Could not start cln mint");
            });

            let lnd_redb_db = MintRedbDatabase::new(&lnd_mint_db_path).unwrap();
            create_mint(mint_addr, lnd_mint_port, lnd_redb_db, lnd_backend).await?;
        }
        _ => panic!("Unknown mint db type: {}", mint_db_kind),
    };

    shutdown_regtest.notify_one();

    Ok(())
}
