use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cdk_common::database::Error;
use cdk_sql_common::database::SqlBackend;
use deadpool_sqlite::{Hook, HookError, Pool, PoolError, Runtime, TimeoutType, Timeouts};

use super::common::Config;
use super::connection::{SqliteConnection, SqliteTransaction};

pub(crate) fn database_error<E>(error: E) -> Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    Error::Database(Box::new(error))
}

/// SQLite backend using Deadpool for all connection pooling.
#[derive(Clone)]
pub struct SqliteBackend {
    pub(crate) inner: Arc<BackendInner>,
}

#[derive(Debug)]
pub(crate) struct BackendInner {
    pool: Option<Pool>,
    pub(crate) runtime: tokio::runtime::Handle,
    in_memory: bool,
    initialized: Arc<AtomicBool>,
}

impl BackendInner {
    pub(crate) fn pool(&self) -> &Pool {
        self.pool
            .as_ref()
            .expect("backend owns its library pool until drop")
    }
    pub(crate) fn discard_memory(&self) {
        if self.in_memory {
            self.pool().close();
        }
    }
}

impl Drop for BackendInner {
    fn drop(&mut self) {
        // deadpool-sqlite 0.8 uses spawn_blocking when dropping connections.
        // Enter the owning runtime even if the last database handle is dropped
        // by a synchronous FFI caller, or after runtime shutdown.
        let _entered = self.runtime.enter();
        drop(self.pool.take());
    }
}

impl fmt::Debug for SqliteBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SqliteBackend")
            .field("in_memory", &self.inner.in_memory)
            .field("status", &self.inner.pool().status())
            .finish()
    }
}

#[async_trait::async_trait]
impl SqlBackend for SqliteBackend {
    type Config = Config;
    type Connection = SqliteConnection;
    type Transaction = SqliteTransaction;

    fn new(config: Config) -> Result<Self, Error> {
        let runtime = tokio::runtime::Handle::try_current().map_err(database_error)?;
        let in_memory = config.path.is_none();
        let initialized = Arc::new(AtomicBool::new(false));
        let init_hook = initialized.clone();
        let driver = deadpool_sqlite::Config::new(config.path.as_deref().unwrap_or(":memory:"));
        let password = config.password;
        let pool = driver
            .builder(Runtime::Tokio1)
            .map_err(database_error)?
            .max_size(if in_memory { 1 } else { 20 })
            .timeouts(Timeouts {
                wait: Some(Duration::from_secs(5)),
                create: Some(Duration::from_secs(10)),
                recycle: Some(Duration::from_secs(10)),
            })
            .post_create(Hook::async_fn(move |conn, _| {
                let password = password.clone();
                let initialized = init_hook.clone();
                Box::pin(async move {
                    // Library recycling may discard a broken connection. Never
                    // let that silently replace an initialized in-memory DB.
                    if in_memory && initialized.load(Ordering::Acquire) {
                        return Err(HookError::message(
                            "In-memory SQLite connection was lost; \
                             database must be reopened explicitly",
                        ));
                    }
                    conn.interact(move |conn| {
                        if let Some(password) = password {
                            conn.pragma_update(None, "key", password)?;
                        }
                        conn.execute_batch(
                            "PRAGMA busy_timeout = 10000;
                             PRAGMA journal_mode = WAL;
                             PRAGMA synchronous = FULL;
                             PRAGMA temp_store = memory;
                             PRAGMA mmap_size = 5242880;
                             PRAGMA cache = shared;",
                        )?;
                        conn.busy_timeout(Duration::from_secs(10))
                    })
                    .await
                    .map_err(|e| HookError::message(e.to_string()))?
                    .map_err(HookError::Backend)?;
                    initialized.store(true, Ordering::Release);
                    Ok(())
                })
            }))
            .build()
            .map_err(database_error)?;
        Ok(Self {
            inner: Arc::new(BackendInner {
                pool: Some(pool),
                runtime,
                in_memory,
                initialized,
            }),
        })
    }

    async fn acquire(&self) -> Result<Self::Connection, Error> {
        match self.inner.pool().get().await {
            Ok(object) => Ok(SqliteConnection::new(object, self.inner.clone())),
            Err(error) => {
                if self.inner.in_memory
                    && self.inner.initialized.load(Ordering::Acquire)
                    && !matches!(error, PoolError::Timeout(TimeoutType::Wait))
                {
                    self.inner.pool().close();
                }
                Err(database_error(error))
            }
        }
    }
}
