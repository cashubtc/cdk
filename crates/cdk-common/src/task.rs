//! Spawn and sleep, each with a native and a wasm implementation, so a caller
//! gets the portable one without having to know which primitive works where.

use std::future::Future;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
use futures::future::{select, Either};
#[cfg(target_arch = "wasm32")]
use futures::pin_mut;
use tokio::task::JoinHandle;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::closure::Closure;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::wasm_bindgen;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsValue;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::spawn_local;

#[cfg(not(target_arch = "wasm32"))]
static GLOBAL_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// Imported unqualified so they resolve against the global scope, which a worker
/// and Node share with the browser while `web_sys::Window` does not.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = "setTimeout")]
    fn set_timeout(callback: &JsValue, millis: u32) -> JsValue;

    #[wasm_bindgen(js_name = "clearTimeout")]
    fn clear_timeout(handle: &JsValue);
}

/// Clears the timer it owns, so a dropped sleep stops holding a Node process
/// open and stops retaining its callback in a browser.
///
/// The handle stays a `JsValue` because Node returns a `Timeout` object where a
/// browser returns a number, and an `i32` import would coerce that object to 0.
#[cfg(target_arch = "wasm32")]
struct Timer(JsValue);

#[cfg(target_arch = "wasm32")]
impl Drop for Timer {
    fn drop(&mut self) {
        clear_timeout(&self.0);
    }
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
/// The timer and its `Closure` live in a local task because neither is `Send`.
/// That task also watches the channel, so dropping this future clears the timer
/// and frees the callback instead of leaving both registered until the original
/// deadline. Either channel result is ignored: the receiver is already gone on
/// the cancellation path, and the timer no longer matters once it has fired.
#[cfg(target_arch = "wasm32")]
pub async fn sleep(duration: Duration) {
    let millis = timeout_millis(duration);
    let (mut tx, rx) = tokio::sync::oneshot::channel();

    spawn_local(async move {
        let (fired_tx, fired_rx) = tokio::sync::oneshot::channel();
        let callback = Closure::once(move || {
            let _ = fired_tx.send(());
        });
        let _timer = Timer(set_timeout(callback.as_ref(), millis));

        let fired = {
            let cancelled = tx.closed();
            pin_mut!(cancelled);
            matches!(select(fired_rx, cancelled).await, Either::Left(_))
        };

        if fired {
            let _ = tx.send(());
        }
    });

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

/// The timer lifetime this module promises, which only a JavaScript runtime can
/// show.
#[cfg(test)]
#[cfg(target_arch = "wasm32")]
mod wasm_tests {
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::{sleep, wasm_bindgen, Duration};

    /// Counts the timers the global scope still holds, keyed by handle so clearing
    /// an already fired timer does not count twice, and hands out a `tick` on the
    /// unpatched timer so waiting never disturbs that count.
    #[wasm_bindgen(inline_js = r#"
        const live = new Set();
        const realSetTimeout = globalThis.setTimeout;
        const realClearTimeout = globalThis.clearTimeout;

        export function install_timer_probe() {
            live.clear();
            globalThis.setTimeout = (callback, millis) => {
                const handle = realSetTimeout(() => {
                    live.delete(handle);
                    callback();
                }, millis);
                live.add(handle);
                return handle;
            };
            globalThis.clearTimeout = (handle) => {
                live.delete(handle);
                realClearTimeout(handle);
            };
        }

        export function pending_timers() {
            return live.size;
        }

        export function tick() {
            return new Promise((resolve) => realSetTimeout(resolve, 0));
        }
    "#)]
    extern "C" {
        fn install_timer_probe();
        fn pending_timers() -> u32;
        async fn tick();
    }

    fn assert_send<F: Send>(_: &F) {}

    #[wasm_bindgen_test]
    async fn dropping_a_sleep_clears_its_timer() {
        install_timer_probe();

        let mut sleeping = Box::pin(sleep(Duration::from_secs(60)));
        assert!(futures::poll!(&mut sleeping).is_pending());
        tick().await;
        assert_eq!(pending_timers(), 1);

        drop(sleeping);
        tick().await;

        assert_eq!(pending_timers(), 0);
    }

    #[wasm_bindgen_test]
    async fn a_fired_sleep_leaves_no_timer_behind() {
        install_timer_probe();

        sleep(Duration::from_millis(20)).await;
        tick().await;

        assert_eq!(pending_timers(), 0);
    }

    #[wasm_bindgen_test]
    fn the_sleep_future_is_send() {
        assert_send(&sleep(Duration::from_millis(1)));
    }
}
