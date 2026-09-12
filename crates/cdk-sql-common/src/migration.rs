//! Migrations whose steps are produced by Rust code rather than read from a `.sql` file.

use std::fmt::Debug;

use async_trait::async_trait;
use cdk_common::database::Error;

use crate::database::DatabaseExecutor;

/// The database a migration is written for.
///
/// Typed rather than a string so a misspelled dialect is a compile error, not a migration that
/// silently never runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dialect {
    /// SQLite, through `cdk-sqlite`.
    Sqlite,
    /// PostgreSQL, through `cdk-postgres`.
    Postgres,
}

impl Dialect {
    /// The driver name this dialect matches, as [`DatabaseExecutor::name`] reports it on the
    /// underlying connection.
    pub fn driver_name(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
        }
    }
}

/// A migration written in Rust.
///
/// It runs on the same connection and inside the same transaction as the SQL migrations, so it can
/// read rows, transform them with CDK types and write them back, which a `.sql` file cannot do. It
/// also lets one migration serve several dialects without duplicating the parts they share.
/// `Debug` is required so a registry entry can be printed when a test or an error message needs to
/// name the migration that misbehaved; migrations hold no state, so deriving it is free.
#[async_trait]
pub trait RustMigration<C>: Debug + Send + Sync
where
    C: DatabaseExecutor,
{
    /// Applies the migration.
    async fn apply(&self, conn: &C) -> Result<(), Error>;
}

/// A Rust migration registered with the runner.
#[derive(Debug)]
pub struct RegisteredRustMigration<C>
where
    C: DatabaseExecutor,
{
    /// The database this entry is for. A migration that must run on more than one dialect is
    /// registered once per dialect, which is why [`RustMigration::apply`] never has to ask which
    /// one it is on: the runner has already decided.
    pub dialect: Dialect,
    /// Name recorded in the `migrations` table. The dialect entries of one migration share a name,
    /// since a database is only ever one dialect.
    pub name: &'static str,
    /// The migration itself.
    pub migration: Box<dyn RustMigration<C>>,
}
