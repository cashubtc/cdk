use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Error, Executor, Pool, Sqlite};

#[inline(always)]
pub async fn create_sqlite_pool(path: &str) -> Result<Pool<Sqlite>, Error> {
    let db_options = SqliteConnectOptions::from_str(path)?
        .busy_timeout(Duration::from_secs(10))
        .read_only(false)
        .create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .min_connections(1)
        .max_connections(1)
        .before_acquire(|conn, _meta| {
            Box::pin(async move {
                // Info: https://phiresky.github.io/blog/2020/sqlite-performance-tuning/
                conn.execute(
                    r#"
                        PRAGMA busy_timeout = 5000;
                        PRAGMA journal_mode = wal;
                        PRAGMA synchronous = normal;
                        PRAGMA temp_store = memory;
                        PRAGMA mmap_size = 30000000000;
                        "#,
                )
                .await?;

                Ok(true)
            })
        })
        .connect_with(db_options)
        .await?;

    Ok(pool)
}
