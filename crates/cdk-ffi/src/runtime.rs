//! Runtime management for FFI bindings
//!
//! This module provides a global Tokio runtime for handling async operations
//! in the FFI bindings. The runtime is lazily initialized and shared across
//! all FFI function calls.
//!
//! # Example Usage (from client code)
//!
//! ```python
//! import cdk_ffi
//!
//! # Initialize the runtime once at application startup
//! cdk_ffi.init_runtime()
//!
//! # Now you can use async FFI functions
//! wallet = await cdk_ffi.Wallet.new(...)
//! ```

use std::sync::Arc;

use once_cell::sync::Lazy;
use tokio::runtime::{Builder, Runtime};

use crate::error::FfiError;

/// Global Tokio runtime instance
static RUNTIME: Lazy<Arc<Runtime>> = Lazy::new(|| {
    Arc::new(
        Builder::new_current_thread()
            .max_blocking_threads(8)
            .thread_name("cdk-ffi-worker")
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime"),
    )
});

/// Get the global runtime instance (for internal use)
///
/// This function is provided for future use cases where we might need
/// to access the runtime directly from within the FFI code.
#[allow(dead_code)]
pub(crate) fn get_runtime() -> Arc<Runtime> {
    RUNTIME.clone()
}

/// Execute a future within the global runtime context
///
/// This function ensures that async operations run within the proper
/// Tokio runtime context, which is especially important for operations
/// that create HTTP clients or other components that require runtime context.
#[allow(dead_code)]
pub(crate) fn block_on<F: std::future::Future>(future: F) -> F::Output {
    RUNTIME.block_on(future)
}

/// Execute a future directly in current-thread runtime
///
/// Since we're using current-thread runtime, we don't need to spawn tasks
/// with Send bounds. This function executes futures directly.
/// For current-thread runtime, this is more efficient than spawning.
#[allow(dead_code)]
pub(crate) async fn execute_local<F>(future: F) -> F::Output
where
    F: std::future::Future,
{
    future.await
}

/// Get the runtime handle for use in async contexts
///
/// This provides access to the runtime handle for spawning tasks
/// and other operations that require runtime access.
#[allow(dead_code)]
pub(crate) fn handle() -> tokio::runtime::Handle {
    RUNTIME.handle().clone()
}
