//! Ownership of federation maintenance tasks and their child quote work.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::FutureExt;
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::MintState;

/// Maximum grace period for in-flight, durably identified work to finish.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default)]
pub(crate) struct FederationTaskGroup {
    tracker: TaskTracker,
    stop: CancellationToken,
    failure: Arc<Mutex<Option<String>>>,
    aborts: Arc<Mutex<Vec<AbortHandle>>>,
}

impl FederationTaskGroup {
    pub(crate) fn stop_token(&self) -> CancellationToken {
        self.stop.clone()
    }

    pub(crate) fn fail(&self, reason: String) {
        if !self.stop.is_cancelled() {
            tracing::error!("{reason}");
            self.failure
                .lock()
                .expect("worker failure lock")
                .get_or_insert(reason);
            self.stop.cancel();
        }
    }

    pub(crate) fn spawn(
        &self,
        name: &'static str,
        worker: bool,
        work: impl Future<Output = ()> + Send + 'static,
    ) {
        let mut aborts = self.aborts.lock().expect("task registry lock");
        if self.stop.is_cancelled() {
            return;
        }
        aborts.retain(|task| !task.is_finished());
        let stop = self.stop.clone();
        let failure = self.failure.clone();
        let task = self.tracker.spawn(async move {
            let result = std::panic::AssertUnwindSafe(work).catch_unwind().await;
            if result.is_err() || (worker && !stop.is_cancelled()) {
                let reason = format!("federation {name} worker stopped unexpectedly");
                tracing::error!("{reason}");
                failure
                    .lock()
                    .expect("worker failure lock")
                    .get_or_insert(reason);
                stop.cancel();
            }
        });
        aborts.push(task.abort_handle());
    }

    async fn shutdown(&self) -> Result<(), cdk::Error> {
        self.stop.cancel();
        self.tracker.close();
        if tokio::time::timeout(SHUTDOWN_GRACE, self.tracker.wait())
            .await
            .is_err()
        {
            // Payments, signatures and broadcasts have durable identities before
            // external I/O. Do not restart them here: recovery must inspect that
            // state after process restart. Abort only after the grace period.
            for task in self.aborts.lock().expect("task registry lock").drain(..) {
                task.abort();
            }
            self.tracker.wait().await;
            return Err(cdk::Error::Custom(
                "federation shutdown grace period expired".to_string(),
            ));
        }
        match self.failure.lock().expect("worker failure lock").take() {
            Some(reason) => Err(cdk::Error::Custom(reason)),
            None => Ok(()),
        }
    }
}

/// Prepared federation workers. Starting them is an explicit daemon lifecycle step.
#[allow(missing_debug_implementations)]
pub struct FederationServices {
    state: MintState,
    peer_sync: Option<super::FederationPeerSyncConfig>,
    dkg: Option<super::FederationDkgAutoStartConfig>,
}

impl FederationServices {
    pub(crate) fn new(
        state: MintState,
        peer_sync: Option<super::FederationPeerSyncConfig>,
        dkg: Option<super::FederationDkgAutoStartConfig>,
    ) -> Self {
        Self {
            state,
            peer_sync,
            dkg,
        }
    }

    /// Start maintenance and retain ownership of every worker and quote task.
    pub async fn start(self) -> Result<RunningFederationServices, cdk::Error> {
        let tasks = self.state.federation.tasks.clone();
        let running = RunningFederationServices {
            tasks: tasks.clone(),
            state: self.state.clone(),
        };
        let startup = (|| -> Result<(), cdk::Error> {
            if let Some(config) = self.dkg {
                super::spawn_federation_dkg_driver(
                    self.state.clone(),
                    self.peer_sync.unwrap_or_default(),
                    config,
                );
            }
            if !self.state.federation.membership.is_active() {
                return Ok(());
            }
            // Validate the peer client before starting any wallet work.
            #[cfg(feature = "federation-bdk")]
            self.state
                .federation
                .wallet_liveness
                .clone()
                .spawn(self.state.clone())?;
            #[cfg(feature = "federation-bdk")]
            super::wallet_liveness::spawn_federation_onchain_sync(&self.state);
            self.state
                .federation
                .payment_observer
                .clone()
                .spawn(self.state.clone());
            #[cfg(feature = "federation-bdk")]
            self.state
                .federation
                .transaction_broadcaster
                .clone()
                .spawn(self.state.clone());
            #[cfg(feature = "federation-bark")]
            super::bark_maintenance::spawn(&self.state);
            if let Some(config) = self.peer_sync {
                super::spawn_federation_peer_sync_loop_with_config(self.state, config)?;
            }
            Ok(())
        })();
        if let Err(error) = startup {
            if let Err(shutdown_error) = running.shutdown().await {
                tracing::error!(%shutdown_error, "Federation startup cleanup failed");
            }
            return Err(error);
        }
        Ok(running)
    }
}

/// Running federation service owner. Dropping the owner requests cancellation.
#[allow(missing_debug_implementations)]
pub struct RunningFederationServices {
    tasks: FederationTaskGroup,
    state: MintState,
}

impl RunningFederationServices {
    /// Resolve when a worker fails, so public listeners can stop serving.
    pub fn stopped(&self) -> impl Future<Output = ()> + Send + 'static {
        self.tasks.stop.clone().cancelled_owned()
    }

    /// Stop scheduling, finish in-flight work, then join every owned task.
    pub async fn shutdown(self) -> Result<(), cdk::Error> {
        let outcome = self.tasks.shutdown().await;
        super::aleph_bft_session::stop_current_federation_aleph_bft_session_for_catch_up(
            &self.state,
        )
        .await;
        outcome
    }
}

impl Drop for RunningFederationServices {
    fn drop(&mut self) {
        self.tasks.stop.cancel();
        self.tasks.tracker.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_drains_workers_and_child_tasks() {
        for _ in 0..3 {
            let tasks = FederationTaskGroup::default();
            let stop = tasks.stop_token();
            tasks.spawn("waiting", true, async move { stop.cancelled().await });
            let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let child_finished = finished.clone();
            tasks.spawn("quote", false, async move {
                tokio::task::yield_now().await;
                child_finished.store(true, std::sync::atomic::Ordering::SeqCst);
            });
            tasks.shutdown().await.expect("clean shutdown");
            assert!(finished.load(std::sync::atomic::Ordering::SeqCst));
            assert_eq!(tasks.tracker.len(), 0);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_aborts_stalled_work_after_the_grace_period() {
        let tasks = FederationTaskGroup::default();
        tasks.spawn("stalled", true, std::future::pending());
        let started = tokio::time::Instant::now();
        assert!(tasks.shutdown().await.is_err());
        assert_eq!(started.elapsed(), SHUTDOWN_GRACE);
        assert_eq!(tasks.tracker.len(), 0);
    }

    #[tokio::test]
    async fn explicit_worker_failure_is_observable_and_preserves_the_first_reason() {
        let tasks = FederationTaskGroup::default();
        tasks.fail("DKG failed".to_string());
        tasks.fail("another failure".to_string());
        assert!(tasks.stop_token().is_cancelled());
        let error = tasks.shutdown().await.expect_err("worker failure");
        assert!(error.to_string().contains("DKG failed"));
    }

    #[tokio::test]
    async fn worker_panic_is_observable_and_stops_siblings() {
        let tasks = FederationTaskGroup::default();
        let stop = tasks.stop_token();
        tasks.spawn("failed", true, async { panic!("injected worker failure") });
        tokio::time::timeout(Duration::from_secs(1), stop.cancelled())
            .await
            .expect("failure wake");
        assert!(tasks.shutdown().await.is_err());
        assert_eq!(tasks.tracker.len(), 0);
    }
}
