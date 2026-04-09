//! Lightweight runtime guard for FFI constructors.
//!
//! When called from an FFI language (Python, Swift, …) there may be no Tokio
//! runtime on the current thread.  `RuntimeGuard` detects this and, if needed,
//! lazily creates a multi-threaded runtime that lives as long as the guard.

use std::future::Future;

use tokio::runtime::{Handle, Runtime};

/// Holds either a borrowed handle to an existing Tokio runtime or an owned
/// runtime created on demand.  Dropping the guard shuts down the owned runtime
/// (if any), so it should be kept alive as long as work may be spawned on it.
pub(crate) struct RuntimeGuard {
    _runtime: Option<Runtime>,
    handle: Handle,
}

impl RuntimeGuard {
    /// Create a new guard.
    ///
    /// * If a Tokio runtime is already running on this thread the guard simply
    ///   captures its [`Handle`] (zero cost).
    /// * Otherwise a new multi-threaded runtime is created and owned by the
    ///   guard.
    pub fn new() -> Result<Self, String> {
        match Handle::try_current() {
            Ok(handle) => Ok(Self {
                _runtime: None,
                handle,
            }),
            Err(_) => {
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
    /// When the guard wraps an *existing* runtime this uses
    /// [`tokio::task::block_in_place`] so we don't starve the caller's worker
    /// threads.  When it owns its own runtime it calls [`Handle::block_on`]
    /// directly.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        if self._runtime.is_some() {
            // We own the runtime — safe to block_on directly.
            self.handle.block_on(future)
        } else {
            // Running inside an external runtime — yield the worker thread.
            tokio::task::block_in_place(|| self.handle.block_on(future))
        }
    }
}
