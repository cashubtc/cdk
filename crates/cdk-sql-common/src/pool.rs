//! Very simple connection pool, to avoid an external dependency on r2d2 and other crates. If this
//! endup work it can be re-used in other parts of the project and may be promoted to its own
//! generic crate

use std::fmt::Debug;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(feature = "prometheus")]
use cdk_prometheus::metrics::METRICS;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

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
pub struct Pool<RM>
where
    RM: DatabasePool,
{
    config: RM::Config,
    queue: Mutex<Vec<(Arc<AtomicBool>, RM::Connection)>>,
    max_size: usize,
    default_timeout: Duration,
    semaphore: Arc<Semaphore>,
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
    #[cfg(feature = "prometheus")]
    start_time: std::time::Instant,
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

            #[cfg(feature = "prometheus")]
            {
                let in_use = self.pool.max_size - self.pool.semaphore.available_permits();
                METRICS.set_db_connections_active(in_use as i64);

                let duration = self.start_time.elapsed().as_secs_f64();

                METRICS.record_db_operation(duration, "drop");
            }

            // The semaphore permit is dropped automatically after this,
            // which wakes any async task waiting in `get()`.
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
        let max_size = config.max_size();
        Arc::new(Self {
            default_timeout: config.default_timeout(),
            max_size,
            config,
            queue: Default::default(),
            semaphore: Arc::new(Semaphore::new(max_size)),
        })
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
        // Acquire a semaphore permit asynchronously. This yields the task instead of
        // blocking the OS thread, preventing Tokio worker thread starvation.
        let permit =
            match tokio::time::timeout(timeout, self.semaphore.clone().acquire_owned()).await {
                Ok(Ok(permit)) => permit,
                Ok(Err(_closed)) => {
                    // Semaphore was closed (pool is being dropped)
                    return Err(Error::Poison);
                }
                Err(_elapsed) => {
                    tracing::warn!(
                        "Timeout waiting for the resource (pool size: {})",
                        self.max_size,
                    );
                    return Err(Error::Timeout);
                }
            };

        #[cfg(feature = "prometheus")]
        {
            let in_use = self.max_size - self.semaphore.available_permits();
            METRICS.set_db_connections_active(in_use as i64);
        }

        // Briefly lock the idle queue to try to pop a non-stale connection.
        // This mutex is held for nanoseconds (just a Vec::pop).
        {
            let mut resources = self.queue.lock().map_err(|_| Error::Poison)?;
            while let Some((stale, resource)) = resources.pop() {
                if !stale.load(Ordering::SeqCst) {
                    return Ok(PooledResource {
                        resource: Some((stale, resource)),
                        pool: self.clone(),
                        _permit: permit,
                        #[cfg(feature = "prometheus")]
                        start_time: std::time::Instant::now(),
                    });
                }
                // Stale connection — drop it and keep looking.
            }
        }

        // No idle connection available — create a new one.
        // The semaphore already guarantees we won't exceed max_size.
        let stale: Arc<AtomicBool> = Arc::new(false.into());
        match RM::new_resource(&self.config, stale.clone(), timeout) {
            Ok(new_resource) => Ok(PooledResource {
                resource: Some((stale, new_resource)),
                pool: self.clone(),
                _permit: permit,
                #[cfg(feature = "prometheus")]
                start_time: std::time::Instant::now(),
            }),
            Err(e) => {
                // Permit is dropped here, releasing the slot back to the semaphore.
                Err(e)
            }
        }
    }
}

impl<RM> Drop for Pool<RM>
where
    RM: DatabasePool,
{
    fn drop(&mut self) {
        // Close the semaphore so no new acquisitions can succeed.
        self.semaphore.close();

        // Drain all idle connections.
        if let Ok(mut resources) = self.queue.lock() {
            while let Some(resource) = resources.pop() {
                RM::drop(resource.1);
            }
        }
    }
}
