//! Migrations whose steps are produced by Rust code rather than read from a `.sql` file.

use async_trait::async_trait;
use cdk_common::database::Error;

use crate::database::DatabaseExecutor;

/// A migration written in Rust.
///
/// It runs on the same connection and inside the same transaction as the SQL migrations, so it can
/// read rows, transform them with CDK types and write them back, which a `.sql` file cannot do. It
/// also lets one migration serve several dialects without duplicating the parts they share.
#[async_trait]
pub trait RustMigration<C>: Send + Sync
where
    C: DatabaseExecutor,
{
    /// Applies the migration.
    async fn apply(&self, conn: &C) -> Result<(), Error>;
}

/// A Rust migration as the runner sees it: the dialect it applies to (empty for every dialect), the
/// name recorded in the `migrations` table, and the migration itself.
///
/// The dialect is carried here rather than read from the connection because the runner is always
/// handed a transaction, whose [`DatabaseExecutor::name`] is `"Transaction"` and not the driver.
pub type RegisteredRustMigration<C> = (&'static str, &'static str, Box<dyn RustMigration<C>>);
