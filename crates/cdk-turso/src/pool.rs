use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use cdk_common::database::Error;
use cdk_sql_common::pool::{self, DatabaseConfig, DatabasePool};

use crate::connection::TursoConnection;

/// An opened Turso database shared by all connections in a pool.
#[derive(Debug, Clone)]
pub struct Config {
    database: Arc<turso::Database>,
}

impl Config {
    /// Open a local database file, or `:memory:` for an ephemeral database.
    pub async fn new<P>(path: P) -> Result<Self, Error>
    where
        P: AsRef<Path>,
    {
        let path = path.as_ref().to_str().ok_or_else(|| {
            Error::Internal("Turso database paths must be valid UTF-8".to_owned())
        })?;
        let database = turso::Builder::new_local(path)
            .build()
            .await
            .map_err(|err| Error::Database(Box::new(err)))?;
        Ok(Self {
            database: Arc::new(database),
        })
    }
}

impl DatabaseConfig for Config {
    fn max_size(&self) -> usize {
        // Turso has one writer. Serialize operations within each pool, avoiding
        // lock contention and ensuring in-memory and on-disk behavior agree.
        1
    }

    fn default_timeout(&self) -> Duration {
        Duration::from_secs(10)
    }
}

/// Connection manager for the shared CDK SQL implementation.
#[derive(Debug)]
pub struct TursoConnectionManager;

impl DatabasePool for TursoConnectionManager {
    type Config = Config;
    type Connection = TursoConnection;
    type Error = turso::Error;

    fn new_resource(
        config: &Self::Config,
        stale: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Result<Self::Connection, pool::Error<Self::Error>> {
        let connection = config.database.connect()?;
        connection.busy_timeout(timeout)?;
        Ok(TursoConnection::new(connection, stale))
    }
}
