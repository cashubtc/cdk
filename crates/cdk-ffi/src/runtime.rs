//! Runtime management for FFI bindings
//!
//! This module provides runtime configuration for async FFI operations.
//! UniFFI handles the async runtime automatically when methods are marked as async.
//!
//! The runtime is optimized for mobile devices (iOS and Android) with
//! appropriate thread pool sizing and resource management.

use once_cell::sync::Lazy;
use tokio::runtime::{Builder, Handle, Runtime};
use std::sync::atomic::{AtomicBool, Ordering};

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
static RUNTIME: Lazy<Runtime> = Lazy::new(|| {
    let worker_threads = get_mobile_worker_threads();

    Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .max_blocking_threads(worker_threads * 2) // Limit blocking thread pool
        .thread_keep_alive(std::time::Duration::from_secs(10)) // Shorter keep-alive for battery
        .enable_all()
        .build()
        .expect("Failed to create Tokio runtime")
});

/// Track if runtime has been initialized
static RUNTIME_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Initialize the runtime and ensure it's available for UniFFI async operations
/// This must be called before any async FFI operations
pub fn init_runtime() {
    if RUNTIME_INITIALIZED.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        // Force runtime initialization by accessing the lazy static
        let _ = &*RUNTIME;
        
        // Spawn a task to keep the runtime active
        RUNTIME.spawn(async {
            // This task keeps the runtime alive
            tokio::task::yield_now().await;
        });
    }
}

/// Get the runtime handle for async operations
pub fn runtime_handle() -> Handle {
    RUNTIME.handle().clone()
}

/// Execute a future on the global runtime
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    RUNTIME.block_on(future)
}

/// Custom async runtime configuration for UniFFI
/// This ensures UniFFI uses our optimized runtime
pub mod uniffi_runtime {
    use super::*;
    
    /// Spawn a future on the global runtime
    pub fn spawn_detached<F>(future: F) 
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        RUNTIME.spawn(future);
    }
    
    /// Execute a future and block on its completion
    pub fn run<F>(future: F) -> F::Output
    where
        F: std::future::Future + Send,
        F::Output: Send,
    {
        RUNTIME.block_on(future)
    }
}
