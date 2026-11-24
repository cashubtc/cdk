//! Ownership of the mint's request-driven asynchronous work.
//!
//! A swap or a melt runs in its own task, so that it finishes even when the
//! caller that asked for it goes away. [`Operations`] owns those tasks: it
//! decides whether new work is admitted, and it counts the work that is still
//! running. A finished task gives its [`OperationPermit`] back, so nothing is
//! retained for completed work.
//!
//! Stopping is a drain: admission closes, running operations are given time to
//! finish, and only the ones that outlast that time are aborted. An aborted
//! operation stops at its next await point, which is the same as the process
//! dying there, so startup recovery resolves whatever it left behind.

use std::future::Future;
use std::sync::Arc;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::Error;

/// Admission state and the number of operations still running.
///
/// Both live in one value so that admitting an operation and closing admission
/// cannot interleave: an operation is either counted before admission closes,
/// or rejected.
#[derive(Debug)]
struct State {
    accepting: bool,
    active: usize,
    /// Cancelled when admission closes. Work that only waits, and owns no
    /// state that needs finishing, ends on it instead of holding up a drain.
    closing: CancellationToken,
    /// Cancelled when the drain runs out of time.
    abort: CancellationToken,
}

/// Owner of the mint's in-flight operations.
#[derive(Debug, Clone)]
pub(crate) struct Operations {
    state: Arc<watch::Sender<State>>,
}

impl Operations {
    /// Create an owner that admits operations.
    ///
    /// A mint admits operations before its first `start()`, so that a mint
    /// used without background services keeps working.
    pub(crate) fn new() -> Self {
        let (state, _) = watch::channel(State {
            accepting: true,
            active: 0,
            closing: CancellationToken::new(),
            abort: CancellationToken::new(),
        });
        Self {
            state: Arc::new(state),
        }
    }

    /// Admit a new operation, or reject it when admission is closed.
    pub(crate) fn admit(&self) -> Result<OperationPermit, Error> {
        let admitted = self.state.send_if_modified(|state| {
            if state.accepting {
                state.active += 1;
            }
            state.accepting
        });

        match admitted {
            true => Ok(self.permit()),
            false => Err(Error::ShuttingDown),
        }
    }

    /// Count follow-up work of an operation that is already under way.
    ///
    /// Unlike [`admit`](Self::admit) this never rejects: the operation was
    /// accepted earlier, and its remaining work still has to be owned.
    pub(crate) fn track(&self) -> OperationPermit {
        self.state.send_modify(|state| state.active += 1);
        self.permit()
    }

    /// Build the permit for an operation that has already been counted.
    fn permit(&self) -> OperationPermit {
        let state = self.state.borrow();
        OperationPermit {
            closing: state.closing.clone(),
            abort: state.abort.clone(),
            state: Arc::clone(&self.state),
        }
    }

    /// Spawn `future` as an operation that holds `permit` until it ends.
    ///
    /// The permit is released when the task completes, panics, or is aborted.
    /// An operation aborted by [`abort`](Self::abort) is dropped at its next
    /// await point and resolves to [`Error::ShuttingDown`].
    pub(crate) fn spawn<F, T>(permit: OperationPermit, future: F) -> JoinHandle<Result<T, Error>>
    where
        F: Future<Output = Result<T, Error>> + Send + 'static,
        T: Send + 'static,
    {
        tokio::spawn(async move {
            let permit = permit;
            tokio::select! {
                biased;
                _ = permit.abort.cancelled() => Err(Error::ShuttingDown),
                result = future => result,
            }
        })
    }

    /// Stop admitting operations.
    pub(crate) fn close(&self) {
        self.state.send_modify(|state| {
            state.accepting = false;
            state.closing.cancel();
        });
    }

    /// Admit operations again.
    pub(crate) fn open(&self) {
        self.state.send_modify(|state| {
            state.accepting = true;
            // A cancelled token stays cancelled, so operations admitted from
            // here on need their own.
            if state.closing.is_cancelled() {
                state.closing = CancellationToken::new();
            }
            if state.abort.is_cancelled() {
                state.abort = CancellationToken::new();
            }
        });
    }

    /// Abort the operations that are still running.
    pub(crate) fn abort(&self) {
        self.state.borrow().abort.cancel();
    }

    /// Number of operations still running.
    pub(crate) fn active(&self) -> usize {
        self.state.borrow().active
    }

    /// Wait until no operation is running.
    pub(crate) async fn wait_idle(&self) {
        let mut state = self.state.subscribe();
        // The sender lives in `self`, so the channel cannot close while this
        // waits.
        let _ = state.wait_for(|state| state.active == 0).await;
    }
}

/// Marks one running operation. Dropping it marks the operation as finished.
#[derive(Debug)]
pub(crate) struct OperationPermit {
    state: Arc<watch::Sender<State>>,
    closing: CancellationToken,
    abort: CancellationToken,
}

impl OperationPermit {
    /// Token cancelled once the mint stops admitting operations.
    pub(crate) fn closing(&self) -> CancellationToken {
        self.closing.clone()
    }
}

impl Drop for OperationPermit {
    fn drop(&mut self) {
        self.state
            .send_modify(|state| state.active = state.active.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admit_counts_until_the_permit_is_dropped() {
        let operations = Operations::new();

        let first = operations.admit().expect("admission is open");
        let second = operations.admit().expect("admission is open");
        assert_eq!(operations.active(), 2);

        drop(first);
        assert_eq!(operations.active(), 1);
        drop(second);
        assert_eq!(operations.active(), 0);
    }

    #[test]
    fn closed_admission_rejects_and_reopens() {
        let operations = Operations::new();
        let running = operations.admit().expect("admission is open");

        operations.close();
        assert!(matches!(operations.admit(), Err(Error::ShuttingDown)));
        // A rejected operation is not counted, and running work is unaffected.
        assert_eq!(operations.active(), 1);

        // Follow-up work of a running operation is still owned.
        let follow_up = operations.track();
        assert_eq!(operations.active(), 2);
        drop(follow_up);
        drop(running);

        operations.open();
        assert!(operations.admit().is_ok());
    }

    #[tokio::test]
    async fn spawned_operation_releases_its_permit() {
        let operations = Operations::new();

        let permit = operations.admit().expect("admission is open");
        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let task = Operations::spawn(permit, async move {
            let _ = wait.await;
            Ok(())
        });
        assert_eq!(operations.active(), 1);

        release.send(()).expect("task is waiting");
        task.await.expect("task joins").expect("task completes");
        assert_eq!(operations.active(), 0);
    }

    #[tokio::test]
    async fn panicked_operation_releases_its_permit() {
        let operations = Operations::new();

        let permit = operations.admit().expect("admission is open");
        let panicked = Operations::spawn(permit, async {
            if true {
                panic!("operation failed");
            }
            Ok(())
        });
        assert!(panicked.await.expect_err("task panics").is_panic());
        assert_eq!(operations.active(), 0);
    }

    #[tokio::test]
    async fn abort_ends_running_operations_and_wait_idle_returns() {
        let operations = Operations::new();

        let permit = operations.admit().expect("admission is open");
        let stuck = Operations::spawn(permit, std::future::pending::<Result<(), Error>>());

        operations.close();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), operations.wait_idle())
                .await
                .is_err(),
            "a running operation keeps the owner busy"
        );

        operations.abort();
        operations.wait_idle().await;
        assert!(matches!(
            stuck.await.expect("task joins"),
            Err(Error::ShuttingDown)
        ));

        // A reopened owner hands out permits that are not already aborted.
        operations.open();
        let permit = operations.admit().expect("admission is open");
        assert!(!permit.closing().is_cancelled());
        let task = Operations::spawn(permit, async { Ok(()) });
        task.await.expect("task joins").expect("task completes");
    }

    #[tokio::test]
    async fn close_signals_waiting_work() {
        let operations = Operations::new();
        let permit = operations.admit().expect("admission is open");
        let closing = permit.closing();

        assert!(!closing.is_cancelled());
        operations.close();
        assert!(closing.is_cancelled());
    }
}
