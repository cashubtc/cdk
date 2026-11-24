//! Fault points in a live AlephBFT session.

use std::fmt;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tokio::sync::Notify;

/// Points where a live AlephBFT session can be made to fail.
///
/// Production sessions use [`NoAlephBftSessionFaults`]. Tests inject failures
/// through their own implementation rather than through test-only fields on
/// the session options.
pub trait AlephBftSessionFaults: fmt::Debug + Send + Sync {
    /// Fail session startup before any task is spawned.
    fn fail_before_task_spawn(&self) -> bool {
        false
    }

    /// Panic inside the root session task once it is activated.
    fn panic_session_task(&self) -> bool {
        false
    }

    /// Panic inside the network pump task once it is activated.
    fn panic_network_pump_task(&self) -> bool {
        false
    }

    /// Fail applying the first finalized payload.
    fn fail_finalized_payload_apply(&self) -> bool {
        false
    }

    /// Pause just before the session is published: notify the first, then
    /// wait for the second.
    fn publication_pause(&self) -> Option<(Arc<Notify>, Arc<Notify>)> {
        None
    }

    /// Flag to set once the root session task is activated.
    fn root_task_activation_observer(&self) -> Option<Arc<AtomicBool>> {
        None
    }

    /// Faults for the session that the next automatic rotation starts.
    fn after_automatic_rotation(&self) -> Arc<dyn AlephBftSessionFaults> {
        Arc::new(NoAlephBftSessionFaults)
    }
}

/// The production session: no injected faults.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAlephBftSessionFaults;

impl AlephBftSessionFaults for NoAlephBftSessionFaults {}

/// Configurable injected faults for tests.
#[cfg(any(test, feature = "test-utils"))]
#[derive(Debug, Clone, Default)]
pub struct TestAlephBftSessionFaults {
    fail_before_task_spawn: bool,
    panic_session_task: bool,
    panic_network_pump_task: bool,
    fail_finalized_payload_apply: bool,
    fail_automatic_rotation_start: bool,
    publication_pause: Option<(Arc<Notify>, Arc<Notify>)>,
    root_task_activation_observer: Option<Arc<AtomicBool>>,
}

#[cfg(any(test, feature = "test-utils"))]
impl TestAlephBftSessionFaults {
    /// Fail startup before any task is spawned.
    #[must_use]
    pub fn with_pre_spawn_failure(mut self) -> Self {
        self.fail_before_task_spawn = true;
        self
    }

    /// Panic inside the root session task.
    #[must_use]
    pub fn with_panicking_session_task(mut self) -> Self {
        self.panic_session_task = true;
        self
    }

    /// Panic inside the network pump task.
    #[must_use]
    pub fn with_panicking_network_pump_task(mut self) -> Self {
        self.panic_network_pump_task = true;
        self
    }

    /// Fail applying the first finalized payload.
    #[must_use]
    pub fn with_finalized_payload_apply_failure(mut self) -> Self {
        self.fail_finalized_payload_apply = true;
        self
    }

    /// Fail startup of the session the next automatic rotation starts.
    #[must_use]
    pub fn with_automatic_rotation_start_failure(mut self) -> Self {
        self.fail_automatic_rotation_start = true;
        self
    }

    /// Pause just before publication.
    #[must_use]
    pub fn with_publication_pause(mut self, reached: Arc<Notify>, release: Arc<Notify>) -> Self {
        self.publication_pause = Some((reached, release));
        self
    }

    /// Set `observer` once the root session task is activated.
    #[must_use]
    pub fn with_root_task_activation_observer(mut self, observer: Arc<AtomicBool>) -> Self {
        self.root_task_activation_observer = Some(observer);
        self
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl AlephBftSessionFaults for TestAlephBftSessionFaults {
    fn fail_before_task_spawn(&self) -> bool {
        self.fail_before_task_spawn
    }

    fn panic_session_task(&self) -> bool {
        self.panic_session_task
    }

    fn panic_network_pump_task(&self) -> bool {
        self.panic_network_pump_task
    }

    fn fail_finalized_payload_apply(&self) -> bool {
        self.fail_finalized_payload_apply
    }

    fn publication_pause(&self) -> Option<(Arc<Notify>, Arc<Notify>)> {
        self.publication_pause.clone()
    }

    fn root_task_activation_observer(&self) -> Option<Arc<AtomicBool>> {
        self.root_task_activation_observer.clone()
    }

    /// The rotated session fails to start when rotation start failure was
    /// requested; task panics and apply failure carry over; the publication
    /// pause and activation observer belong to the first session only.
    fn after_automatic_rotation(&self) -> Arc<dyn AlephBftSessionFaults> {
        Arc::new(Self {
            fail_before_task_spawn: self.fail_automatic_rotation_start,
            fail_automatic_rotation_start: false,
            publication_pause: None,
            root_task_activation_observer: None,
            ..self.clone()
        })
    }
}
