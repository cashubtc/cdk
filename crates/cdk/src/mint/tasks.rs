//! Ownership of the mint's request-driven asynchronous work.
//!
//! A swap or a melt runs in its own task, so that it finishes even when the
//! caller that asked for it goes away. [`Operations`] owns those tasks: it
//! decides whether new work is admitted, and it counts the work that is still
//! running. A finished task gives its [`OperationPermit`] back, so nothing is
//! retained for completed work.

use std::future::Future;
use std::sync::Arc;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::Error;

/// Admission state and the number of operations still running.
///
/// Both live in one value so that admitting an operation and closing admission
/// cannot interleave: an operation is either counted before admission closes,
/// or rejected.
#[derive(Debug, Clone, Copy)]
struct State {
    accepting: bool,
    active: usize,
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
            true => Ok(OperationPermit {
                state: Arc::clone(&self.state),
            }),
            false => Err(Error::ShuttingDown),
        }
    }

    /// Count follow-up work of an operation that is already under way.
    ///
    /// Unlike [`admit`](Self::admit) this never rejects: the operation was
    /// accepted earlier, and its remaining work still has to be owned.
    pub(crate) fn track(&self) -> OperationPermit {
        self.state.send_modify(|state| state.active += 1);
        OperationPermit {
            state: Arc::clone(&self.state),
        }
    }

    /// Spawn `future` as an operation that holds `permit` until it ends.
    ///
    /// The permit is released when the task completes, panics, or is aborted.
    pub(crate) fn spawn<F>(permit: OperationPermit, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        tokio::spawn(async move {
            let _permit = permit;
            future.await
        })
    }

    /// Stop admitting operations.
    pub(crate) fn close(&self) {
        self.state.send_modify(|state| state.accepting = false);
    }

    /// Admit operations again.
    pub(crate) fn open(&self) {
        self.state.send_modify(|state| state.accepting = true);
    }

    /// Number of operations still running.
    #[cfg(test)]
    pub(crate) fn active(&self) -> usize {
        self.state.borrow().active
    }
}

/// Marks one running operation. Dropping it marks the operation as finished.
#[derive(Debug)]
pub(crate) struct OperationPermit {
    state: Arc<watch::Sender<State>>,
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
        });
        assert_eq!(operations.active(), 1);

        release.send(()).expect("task is waiting");
        task.await.expect("task completes");
        assert_eq!(operations.active(), 0);
    }

    #[tokio::test]
    async fn panicked_and_aborted_operations_release_their_permits() {
        let operations = Operations::new();

        let permit = operations.admit().expect("admission is open");
        let panicked = Operations::spawn(permit, async { panic!("operation failed") });
        assert!(panicked.await.expect_err("task panics").is_panic());
        assert_eq!(operations.active(), 0);

        let permit = operations.admit().expect("admission is open");
        let aborted = Operations::spawn(permit, std::future::pending::<()>());
        aborted.abort();
        assert!(aborted.await.expect_err("task is aborted").is_cancelled());
        assert_eq!(operations.active(), 0);
    }
}
