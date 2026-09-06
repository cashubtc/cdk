//! Lightweight runtime guard for FFI constructors.
//!
//! When called from an FFI language (Python, Swift, …) there may be no Tokio
//! runtime on the current thread.  `RuntimeGuard` detects this and, if needed,
//! lazily creates a multi-threaded runtime that lives as long as the guard.

use std::future::Future;

use tokio::runtime::{Handle, Runtime, RuntimeFlavor};

/// Holds either a borrowed handle to an existing Tokio runtime or an owned
/// runtime created on demand.  Dropping the guard shuts down the owned runtime
/// (if any), so it should be kept alive as long as work may be spawned on it.
pub(crate) struct RuntimeGuard {
    _runtime: Option<Runtime>,
    handle: Handle,
}

impl Drop for RuntimeGuard {
    fn drop(&mut self) {
        if let Some(runtime) = self._runtime.take() {
            // Wallet handles are often released by a UniFFI future completion
            // running on Tokio. Runtime's default Drop blocks and panics in
            // that context, so use a non-blocking shutdown there. Outside a
            // runtime, retain Tokio's normal graceful shutdown behavior.
            if Handle::try_current().is_ok() {
                runtime.shutdown_background();
            } else {
                drop(runtime);
            }
        }
    }
}

impl RuntimeGuard {
    /// Create a new guard.
    ///
    /// Reuse an existing multi-threaded runtime when possible. A synchronous
    /// constructor cannot drive the caller's single-threaded runtime while
    /// blocking it, so that case needs an independent runtime as well.
    pub fn new() -> Result<Self, String> {
        match Handle::try_current() {
            Ok(handle) if handle.runtime_flavor() == RuntimeFlavor::MultiThread => Ok(Self {
                _runtime: None,
                handle,
            }),
            _ => {
                let rt = Runtime::new().map_err(|e| format!("Failed to create runtime: {e}"))?;
                let handle = rt.handle().clone();
                Ok(Self {
                    _runtime: Some(rt),
                    handle,
                })
            }
        }
    }

    /// Run a future to completion on the runtime.
    ///
    /// Yield a multi-threaded caller's worker when necessary. Single-threaded
    /// callers use a scoped thread to avoid nesting `block_on` or invoking
    /// `block_in_place`, neither of which Tokio permits in that context.
    pub fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future + Send,
        F::Output: Send,
    {
        match Handle::try_current() {
            Err(_) => self.handle.block_on(future),
            Ok(handle) if handle.runtime_flavor() == RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| self.handle.block_on(future))
            }
            Ok(_) => std::thread::scope(|scope| {
                match scope.spawn(|| self.handle.block_on(future)).join() {
                    Ok(result) => result,
                    Err(panic) => std::panic::resume_unwind(panic),
                }
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn synchronous_construction_inside_a_single_thread_runtime() {
        let guard = RuntimeGuard::new().expect("runtime should start");
        assert!(guard._runtime.is_some());
        assert_eq!(
            guard.block_on(async {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                42
            }),
            42
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn owned_runtime_can_be_released_from_async_context() {
        let guard = std::thread::spawn(|| RuntimeGuard::new().expect("runtime should start"))
            .join()
            .expect("runtime constructor thread should not panic");
        assert!(guard._runtime.is_some());

        drop(guard);
    }
}
