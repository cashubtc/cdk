use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Error, Pool, Sqlite};

#[inline(always)]
pub async fn create_sqlite_pool(
    path: &str,
    #[cfg(feature = "sqlcipher")] password: String,
) -> Result<Pool<Sqlite>, Error> {
    let db_options = SqliteConnectOptions::from_str(path)?
        .busy_timeout(Duration::from_secs(10))
        .read_only(false)
        .pragma("busy_timeout", "5000")
        .pragma("journal_mode", "wal")
        .pragma("synchronous", "normal")
        .pragma("temp_store", "memory")
        .pragma("mmap_size", "30000000000")
        .shared_cache(true)
        .create_if_missing(true);

    #[cfg(feature = "sqlcipher")]
    let db_options = db_options.pragma("key", password);

    let is_memory = path.contains(":memory:");

    let options = SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(1);

    let pool = if is_memory {
        // Make sure that the connection is not closed after the first query, or any query, as long
        // as the pool is not dropped
        options
            .idle_timeout(None)
            .max_lifetime(None)
            .test_before_acquire(false)
    } else {
        options
    }
    .connect_with(db_options)
    .await?;

    Ok(pool)
}
