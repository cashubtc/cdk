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

    let pool = SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .connect_with(db_options)
        .await?;

    if path.contains(":memory:") {
        // Ensure that the pool has the minimum number of connections open
        // This makes sure the pool initializes with exactly one connection.
        // This is necessary because `min_connections` does not guarantee
        // an immediate connection unless it is actively used or explicitly initialized.
        let mut connection = pool.acquire().await?;

        // Hold the connection long enough that it's registered in the pool
        // You can even run a simple query to "warm it up"
        let _ = sqlx::query("SELECT 1").execute(&mut *connection).await?;
    }

    Ok(pool)
}
