//! Just-In-Time lazy initialization for async operations
//!
//! This module provides `JitCell`, a lazy-initialized cell that executes an async
//! initialization function on first access. Subsequent accesses return the cached result.
//! This enables parallel initialization of independent components while respecting
//! dependency chains.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{Mutex, OnceCell};

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A lazy-initialized cell that executes an async initialization function on first access.
///
/// The initialization function is executed only once, even if multiple tasks attempt to
/// access the value concurrently. Subsequent accesses return a clone of the cached result.
///
/// # Example
///
/// ```
/// use anyhow::Result;
/// use cdk_integration_tests::jit::JitCell;
///
/// async fn example() -> Result<()> {
///     let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
///
///     let cell = JitCell::new_async({
///         let counter = counter.clone();
///         move || async move {
///             counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
///             Ok(42)
///         }
///     });
///
///     // Clone the cell to simulate multiple references
///     let cell2 = cell.clone();
///
///     // Both get() calls will return 42, but initialization runs only once
///     let val1 = cell.get().await?;
///     let val2 = cell2.get().await?;
///
///     assert_eq!(val1, 42);
///     assert_eq!(val2, 42);
///     assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
///
///     Ok(())
/// }
/// ```
#[derive(Clone)]
pub struct JitCell<T> {
    /// The cached result of initialization
    cell: Arc<OnceCell<T>>,
    /// The initialization function, stored as an Option so it can be taken on first use
    init_fn: Arc<Mutex<Option<BoxFuture<'static, Result<T>>>>>,
}

impl<T> JitCell<T> {
    /// Create a new `JitCell` with an async initialization function.
    ///
    /// The initialization function will not be called until the first call to `get()`.
    ///
    /// # Type Parameters
    ///
    /// * `F` - A function that returns a Future
    /// * `Fut` - The Future type returned by `F`
    ///
    /// # Arguments
    ///
    /// * `init_fn` - An async function that produces a `Result<T>`
    ///
    /// # Example
    ///
    /// ```
    /// use anyhow::Result;
    /// use cdk_integration_tests::jit::JitCell;
    ///
    /// async fn create_expensive_resource() -> Result<String> {
    ///     // Simulate expensive initialization
    ///     tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    ///     Ok("resource".to_string())
    /// }
    ///
    /// let cell = JitCell::new_async(|| async { create_expensive_resource().await });
    /// ```
    pub fn new_async<F, Fut>(init_fn: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        Self {
            cell: Arc::new(OnceCell::new()),
            init_fn: Arc::new(Mutex::new(Some(Box::pin(init_fn())))),
        }
    }

    /// Get the value, initializing it if this is the first access.
    ///
    /// If initialization has not yet occurred, this will execute the initialization
    /// function provided to `new_async()`. If multiple tasks call `get()` concurrently,
    /// only one will execute the initialization function, and the others will wait for
    /// it to complete.
    ///
    /// If initialization fails, the error is returned and initialization will be
    /// retried on the next call to `get()`.
    ///
    /// # Returns
    ///
    /// * `Ok(T)` - A clone of the initialized value
    /// * `Err(anyhow::Error)` - If initialization failed
    ///
    /// # Example
    ///
    /// ```
    /// use anyhow::Result;
    /// use cdk_integration_tests::jit::JitCell;
    ///
    /// async fn example() -> Result<()> {
    ///     let cell = JitCell::new_async(|| async { Ok(vec![1, 2, 3]) });
    ///
    ///     let value = cell.get().await?;
    ///     assert_eq!(value, vec![1, 2, 3]);
    ///
    ///     Ok(())
    /// }
    /// ```
    pub async fn get(&self) -> Result<T>
    where
        T: Clone,
    {
        // Fast path: if already initialized, return immediately
        if let Some(value) = self.cell.get() {
            tracing::debug!(
                "JitCell::get() - returning cached value (type: {})",
                std::any::type_name::<T>()
            );
            return Ok(value.clone());
        }

        // Slow path: need to initialize
        tracing::debug!(
            "JitCell::get() - initializing (type: {})",
            std::any::type_name::<T>()
        );

        // OnceCell::get_or_init ensures only one initialization runs
        let value = self
            .cell
            .get_or_try_init(|| async {
                // Take the initialization function (this can only happen once)
                let mut init_guard = self.init_fn.lock().await;
                if let Some(fut) = init_guard.take() {
                    // Run the initialization
                    tracing::debug!(
                        "JitCell::get() - running initialization future (type: {})",
                        std::any::type_name::<T>()
                    );
                    let result = fut.await;
                    match &result {
                        Ok(_) => tracing::debug!(
                            "JitCell::get() - initialization successful, storing in cell (type: {})",
                            std::any::type_name::<T>()
                        ),
                        Err(e) => tracing::error!(
                            error = %e,
                            "JitCell::get() - initialization failed (type: {})",
                            std::any::type_name::<T>()
                        ),
                    }
                    result
                } else {
                    // This shouldn't happen with OnceCell, but handle it gracefully
                    Err(anyhow::anyhow!(
                        "JitCell initialization function has already been consumed"
                    ))
                }
            })
            .await?;

        tracing::debug!(
            "JitCell::get() - returning initialized value (type: {})",
            std::any::type_name::<T>()
        );
        Ok(value.clone())
    }

    /// Check if the cell has been initialized without triggering initialization.
    ///
    /// # Returns
    ///
    /// * `true` if the cell has been initialized
    /// * `false` if initialization has not yet occurred
    pub fn is_initialized(&self) -> bool {
        self.cell.get().is_some()
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for JitCell<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JitCell")
            .field("initialized", &self.is_initialized())
            .field(
                "value",
                &self
                    .cell
                    .get()
                    .map(|v| format!("{:?}", v))
                    .unwrap_or_else(|| "<not initialized>".to_string()),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc as StdArc;

    use tokio::time::{sleep, Duration};

    use super::*;

    #[tokio::test]
    async fn test_basic_initialization() {
        let cell = JitCell::new_async(|| async { Ok(42) });

        assert!(!cell.is_initialized());

        let value = cell.get().await.unwrap();
        assert_eq!(value, 42);

        assert!(cell.is_initialized());
    }

    #[tokio::test]
    async fn test_initialization_runs_once() {
        let counter = StdArc::new(AtomicU32::new(0));

        let cell = JitCell::new_async({
            let counter = counter.clone();
            move || async move {
                counter.fetch_add(1, Ordering::SeqCst);
                sleep(Duration::from_millis(50)).await;
                Ok(42)
            }
        });

        // Call get() multiple times
        let v1 = cell.get().await.unwrap();
        let v2 = cell.get().await.unwrap();
        let v3 = cell.get().await.unwrap();

        assert_eq!(v1, 42);
        assert_eq!(v2, 42);
        assert_eq!(v3, 42);

        // Initialization should have run exactly once
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_concurrent_access() {
        let counter = StdArc::new(AtomicU32::new(0));

        let cell = JitCell::new_async({
            let counter = counter.clone();
            move || async move {
                counter.fetch_add(1, Ordering::SeqCst);
                sleep(Duration::from_millis(100)).await;
                Ok("initialized".to_string())
            }
        });

        // Spawn multiple concurrent tasks
        let cell2 = cell.clone();
        let cell3 = cell.clone();
        let cell4 = cell.clone();

        let (r1, r2, r3, r4) = tokio::join!(
            tokio::spawn(async move { cell.get().await }),
            tokio::spawn(async move { cell2.get().await }),
            tokio::spawn(async move { cell3.get().await }),
            tokio::spawn(async move { cell4.get().await }),
        );

        assert_eq!(r1.unwrap().unwrap(), "initialized");
        assert_eq!(r2.unwrap().unwrap(), "initialized");
        assert_eq!(r3.unwrap().unwrap(), "initialized");
        assert_eq!(r4.unwrap().unwrap(), "initialized");

        // Initialization should have run exactly once despite concurrent access
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_clone_shares_state() {
        let counter = StdArc::new(AtomicU32::new(0));

        let cell1 = JitCell::new_async({
            let counter = counter.clone();
            move || async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(100)
            }
        });

        let cell2 = cell1.clone();

        // Initialize via cell1
        assert_eq!(cell1.get().await.unwrap(), 100);
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // cell2 should see the same initialized value
        assert_eq!(cell2.get().await.unwrap(), 100);
        assert_eq!(counter.load(Ordering::SeqCst), 1); // Still 1, no re-init

        // Both should report as initialized
        assert!(cell1.is_initialized());
        assert!(cell2.is_initialized());
    }

    #[tokio::test]
    async fn test_error_propagation() {
        let cell: JitCell<i32> =
            JitCell::new_async(|| async { Err(anyhow::anyhow!("initialization failed")) });

        let result = cell.get().await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "initialization failed");
    }

    #[tokio::test]
    async fn test_dependency_chain() {
        // Simulate a dependency chain: cell2 depends on cell1
        let cell1 = JitCell::new_async(|| async {
            sleep(Duration::from_millis(50)).await;
            Ok(10)
        });

        let cell2 = JitCell::new_async({
            let cell1 = cell1.clone();
            move || async move {
                let val1 = cell1.get().await?;
                Ok(val1 * 2)
            }
        });

        let cell3 = JitCell::new_async({
            let cell1 = cell1.clone();
            let cell2 = cell2.clone();
            move || async move {
                let val1 = cell1.get().await?;
                let val2 = cell2.get().await?;
                Ok(val1 + val2)
            }
        });

        // Getting cell3 should trigger the whole chain
        let result = cell3.get().await.unwrap();
        assert_eq!(result, 30); // 10 + (10 * 2)

        // All should be initialized
        assert!(cell1.is_initialized());
        assert!(cell2.is_initialized());
        assert!(cell3.is_initialized());
    }

    #[tokio::test]
    async fn test_parallel_independent_cells() {
        let counter1 = StdArc::new(AtomicU32::new(0));
        let counter2 = StdArc::new(AtomicU32::new(0));

        let cell1 = JitCell::new_async({
            let counter = counter1.clone();
            move || async move {
                counter.fetch_add(1, Ordering::SeqCst);
                sleep(Duration::from_millis(100)).await;
                Ok("cell1".to_string())
            }
        });

        let cell2 = JitCell::new_async({
            let counter = counter2.clone();
            move || async move {
                counter.fetch_add(1, Ordering::SeqCst);
                sleep(Duration::from_millis(100)).await;
                Ok("cell2".to_string())
            }
        });

        // Initialize both in parallel
        let start = std::time::Instant::now();
        let (r1, r2) = tokio::join!(cell1.get(), cell2.get());
        let elapsed = start.elapsed();

        assert_eq!(r1.unwrap(), "cell1");
        assert_eq!(r2.unwrap(), "cell2");

        // Should complete in ~100ms (parallel), not ~200ms (sequential)
        assert!(
            elapsed < Duration::from_millis(150),
            "Parallel initialization took too long: {:?}",
            elapsed
        );

        assert_eq!(counter1.load(Ordering::SeqCst), 1);
        assert_eq!(counter2.load(Ordering::SeqCst), 1);
    }
}
