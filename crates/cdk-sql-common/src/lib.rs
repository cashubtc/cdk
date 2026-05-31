//! SQLite storage backend for cdk

mod common;
pub mod database;
mod keyvalue;
mod macros;
pub mod pool;
pub mod stmt;
pub mod value;

pub use cdk_common::database::ConversionError;
pub use common::{migrate, run_db_operation, run_db_operation_sync};

#[cfg(feature = "mint")]
pub mod mint;
#[cfg(feature = "wallet")]
pub mod wallet;

#[cfg(all(test, feature = "prometheus"))]
mod metrics_test_lock {
    use tokio::sync::{Mutex, MutexGuard};

    static METRICS_TEST_LOCK: Mutex<()> = Mutex::const_new(());

    pub(crate) async fn lock() -> MutexGuard<'static, ()> {
        METRICS_TEST_LOCK.lock().await
    }
}

#[cfg(feature = "mint")]
pub use mint::SQLMintDatabase;
#[cfg(feature = "wallet")]
pub use wallet::SQLWalletDatabase;
