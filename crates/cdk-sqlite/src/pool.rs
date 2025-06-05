//! Very simple connection pool, to avoid an external dependency on r2d2 and other crates. If this
//! endup work it can be re-used in other parts of the project and may be promoted to its own
//! generic crate

use std::fmt::Debug;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

#[derive(thiserror::Error, Debug)]
pub enum Error<E> {
    /// Mutex Poison Error
    #[error("Internal: PoisonError")]
    PoisonError,

    /// Internal database error
    #[error(transparent)]
    ResourceError(#[from] E),
}

/// Trait to manage resources
pub trait ResourceManager: Debug {
    type Resource: Debug;

    type Config: Debug;

    type Error: Debug;

    /// Creates a new resource with a given config
    fn new_resource(config: &Self::Config) -> Result<Self::Resource, Error<Self::Error>>;

    /// The object is dropped
    fn drop(_resource: Self::Resource) {}
}

/// Generic connection pool of resources R
#[derive(Debug)]
pub struct Pool<RM>
where
    RM: ResourceManager,
{
    config: RM::Config,
    queue: Mutex<Vec<RM::Resource>>,
    in_use: AtomicUsize,
    max_size: usize,
    waiter: Condvar,
}

pub struct WrappedResource<RM>
where
    RM: ResourceManager,
{
    resource: Option<RM::Resource>,
    pool: Arc<Pool<RM>>,
}

impl<RM> Drop for WrappedResource<RM>
where
    RM: ResourceManager,
{
    fn drop(&mut self) {
        if let Some(resource) = self.resource.take() {
            let mut active_resource = self.pool.queue.lock().expect("active_resource");
            active_resource.push(resource);
            self.pool.in_use.fetch_sub(1, Ordering::AcqRel);

            // Notify a waiting thread
            self.pool.waiter.notify_one();
        }
    }
}

impl<RM> Deref for WrappedResource<RM>
where
    RM: ResourceManager,
{
    type Target = RM::Resource;

    fn deref(&self) -> &Self::Target {
        self.resource.as_ref().expect("resource already dropped")
    }
}

impl<RM> DerefMut for WrappedResource<RM>
where
    RM: ResourceManager,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.resource.as_mut().expect("resource already dropped")
    }
}

impl<RM> Pool<RM>
where
    RM: ResourceManager,
{
    /// Creates a new pool
    pub fn new(config: RM::Config, max_size: usize) -> Arc<Self> {
        Arc::new(Self {
            config,
            queue: Default::default(),
            in_use: Default::default(),
            waiter: Default::default(),
            max_size,
        })
    }

    pub fn get(self: &Arc<Self>) -> Result<WrappedResource<RM>, Error<RM::Error>> {
        let mut resources = self.queue.lock().map_err(|_| Error::PoisonError)?;

        loop {
            if let Some(resource) = resources.pop() {
                drop(resources);
                self.in_use.fetch_add(1, Ordering::AcqRel);

                return Ok(WrappedResource {
                    resource: Some(resource),
                    pool: self.clone(),
                });
            }

            if self.in_use.load(Ordering::Relaxed) < self.max_size {
                drop(resources);
                self.in_use.fetch_add(1, Ordering::AcqRel);

                return Ok(WrappedResource {
                    resource: Some(RM::new_resource(&self.config)?),
                    pool: self.clone(),
                });
            }

            resources = self
                .waiter
                .wait(resources)
                .map_err(|_| Error::PoisonError)?;
        }
    }
}

impl<RM> Drop for Pool<RM>
where
    RM: ResourceManager,
{
    fn drop(&mut self) {
        if let Ok(mut resources) = self.queue.lock() {
            loop {
                while let Some(resource) = resources.pop() {
                    RM::drop(resource);
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
