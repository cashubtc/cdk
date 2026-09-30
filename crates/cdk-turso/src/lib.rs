//! Native, embedded Turso storage for CDK.
//!
//! Open a database with [`Config::new`], then pass the configuration to the
//! wallet, mint, or authentication database constructor. Connections share one database handle, including
//! for in-memory databases.

mod connection;
mod pool;

pub use self::pool::{Config, TursoConnectionManager};

#[cfg(feature = "wallet")]
pub mod wallet;
#[cfg(feature = "wallet")]
pub use self::wallet::WalletTursoDatabase;

#[cfg(feature = "mint")]
pub mod mint;
#[cfg(feature = "mint")]
pub use self::mint::{MintTursoAuthDatabase, MintTursoDatabase};
