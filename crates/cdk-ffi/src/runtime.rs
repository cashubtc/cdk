//! Runtime management for FFI bindings
//!
//! This module provides runtime configuration for async FFI operations.
//! UniFFI handles the async runtime automatically when methods are marked as async.
//!
//! The runtime is optimized for mobile devices (iOS and Android) with
//! appropriate thread pool sizing and resource management.

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
/// This is used by UniFFI for async operations
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

/// Initialize the runtime (called by UniFFI when needed)
pub fn init_runtime() {
    // Force runtime initialization
    let _ = &*RUNTIME;
}
