use std::env;

use anyhow::Result;
use cdk::cdk_database::mint_memory::{MintMemoryAuthDatabase, MintMemoryDatabase};
use cdk_integration_tests::init_auth_mint::start_fake_mint_with_auth;
use cdk_integration_tests::init_regtest::get_temp_dir;
use cdk_redb::mint::MintRedbAuthDatabase;
use cdk_redb::MintRedbDatabase;
use cdk_sqlite::mint::MintSqliteAuthDatabase;
use cdk_sqlite::MintSqliteDatabase;
use tracing_subscriber::EnvFilter;

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

    // Parse input
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let addr = "127.0.0.1";
    let port = 8087;

    let mint_db_kind = env::var("MINT_DATABASE")?;
    let openid_discovery = env::var("OPENID_DISCOVERY")?;

    match mint_db_kind.as_str() {
        "MEMORY" => {
            let auth_mint_db = MintMemoryAuthDatabase::default();
            start_fake_mint_with_auth(
                addr,
                port,
                openid_discovery,
                MintMemoryDatabase::default(),
                auth_mint_db,
            )
            .await?;
        }
        "SQLITE" => {
            let sqlite_db = MintSqliteDatabase::new(&get_temp_dir().join("mint")).await?;
            sqlite_db.migrate().await;

            let auth_db = MintSqliteAuthDatabase::new(&get_temp_dir().join("mint-auth")).await?;

            auth_db.migrate().await;

            start_fake_mint_with_auth(addr, port, openid_discovery, sqlite_db, auth_db).await?;
        }
        "REDB" => {
            let redb_db = MintRedbDatabase::new(&get_temp_dir().join("mint"))?;

            let auth_db = MintRedbAuthDatabase::new(&get_temp_dir().join("mint-auth"))?;
            start_fake_mint_with_auth(addr, port, openid_discovery, redb_db, auth_db).await?;
        }
        _ => panic!("Unknown mint db type: {}", mint_db_kind),
    };
    Ok(())
}
