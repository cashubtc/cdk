use std::fmt;

use cdk_common::database::Error;
use cdk_sql_common::database::SqlBackend;
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime, Timeouts};

use super::connection::{PostgresConnection, PostgresTransaction};
use super::PgConfig;

pub(crate) fn database_error<E>(error: E) -> Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    Error::Database(Box::new(error))
}

/// PostgreSQL backend using Deadpool for all connection pooling.
#[derive(Clone)]
pub struct PostgresBackend {
    pub(crate) pool: Pool,
    config: PgConfig,
}

impl fmt::Debug for PostgresBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PostgresBackend")
            .field("config", &self.config)
            .field("status", &self.pool.status())
            .finish()
    }
}

#[async_trait::async_trait]
impl SqlBackend for PostgresBackend {
    type Config = PgConfig;
    type Connection = PostgresConnection;
    type Transaction = PostgresTransaction;

    fn new(config: Self::Config) -> Result<Self, Error> {
        let (driver, tls) = config.driver_config()?;
        let manager_config = ManagerConfig {
            recycling_method: RecyclingMethod::Verified,
        };
        let manager = match tls {
            Some(tls) => Manager::from_config(driver, tls, manager_config),
            None => Manager::from_config(driver, tokio_postgres::NoTls, manager_config),
        };
        let pool = Pool::builder(manager)
            .max_size(config.max_connections)
            .runtime(Runtime::Tokio1)
            .timeouts(Timeouts {
                wait: Some(config.connection_timeout),
                create: Some(config.connection_timeout),
                recycle: Some(config.connection_timeout),
            })
            .build()
            .map_err(database_error)?;
        Ok(Self { pool, config })
    }

    async fn acquire(&self) -> Result<Self::Connection, Error> {
        let object = self.pool.get().await.map_err(database_error)?;
        Ok(PostgresConnection::new(
            object,
            self.config.schema.clone(),
            self.config.connection_timeout,
        ))
    }

    async fn begin_migration(&self) -> Result<Self::Transaction, Error> {
        let conn = self.acquire().await?;
        // Bootstrap before selecting a schema that may not exist yet.
        conn.begin_migration().await
    }
}
