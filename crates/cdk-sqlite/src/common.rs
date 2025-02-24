use std::fs::remove_file;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Error, Pool, Sqlite};

static FILE_ID: AtomicU64 = AtomicU64::new(0);

/// A wrapper around a `Pool<Sqlite>` that may delete the database file when dropped in order to by
/// pass the SQLx bug with pools and in-memory databases.
///
/// [1] https://github.com/launchbadge/sqlx/issues/362
/// [2] https://github.com/launchbadge/sqlx/issues/2510
#[derive(Debug, Clone)]
pub struct SqlitePool {
    pool: Pool<Sqlite>,
    path: String,
    delete: bool,
}

impl Drop for SqlitePool {
    fn drop(&mut self) {
        if self.delete {
            let _ = remove_file(&self.path);
        }
    }
}

impl Deref for SqlitePool {
    type Target = Pool<Sqlite>;

    fn deref(&self) -> &Self::Target {
        &self.pool
    }
}

#[inline(always)]
pub async fn create_sqlite_pool(path: &str) -> Result<SqlitePool, Error> {
    let (path, delete) = if path.ends_with(":memory:") {
        (
            format!(
                "in-memory-{}-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                FILE_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            ),
            true,
        )
    } else {
        (path.to_owned(), false)
    };

    let db_options = SqliteConnectOptions::from_str(&path)?
        .journal_mode(if delete {
            sqlx::sqlite::SqliteJournalMode::Memory
        } else {
            sqlx::sqlite::SqliteJournalMode::Wal
        })
        .busy_timeout(Duration::from_secs(5))
        .read_only(false)
        .create_if_missing(true)
        .auto_vacuum(sqlx::sqlite::SqliteAutoVacuum::Full);

    Ok(SqlitePool {
        pool: SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(db_options)
            .await?,
        delete,
        path,
    })
}
