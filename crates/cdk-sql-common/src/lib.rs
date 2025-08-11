//! SQLite storage backend for cdk

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

mod common;
pub mod database;
mod macros;
pub mod pool;
pub mod stmt;
pub mod value;

pub use cdk_common::database::ConversionError;
pub use common::{run_db_operation, run_db_operation_sync};

#[cfg(feature = "mint")]
pub mod mint;
#[cfg(feature = "wallet")]
pub mod wallet;

#[cfg(feature = "mint")]
pub use mint::SQLMintDatabase;
#[cfg(feature = "wallet")]
pub use wallet::SQLWalletDatabase;
