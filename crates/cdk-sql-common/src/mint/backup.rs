//! Backup database implementation

use async_trait::async_trait;
use bitcoin::hashes::{sha256, Hash, HashEngine};
use cdk_common::database::mint::{BackupDatabase, BackupFormat, BackupResult};
use cdk_common::database::Error;
use cdk_common::util::hex;
use cdk_common::util::unix_time;
use tracing::instrument;

use super::SQLMintDatabase;
use crate::database::DatabaseExecutor;
use crate::pool::DatabasePool;
use crate::stmt::query;

#[async_trait]
impl<RM> BackupDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    #[instrument(skip(self), err)]
    async fn create_backup(&self, format: BackupFormat) -> Result<BackupResult, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        let engine = RM::Connection::name().to_string();

        if format == BackupFormat::Sqlite && engine != "sqlite" {
            return Err(Error::Internal(
                "SQLite backup format is only supported for SQLite databases".to_string(),
            ));
        }

        let data = match format {
            BackupFormat::Sqlite => create_sqlite_backup(&*conn).await?,
            BackupFormat::Sql => create_sql_dump(&*conn, &engine).await?,
        };

        let checksum = checksum_hex(&data);

        Ok(BackupResult {
            data,
            format,
            checksum,
            created_at: unix_time(),
            database_engine: engine,
        })
    }
}

/// Create a SQLite backup using VACUUM INTO for a consistent snapshot.
async fn create_sqlite_backup<C: DatabaseExecutor>(conn: &C) -> Result<Vec<u8>, Error> {
    let temp_path = std::env::temp_dir().join(format!("cdk_backup_{}.db", unix_time()));
    let temp_path_str = temp_path.to_string_lossy();

    query(&format!("VACUUM INTO '{}'", temp_path_str))?
        .execute(conn)
        .await?;

    let backup_data = std::fs::read(&temp_path).map_err(|e| {
        Error::Database(Box::new(std::io::Error::other(format!(
            "Failed to read backup file: {}",
            e
        ))))
    })?;

    // Best-effort cleanup
    let _ = std::fs::remove_file(&temp_path);

    Ok(backup_data)
}

/// Create a SQL dump of all tables.
async fn create_sql_dump<C: DatabaseExecutor>(conn: &C, engine: &str) -> Result<Vec<u8>, Error> {
    let mut sql_dump = String::new();

    // Get all table names
    let tables_query = if engine == "sqlite" {
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
    } else {
        "SELECT tablename FROM pg_tables WHERE schemaname = 'public' ORDER BY tablename"
    };

    let table_rows = query(tables_query)?.fetch_all(conn).await?;

    for row in table_rows {
        let table_name = match &row[0] {
            crate::stmt::Column::Text(name) => name.clone(),
            _ => continue,
        };

        // Add table header comment
        sql_dump.push_str(&format!("\n-- Table: {}\n", table_name));

        // Get all rows from the table
        let select_query = format!("SELECT * FROM {}", table_name);
        let rows = query(&select_query)?.fetch_all(conn).await?;

        for row in rows {
            let values: Vec<String> = row
                .iter()
                .map(|col| match col {
                    crate::stmt::Column::Null => "NULL".to_string(),
                    crate::stmt::Column::Integer(i) => i.to_string(),
                    crate::stmt::Column::Real(f) => f.to_string(),
                    crate::stmt::Column::Text(s) => format!("'{}'", s.replace('\'', "''")),
                    crate::stmt::Column::Blob(b) => format!("X'{}'", hex::encode(b)),
                })
                .collect();

            if !values.is_empty() {
                sql_dump.push_str(&format!(
                    "INSERT INTO {} VALUES ({});\n",
                    table_name,
                    values.join(", ")
                ));
            }
        }
    }

    Ok(sql_dump.into_bytes())
}

/// Calculate SHA256 checksum and return as hex string.
fn checksum_hex(data: &[u8]) -> String {
    let mut hasher = sha256::Hash::engine();
    hasher.input(data);
    let hash = sha256::Hash::from_engine(hasher);
    hex::encode(hash.as_byte_array())
}
