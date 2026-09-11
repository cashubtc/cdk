//! Points every wallet table at `mint.id` instead of `mint.mint_url`.
//!
//! A mint URL is a mutable attribute of a mint, not its identity. Keying every table on the URL
//! meant a mint that moves (NUT-06 `urls`) had to have each of those tables rewritten, and any
//! table missed left rows stranded under the old URL. Tables now reference an internal mint id that
//! never changes, so moving a mint is a single `UPDATE` on `mint`.
//!
//! This was two `.sql` files, one per dialect, that repeated the same six-statement block once per
//! table. Here the repetition is a loop over [`TABLES`] and the two dialects sit side by side, so a
//! table can no longer be converted in one dialect and forgotten in the other.

use async_trait::async_trait;
use cdk_common::database::Error;

use crate::database::DatabaseExecutor;
use crate::migration::RustMigration;
use crate::stmt::query;

pub(super) const NAME: &str = "20260902000000_mint_internal_id.rs";

/// A table that referenced a mint by URL and now references it by `mint.id`.
struct MintRef {
    table: &'static str,
    /// `melt_quote.mint_url` was nullable, so its `mint_id` stays nullable on PostgreSQL and the
    /// orphan backfill has to skip its NULLs on both dialects.
    nullable: bool,
    /// SQLite cannot drop a column a foreign key depends on, so `keyset` is rebuilt there instead
    /// of going through the generic add/update/drop path.
    sqlite_rebuilt: bool,
    /// Index over the old `mint_url` column, which neither engine will let us drop underneath.
    stale_mint_url_index: Option<&'static str>,
}

/// Every name below is a literal in this file, never a runtime or user-supplied value, so
/// interpolating them into SQL cannot inject anything. `batch` rejects placeholders and identifiers
/// cannot be bound in any case, so interpolation is also the only option.
const TABLES: &[MintRef] = &[
    MintRef {
        table: "keyset",
        nullable: false,
        sqlite_rebuilt: true,
        stale_mint_url_index: None,
    },
    MintRef {
        table: "proof",
        nullable: false,
        sqlite_rebuilt: false,
        stale_mint_url_index: None,
    },
    MintRef {
        table: "mint_quote",
        nullable: false,
        sqlite_rebuilt: false,
        stale_mint_url_index: None,
    },
    MintRef {
        table: "melt_quote",
        nullable: true,
        sqlite_rebuilt: false,
        stale_mint_url_index: None,
    },
    MintRef {
        table: "transactions",
        nullable: false,
        sqlite_rebuilt: false,
        stale_mint_url_index: Some("mint_url_index"),
    },
    MintRef {
        table: "wallet_sagas",
        nullable: false,
        sqlite_rebuilt: false,
        stale_mint_url_index: Some("wallet_sagas_mint_url_index"),
    },
];

/// SQLite cannot add `AUTOINCREMENT` to an existing table, nor drop the column `keyset`'s foreign
/// key depends on, so both tables are rebuilt.
///
/// The order matters. `keyset` references `mint(mint_url)` `ON DELETE CASCADE`, and a `DROP TABLE`
/// with foreign keys enforced performs an implicit `DELETE FROM` that fires that cascade. Dropping
/// the old `mint` before detaching `keyset` therefore deletes every keyset row. Renaming the old
/// `mint` aside instead repoints the cascade at a table nothing needs any more, which is then
/// dropped last, once `keyset` has been rebuilt against the new one.
///
/// `PRAGMA foreign_keys` cannot be used to avoid this: SQLite ignores it inside a transaction, and
/// the runner always holds one. rusqlite's bundled SQLite enforces foreign keys by default.
const REBUILD_MINT_AND_KEYSET: &[&str] = &[
    "CREATE TABLE mint_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    mint_url TEXT NOT NULL UNIQUE,
    name TEXT,
    pubkey BLOB,
    version TEXT,
    description TEXT,
    description_long TEXT,
    contact TEXT,
    nuts TEXT,
    motd TEXT,
    icon_url TEXT,
    mint_time INTEGER,
    urls TEXT,
    tos_url TEXT
)",
    "INSERT INTO mint_new (
    mint_url, name, pubkey, version, description, description_long,
    contact, nuts, motd, icon_url, mint_time, urls, tos_url
)
SELECT
    mint_url, name, pubkey, version, description, description_long,
    contact, nuts, motd, icon_url, mint_time, urls, tos_url
FROM mint",
    "ALTER TABLE mint RENAME TO mint_old",
    "ALTER TABLE mint_new RENAME TO mint",
    "CREATE TABLE keyset_new (
    id TEXT PRIMARY KEY,
    mint_id INTEGER NOT NULL,
    keyset_u32 INTEGER,
    unit TEXT NOT NULL,
    active BOOL NOT NULL,
    input_fee_ppk INTEGER,
    final_expiry INTEGER DEFAULT NULL,
    FOREIGN KEY(mint_id) REFERENCES mint(id) ON DELETE CASCADE
)",
    "INSERT INTO keyset_new (id, mint_id, keyset_u32, unit, active, input_fee_ppk, final_expiry)
SELECT k.id, m.id, k.keyset_u32, k.unit, k.active, k.input_fee_ppk, k.final_expiry
FROM keyset k
JOIN mint m ON m.mint_url = k.mint_url",
    "DROP TABLE keyset",
    "ALTER TABLE keyset_new RENAME TO keyset",
    "DROP TABLE mint_old",
    "CREATE UNIQUE INDEX IF NOT EXISTS keyset_u32_unique_keyset ON keyset(keyset_u32)",
    "CREATE INDEX IF NOT EXISTS keyset_mint_id_index ON keyset(mint_id)",
];

/// PostgreSQL can add the identity column in place, but the primary key has to move off `mint_url`,
/// and `keyset`'s foreign key points at that key, so it goes first.
const REKEY_MINT: &[&str] = &[
    "ALTER TABLE mint ADD COLUMN id BIGINT GENERATED BY DEFAULT AS IDENTITY",
    "ALTER TABLE keyset DROP CONSTRAINT IF EXISTS keyset_mint_url_fkey",
    "ALTER TABLE mint DROP CONSTRAINT mint_pkey",
    "ALTER TABLE mint ADD PRIMARY KEY (id)",
    "ALTER TABLE mint ALTER COLUMN mint_url SET NOT NULL",
    "ALTER TABLE mint ADD CONSTRAINT mint_mint_url_key UNIQUE (mint_url)",
];

/// Rows could reference a mint URL that was never added to `mint`. Give those a mint row so nothing
/// is stranded once the reference becomes an id.
///
/// PostgreSQL requires a name for a subquery in `FROM`; SQLite tolerates one, so both dialects
/// share this statement.
fn orphan_backfill() -> String {
    let sources = TABLES
        .iter()
        .map(|mint_ref| {
            let table = mint_ref.table;
            if mint_ref.nullable {
                format!("SELECT mint_url FROM {table} WHERE mint_url IS NOT NULL")
            } else {
                format!("SELECT mint_url FROM {table}")
            }
        })
        .collect::<Vec<_>>()
        .join(" UNION ");

    format!(
        "INSERT INTO mint (mint_url) SELECT mint_url FROM ({sources}) AS referenced \
         WHERE mint_url NOT IN (SELECT mint_url FROM mint)"
    )
}

/// The statements this migration runs on SQLite, in order.
fn sqlite_statements() -> Vec<String> {
    let mut statements = vec![orphan_backfill()];

    statements.extend(REBUILD_MINT_AND_KEYSET.iter().map(|sql| (*sql).to_owned()));
    statements.extend(
        TABLES
            .iter()
            .filter_map(|mint_ref| mint_ref.stale_mint_url_index)
            .map(|index| format!("DROP INDEX IF EXISTS {index}")),
    );

    for mint_ref in TABLES.iter().filter(|mint_ref| !mint_ref.sqlite_rebuilt) {
        let table = mint_ref.table;
        statements.extend([
            format!("ALTER TABLE {table} ADD COLUMN mint_id INTEGER"),
            format!(
                "UPDATE {table} SET mint_id = \
                 (SELECT m.id FROM mint m WHERE m.mint_url = {table}.mint_url)"
            ),
            format!("ALTER TABLE {table} DROP COLUMN mint_url"),
            format!("CREATE INDEX IF NOT EXISTS {table}_mint_id_index ON {table}(mint_id)"),
        ]);
    }

    statements
}

/// The statements this migration runs on PostgreSQL, in order.
fn postgres_statements() -> Vec<String> {
    let mut statements = vec![orphan_backfill()];

    statements.extend(REKEY_MINT.iter().map(|sql| (*sql).to_owned()));

    for mint_ref in TABLES {
        let table = mint_ref.table;

        statements.push(format!("ALTER TABLE {table} ADD COLUMN mint_id BIGINT"));
        statements.push(format!(
            "UPDATE {table} SET mint_id = mint.id FROM mint WHERE mint.mint_url = {table}.mint_url"
        ));

        if !mint_ref.nullable {
            statements.push(format!(
                "ALTER TABLE {table} ALTER COLUMN mint_id SET NOT NULL"
            ));
        }

        if let Some(index) = mint_ref.stale_mint_url_index {
            statements.push(format!("DROP INDEX IF EXISTS {index}"));
        }

        statements.push(format!("ALTER TABLE {table} DROP COLUMN mint_url"));
        statements.push(format!(
            "ALTER TABLE {table} ADD CONSTRAINT {table}_mint_id_fkey \
             FOREIGN KEY (mint_id) REFERENCES mint(id) ON DELETE CASCADE"
        ));
        statements.push(format!(
            "CREATE INDEX IF NOT EXISTS {table}_mint_id_index ON {table}(mint_id)"
        ));
    }

    statements
}

async fn run<C>(conn: &C, statements: Vec<String>) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    for statement in statements {
        query(&statement)?.batch(conn).await?;
    }

    Ok(())
}

/// The SQLite form of the migration.
///
/// `PRAGMA foreign_keys` is deliberately not touched: SQLite ignores it inside a transaction, and
/// the runner always holds one. Nothing in `cdk-sqlite` turns foreign keys on, so the rebuilds and
/// column drops below are unconstrained.
pub(super) struct Sqlite;

#[async_trait]
impl<C> RustMigration<C> for Sqlite
where
    C: DatabaseExecutor,
{
    async fn apply(&self, conn: &C) -> Result<(), Error> {
        run(conn, sqlite_statements()).await
    }
}

/// The PostgreSQL form of the migration.
pub(super) struct Postgres;

#[async_trait]
impl<C> RustMigration<C> for Postgres
where
    C: DatabaseExecutor,
{
    async fn apply(&self, conn: &C) -> Result<(), Error> {
        run(conn, postgres_statements()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolated_names_are_plain_identifiers() {
        let identifier = |name: &str| name.chars().all(|c| c.is_ascii_lowercase() || c == '_');

        for mint_ref in TABLES {
            assert!(identifier(mint_ref.table), "{}", mint_ref.table);

            if let Some(index) = mint_ref.stale_mint_url_index {
                assert!(identifier(index), "{index}");
            }
        }
    }

    #[test]
    fn sqlite_converts_every_table_exactly_once() {
        let statements = sqlite_statements();

        for mint_ref in TABLES {
            let table = mint_ref.table;
            let drops = statements
                .iter()
                .filter(|sql| **sql == format!("ALTER TABLE {table} DROP COLUMN mint_url"))
                .count();

            assert_eq!(drops, usize::from(!mint_ref.sqlite_rebuilt), "{table}");
        }

        assert!(statements
            .iter()
            .any(|sql| sql.contains("keyset_mint_id_index")));
    }

    #[test]
    fn postgres_keeps_melt_quote_nullable() {
        let statements = postgres_statements();

        for mint_ref in TABLES {
            let table = mint_ref.table;
            let not_null = statements
                .iter()
                .any(|sql| **sql == format!("ALTER TABLE {table} ALTER COLUMN mint_id SET NOT NULL"));

            assert_eq!(not_null, !mint_ref.nullable, "{table}");
        }
    }

    #[test]
    fn backfill_skips_nullable_mint_urls() {
        let backfill = orphan_backfill();

        assert!(backfill.contains("SELECT mint_url FROM melt_quote WHERE mint_url IS NOT NULL"));
        assert!(backfill.contains("SELECT mint_url FROM proof UNION"));
    }
}

