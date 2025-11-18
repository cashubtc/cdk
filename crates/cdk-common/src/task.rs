//! Thin wrapper for spawn and spawn_local for native and wasm.

use std::future::Future;

use tokio::task::JoinHandle;

/// Spawns a new asynchronous task returning nothing
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(future)
}

/// Spawns a new asynchronous task returning nothing
#[cfg(target_arch = "wasm32")]
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
    F::Output: 'static,
{
    tokio::task::spawn_local(future)
}
