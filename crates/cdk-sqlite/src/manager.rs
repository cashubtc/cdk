//! Deadpool manager for rusqlite connections.
//!
//! Implemented here rather than through `deadpool-sqlite`, which pins a single
//! rusqlite release. This keeps cdk-sqlite's supported rusqlite range so
//! applications can choose a version compatible with their other dependencies.

use std::sync::{Arc, Mutex, PoisonError};

use deadpool::managed::{self, Metrics, RecycleError, RecycleResult};
use rusqlite::{ffi, Connection};
use tokio::runtime::Handle;
use tokio::sync::Notify;

pub(crate) type Pool = managed::Pool<Manager>;
pub(crate) type Object = managed::Object<Manager>;
pub(crate) type PoolError = managed::PoolError<rusqlite::Error>;

/// Failure to run a closure on a pooled SQLite connection.
#[derive(Debug, thiserror::Error)]
pub(crate) enum InteractError {
    /// The closure panicked.
    #[error("SQLite blocking operation panicked")]
    Panic,
    /// The closure did not run, or the connection is no longer usable.
    #[error("SQLite blocking operation was aborted")]
    Aborted,
}

/// Physical connections that are open or still closing, including detached ones.
#[derive(Debug, Default)]
pub(crate) struct OpenConnections {
    count: Mutex<usize>,
    closed: Notify,
}

impl OpenConnections {
    fn count(&self) -> std::sync::MutexGuard<'_, usize> {
        self.count.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Wait until every connection has finished closing.
    pub(crate) async fn wait_closed(&self) {
        loop {
            let closed = self.closed.notified();
            tokio::pin!(closed);
            // Register before checking so a concurrent final close is not missed.
            closed.as_mut().enable();
            if *self.count() == 0 {
                return;
            }
            closed.await;
        }
    }
}

/// Counts one physical connection until dropped.
#[derive(Debug)]
struct OpenGuard(Arc<OpenConnections>);

impl OpenGuard {
    fn new(open: Arc<OpenConnections>) -> Self {
        *open.count() += 1;
        Self(open)
    }
}

impl Drop for OpenGuard {
    fn drop(&mut self) {
        let mut count = self.0.count();
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.0.closed.notify_waiters();
        }
    }
}

/// Closes the connection, waiting for any operation still using it.
#[derive(Debug)]
struct Closer {
    conn: Arc<Mutex<Option<Connection>>>,
    // Dropped after `Drop::drop` has closed the connection.
    _open: OpenGuard,
}

impl Drop for Closer {
    fn drop(&mut self) {
        let mut conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        drop(conn.take());
    }
}

/// Prefer the caller's runtime, as `tokio::task::spawn_blocking` would. The
/// runtime that opened the database is a fallback for callers outside Tokio,
/// such as a synchronous foreign caller dropping the last database handle.
fn blocking_runtime(fallback: &Handle) -> Handle {
    Handle::try_current().unwrap_or_else(|_| fallback.clone())
}

/// A pooled SQLite connection whose blocking work runs on Tokio's blocking pool.
#[derive(Debug)]
pub(crate) struct SqliteHandle {
    closer: Option<Closer>,
    runtime: Handle,
}

impl SqliteHandle {
    fn conn(&self) -> &Arc<Mutex<Option<Connection>>> {
        &self
            .closer
            .as_ref()
            .expect("handle owns its connection until drop")
            .conn
    }

    /// Run `operation` with exclusive access to the connection.
    ///
    /// Cancelling the returned future does not cancel a closure that has
    /// already been dispatched; it still runs to completion.
    pub(crate) async fn interact<F, R>(&self, operation: F) -> Result<R, InteractError>
    where
        F: FnOnce(&mut Connection) -> R + Send + 'static,
        R: Send + 'static,
    {
        let conn = self.conn().clone();
        let span = tracing::Span::current();
        blocking_runtime(&self.runtime)
            .spawn_blocking(move || {
                let _span = span.enter();
                let mut conn = conn.lock().map_err(|_| InteractError::Aborted)?;
                let conn = conn.as_mut().ok_or(InteractError::Aborted)?;
                Ok(operation(conn))
            })
            .await
            .map_err(|error| {
                if error.is_panic() {
                    InteractError::Panic
                } else {
                    InteractError::Aborted
                }
            })?
    }

    /// A panicked operation leaves the connection in an unknown state.
    fn is_poisoned(&self) -> bool {
        self.conn().is_poisoned()
    }
}

impl Drop for SqliteHandle {
    fn drop(&mut self) {
        if let Some(closer) = self.closer.take() {
            // Closing can checkpoint the WAL, so keep it off async workers. If
            // the runtime has shut down, the closure is dropped and the
            // connection closes on this thread instead.
            drop(blocking_runtime(&self.runtime).spawn_blocking(move || drop(closer)));
        }
    }
}

/// Opens SQLite connections for a Deadpool pool.
#[derive(Debug)]
pub(crate) struct Manager {
    path: String,
    runtime: Handle,
    open: Arc<OpenConnections>,
}

impl Manager {
    pub(crate) fn new(path: String, runtime: Handle, open: Arc<OpenConnections>) -> Self {
        Self {
            path,
            runtime,
            open,
        }
    }
}

impl managed::Manager for Manager {
    type Type = SqliteHandle;
    type Error = rusqlite::Error;

    async fn create(&self) -> Result<SqliteHandle, rusqlite::Error> {
        let path = self.path.clone();
        // Count the connection before opening it, so a cancelled checkout
        // still waits for the open connection to close.
        let open = OpenGuard::new(self.open.clone());
        let closer = blocking_runtime(&self.runtime)
            .spawn_blocking(move || {
                Connection::open(path).map(|conn| Closer {
                    conn: Arc::new(Mutex::new(Some(conn))),
                    _open: open,
                })
            })
            .await
            .map_err(|_| {
                rusqlite::Error::SqliteFailure(
                    ffi::Error::new(ffi::SQLITE_CANTOPEN),
                    Some("SQLite open task failed".to_owned()),
                )
            })??;
        Ok(SqliteHandle {
            closer: Some(closer),
            runtime: self.runtime.clone(),
        })
    }

    async fn recycle(
        &self,
        conn: &mut SqliteHandle,
        _: &Metrics,
    ) -> RecycleResult<rusqlite::Error> {
        // Local connections cannot drop like network sockets, so there is no
        // round trip to verify. Never yielding here also means a cancelled
        // checkout cannot discard the connection (and an in-memory database).
        if conn.is_poisoned() {
            return Err(RecycleError::message(
                "SQLite connection panicked during an operation",
            ));
        }
        Ok(())
    }
}
