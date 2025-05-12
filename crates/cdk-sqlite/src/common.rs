use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;

/// Create a configured rusqlite connection to a SQLite database.
/// For SQLCipher support, enable the "sqlcipher" feature and pass a password.
pub fn create_sqlite_pool(
    path: &str,
    #[cfg(feature = "sqlcipher")] password: String,
) -> Result<Pool<SqliteConnectionManager>, r2d2::Error> {
    let (manager, is_memory) = if path.contains(":memory:") {
        (SqliteConnectionManager::memory(), true)
    } else {
        (SqliteConnectionManager::file(path), false)
    };

    let manager = manager.with_init(|conn| {
        // Apply pragmas
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.pragma_update(None, "journal_mode", "wal")?;
        conn.pragma_update(None, "synchronous", "normal")?;
        conn.pragma_update(None, "temp_store", "memory")?;
        conn.pragma_update(None, "mmap_size", 30000000000i64)?;
        conn.pragma_update(None, "cache", "shared")?;

        #[cfg(feature = "sqlcipher")]
        conn.pragma_update(None, "key", &password)?;

        Ok(())
    });

    r2d2::Pool::builder()
        .max_size(if is_memory { 1 } else { 20 })
        .build(manager)
}
