//! Wallet migrations written in Rust.
//!
//! The directory is deliberately not named `migrations`: `build.rs` walks the source tree for that
//! name and would try to embed these files as SQL.

use crate::database::DatabaseExecutor;
use crate::migration::RegisteredRustMigration;

mod mint_internal_id;

/// Wallet Rust migrations, ordered by the numeric prefix of their names so they interleave with the
/// SQL migrations by date.
pub fn rust_migrations<C>() -> Vec<RegisteredRustMigration<C>>
where
    C: DatabaseExecutor + 'static,
{
    vec![
        (
            "sqlite",
            mint_internal_id::NAME,
            Box::new(mint_internal_id::Sqlite),
        ),
        (
            "postgres",
            mint_internal_id::NAME,
            Box::new(mint_internal_id::Postgres),
        ),
    ]
}
