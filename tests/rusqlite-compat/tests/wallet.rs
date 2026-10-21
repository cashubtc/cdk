use cdk_common::wallet_db_test;
use cdk_sqlite::{wallet, WalletSqliteDatabase};

async fn provide_db(_test_name: String) -> WalletSqliteDatabase {
    wallet::memory::empty().await.expect("in-memory database")
}

wallet_db_test!(provide_db);
