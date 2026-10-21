use cdk_common::mint_db_test;
use cdk_sqlite::{mint, MintSqliteDatabase};

async fn provide_db(_test_name: String) -> MintSqliteDatabase {
    mint::memory::empty().await.expect("in-memory database")
}

mint_db_test!(provide_db);
