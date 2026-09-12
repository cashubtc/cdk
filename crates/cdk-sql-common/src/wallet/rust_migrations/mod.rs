//! Wallet migrations written in Rust.
//!
//! The directory is deliberately not named `migrations`: `build.rs` walks the source tree for that
//! name and would try to embed these files as SQL.

use crate::database::DatabaseExecutor;
use crate::migration::{Dialect, RegisteredRustMigration};

mod mint_internal_id;

/// The name `mint_internal_id` is recorded under, so a test can build the schema as of just before
/// it without reaching into the module.
#[cfg(feature = "test")]
pub(crate) const MINT_INTERNAL_ID_NAME: &str = mint_internal_id::NAME;

/// Wallet Rust migrations, ordered by the numeric prefix of their names so they interleave with the
/// SQL migrations by date.
pub fn rust_migrations<C>() -> Vec<RegisteredRustMigration<C>>
where
    C: DatabaseExecutor + 'static,
{
    vec![
        RegisteredRustMigration {
            dialect: Dialect::Sqlite,
            name: mint_internal_id::NAME,
            migration: Box::new(mint_internal_id::Sqlite),
        },
        RegisteredRustMigration {
            dialect: Dialect::Postgres,
            name: mint_internal_id::NAME,
            migration: Box::new(mint_internal_id::Postgres),
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::rust_migrations;
    use crate::fake_executor::FakeExecutor;
    use crate::wallet::migrations::MIGRATIONS;

    #[test]
    fn no_dialect_registers_the_same_name_twice() {
        let mut seen = HashSet::new();

        for entry in rust_migrations::<FakeExecutor>() {
            assert!(
                seen.insert((entry.dialect, entry.name)),
                "{:?} registers {} twice, so the second would fail on the migrations primary key",
                entry.dialect,
                entry.name
            );
        }
    }

    #[test]
    fn no_name_collides_with_a_sql_migration() {
        for entry in rust_migrations::<FakeExecutor>() {
            assert!(
                !MIGRATIONS.iter().any(|(_, name, _)| *name == entry.name),
                "{} is also a .sql migration; one of them would be skipped as already applied",
                entry.name
            );
        }
    }
}
