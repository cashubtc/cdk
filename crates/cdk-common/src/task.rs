//! Thin wrapper for spawn and spawn_local for native and wasm.

use std::future::Future;
use std::sync::OnceLock;

use tokio::task::JoinHandle;

#[cfg(not(target_arch = "wasm32"))]
static GLOBAL_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// Spawns a new asynchronous task returning nothing
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(future)
    } else {
        // No runtime on this thread (FFI/regular sync context):
        // use (or lazily create) a global runtime and spawn on it.
        GLOBAL_RUNTIME
            .get_or_init(|| {
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build global Tokio runtime")
            })
            .spawn(future)
    }
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
