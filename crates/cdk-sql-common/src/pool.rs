//! Very simple connection pool, to avoid an external dependency on r2d2 and other crates. If this
//! endup work it can be re-used in other parts of the project and may be promoted to its own
//! generic crate

use std::fmt::Debug;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[cfg(feature = "prometheus")]
use cdk_prometheus::metrics::METRICS;

use crate::database::DatabaseConnector;

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
    type Connection: DatabaseConnector;

    /// The configuration that is needed in order to create the resource
    type Config: DatabaseConfig;

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

    /// The object is dropped
    fn drop(_resource: Self::Connection) {}
}

/// Generic connection pool of resources R
#[derive(Debug)]
pub struct Pool<RM>
where
    RM: DatabasePool,
{
    config: RM::Config,
    queue: Mutex<Vec<(Arc<AtomicBool>, RM::Connection)>>,
    in_use: AtomicUsize,
    max_size: usize,
    default_timeout: Duration,
    waiter: Condvar,
}

/// The pooled resource
pub struct PooledResource<RM>
where
    RM: DatabasePool,
{
    resource: Option<(Arc<AtomicBool>, RM::Connection)>,
    pool: Arc<Pool<RM>>,
    #[cfg(feature = "prometheus")]
    start_time: Instant,
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
        if let Some(resource) = self.resource.take() {
            let mut active_resource = self.pool.queue.lock().expect("active_resource");
            active_resource.push(resource);
            let _in_use = self.pool.in_use.fetch_sub(1, Ordering::AcqRel);

            #[cfg(feature = "prometheus")]
            {
                METRICS.set_db_connections_active(_in_use as i64);

                let duration = self.start_time.elapsed().as_secs_f64();

                METRICS.record_db_operation(duration, "drop");
            }

            // Notify a waiting thread
            self.pool.waiter.notify_one();
        }
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
    RM: DatabasePool,
{
    /// Creates a new pool
    pub fn new(config: RM::Config) -> Arc<Self> {
        Arc::new(Self {
            default_timeout: config.default_timeout(),
            max_size: config.max_size(),
            config,
            queue: Default::default(),
            in_use: Default::default(),
            waiter: Default::default(),
        })
    }

    /// Similar to get_timeout but uses the default timeout value.
    #[inline(always)]
    pub fn get(self: &Arc<Self>) -> Result<PooledResource<RM>, Error<RM::Error>> {
        self.get_timeout(self.default_timeout)
    }

    /// Increments the in_use connection counter and updates the metric
    fn increment_connection_counter(&self) -> usize {
        let in_use = self.in_use.fetch_add(1, Ordering::AcqRel);

        #[cfg(feature = "prometheus")]
        {
            METRICS.set_db_connections_active(in_use as i64);
        }

        in_use
    }

    /// Get a new resource or fail after timeout is reached.
    ///
    /// This function will return a free resource or create a new one if there is still room for it;
    /// otherwise, it will wait for a resource to be released for reuse.
    #[inline(always)]
    pub fn get_timeout(
        self: &Arc<Self>,
        timeout: Duration,
    ) -> Result<PooledResource<RM>, Error<RM::Error>> {
        let mut resources = self.queue.lock().map_err(|_| Error::Poison)?;
        let time = Instant::now();

        loop {
            if let Some((stale, resource)) = resources.pop() {
                if !stale.load(Ordering::SeqCst) {
                    // Increment counter BEFORE releasing the mutex to prevent race condition
                    // where another thread sees in_use < max_size and creates a duplicate connection.
                    // For in-memory SQLite, each connection is a separate database, so this race
                    // would cause some connections to miss migrations.
                    self.increment_connection_counter();
                    drop(resources);

                    return Ok(PooledResource {
                        resource: Some((stale, resource)),
                        pool: self.clone(),
                        #[cfg(feature = "prometheus")]
                        start_time: Instant::now(),
                    });
                }
            }

            if self.in_use.load(Ordering::Relaxed) < self.max_size {
                // Increment counter BEFORE releasing the mutex to prevent race condition.
                // This ensures other threads see the updated count and wait instead of
                // creating additional connections beyond max_size.
                self.increment_connection_counter();
                drop(resources);
                let stale: Arc<AtomicBool> = Arc::new(false.into());
                match RM::new_resource(&self.config, stale.clone(), timeout) {
                    Ok(new_resource) => {
                        return Ok(PooledResource {
                            resource: Some((stale, new_resource)),
                            pool: self.clone(),
                            #[cfg(feature = "prometheus")]
                            start_time: Instant::now(),
                        });
                    }
                    Err(e) => {
                        // If resource creation fails, decrement the counter we just incremented
                        self.in_use.fetch_sub(1, Ordering::AcqRel);
                        return Err(e);
                    }
                }
            }

            resources = self
                .waiter
                .wait_timeout(resources, timeout)
                .map_err(|_| Error::Poison)
                .and_then(|(lock, timeout_result)| {
                    if timeout_result.timed_out() {
                        tracing::warn!(
                            "Timeout waiting for the resource (pool size: {}). Waited {} ms",
                            self.max_size,
                            time.elapsed().as_millis()
                        );
                        Err(Error::Timeout)
                    } else {
                        Ok(lock)
                    }
                })?;
        }
    }
}

impl<RM> Drop for Pool<RM>
where
    RM: DatabasePool,
{
    fn drop(&mut self) {
        if let Ok(mut resources) = self.queue.lock() {
            loop {
                while let Some(resource) = resources.pop() {
                    RM::drop(resource.1);
                }

                if self.in_use.load(Ordering::Relaxed) == 0 {
                    break;
                }

                resources = if let Ok(resources) = self.waiter.wait(resources) {
                    resources
                } else {
                    break;
                };
            }
        }
    }
}
