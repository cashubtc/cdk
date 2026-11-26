//! A bounded pool for CPU-heavy mint work, kept off the async workers.
//!
//! Jobs run on Tokio's blocking pool, at most one per slot at a time. A job
//! that finds every slot taken either waits for one ([`Admission::Unbounded`])
//! or joins a short, bounded queue and is refused with [`Error::Busy`] once the
//! queue is full or its wait runs out ([`Admission::Bounded`]).

use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use futures::future::{self, Either};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::Error;

/// How long a bounded job waits for a slot before it is refused.
pub const DEFAULT_MAX_WAIT: Duration = Duration::from_secs(1);

/// Bounded jobs that may wait per slot before new ones are refused outright.
pub const DEFAULT_WAITERS_PER_SLOT: usize = 4;

/// What a job does when every slot is taken.
///
/// Callers choose by what a refusal would cost them, not by the kind of work:
/// the same signing job may be refusable in one flow and not in another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Admission {
    /// Wait in the bounded queue, up to the pool's maximum wait, then fail with
    /// [`Error::Busy`]. For work that precedes any state change, so the caller
    /// can retry the whole request.
    Bounded,
    /// Wait for as long as it takes. For work that follows a state change a
    /// refusal would have to unwind.
    ///
    /// Unbounded jobs share the slots with bounded ones in arrival order, so a
    /// backlog of them can make every bounded job time out. Reserve this for
    /// work that only follows an already-admitted bounded job, which keeps
    /// that backlog in check.
    Unbounded,
}

/// A bounded pool for CPU-heavy work.
#[derive(Debug)]
pub struct WorkPool {
    slots: Arc<Semaphore>,
    waiters: Semaphore,
    max_wait: Duration,
}

impl WorkPool {
    /// A pool running at most `slots` jobs at once, with at most `waiters`
    /// bounded jobs queued for up to `max_wait` each.
    ///
    /// `slots` is non-zero because a pool without slots would leave every
    /// unbounded job waiting forever. `waiters` may be zero, so bounded jobs are
    /// refused whenever every slot is taken.
    pub fn new(slots: NonZeroUsize, waiters: usize, max_wait: Duration) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(slots.get())),
            waiters: Semaphore::new(waiters),
            max_wait,
        }
    }

    /// A pool with one slot per CPU available to this process, as reported by
    /// [`std::thread::available_parallelism`], and the default queue.
    pub fn sized_to_host() -> Self {
        let slots = std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
        Self::new(
            slots,
            slots.get().saturating_mul(DEFAULT_WAITERS_PER_SLOT),
            DEFAULT_MAX_WAIT,
        )
    }

    /// Run `job` once a slot is free.
    ///
    /// The slot is held by the job itself, so it stays taken until the work
    /// ends even if the caller is cancelled. Fails with [`Error::Busy`] when a
    /// bounded job is not admitted, and with [`Error::Internal`] if the job
    /// panics.
    pub async fn run<T, F>(&self, admission: Admission, job: F) -> Result<T, Error>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let slot = self.admit(admission).await?;

        // wasm32 has no blocking pool; run in place there.
        #[cfg(target_arch = "wasm32")]
        {
            let _slot = slot;
            Ok(job())
        }

        #[cfg(not(target_arch = "wasm32"))]
        tokio::task::spawn_blocking(move || {
            let _slot = slot;
            job()
        })
        .await
        .map_err(|err| {
            tracing::error!("Work pool job failed: {}", err);
            Error::Internal
        })
    }

    async fn admit(&self, admission: Admission) -> Result<OwnedSemaphorePermit, Error> {
        // A released slot goes to the longest waiter first, so this only
        // succeeds when nobody is queued.
        if let Ok(slot) = self.slots.clone().try_acquire_owned() {
            return Ok(slot);
        }

        match admission {
            Admission::Unbounded => self
                .slots
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| Error::Internal),
            Admission::Bounded => {
                let _waiting = self.waiters.try_acquire().map_err(|_| Error::Busy)?;
                let slot = pin!(self.slots.clone().acquire_owned());
                let deadline = pin!(crate::task::sleep(self.max_wait));
                match future::select(slot, deadline).await {
                    Either::Left((slot, _)) => slot.map_err(|_| Error::Internal),
                    Either::Right(_) => Err(Error::Busy),
                }
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::sync::mpsc;

    use super::*;

    /// Occupy one slot until the returned sender is dropped.
    async fn occupy(pool: &Arc<WorkPool>) -> mpsc::Sender<()> {
        let (release, held) = mpsc::channel::<()>();
        let (started, running) = tokio::sync::oneshot::channel();
        let pool = pool.clone();
        tokio::spawn(async move {
            pool.run(Admission::Unbounded, move || {
                let _ = started.send(());
                let _ = held.recv();
            })
            .await
        });
        running.await.expect("occupying job started");
        release
    }

    #[tokio::test]
    async fn runs_a_job_on_a_free_slot() {
        let pool = WorkPool::new(NonZeroUsize::MIN, 0, Duration::ZERO);
        assert_eq!(pool.run(Admission::Bounded, || 7).await.unwrap(), 7);
    }

    #[tokio::test]
    async fn refuses_a_bounded_job_when_the_queue_is_full() {
        let pool = Arc::new(WorkPool::new(NonZeroUsize::MIN, 0, Duration::from_secs(60)));
        let _release = occupy(&pool).await;

        assert!(matches!(
            pool.run(Admission::Bounded, || ()).await,
            Err(Error::Busy)
        ));
    }

    #[tokio::test]
    async fn refuses_a_bounded_job_whose_wait_runs_out() {
        let pool = Arc::new(WorkPool::new(
            NonZeroUsize::MIN,
            1,
            Duration::from_millis(20),
        ));
        let _release = occupy(&pool).await;

        assert!(matches!(
            pool.run(Admission::Bounded, || ()).await,
            Err(Error::Busy)
        ));
    }

    #[tokio::test]
    async fn a_bounded_job_runs_once_a_slot_frees() {
        let pool = Arc::new(WorkPool::new(NonZeroUsize::MIN, 1, Duration::from_secs(60)));
        let release = occupy(&pool).await;

        let mut waiting = tokio::spawn({
            let pool = pool.clone();
            async move { pool.run(Admission::Bounded, || 7).await }
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut waiting)
                .await
                .is_err(),
            "a bounded job waits in the queue while every slot is taken"
        );
        drop(release);

        assert_eq!(waiting.await.unwrap().unwrap(), 7);
    }

    #[tokio::test]
    async fn an_unbounded_job_outwaits_the_bounded_limits() {
        let pool = Arc::new(WorkPool::new(NonZeroUsize::MIN, 0, Duration::ZERO));
        let release = occupy(&pool).await;

        let mut waiting = tokio::spawn({
            let pool = pool.clone();
            async move { pool.run(Admission::Unbounded, || 7).await }
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut waiting)
                .await
                .is_err(),
            "an unbounded job waits while every slot is taken"
        );
        drop(release);

        assert_eq!(waiting.await.unwrap().unwrap(), 7);
    }
}
