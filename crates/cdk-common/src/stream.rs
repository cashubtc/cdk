//! Supervision for long-lived streaming subscriptions.
//!
//! [`SupervisedStream`] owns the reconnect, backoff, and shutdown loop that
//! every consumer of a long-lived stream (a gRPC server-stream, a payment
//! backend's event stream) would otherwise hand-roll. It is transport-agnostic:
//! the implementor supplies how to open a fresh stream and how to handle each
//! item, holding its connection state as fields rather than cloning it into
//! per-call closures.

use std::fmt;
use std::future::Future;

use futures::{pin_mut, Stream, StreamExt};

/// Capped exponential backoff for [`SupervisedStream`] reconnect attempts.
///
/// Only consecutive connect failures grow the delay. After a stream that opened
/// closes or errors, the supervisor waits exactly `initial` before reconnecting:
/// the connection was working, so the reconnect should be prompt, and this floor
/// still stops a server that accepts then instantly drops from spinning in a hot
/// loop. A successful connect resets the growth, so an endpoint that recovers is
/// not held to a delay earned by earlier failures.
///
/// `initial` must be non-zero (a zero `initial` never grows, `0 * 2 == 0`, so
/// the backoff would degenerate into a busy reconnect loop) and `max` must be at
/// least `initial` (a smaller cap would clamp the delay below the floor).
/// [`SupervisedStream::supervise`] also clamps both at runtime, so a violated
/// invariant is corrected rather than acted on, but passing sane values keeps
/// the delays predictable.
#[derive(Debug, Clone, Copy)]
pub struct BackoffPolicy {
    /// Delay before the first reconnect, the fixed delay after an opened stream
    /// closes or errors, and the floor a successful connect resets the growing
    /// backoff to. Must be non-zero.
    pub initial: std::time::Duration,
    /// Upper bound the delay is capped at while backing off across connect
    /// failures. Must be at least `initial`.
    pub max: std::time::Duration,
}

/// A supervised, self-reconnecting streaming subscription.
///
/// Implementors own the connection state (clients, channels, publishers) as
/// fields, so the reconnect/backoff/shutdown loop can hand each item to
/// [`on_message`](Self::on_message) without the per-item state-cloning a
/// closure-based supervisor forces. The provided [`supervise`](Self::supervise)
/// method owns that loop; an implementor supplies only how to connect, how to
/// handle an item, and (optionally) how to tear down.
#[async_trait::async_trait]
pub trait SupervisedStream: Send {
    /// Item the stream yields and [`on_message`](Self::on_message) consumes.
    type Item: Send;
    /// Error a failed connect attempt yields. Logged, then retried.
    type ConnectError: fmt::Display + Send;
    /// Error the stream may yield per item. Logged, then reconnected.
    type StreamError: fmt::Display + Send;
    /// The stream a successful [`connect`](Self::connect) opens.
    type Stream: Stream<Item = Result<Self::Item, Self::StreamError>> + Send;

    /// Identifies this subscription in the supervisor's reconnect/close logs, so
    /// a connect failure or dropped stream names the subscription that produced
    /// it rather than an anonymous line.
    fn name(&self) -> &str;

    /// Backoff policy for reconnect attempts.
    fn backoff_policy(&self) -> BackoffPolicy;

    /// Open a fresh stream of items.
    async fn connect(&mut self) -> Result<Self::Stream, Self::ConnectError>;

    /// Handle one delivered item. Awaited to completion (item processing is
    /// sequential, and a slow handler holds the loop, so a handler that must
    /// not block the read should offload its work).
    async fn on_message(&mut self, item: Self::Item);

    /// Teardown run once, after [`supervise`](Self::supervise) observes shutdown
    /// and before it returns. Default: nothing. Cancelling a stream token or
    /// releasing a resource belongs here rather than after the `supervise` call.
    async fn on_shutdown(&mut self) {}

    /// Keep the subscription alive across reconnects until `shutdown` resolves.
    ///
    /// Every item [`connect`](Self::connect) yields is handed to
    /// [`on_message`](Self::on_message). The backoff grows only across
    /// consecutive connect failures and resets the moment a connect succeeds, so
    /// it tracks connection health rather than stream activity: an endpoint that
    /// keeps refusing connections is backed off exponentially, while a stream
    /// that opens (even if it never delivers a message) and later closes or
    /// errors reconnects after the fixed `initial` delay. An idle-but-open stream
    /// that drops is therefore never penalized by a delay left over from earlier
    /// connect failures.
    ///
    /// `shutdown` stops the supervisor promptly whenever it is waiting: to
    /// connect, for the next item, or during a backoff. It does not interrupt an
    /// in-flight `on_message`. [`on_shutdown`](Self::on_shutdown) runs on the way
    /// out, on every exit path.
    async fn supervise<S>(&mut self, shutdown: S)
    where
        S: Future<Output = ()> + Send,
    {
        // Correct a degenerate policy consistently in every build rather than
        // trusting the implementor: a zero `initial` would make the backoff
        // busy-loop (`0 * 2 == 0`), and a `max` below `initial` would clamp the
        // delay under the floor. Clamp once, then read only these locals for the
        // rest of the loop.
        let policy = self.backoff_policy();
        let initial = policy.initial.max(std::time::Duration::from_millis(1));
        let max = policy.max.max(initial);

        pin_mut!(shutdown);
        // Grows only across consecutive connect failures; a successful connect
        // resets it. So it tracks connection health, not stream idleness.
        let mut backoff = initial;

        'outer: loop {
            let connect_result = tokio::select! {
                biased;
                _ = &mut shutdown => break 'outer,
                result = self.connect() => result,
            };

            let wait = match connect_result {
                Ok(stream) => {
                    // The connection opened, so it is healthy regardless of
                    // whether it ever delivers a message. Reset the backoff here
                    // (not per message) so an idle-but-open stream that later
                    // drops reconnects at the floor rather than at a delay left
                    // over from earlier connect failures.
                    backoff = initial;
                    pin_mut!(stream);
                    loop {
                        let next = tokio::select! {
                            biased;
                            _ = &mut shutdown => break 'outer,
                            next = stream.next() => next,
                        };

                        match next {
                            Some(Ok(item)) => self.on_message(item).await,
                            Some(Err(err)) => {
                                tracing::warn!(name = self.name(), "Stream error: {err}");
                                break;
                            }
                            None => {
                                tracing::debug!(name = self.name(), "Stream closed by the server");
                                break;
                            }
                        }
                    }
                    // An opened stream closed or errored. Wait the fixed floor,
                    // not the growing backoff: the connection was working, so a
                    // reconnect should be prompt.
                    initial
                }
                Err(err) => {
                    tracing::warn!(name = self.name(), "Could not open stream: {err}");
                    // Wait the current backoff, then grow it, so a persistently
                    // failing endpoint is not hammered.
                    let wait = backoff;
                    // Saturating so a large `initial` cannot overflow the
                    // doubling; `max` clamps it back down regardless.
                    backoff = backoff.saturating_mul(2).min(max);
                    wait
                }
            };

            // Shutdown during the wait ends the loop immediately.
            tokio::select! {
                biased;
                _ = &mut shutdown => break 'outer,
                _ = tokio::time::sleep(wait) => {}
            }
        }

        self.on_shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use futures::stream;
    use tokio::sync::Notify;
    use tokio::time::Instant;

    use super::*;

    /// Stream error type used across the tests. `&'static str` is `Display`.
    type TestErr = &'static str;
    /// Concrete stream type each `connect` returns, so the `Ok` and `Err` arms
    /// share one type without boxing.
    type ItemStream = stream::Iter<std::vec::IntoIter<Result<u32, TestErr>>>;
    /// One scripted connect outcome: a failed connect, or a stream of items.
    type ConnectStep = Result<Vec<Result<u32, TestErr>>, TestErr>;

    /// A scripted [`SupervisedStream`] implementor. Each connect consumes the
    /// next `steps` entry (the last entry repeats), and the stop conditions fire
    /// `shutdown` from `connect` or `on_message` so a test drives the loop to a
    /// deterministic end.
    struct Harness {
        policy: BackoffPolicy,
        steps: Vec<ConnectStep>,
        shutdown: Arc<Notify>,
        connects: Arc<AtomicUsize>,
        received: Arc<Mutex<Vec<u32>>>,
        shutdown_ran: Arc<AtomicBool>,
        /// Fire shutdown from `connect` once this many attempts have started.
        stop_after_connects: Option<usize>,
        /// Fire shutdown from `on_message` when this item arrives.
        stop_on_item: Option<u32>,
        /// Fire shutdown from `on_message` once this many items are received.
        stop_at_len: Option<usize>,
    }

    impl Harness {
        fn new(policy: BackoffPolicy, steps: Vec<ConnectStep>) -> Self {
            Self {
                policy,
                steps,
                shutdown: Arc::new(Notify::new()),
                connects: Arc::new(AtomicUsize::new(0)),
                received: Arc::new(Mutex::new(Vec::new())),
                shutdown_ran: Arc::new(AtomicBool::new(false)),
                stop_after_connects: None,
                stop_on_item: None,
                stop_at_len: None,
            }
        }
    }

    // The connect counter is bumped inside `connect`'s body, not in the loop.
    // `tokio::select!` evaluates the `self.connect()` branch expression eagerly
    // every iteration, even on the pass where `shutdown` wins, so only the poll
    // of the future (running the body) marks a real connection attempt.
    #[async_trait::async_trait]
    impl SupervisedStream for Harness {
        type Item = u32;
        type ConnectError = TestErr;
        type StreamError = TestErr;
        type Stream = ItemStream;

        fn name(&self) -> &str {
            "test"
        }

        fn backoff_policy(&self) -> BackoffPolicy {
            self.policy
        }

        async fn connect(&mut self) -> Result<Self::Stream, TestErr> {
            let n = self.connects.fetch_add(1, Ordering::SeqCst);
            if let Some(k) = self.stop_after_connects {
                if n + 1 >= k {
                    self.shutdown.notify_one();
                }
            }
            let idx = n.min(self.steps.len() - 1);
            self.steps[idx].clone().map(stream::iter)
        }

        async fn on_message(&mut self, item: u32) {
            let mut v = self.received.lock().expect("lock");
            v.push(item);
            if self.stop_on_item == Some(item) {
                self.shutdown.notify_one();
            }
            if self.stop_at_len.is_some_and(|l| v.len() >= l) {
                self.shutdown.notify_one();
            }
        }

        async fn on_shutdown(&mut self) {
            self.shutdown_ran.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_before_first_connect_never_connects() {
        let mut h = Harness::new(
            BackoffPolicy {
                initial: Duration::from_millis(10),
                max: Duration::from_secs(1),
            },
            vec![Ok(vec![])],
        );
        // Already signalled: the very first select must pick shutdown.
        h.shutdown.notify_one();
        let connects = Arc::clone(&h.connects);
        let shutdown = Arc::clone(&h.shutdown);

        h.supervise(async move { shutdown.notified().await }).await;

        assert_eq!(connects.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn forwards_items_and_reconnects_until_shutdown() {
        // Each connection yields two items, so reaching four proves a reconnect.
        let mut h = Harness::new(
            BackoffPolicy {
                initial: Duration::from_millis(10),
                max: Duration::from_secs(1),
            },
            vec![Ok(vec![Ok(0), Ok(1)]), Ok(vec![Ok(2), Ok(3)])],
        );
        h.stop_at_len = Some(4);
        let connects = Arc::clone(&h.connects);
        let received = Arc::clone(&h.received);
        let shutdown = Arc::clone(&h.shutdown);

        h.supervise(async move { shutdown.notified().await }).await;

        assert_eq!(*received.lock().expect("lock"), vec![0, 1, 2, 3]);
        assert_eq!(connects.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn item_error_reconnects_and_skips_rest_of_stream() {
        // The error breaks the first stream before `11` is reached.
        let mut h = Harness::new(
            BackoffPolicy {
                initial: Duration::from_millis(10),
                max: Duration::from_secs(1),
            },
            vec![
                Ok(vec![Ok(10), Err("mid-stream"), Ok(11)]),
                Ok(vec![Ok(20)]),
            ],
        );
        h.stop_on_item = Some(20);
        let received = Arc::clone(&h.received);
        let shutdown = Arc::clone(&h.shutdown);

        h.supervise(async move { shutdown.notified().await }).await;

        // `11` is never delivered: the error terminated that stream first.
        assert_eq!(*received.lock().expect("lock"), vec![10, 20]);
    }

    #[tokio::test(start_paused = true)]
    async fn opened_stream_error_waits_floor_not_grown_backoff() {
        // Two connect failures grow the backoff, then a stream opens and
        // immediately errors. The successful connect resets the backoff, so the
        // wait after the stream error is the fixed floor, not the grown delay:
        // only connect failures back off, an opened stream that errors does not.
        let mut h = Harness::new(
            BackoffPolicy {
                initial: Duration::from_millis(100),
                max: Duration::from_secs(10),
            },
            vec![
                // Fails: sleep 100ms floor, backoff doubles to 200ms.
                Err("connect refused"),
                // Fails: sleep 200ms, backoff doubles to 400ms.
                Err("connect refused"),
                // Opens then errors: reset to the floor, then wait the fixed
                // 100ms, not the 400ms the failures had reached.
                Ok(vec![Err("mid-stream")]),
            ],
        );
        h.stop_after_connects = Some(4);
        let attempts = Arc::clone(&h.connects);
        let shutdown = Arc::clone(&h.shutdown);
        let start = Instant::now();

        h.supervise(async move { shutdown.notified().await }).await;

        assert_eq!(attempts.load(Ordering::SeqCst), 4);
        // 100 (fail) + 200 (fail) + 100 (floor after the stream error) = 400ms.
        // If a stream error grew the backoff, the third wait would have been
        // 400ms, for 700ms total.
        assert_eq!(start.elapsed(), Duration::from_millis(400));
    }

    #[tokio::test(start_paused = true)]
    async fn connect_failures_back_off_exponentially() {
        let mut h = Harness::new(
            BackoffPolicy {
                initial: Duration::from_millis(100),
                max: Duration::from_millis(400),
            },
            vec![Err("connect refused")],
        );
        // Stop after the fourth failed attempt.
        h.stop_after_connects = Some(4);
        let attempts = Arc::clone(&h.connects);
        let shutdown = Arc::clone(&h.shutdown);
        let start = Instant::now();

        h.supervise(async move { shutdown.notified().await }).await;

        assert_eq!(attempts.load(Ordering::SeqCst), 4);
        // Backoff sleeps precede each doubling, so the delays between the four
        // attempts are 100 + 200 + 400 (capped) = 700ms. The paused clock only
        // advances for the elapsed sleeps.
        assert_eq!(start.elapsed(), Duration::from_millis(700));
    }

    #[tokio::test(start_paused = true)]
    async fn successful_connect_resets_backoff_even_without_messages() {
        // Two connect failures grow the backoff, then a stream opens but never
        // delivers a message before closing. The reset happens on the successful
        // connect, not on a delivery, so the disconnect waits the 100ms floor
        // rather than the elevated backoff.
        let mut h = Harness::new(
            BackoffPolicy {
                initial: Duration::from_millis(100),
                max: Duration::from_secs(10),
            },
            vec![
                // Fails: sleep 100ms floor, backoff doubles to 200ms.
                Err("connect refused"),
                // Fails: sleep 200ms, backoff doubles to 400ms.
                Err("connect refused"),
                // Opens but yields nothing and closes: reset to the floor, then
                // wait the fixed 100ms, not the 400ms the failures had reached.
                Ok(vec![]),
            ],
        );
        h.stop_after_connects = Some(4);
        let attempts = Arc::clone(&h.connects);
        let shutdown = Arc::clone(&h.shutdown);
        let start = Instant::now();

        h.supervise(async move { shutdown.notified().await }).await;

        assert_eq!(attempts.load(Ordering::SeqCst), 4);
        // 100 (fail) + 200 (fail) + 100 (floor after the empty stream) = 400ms.
        // Under a per-failure backoff that ignored the successful connect, the
        // third wait would have been 400ms, for 700ms total.
        assert_eq!(start.elapsed(), Duration::from_millis(400));
    }

    #[tokio::test(start_paused = true)]
    async fn max_below_initial_is_clamped_to_the_floor() {
        // `max` below `initial` is a caller mistake; the supervisor clamps it up
        // to `initial` so the delay never drops below the floor.
        let mut h = Harness::new(
            BackoffPolicy {
                initial: Duration::from_millis(200),
                max: Duration::from_millis(100),
            },
            vec![Err("connect refused")],
        );
        // Stop after the third failed attempt, so two backoff sleeps elapse.
        h.stop_after_connects = Some(3);
        let attempts = Arc::clone(&h.connects);
        let shutdown = Arc::clone(&h.shutdown);
        let start = Instant::now();

        h.supervise(async move { shutdown.notified().await }).await;

        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        // Both sleeps are the 200ms floor: without the clamp the second would be
        // `min(400, 100) = 100ms`, giving 300ms total instead of 400ms.
        assert_eq!(start.elapsed(), Duration::from_millis(400));
    }

    #[tokio::test(start_paused = true)]
    async fn on_shutdown_runs_after_supervise_returns() {
        let mut h = Harness::new(
            BackoffPolicy {
                initial: Duration::from_millis(10),
                max: Duration::from_secs(1),
            },
            vec![Ok(vec![])],
        );
        h.shutdown.notify_one();
        let shutdown_ran = Arc::clone(&h.shutdown_ran);
        let shutdown = Arc::clone(&h.shutdown);

        h.supervise(async move { shutdown.notified().await }).await;

        assert!(shutdown_ran.load(Ordering::SeqCst));
    }
}
