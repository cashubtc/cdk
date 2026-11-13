//! Spawn and sleep, each with a native and a wasm implementation, so a caller
//! gets the portable one without having to know which primitive works where.

use std::future::Future;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;
use std::time::Duration;

use tokio::task::JoinHandle;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::closure::Closure;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::wasm_bindgen;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsValue;

#[cfg(not(target_arch = "wasm32"))]
static GLOBAL_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// Imported unqualified so it resolves against the global scope, which a worker
/// and Node share with the browser while `web_sys::Window` does not.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = "setTimeout")]
    fn set_timeout(callback: &JsValue, millis: u32);
}

/// Spawns a new asynchronous task returning nothing
///
/// # Panics
///
/// Panics if the global Tokio runtime cannot be created when no runtime exists on the current thread.
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

/// Sleeps for the given duration.
#[cfg(not(target_arch = "wasm32"))]
pub async fn sleep(duration: Duration) {
    tokio::time::sleep(duration).await;
}

/// Sleeps for the given duration.
///
/// `tokio::time` has no timer driver under wasm, so `setTimeout` stands in. The
/// callback is bridged through a channel so the awaited future stays `Send` for
/// callers such as `SupervisedStream::supervise`, which box a `Send` future.
///
/// Dropping the future does not cancel the timer, where the native arm does:
/// the callback still fires and sends into a receiver that is already gone, so
/// both channel results are ignored.
///
/// The handle `Closure::once_into_js` returns is scoped so it drops before the
/// await rather than becoming part of the future: `JsValue` is `Send` only on
/// recent wasm-bindgen, and never under `+atomics`.
#[cfg(target_arch = "wasm32")]
pub async fn sleep(duration: Duration) {
    let millis = timeout_millis(duration);
    let (tx, rx) = tokio::sync::oneshot::channel();

    {
        let callback = Closure::once_into_js(move || {
            let _ = tx.send(());
        });
        set_timeout(&callback, millis);
    }

    let _ = rx.await;
}

/// Clamps to `i32::MAX` because `setTimeout` takes a WebIDL `long`: a larger
/// delay wraps to a negative number, which the spec coerces to zero, so the
/// sleep would return at once instead of waiting.
#[cfg(any(target_arch = "wasm32", test))]
fn timeout_millis(duration: Duration) -> u32 {
    duration.as_millis().min(i32::MAX as u128) as u32
}

#[cfg(test)]
mod tests {
    use super::{timeout_millis, Duration};

    #[test]
    fn clamps_at_the_settimeout_long_boundary() {
        let max = i32::MAX as u32;

        assert_eq!(timeout_millis(Duration::ZERO), 0);
        assert_eq!(timeout_millis(Duration::from_millis(1)), 1);
        assert_eq!(
            timeout_millis(Duration::from_millis(max as u64 - 1)),
            max - 1
        );
        assert_eq!(timeout_millis(Duration::from_millis(max as u64)), max);
        assert_eq!(timeout_millis(Duration::from_millis(max as u64 + 1)), max);
        assert_eq!(timeout_millis(Duration::from_millis(u32::MAX as u64)), max);
        assert_eq!(timeout_millis(Duration::MAX), max);
    }
}
