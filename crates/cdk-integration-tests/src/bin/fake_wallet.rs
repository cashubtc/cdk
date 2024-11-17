use std::env;

use anyhow::Result;
use cdk::cdk_database::mint_memory::MintMemoryDatabase;
use cdk_integration_tests::init_fake_wallet::start_fake_mint;
use cdk_integration_tests::init_regtest::get_temp_dir;
use cdk_redb::MintRedbDatabase;
use cdk_sqlite::MintSqliteDatabase;

#[tokio::main]
async fn main() -> Result<()> {
    let addr = "127.0.0.1";
    let port = 8086;

    let mint_db_kind = env::var("MINT_DATABASE")?;

    match mint_db_kind.as_str() {
        "MEMORY" => {
            start_fake_mint(addr, port, MintMemoryDatabase::default()).await?;
        }
        "SQLITE" => {
            let sqlite_db = MintSqliteDatabase::new(&get_temp_dir().join("mint")).await?;
            sqlite_db.migrate().await;
            start_fake_mint(addr, port, sqlite_db).await?;
        }
        "REDB" => {
            let redb_db = MintRedbDatabase::new(&get_temp_dir().join("mint")).unwrap();
            start_fake_mint(addr, port, redb_db).await?;
        }
        _ => panic!("Unknown mint db type: {}", mint_db_kind),
    };
    Ok(())
}
