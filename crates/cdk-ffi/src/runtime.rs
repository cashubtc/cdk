//! Runtime management for FFI bindings
//!
//! This module provides a global current-thread Tokio runtime for handling
//! async operations in the FFI bindings. The runtime is lazily initialized
//! and shared across all FFI function calls.
//!
//! The current-thread runtime is used to ensure compatibility with iOS
//! threading restrictions while maintaining sync FFI interfaces through
//! block_on calls.

use std::sync::Arc;

use once_cell::sync::Lazy;
use tokio::runtime::{Builder, Runtime};

/// Global current-thread Tokio runtime instance
static RUNTIME: Lazy<Arc<Runtime>> = Lazy::new(|| {
    Arc::new(
        Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime"),
    )
});

/// Execute a future within the global current-thread runtime context
///
/// This function blocks the current thread until the future completes,
/// running all async operations on the current thread. This approach
/// is iOS-compatible while maintaining synchronous FFI interfaces.
pub(crate) fn block_on<F: std::future::Future>(future: F) -> F::Output {
    RUNTIME.block_on(future)
}
