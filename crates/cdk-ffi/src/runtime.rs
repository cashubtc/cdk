//! Runtime management for FFI bindings
//!
//! This module provides a global multi-threaded Tokio runtime for handling
//! async operations in the FFI bindings. The runtime is lazily initialized
//! and shared across all FFI function calls.
//!
//! The multi-threaded runtime is configured to be compatible with mobile
//! devices (iOS and Android) with optimized thread pool sizing and resource
//! management while maintaining sync FFI interfaces through block_on calls.

use std::sync::Arc;

use once_cell::sync::Lazy;
use tokio::runtime::{Builder, Runtime};

/// Get optimal worker thread count for mobile devices
fn get_mobile_worker_threads() -> usize {
    // Get logical CPU count, but cap it for mobile battery efficiency
    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    
    // Limit to 4 threads max for mobile devices to balance performance and battery life
    // Minimum of 2 threads to ensure responsiveness
    std::cmp::max(2, std::cmp::min(cpu_count, 4))
}

/// Global multi-threaded Tokio runtime instance optimized for mobile devices
static RUNTIME: Lazy<Arc<Runtime>> = Lazy::new(|| {
    let worker_threads = get_mobile_worker_threads();
    
    Arc::new(
        Builder::new_multi_thread()
            .worker_threads(worker_threads)
            .max_blocking_threads(worker_threads * 2) // Limit blocking thread pool
            .thread_keep_alive(std::time::Duration::from_secs(10)) // Shorter keep-alive for battery
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime"),
    )
});

/// Execute a future within the global multi-threaded runtime context
///
/// This function blocks the current thread until the future completes,
/// utilizing a mobile-optimized thread pool for async operations. The runtime
/// dynamically adjusts to device capabilities while being compatible with
/// both iOS and Android platforms and maintaining synchronous FFI interfaces.
pub(crate) fn block_on<F: std::future::Future>(future: F) -> F::Output {
    RUNTIME.block_on(future)
}
