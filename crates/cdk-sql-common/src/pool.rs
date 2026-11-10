//! Very simple connection pool, to avoid an external dependency on r2d2 and other crates. If this
//! endup work it can be re-used in other parts of the project and may be promoted to its own
//! generic crate

use std::fmt::Debug;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

#[cfg(feature = "prometheus")]
use cdk_prometheus::metrics::METRICS;
use tokio::sync::{OwnedRwLockReadGuard, OwnedSemaphorePermit, RwLock, Semaphore};

use crate::database::DatabaseConnector;

// Drop cannot await disposal. Outside Tokio there is no worker to dispatch to.
fn dispose_on_blocking_worker<F>(dispose: F)
where
    F: FnOnce() + Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => {
            runtime.spawn_blocking(dispose);
        }
        Err(_) => dispose(),
    }
}

/// Pool error
#[derive(Debug, thiserror::Error)]
pub enum Error<E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    /// Mutex Poison Error
    #[error("Internal: PoisonError")]
    Poison,

    /// Timeout error
    #[error("Timed out waiting for a resource")]
    Timeout,

    /// The pool has been closed
    #[error("Database pool is closed")]
    Closed,

    /// No Tokio runtime is available for pool operations
    #[error("Database pool operations require a Tokio runtime")]
    MissingRuntime,

    /// A blocking database worker failed
    #[error("Database worker task failed: {0}")]
    Task(tokio::task::JoinError),

    /// Internal database error
    #[error(transparent)]
    Resource(#[from] E),
}

/// Configuration
pub trait DatabaseConfig: Clone + Debug + Send + Sync {
    /// Max resource sizes
    fn max_size(&self) -> usize;

    /// Default timeout
    fn default_timeout(&self) -> Duration;
}

/// Trait to manage resources
pub trait DatabasePool: Debug {
    /// The resource to be pooled
    type Connection: DatabaseConnector + 'static;

    /// The configuration that is needed in order to create the resource
    type Config: DatabaseConfig + 'static;

    /// The error the resource may return when creating a new instance
    type Error: Debug + std::error::Error + Send + Sync + 'static;

    /// Creates a new resource with a given config.
    ///
    /// If `stale` is ever set to TRUE it is assumed the resource is no longer valid and it will be
    /// dropped.
    fn new_resource(
        config: &Self::Config,
        stale: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Result<Self::Connection, Error<Self::Error>>;

    /// Close a resource, waiting for any detached operations to finish.
    ///
    /// The pool runs this on a blocking worker when a Tokio runtime is available.
    fn drop(_resource: Self::Connection) {}
}

/// Generic connection pool of resources R
pub struct Pool<RM>
where
    RM: DatabasePool,
{
    config: RM::Config,
    queue: Mutex<Vec<(Arc<AtomicBool>, RM::Connection)>>,
    max_size: usize,
    default_timeout: Duration,
    semaphore: Arc<Semaphore>,
    // Read guards cover loans, constructors, and stale-connection disposal.
    // Closing takes the write guard before draining idle connections.
    lifecycle: Arc<RwLock<()>>,
}

impl<RM> Debug for Pool<RM>
where
    RM: DatabasePool,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pool")
            .field("config", &self.config)
            .field("max_size", &self.max_size)
            .field("default_timeout", &self.default_timeout)
            .field("available_permits", &self.semaphore.available_permits())
            .finish()
    }
}

/// The pooled resource
pub struct PooledResource<RM>
where
    RM: DatabasePool,
{
    resource: Option<(Arc<AtomicBool>, RM::Connection)>,
    pool: Arc<Pool<RM>>,
    _permit: OwnedSemaphorePermit,
    lifecycle: Option<OwnedRwLockReadGuard<()>>,
    #[cfg(feature = "prometheus")]
    start_time: std::time::Instant,
}

impl<RM> PooledResource<RM>
where
    RM: DatabasePool,
{
    pub(crate) fn stale(&self) -> Arc<AtomicBool> {
        self.resource
            .as_ref()
            .expect("resource already dropped")
            .0
            .clone()
    }
}

impl<RM> Debug for PooledResource<RM>
where
    RM: DatabasePool,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Resource: {:?}", self.resource)
    }
}

impl<RM> Drop for PooledResource<RM>
where
    RM: DatabasePool,
{
    fn drop(&mut self) {
        if let Some((stale, resource)) = self.resource.take() {
            if stale.load(Ordering::SeqCst) {
                let lifecycle = self.lifecycle.take();
                dispose_on_blocking_worker(move || {
                    let _lifecycle = lifecycle;
                    RM::drop(resource);
                });
            } else {
                let mut active_resource = self.pool.queue.lock().expect("active_resource");
                active_resource.push((stale, resource));
            }
        }

        #[cfg(feature = "prometheus")]
        {
            METRICS.decrement_db_connections_active();
            let duration = self.start_time.elapsed().as_secs_f64();
            METRICS.record_db_operation(duration, "drop");
        }
        // The permit is released only after returning the resource or abandoning
        // a failed constructor. A cancelled constructor still owns this loan.
    }
}

impl<RM> Deref for PooledResource<RM>
where
    RM: DatabasePool,
{
    type Target = RM::Connection;

    fn deref(&self) -> &Self::Target {
        &self.resource.as_ref().expect("resource already dropped").1
    }
}

impl<RM> DerefMut for PooledResource<RM>
where
    RM: DatabasePool,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.resource.as_mut().expect("resource already dropped").1
    }
}

impl<RM> Pool<RM>
where
    RM: DatabasePool + 'static,
{
    /// Creates a new pool
    pub fn new(config: RM::Config) -> Arc<Self> {
        let max_size = config.max_size();
        Arc::new(Self {
            default_timeout: config.default_timeout(),
            max_size,
            config,
            queue: Default::default(),
            semaphore: Arc::new(Semaphore::new(max_size)),
            lifecycle: Arc::new(RwLock::new(())),
        })
    }

    /// Permanently close this pool and all its clones.
    ///
    /// Rejects new checkouts, waits for outstanding loans and cleanup, then closes
    /// idle connections on a blocking worker. Release borrowed connections before
    /// awaiting this method. Cancellation leaves the pool closed to new checkouts;
    /// calling `close` again waits for any closure already dispatched to finish.
    pub async fn close(self: &Arc<Self>) -> Result<(), Error<RM::Error>> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| Error::MissingRuntime)?;
        self.semaphore.close();
        let lifecycle = self.lifecycle.clone().write_owned().await;
        let pool = self.clone();
        runtime
            .spawn_blocking(move || {
                let _lifecycle = lifecycle;
                let resources = {
                    let mut queue = pool.queue.lock().map_err(|_| Error::Poison)?;
                    std::mem::take(&mut *queue)
                };
                for (_, resource) in resources {
                    RM::drop(resource);
                }
                Ok(())
            })
            .await
            .map_err(Error::Task)?
    }

    /// Similar to get_timeout but uses the default timeout value.
    #[inline(always)]
    pub async fn get(self: &Arc<Self>) -> Result<PooledResource<RM>, Error<RM::Error>> {
        self.get_timeout(self.default_timeout).await
    }

    /// Get a new resource or fail after timeout is reached.
    ///
    /// This function will return a free resource or create a new one if there is still room for it;
    /// otherwise, it will asynchronously wait for a resource to be released for reuse.
    #[inline(always)]
    pub async fn get_timeout(
        self: &Arc<Self>,
        timeout: Duration,
    ) -> Result<PooledResource<RM>, Error<RM::Error>> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| Error::MissingRuntime)?;

        // Fast path: try to grab a permit without waiting.
        let permit = match self.semaphore.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(tokio::sync::TryAcquireError::Closed) => return Err(Error::Closed),
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                // All permits are in use — wait asynchronously.  This yields
                // the task instead of blocking the OS thread, preventing Tokio
                // worker thread starvation.
                tracing::debug!(
                    "Pool exhausted (size: {}), waiting for a connection",
                    self.max_size,
                );
                tokio::time::timeout(timeout, self.semaphore.clone().acquire_owned())
                    .await
                    .map_err(|_| Error::Timeout)?
                    .map_err(|_| Error::Closed)?
            }
        };

        let lifecycle = self.lifecycle.clone().read_owned().await;
        if self.semaphore.is_closed() {
            return Err(Error::Closed);
        }

        #[cfg(feature = "prometheus")]
        METRICS.increment_db_connections_active();

        let mut loan = PooledResource {
            resource: None,
            pool: self.clone(),
            _permit: permit,
            lifecycle: Some(lifecycle),
            #[cfg(feature = "prometheus")]
            start_time: std::time::Instant::now(),
        };

        // Briefly lock the idle queue to try to pop a non-stale connection.
        // This mutex is held for nanoseconds (just a Vec::pop).
        let mut discarded = Vec::new();
        {
            let mut resources = match self.queue.lock() {
                Ok(resources) => resources,
                Err(_) => return Err(Error::Poison),
            };
            while let Some((stale, resource)) = resources.pop() {
                if !stale.load(Ordering::SeqCst) {
                    loan.resource = Some((stale, resource));
                    break;
                }
                discarded.push(resource);
            }
        }

        if loan.resource.is_some() && discarded.is_empty() {
            return Ok(loan);
        }

        // Opening a connection and configuring its journal can block too. The
        // worker owns the permit, including after cancellation of this await.
        let span = tracing::Span::current();
        runtime
            .spawn_blocking(move || {
                span.in_scope(move || {
                    for resource in discarded {
                        RM::drop(resource);
                    }
                    if loan.resource.is_none() {
                        let stale = Arc::new(AtomicBool::new(false));
                        let connection =
                            RM::new_resource(&loan.pool.config, stale.clone(), timeout)?;
                        loan.resource = Some((stale, connection));
                    }
                    Ok(loan)
                })
            })
            .await
            .map_err(Error::Task)?
    }
}

impl<RM> Drop for Pool<RM>
where
    RM: DatabasePool,
{
    fn drop(&mut self) {
        // Close the semaphore so no new acquisitions can succeed.
        self.semaphore.close();

        // Drop is best effort; callers needing a completed close must await close().
        let resources =
            std::mem::take(self.queue.get_mut().unwrap_or_else(PoisonError::into_inner));
        if resources.is_empty() {
            return;
        }
        dispose_on_blocking_worker(move || {
            for (_, resource) in resources {
                RM::drop(resource);
            }
        });
    }
}

#[cfg(all(test, feature = "prometheus"))]
mod tests {
    use std::fmt;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    use cdk_common::database::Error as DatabaseError;
    use cdk_prometheus::METRICS;

    use super::{DatabaseConfig, DatabasePool, Error, Pool};
    use crate::database::{DatabaseConnector, DatabaseExecutor, DatabaseTransaction};
    use crate::stmt::{Column, Statement};

    #[derive(Debug, Clone)]
    struct TestConfig {
        max_size: usize,
        default_timeout: Duration,
        fail_new_resource: bool,
    }

    impl DatabaseConfig for TestConfig {
        fn max_size(&self) -> usize {
            self.max_size
        }

        fn default_timeout(&self) -> Duration {
            self.default_timeout
        }
    }

    #[derive(Debug)]
    struct TestResourceError;

    impl fmt::Display for TestResourceError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("test resource error")
        }
    }

    impl std::error::Error for TestResourceError {}

    #[derive(Debug)]
    struct TestConnection;

    #[async_trait::async_trait]
    impl DatabaseExecutor for TestConnection {
        fn name() -> &'static str {
            "test"
        }

        async fn execute(&self, _statement: Statement) -> Result<usize, DatabaseError> {
            Ok(0)
        }

        async fn fetch_one(
            &self,
            _statement: Statement,
        ) -> Result<Option<Vec<Column>>, DatabaseError> {
            Ok(None)
        }

        async fn fetch_all(
            &self,
            _statement: Statement,
        ) -> Result<Vec<Vec<Column>>, DatabaseError> {
            Ok(Vec::new())
        }

        async fn pluck(&self, _statement: Statement) -> Result<Option<Column>, DatabaseError> {
            Ok(None)
        }

        async fn batch(&self, _statement: Statement) -> Result<(), DatabaseError> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct TestTransaction;

    #[async_trait::async_trait]
    impl DatabaseTransaction<TestConnection> for TestTransaction {
        async fn commit(_conn: &mut TestConnection) -> Result<(), DatabaseError> {
            Ok(())
        }

        async fn begin(_conn: &mut TestConnection) -> Result<(), DatabaseError> {
            Ok(())
        }

        async fn rollback(_conn: &mut TestConnection) -> Result<(), DatabaseError> {
            Ok(())
        }
    }

    impl DatabaseConnector for TestConnection {
        type Transaction = TestTransaction;
    }

    #[derive(Debug)]
    struct TestPool;

    impl DatabasePool for TestPool {
        type Connection = TestConnection;
        type Config = TestConfig;
        type Error = TestResourceError;

        fn new_resource(
            config: &Self::Config,
            _stale: Arc<AtomicBool>,
            _timeout: Duration,
        ) -> Result<Self::Connection, Error<Self::Error>> {
            if config.fail_new_resource {
                Err(Error::Resource(TestResourceError))
            } else {
                Ok(TestConnection)
            }
        }
    }

    fn test_config(max_size: usize, fail_new_resource: bool) -> TestConfig {
        TestConfig {
            max_size,
            default_timeout: Duration::from_millis(10),
            fail_new_resource,
        }
    }

    fn db_connections_active() -> f64 {
        for family in METRICS.registry().gather() {
            if family.name() != "cdk_db_connections_active" {
                continue;
            }

            return family
                .get_metric()
                .first()
                .expect("active connections metric should exist")
                .get_gauge()
                .value();
        }

        panic!("active connections metric should be registered");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn active_connections_gauge_tracks_current_checkout_and_drop_counts() {
        let _lock = crate::metrics_test_lock::lock().await;
        METRICS.set_db_connections_active(0);

        let pool = Pool::<TestPool>::new(test_config(2, false));

        let first = pool
            .get()
            .await
            .expect("first resource should be checked out");
        assert_eq!(db_connections_active(), 1.0);

        let second = pool
            .get()
            .await
            .expect("second resource should be checked out");
        assert_eq!(db_connections_active(), 2.0);

        drop(first);
        assert_eq!(db_connections_active(), 1.0);

        drop(second);
        assert_eq!(db_connections_active(), 0.0);
        assert_eq!(pool.semaphore.available_permits(), pool.max_size);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn active_connections_gauge_is_restored_when_resource_creation_fails() {
        let _lock = crate::metrics_test_lock::lock().await;
        METRICS.set_db_connections_active(0);

        let pool = Pool::<TestPool>::new(test_config(1, true));
        let result = pool.get().await;

        assert!(matches!(result, Err(Error::Resource(_))));
        assert_eq!(db_connections_active(), 0.0);
        assert_eq!(pool.semaphore.available_permits(), pool.max_size);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn active_connections_gauge_aggregates_multiple_pools() {
        let _lock = crate::metrics_test_lock::lock().await;
        METRICS.set_db_connections_active(0);

        let regular_pool = Pool::<TestPool>::new(test_config(1, false));
        let second_pool = Pool::<TestPool>::new(test_config(1, false));
        let regular = regular_pool.get().await.expect("regular resource");
        let second = second_pool.get().await.expect("second resource");

        assert_eq!(db_connections_active(), 2.0);

        drop(regular);
        drop(second);
        assert_eq!(db_connections_active(), 0.0);
    }
}
