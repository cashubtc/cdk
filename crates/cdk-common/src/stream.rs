//! Supervision for long-lived streaming subscriptions.
//!
//! [`supervise_stream`] owns the reconnect, backoff, and shutdown loop that
//! every consumer of a long-lived stream (a gRPC server-stream, a payment
//! backend's event stream) would otherwise hand-roll. It is transport-agnostic:
//! the caller supplies a `connect` closure that opens a fresh stream and a
//! handler for each item.

use std::fmt;

use futures::{pin_mut, Stream, StreamExt};

/// Capped exponential backoff for [`supervise_stream`] reconnect attempts.
///
/// The supervisor always waits at least `initial` before any reconnect,
/// including right after a healthy stream cycles: this floor is deliberate, so
/// a server that accepts a connection, delivers a message, and immediately
/// drops cannot be reconnected to in a hot loop. Callers that want faster
/// reconnects after a healthy drop should lower `initial` rather than remove the
/// floor.
///
/// `initial` must be non-zero (a zero `initial` never grows, `0 * 2 == 0`, so
/// the backoff would degenerate into a busy reconnect loop) and `max` must be at
/// least `initial` (a smaller cap would clamp the delay below the floor a
/// delivered message resets to). [`supervise_stream`] also clamps both at
/// runtime, so a violated invariant is corrected rather than acted on, but
/// passing sane values keeps the delays predictable.
#[derive(Debug, Clone, Copy)]
pub struct BackoffPolicy {
    /// Delay before the first reconnect, and the floor the delay resets to
    /// once a message has been delivered. Must be non-zero.
    pub initial: std::time::Duration,
    /// Upper bound the delay is capped at while backing off. Must be at least
    /// `initial`.
    pub max: std::time::Duration,
}

/// Keep a streaming subscription alive across reconnects.
///
/// `connect` opens a fresh stream of items. Every item it yields is handed to
/// `on_message`, which is awaited to completion (item processing is sequential,
/// and a slow handler holds the loop, so a handler that must not block the read
/// should offload its work). When the stream ends (a clean close or a per-item
/// error) or a connect attempt fails, the supervisor waits out a capped
/// exponential backoff and reconnects. The backoff resets once a message is
/// delivered, so a healthy connection that later drops reconnects quickly while
/// an endpoint that keeps failing is not hammered.
///
/// `shutdown` stops the supervisor promptly whenever it is waiting: to connect,
/// for the next item, or during a backoff. It does not interrupt an in-flight
/// `on_message`.
///
/// This owns only the reconnect, backoff, and shutdown machinery. Opening the
/// stream, decoding items, and acting on them stay with the caller. Any
/// teardown that must run when the subscription stops (cancelling a token, say)
/// belongs on the line after this call returns.
pub async fn supervise_stream<T, E1, E2, C, CFut, St, F, Fut, S>(
    policy: BackoffPolicy,
    shutdown: S,
    mut connect: C,
    mut on_message: F,
) where
    C: FnMut() -> CFut,
    CFut: std::future::Future<Output = Result<St, E1>>,
    St: Stream<Item = Result<T, E2>>,
    E1: fmt::Display,
    E2: fmt::Display,
    F: FnMut(T) -> Fut,
    Fut: std::future::Future<Output = ()>,
    S: std::future::Future<Output = ()>,
{
    // Correct a degenerate policy consistently in every build rather than
    // trusting the caller: a zero `initial` would make the backoff busy-loop
    // (`0 * 2 == 0`), and a `max` below `initial` would clamp the delay under
    // the floor a delivered message resets to. Clamp once, then read only these
    // locals for the rest of the loop.
    let initial = policy.initial.max(std::time::Duration::from_millis(1));
    let max = policy.max.max(initial);

    tokio::pin!(shutdown);
    let mut backoff = initial;

    loop {
        let connect_result = tokio::select! {
            biased;
            _ = &mut shutdown => return,
            result = connect() => result,
        };

        match connect_result {
            Ok(stream) => {
                pin_mut!(stream);
                loop {
                    let next = tokio::select! {
                        biased;
                        _ = &mut shutdown => return,
                        next = stream.next() => next,
                    };

                    match next {
                        Some(Ok(item)) => {
                            // A delivered message proves the connection is
                            // productive, so a later drop reconnects quickly.
                            backoff = initial;
                            on_message(item).await;
                        }
                        Some(Err(err)) => {
                            tracing::warn!("Stream error: {err}");
                            break;
                        }
                        None => {
                            tracing::debug!("Stream closed by the server");
                            break;
                        }
                    }
                }
            }
            Err(err) => {
                tracing::warn!("Could not open stream: {err}");
            }
        }

        // Wait before reconnecting so a persistently failing endpoint is not
        // hammered. Shutdown during the wait ends the loop immediately.
        tokio::select! {
            biased;
            _ = &mut shutdown => return,
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(max);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use futures::stream;
    use tokio::sync::Notify;
    use tokio::time::Instant;

    use super::*;

    /// Stream error type used across the tests. `&'static str` is `Display`.
    type TestErr = &'static str;
    /// Concrete stream type the `connect` closures return, so the `Ok` and
    /// `Err` arms share one type without boxing.
    type ItemStream = stream::Iter<std::vec::IntoIter<Result<u32, TestErr>>>;

    fn stream_of(items: Vec<Result<u32, TestErr>>) -> ItemStream {
        stream::iter(items)
    }

    /// A `connect` that never fails and never yields; used when the test only
    /// exercises shutdown.
    fn never_yields() -> Result<ItemStream, TestErr> {
        Ok(stream_of(vec![]))
    }

    // `connect` closures increment their counters inside the returned future,
    // not in the closure body. `tokio::select!` evaluates the `connect()` branch
    // expression eagerly every loop iteration, even on the pass where `shutdown`
    // wins, so only the poll of the future marks a real connection attempt.

    #[tokio::test(start_paused = true)]
    async fn shutdown_before_first_connect_never_connects() {
        let shutdown = Arc::new(Notify::new());
        // Already signalled: the very first select must pick shutdown.
        shutdown.notify_one();

        let connects = Arc::new(AtomicUsize::new(0));
        let connects_conn = Arc::clone(&connects);
        let shutdown_wait = Arc::clone(&shutdown);

        supervise_stream(
            BackoffPolicy {
                initial: Duration::from_millis(10),
                max: Duration::from_secs(1),
            },
            async move { shutdown_wait.notified().await },
            move || {
                let connects = Arc::clone(&connects_conn);
                async move {
                    connects.fetch_add(1, Ordering::SeqCst);
                    never_yields()
                }
            },
            |_item: u32| std::future::ready(()),
        )
        .await;

        assert_eq!(connects.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn forwards_items_and_reconnects_until_shutdown() {
        let shutdown = Arc::new(Notify::new());
        let received = Arc::new(Mutex::new(Vec::new()));
        let connects = Arc::new(AtomicUsize::new(0));

        let connects_conn = Arc::clone(&connects);
        let received_msg = Arc::clone(&received);
        let shutdown_msg = Arc::clone(&shutdown);
        let shutdown_wait = Arc::clone(&shutdown);

        supervise_stream(
            BackoffPolicy {
                initial: Duration::from_millis(10),
                max: Duration::from_secs(1),
            },
            async move { shutdown_wait.notified().await },
            move || {
                let connects = Arc::clone(&connects_conn);
                async move {
                    // Each connection yields two items, so reaching four proves
                    // a reconnect happened.
                    let n = connects.fetch_add(1, Ordering::SeqCst) as u32;
                    Ok::<ItemStream, TestErr>(stream_of(vec![Ok(n * 2), Ok(n * 2 + 1)]))
                }
            },
            move |item: u32| {
                let received = Arc::clone(&received_msg);
                let shutdown = Arc::clone(&shutdown_msg);
                async move {
                    let mut v = received.lock().expect("lock");
                    v.push(item);
                    if v.len() >= 4 {
                        shutdown.notify_one();
                    }
                }
            },
        )
        .await;

        assert_eq!(*received.lock().expect("lock"), vec![0, 1, 2, 3]);
        assert_eq!(connects.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn item_error_reconnects_and_skips_rest_of_stream() {
        let shutdown = Arc::new(Notify::new());
        let received = Arc::new(Mutex::new(Vec::new()));
        let connects = Arc::new(AtomicUsize::new(0));

        let connects_conn = Arc::clone(&connects);
        let received_msg = Arc::clone(&received);
        let shutdown_msg = Arc::clone(&shutdown);
        let shutdown_wait = Arc::clone(&shutdown);

        supervise_stream(
            BackoffPolicy {
                initial: Duration::from_millis(10),
                max: Duration::from_secs(1),
            },
            async move { shutdown_wait.notified().await },
            move || {
                let connects = Arc::clone(&connects_conn);
                async move {
                    let n = connects.fetch_add(1, Ordering::SeqCst);
                    if n == 0 {
                        // The error breaks the stream before `11` is reached.
                        Ok::<ItemStream, TestErr>(stream_of(vec![
                            Ok(10),
                            Err("mid-stream"),
                            Ok(11),
                        ]))
                    } else {
                        Ok(stream_of(vec![Ok(20)]))
                    }
                }
            },
            move |item: u32| {
                let received = Arc::clone(&received_msg);
                let shutdown = Arc::clone(&shutdown_msg);
                async move {
                    received.lock().expect("lock").push(item);
                    if item == 20 {
                        shutdown.notify_one();
                    }
                }
            },
        )
        .await;

        // `11` is never delivered: the error terminated that stream first.
        assert_eq!(*received.lock().expect("lock"), vec![10, 20]);
    }

    #[tokio::test(start_paused = true)]
    async fn connect_failures_back_off_exponentially() {
        let shutdown = Arc::new(Notify::new());
        let attempts = Arc::new(AtomicUsize::new(0));

        let attempts_conn = Arc::clone(&attempts);
        let shutdown_conn = Arc::clone(&shutdown);
        let shutdown_wait = Arc::clone(&shutdown);
        let start = Instant::now();

        supervise_stream(
            BackoffPolicy {
                initial: Duration::from_millis(100),
                max: Duration::from_millis(400),
            },
            async move { shutdown_wait.notified().await },
            move || {
                let attempts = Arc::clone(&attempts_conn);
                let shutdown = Arc::clone(&shutdown_conn);
                async move {
                    // Stop after the fourth failed attempt.
                    if attempts.fetch_add(1, Ordering::SeqCst) + 1 >= 4 {
                        shutdown.notify_one();
                    }
                    Err::<ItemStream, TestErr>("connect refused")
                }
            },
            |_item: u32| std::future::ready(()),
        )
        .await;

        assert_eq!(attempts.load(Ordering::SeqCst), 4);
        // Backoff sleeps precede each doubling, so the delays between the four
        // attempts are 100 + 200 + 400 (capped) = 700ms. The paused clock only
        // advances for the elapsed sleeps.
        assert_eq!(start.elapsed(), Duration::from_millis(700));
    }

    #[tokio::test(start_paused = true)]
    async fn delivered_message_resets_backoff() {
        let shutdown = Arc::new(Notify::new());
        let attempts = Arc::new(AtomicUsize::new(0));

        let attempts_conn = Arc::clone(&attempts);
        let shutdown_conn = Arc::clone(&shutdown);
        let shutdown_wait = Arc::clone(&shutdown);
        let start = Instant::now();

        supervise_stream(
            BackoffPolicy {
                initial: Duration::from_millis(100),
                max: Duration::from_secs(10),
            },
            async move { shutdown_wait.notified().await },
            move || {
                let attempts = Arc::clone(&attempts_conn);
                let shutdown = Arc::clone(&shutdown_conn);
                async move {
                    match attempts.fetch_add(1, Ordering::SeqCst) {
                        // Fails once: the following sleep is the 100ms floor,
                        // and backoff then doubles to 200ms.
                        0 => Err::<ItemStream, TestErr>("connect refused"),
                        // Delivers a message, which resets backoff to the 100ms
                        // floor before the stream closes.
                        1 => Ok(stream_of(vec![Ok(1)])),
                        // Shuts the loop down on the next attempt.
                        _ => {
                            shutdown.notify_one();
                            Ok(stream_of(vec![]))
                        }
                    }
                }
            },
            |_item: u32| std::future::ready(()),
        )
        .await;

        // 100ms sleep after the failed attempt, then 100ms (reset by the
        // delivery) after the second stream closes: 200ms. Without the reset the
        // second sleep would have been 200ms, for 300ms total.
        assert_eq!(start.elapsed(), Duration::from_millis(200));
    }

    #[tokio::test(start_paused = true)]
    async fn max_below_initial_is_clamped_to_the_floor() {
        let shutdown = Arc::new(Notify::new());
        let attempts = Arc::new(AtomicUsize::new(0));

        let attempts_conn = Arc::clone(&attempts);
        let shutdown_conn = Arc::clone(&shutdown);
        let shutdown_wait = Arc::clone(&shutdown);
        let start = Instant::now();

        supervise_stream(
            // `max` below `initial` is a caller mistake; the supervisor clamps it
            // up to `initial` so the delay never drops below the floor.
            BackoffPolicy {
                initial: Duration::from_millis(200),
                max: Duration::from_millis(100),
            },
            async move { shutdown_wait.notified().await },
            move || {
                let attempts = Arc::clone(&attempts_conn);
                let shutdown = Arc::clone(&shutdown_conn);
                async move {
                    // Stop after the third failed attempt, so two backoff sleeps
                    // elapse.
                    if attempts.fetch_add(1, Ordering::SeqCst) + 1 >= 3 {
                        shutdown.notify_one();
                    }
                    Err::<ItemStream, TestErr>("connect refused")
                }
            },
            |_item: u32| std::future::ready(()),
        )
        .await;

        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        // Both sleeps are the 200ms floor: without the clamp the second would be
        // `min(400, 100) = 100ms`, giving 300ms total instead of 400ms.
        assert_eq!(start.elapsed(), Duration::from_millis(400));
    }
}
