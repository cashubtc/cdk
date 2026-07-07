//! Lightweight runtime guard for FFI constructors.
//!
//! When called from an FFI language (Python, Swift, …) there may be no Tokio
//! runtime on the current thread.  `RuntimeGuard` detects this and, if needed,
//! falls back to a process-wide shared runtime.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::{Handle, Runtime};

/// Process-wide runtime for FFI calls made outside any Tokio context.
///
/// This must be a single long-lived runtime (never per-call): work started
/// during a constructor may spawn background tasks that need to outlive it —
/// e.g. the arti Tor client spawns its circuit/channel reactors on the runtime
/// that bootstraps it, and they die if that runtime is dropped.
static SHARED_RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn shared_runtime() -> Result<&'static Runtime, String> {
    if let Some(rt) = SHARED_RUNTIME.get() {
        return Ok(rt);
    }
    // Racing threads may each build a runtime here, but get_or_init only
    // publishes one; the losers are dropped before any work runs on them.
    let rt = Runtime::new().map_err(|e| format!("Failed to create runtime: {e}"))?;
    Ok(SHARED_RUNTIME.get_or_init(|| rt))
}

/// Run a future on the process-wide shared runtime and await its result.
///
/// UniFFI's `async_runtime = "tokio"` support enters a Tokio context but still
/// lets the foreign bindings manually poll Rust futures. That is enough for
/// many small async operations, but runtime-heavy clients such as arti expect a
/// real Tokio task/runtime to own polling and spawned reactor work.
pub(crate) async fn run_on_shared<F, T>(future: F) -> Result<T, String>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let handle = shared_runtime()?.handle().clone();
    handle
        .spawn(future)
        .await
        .map_err(|e| format!("Shared runtime task failed: {e}"))
}

/// Holds a handle either to the caller's existing Tokio runtime or to the
/// process-wide shared runtime.
pub(crate) struct RuntimeGuard {
    borrowed: bool,
    handle: Handle,
}

impl RuntimeGuard {
    /// Create a new guard.
    ///
    /// * If a Tokio runtime is already running on this thread the guard simply
    ///   captures its [`Handle`] (zero cost).
    /// * Otherwise it uses the process-wide shared runtime, creating it on
    ///   first use.
    pub fn new() -> Result<Self, String> {
        match Handle::try_current() {
            Ok(handle) => Ok(Self {
                borrowed: true,
                handle,
            }),
            Err(_) => {
                let handle = shared_runtime()?.handle().clone();
                Ok(Self {
                    borrowed: false,
                    handle,
                })
            }
        }
    }

    /// Run a future to completion on the runtime.
    ///
    /// When the guard wraps the caller's *existing* runtime this uses
    /// [`tokio::task::block_in_place`] so we don't starve the caller's worker
    /// threads.  When it uses the shared runtime the calling thread is a plain
    /// FFI thread, so [`Handle::block_on`] is safe.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        if self.borrowed {
            // Running inside an external runtime — yield the worker thread.
            tokio::task::block_in_place(|| self.handle.block_on(future))
        } else {
            self.handle.block_on(future)
        }
    }
}
