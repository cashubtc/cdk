//! Cashu Mint

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::time::Duration;

use arc_swap::ArcSwap;
use cdk_common::common::{PaymentProcessorKey, QuoteTTL};
use cdk_common::database::mint::Acquired;
use cdk_common::database::{self, DynMintAuthDatabase, DynMintDatabase};
use cdk_common::nuts::{BlindSignature, BlindedMessage, CurrencyUnit, Id};
use cdk_common::payment::{DynMintPayment, WaitPaymentResponse};
pub use cdk_common::quote_id::QuoteId;
use cdk_common::stream::{BackoffPolicy, SupervisedStream};
#[cfg(feature = "prometheus")]
use cdk_prometheus::MintMetricGuard;
use cdk_signatory::signatory::{Signatory, SignatoryKeySet, SignatoryKeysets};
use futures::{Stream, StreamExt};
use nut21::ProtectedEndpoint;
use subscription::PubSubManager;
use tokio::sync::{mpsc, watch, Mutex, Notify};
use tokio::task::{JoinHandle, JoinSet};
use tracing::instrument;

use crate::error::Error;
use crate::fees::calculate_fee;
use crate::nuts::*;
use crate::{Amount, OidcClient};

pub(crate) mod auth;
mod builder;
mod check_spendable;
mod issue;
mod keysets;
mod melt;
mod payment_backend;
mod proofs;
mod saga_recovery;
mod start_up_check;
mod subscription;
mod swap;
mod verification;

pub use builder::{KeysetRotation, MintBuilder, MintMeltLimits, UnitConfig};
pub use cdk_common::mint::{MeltQuote, MintKeySetInfo, MintQuote};
pub use cdk_common::mint_quote::{MintQuoteRequest, MintQuoteResponse};
pub use issue::MintInput;
pub use melt::PendingMelt;
pub use verification::Verification;

const CDK_MINT_PRIMARY_NAMESPACE: &str = "cdk_mint";
const CDK_MINT_CONFIG_SECONDARY_NAMESPACE: &str = "config";
const CDK_MINT_CONFIG_KV_KEY: &str = "mint_info";
const CDK_MINT_QUOTE_TTL_KV_KEY: &str = "quote_ttl";

/// Cashu Mint
#[derive(Clone)]
pub struct Mint {
    /// Signatory backend.
    ///
    /// It is implemented in the cdk-signatory crate, and it can be embedded in the mint or it can
    /// be a gRPC client to a remote signatory server.
    signatory: Arc<dyn Signatory + Send + Sync>,
    /// Mint Storage backend
    localstore: DynMintDatabase,
    /// Auth Storage backend (only available with auth feature)
    auth_localstore: Option<DynMintAuthDatabase>,
    /// Payment processors for mint, keyed by (unit, method).
    ///
    /// Swappable so `register_payment_processor`/`deregister_payment_processor`
    /// can add or remove entries on a running mint; reads are a lock-free `.load()`.
    payment_processors: Arc<ArcSwap<HashMap<PaymentProcessorKey, DynMintPayment>>>,
    /// Serializes `register_payment_processor`/`deregister_payment_processor`
    /// read-modify-write cycles, mirroring `keyset_store_lock`.
    payment_processor_lock: Arc<Mutex<()>>,
    /// Subscription manager
    pubsub_manager: Arc<PubSubManager>,
    oidc_client: Option<OidcClient>,
    /// In-memory keyset
    keysets: Arc<ArcSwap<Vec<SignatoryKeySet>>>,
    /// Serializes writes to `keysets`.
    ///
    /// Both a mint-initiated `rotate_keyset` and the signatory subscription
    /// drain task replace the snapshot. Holding this lock across "read the
    /// freshest signatory snapshot, then store it" makes the last write always
    /// the newest one, so a stale snapshot can never overwrite a newer one.
    keyset_store_lock: Arc<Mutex<()>>,
    /// Serializes live melt dispatch and reconciliation for each quote within
    /// this mint process.
    ///
    /// The database remains the source of truth. This guard only closes the
    /// single-instance window where a status check could observe `Unpaid`,
    /// compensate the saga, and then let an already-running task dispatch the
    /// payment. Weak entries avoid retaining every quote id for the lifetime
    /// of the mint.
    melt_quote_locks: Arc<Mutex<HashMap<QuoteId, Weak<Mutex<()>>>>>,
    /// Background task management
    task_state: Arc<Mutex<TaskState>>,
    /// Maximum number of inputs allowed per transaction
    max_inputs: usize,
    /// Maximum number of outputs allowed per transaction
    max_outputs: usize,
}

impl std::fmt::Debug for Mint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mint").finish_non_exhaustive()
    }
}

/// State for managing background tasks
#[derive(Default)]
struct TaskState {
    /// Shutdown signal for all background tasks
    shutdown_notify: Option<Arc<Notify>>,
    /// Handle to the main supervisor task
    supervisor_handle: Option<JoinHandle<Result<(), Error>>>,
    /// Handle to the keyset drain task
    keyset_drain_handle: Option<JoinHandle<()>>,
    /// Keyset subscription retained from construction, drained once by the first
    /// `start()`. `None` after it has been taken; a restart re-subscribes.
    keyset_updates: Option<watch::Receiver<SignatoryKeysets>>,
    /// Feeds processors registered after `start()` to the running consumer
    /// supervisor. `None` when the mint isn't started; may go stale (receiver
    /// already dropped) right after `stop()`, which the send side treats as
    /// "no running supervisor to notify" rather than an error.
    new_processor_tx: Option<mpsc::Sender<DynMintPayment>>,
}

/// Supervised subscription to a single payment processor's event stream.
///
/// Holds the state the handlers need as fields, so `on_message` reads
/// `&self.field` instead of the closure-based supervisor's per-item cloning.
struct PaymentWaiter {
    /// Identifies the processor in the supervisor's logs (unit and method).
    name: String,
    mint: Arc<Mint>,
    processor: DynMintPayment,
    localstore: DynMintDatabase,
    pubsub_manager: Arc<PubSubManager>,
}

#[async_trait::async_trait]
impl SupervisedStream for PaymentWaiter {
    type Item = cdk_common::payment::Event;
    // `DynMintPayment::wait_payment_event` fails with `Error`; its stream is
    // already decoded and never yields a stream-level error, so wrap items in
    // `Ok` with an `Infallible` error to satisfy the `Result` item contract.
    type ConnectError = Error;
    type StreamError = std::convert::Infallible;
    type Stream = Pin<Box<dyn Stream<Item = Result<Self::Item, Self::StreamError>> + Send>>;

    fn name(&self) -> &str {
        &self.name
    }

    fn backoff_policy(&self) -> BackoffPolicy {
        // Keep `initial` short so payment-detection latency stays low when a
        // healthy backend cycles its stream (that fixed floor is the wait after
        // every close); the cap only throttles a backend that keeps refusing
        // connections. See `BackoffPolicy` for the full semantics.
        BackoffPolicy {
            initial_connect_backoff: Duration::from_millis(250),
            max_connect_backoff: Duration::from_secs(30),
        }
    }

    async fn connect(&mut self) -> Result<Self::Stream, Error> {
        let stream = self.processor.wait_payment_event().await?;
        Ok(Box::pin(stream.map(Ok::<_, std::convert::Infallible>)))
    }

    async fn on_message(&mut self, event: Self::Item) {
        match event {
            cdk_common::payment::Event::PaymentReceived(wait_payment_response) => {
                if let Err(e) = Mint::handle_payment_notification(
                    &self.localstore,
                    &self.pubsub_manager,
                    wait_payment_response,
                )
                .await
                {
                    tracing::warn!("Payment notification error: {:?}", e);
                }
            }
            cdk_common::payment::Event::PaymentSuccessful { quote_id, details } => {
                tracing::info!(
                    "Outgoing payment confirmed for quote {}: status {}",
                    quote_id,
                    details.status,
                );

                if let Err(e) = Mint::handle_successful_melt_payment_event(
                    &self.mint,
                    &self.localstore,
                    &self.pubsub_manager,
                    &quote_id,
                    details,
                )
                .await
                {
                    tracing::warn!(
                        "Failed to process successful payment event for quote {}: {}",
                        quote_id,
                        e
                    );
                }
            }
            cdk_common::payment::Event::PaymentFailed { quote_id, reason } => {
                tracing::warn!("Outgoing payment failed for quote {}: {}", quote_id, reason,);

                if let Err(e) = Mint::handle_failed_melt_payment_event(
                    &self.mint,
                    &self.localstore,
                    &self.pubsub_manager,
                    &quote_id,
                )
                .await
                {
                    tracing::warn!(
                        "Failed to process failed payment event for quote {}: {}",
                        quote_id,
                        e
                    );
                }
            }
        }
    }

    async fn on_shutdown(&mut self) {
        // The loop stopped because shutdown fired; run the teardown the old
        // inner and outer shutdown arms performed.
        self.processor.cancel_payment_event_stream();
    }
}

impl Mint {
    /// Returns the process-local coordination lock for a melt quote.
    pub(super) async fn melt_quote_lock(&self, quote_id: &QuoteId) -> Arc<Mutex<()>> {
        let mut locks = self.melt_quote_locks.lock().await;

        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(quote_id).and_then(Weak::upgrade) {
            return lock;
        }

        let lock = Arc::new(Mutex::new(()));
        locks.insert(quote_id.clone(), Arc::downgrade(&lock));
        lock
    }

    /// Create new [`Mint`] without authentication
    pub async fn new(
        mint_info: MintInfo,
        signatory: Arc<dyn Signatory + Send + Sync>,
        localstore: DynMintDatabase,
        payment_processors: HashMap<PaymentProcessorKey, DynMintPayment>,
        max_inputs: usize,
        max_outputs: usize,
    ) -> Result<Self, Error> {
        Self::new_internal(
            mint_info,
            signatory,
            localstore,
            None,
            payment_processors,
            max_inputs,
            max_outputs,
        )
        .await
    }

    /// Create new [`Mint`] with authentication support
    pub async fn new_with_auth(
        mint_info: MintInfo,
        signatory: Arc<dyn Signatory + Send + Sync>,
        localstore: DynMintDatabase,
        auth_localstore: DynMintAuthDatabase,
        payment_processors: HashMap<PaymentProcessorKey, DynMintPayment>,
        max_inputs: usize,
        max_outputs: usize,
    ) -> Result<Self, Error> {
        Self::new_internal(
            mint_info,
            signatory,
            localstore,
            Some(auth_localstore),
            payment_processors,
            max_inputs,
            max_outputs,
        )
        .await
    }

    /// Internal function to create a new [`Mint`] with shared logic
    #[inline]
    async fn new_internal(
        mint_info: MintInfo,
        signatory: Arc<dyn Signatory + Send + Sync>,
        localstore: DynMintDatabase,
        auth_localstore: Option<DynMintAuthDatabase>,
        payment_processors: HashMap<PaymentProcessorKey, DynMintPayment>,
        max_inputs: usize,
        max_outputs: usize,
    ) -> Result<Self, Error> {
        // Subscribe up front and bootstrap the in-memory snapshot from the same
        // receiver that keeps it fresh. `borrow_and_update` pins the receiver
        // cursor to this snapshot, so any signatory rotation that lands before
        // `start()` spawns the drain task makes the loop's first `changed()`
        // return immediately instead of being silently skipped.
        let mut keyset_updates = signatory.subscribe_keysets().await?;
        let keysets = keyset_updates.borrow_and_update().clone();
        let active_keys = keysets
            .keysets
            .iter()
            .filter(|keyset| keyset.active && keyset.unit != CurrencyUnit::Auth)
            .count();
        // The signatory may not report an active keyset yet (for example a
        // remote signatory that is still connecting). Start anyway and let the
        // keyset drain task install keysets if they arrive over the
        // subscription; endpoints return keyset errors until then.
        if active_keys == 0 {
            tracing::warn!(
                "Signatory {} has no active keysets yet; starting without them",
                signatory.name(),
            );
        } else {
            tracing::info!(
                "Using Signatory {} with {} active keys",
                signatory.name(),
                active_keys,
            );
        }

        // Persist missing pubkey early to avoid losing it on next boot and ensure stable identity across restarts
        let mut computed_info = mint_info;
        if computed_info.pubkey.is_none() {
            computed_info.pubkey = Some(keysets.pubkey);
        }

        match localstore
            .kv_read(
                CDK_MINT_PRIMARY_NAMESPACE,
                CDK_MINT_CONFIG_SECONDARY_NAMESPACE,
                CDK_MINT_CONFIG_KV_KEY,
            )
            .await?
        {
            Some(bytes) => {
                let mut stored: MintInfo = serde_json::from_slice(&bytes)?;
                let mut mutated = false;
                if stored.pubkey.is_none() && computed_info.pubkey.is_some() {
                    stored.pubkey = computed_info.pubkey;
                    mutated = true;
                }

                // Merge auth settings from computed_info if stored doesn't have them
                // Protected endpoints will be populated dynamically from auth database
                {
                    if stored.nuts.nut21.is_none() && computed_info.nuts.nut21.is_some() {
                        stored.nuts.nut21 = computed_info.nuts.nut21.clone();
                        mutated = true;
                    }
                    if stored.nuts.nut22.is_none() && computed_info.nuts.nut22.is_some() {
                        stored.nuts.nut22 = computed_info.nuts.nut22.clone();
                        mutated = true;
                    }
                }

                if mutated {
                    let updated = serde_json::to_vec(&stored)?;
                    let mut tx = localstore.begin_transaction().await?;
                    tx.kv_write(
                        CDK_MINT_PRIMARY_NAMESPACE,
                        CDK_MINT_CONFIG_SECONDARY_NAMESPACE,
                        CDK_MINT_CONFIG_KV_KEY,
                        &updated,
                    )
                    .await?;
                    tx.commit().await?;
                }
            }
            None => {
                let bytes = serde_json::to_vec(&computed_info)?;
                let mut tx = localstore.begin_transaction().await?;
                tx.kv_write(
                    CDK_MINT_PRIMARY_NAMESPACE,
                    CDK_MINT_CONFIG_SECONDARY_NAMESPACE,
                    CDK_MINT_CONFIG_KV_KEY,
                    &bytes,
                )
                .await?;
                tx.commit().await?;
            }
        }

        let payment_processors = Arc::new(ArcSwap::new(Arc::new(payment_processors)));

        Ok(Self {
            signatory,
            pubsub_manager: PubSubManager::new((localstore.clone(), payment_processors.clone())),
            localstore,
            oidc_client: computed_info.nuts.nut21.as_ref().map(|nut21| {
                OidcClient::new(
                    nut21.openid_discovery.clone(),
                    Some(nut21.client_id.clone()),
                )
            }),
            payment_processors,
            payment_processor_lock: Arc::new(Mutex::new(())),
            auth_localstore,
            keysets: Arc::new(ArcSwap::new(keysets.keysets.into())),
            keyset_store_lock: Arc::new(Mutex::new(())),
            melt_quote_locks: Arc::new(Mutex::new(HashMap::new())),
            task_state: Arc::new(Mutex::new(TaskState {
                keyset_updates: Some(keyset_updates),
                ..Default::default()
            })),
            max_inputs,
            max_outputs,
        })
    }

    /// Start the mint's background services and operations
    ///
    /// This function immediately starts background services and returns. The background
    /// tasks will continue running until `stop()` is called.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if background services started successfully, or an `Error`
    /// if startup failed.
    ///
    /// # Background Services
    ///
    /// Currently manages:
    /// - Payment processor initialization and startup
    /// - Invoice payment monitoring across all configured payment processors
    pub async fn start(&self) -> Result<(), Error> {
        // Recover from incomplete swap sagas
        // This cleans up incomplete swap operations using persisted saga state
        if let Err(e) = self.recover_from_incomplete_sagas().await {
            tracing::error!("Failed to recover incomplete swap sagas: {}", e);
            // Don't fail startup
        }

        // Recover from incomplete melt sagas
        // This cleans up incomplete melt operations using persisted saga state
        // Now includes checking payment status with the payment backend to determine
        // whether to finalize (if paid) or compensate (if failed/unpaid)
        if let Err(e) = self.recover_from_incomplete_melt_sagas().await {
            tracing::error!("Failed to recover incomplete melt sagas: {}", e);
            // Don't fail startup
        }

        let mut task_state = self.task_state.lock().await;

        // Prevent starting if already running
        if task_state.shutdown_notify.is_some() {
            return Err(Error::Internal); // Already started
        }

        // Start all payment processors first
        tracing::info!("Starting payment processors...");
        let payment_processors = self.payment_processors.load_full();
        let mut seen_processors = Vec::new();
        for (key, processor) in payment_processors.iter() {
            // Skip if we've already spawned a task for this processor instance
            if seen_processors.iter().any(|p| Arc::ptr_eq(p, processor)) {
                continue;
            }

            seen_processors.push(Arc::clone(processor));

            tracing::info!("Starting payment wait task for {:?}", key);

            match processor.start().await {
                Ok(()) => {
                    tracing::debug!("Successfully started payment processor for {:?}", key);
                }
                Err(e) => {
                    // Log the error but continue with other processors
                    tracing::error!("Failed to start payment processor for {:?}: {}", key, e);
                    return Err(e.into());
                }
            }
        }

        tracing::info!("Payment processor startup completed");

        // Create shutdown signal
        let shutdown_notify = Arc::new(Notify::new());

        // Channel for processors registered after this point; the supervisor
        // task drains it to spawn consumers for units added while running.
        let (new_processor_tx, new_processor_rx) = mpsc::channel::<DynMintPayment>(16);

        // Clone required components for the background task
        let mint_clone = Arc::new(self.clone());
        let localstore = Arc::clone(&self.localstore);
        let pubsub_manager = Arc::clone(&self.pubsub_manager);
        let shutdown_clone = shutdown_notify.clone();

        // Spawn the supervisor task
        let supervisor_handle = tokio::spawn(async move {
            Self::wait_for_paid_invoices(
                mint_clone,
                payment_processors,
                new_processor_rx,
                localstore,
                pubsub_manager,
                shutdown_clone,
            )
            .await
        });

        // Keyset refresh: drain signatory keyset updates into the in-memory
        // keysets. A signatory-side rotation reaches the mint here without a
        // restart or a mint-initiated rotate.
        //
        // The receiver is normally the one retained from construction, whose
        // cursor is pinned to the bootstrapped snapshot. On a restart after
        // `stop()` that receiver was already consumed, so re-subscribe. A fresh
        // `subscribe()` marks the current value as seen, so the re-subscribe
        // branch applies the current snapshot immediately before waiting for the
        // next change; the retained branch does not, since construction already
        // seeded the same snapshot into the ArcSwap.
        let keyset_updates = match task_state.keyset_updates.take() {
            Some(rx) => Some(rx),
            None => match self.signatory.subscribe_keysets().await {
                Ok(mut rx) => {
                    let _store = self.keyset_store_lock.lock().await;
                    let current = rx.borrow_and_update().keysets.clone();
                    if !current.is_empty() {
                        self.keysets.store(Arc::new(current));
                    }
                    Some(rx)
                }
                Err(err) => {
                    tracing::warn!("Could not subscribe to signatory keyset updates: {}", err);
                    None
                }
            },
        };

        let keyset_drain_handle = if let Some(mut keyset_updates) = keyset_updates {
            let keysets = self.keysets.clone();
            let keyset_store_lock = Arc::clone(&self.keyset_store_lock);
            let shutdown = shutdown_notify.clone();
            Some(tokio::spawn(async move {
                // Register the shutdown waiter once and hold it across
                // iterations. A fresh `shutdown.notified()` per loop would be
                // dropped whenever the `changed` branch wins the select,
                // deregistering the waiter for the duration of the handler. A
                // `notify_waiters()` from `stop()` in that window buffers
                // nothing, so it would be lost and the next `notified()` would
                // wait forever, hanging `stop()` on the drain handle. A single
                // pinned future stays registered, so shutdown is never missed.
                let shutdown_wait = shutdown.notified();
                tokio::pin!(shutdown_wait);
                loop {
                    tokio::select! {
                        _ = &mut shutdown_wait => break,
                        changed = keyset_updates.changed() => {
                            if changed.is_err() {
                                // Signatory dropped the sender; stop draining.
                                break;
                            }
                            // Serialize with mint-initiated rotations: read the
                            // freshest snapshot and store it under the lock, so a
                            // concurrent rotate cannot land a stale snapshot after
                            // this newer one.
                            let _store = keyset_store_lock.lock().await;
                            let updated =
                                keyset_updates.borrow_and_update().keysets.clone();
                            if updated.is_empty() {
                                continue;
                            }
                            keysets.store(Arc::new(updated));
                        }
                    }
                }
            }))
        } else {
            None
        };

        // Store the handles
        task_state.shutdown_notify = Some(shutdown_notify);
        task_state.supervisor_handle = Some(supervisor_handle);
        task_state.keyset_drain_handle = keyset_drain_handle;
        task_state.new_processor_tx = Some(new_processor_tx);

        // Give the background task a tiny bit of time to start waiting
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        tracing::info!("Mint background services started");
        Ok(())
    }

    /// Stop all background services and wait for graceful shutdown
    ///
    /// This function signals all background tasks to shut down and waits for them
    /// to complete gracefully. It's safe to call multiple times.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` when all background services have shut down cleanly, or an
    /// `Error` if there was an issue during shutdown.
    pub async fn stop(&self) -> Result<(), Error> {
        let mut task_state = self.task_state.lock().await;

        // Take the handles out of the state
        let shutdown_notify = task_state.shutdown_notify.take();
        let supervisor_handle = task_state.supervisor_handle.take();
        let keyset_drain_handle = task_state.keyset_drain_handle.take();

        // If nothing to stop, return early
        let (shutdown_notify, supervisor_handle) = match (shutdown_notify, supervisor_handle) {
            (Some(notify), Some(handle)) => (notify, handle),
            _ => {
                tracing::debug!("Stop called but no background services were running");
                // Still try to stop payment processors
                return self.stop_payment_processors().await;
            }
        };

        // Drop the lock before waiting
        drop(task_state);

        tracing::info!("Stopping mint background services...");

        // Signal shutdown
        shutdown_notify.notify_waiters();

        // Wait for supervisor to complete
        let result = match supervisor_handle.await {
            Ok(result) => {
                tracing::info!("Mint background services stopped");
                result
            }
            Err(join_error) => {
                tracing::error!("Background service task panicked: {:?}", join_error);
                Err(Error::Internal)
            }
        };

        // Wait for the keyset drain task to complete
        if let Some(handle) = keyset_drain_handle {
            if let Err(join_error) = handle.await {
                tracing::error!("Keyset drain task panicked: {:?}", join_error);
            }
        }

        // Stop all payment processors
        self.stop_payment_processors().await?;

        result
    }

    /// Stop all payment processors
    async fn stop_payment_processors(&self) -> Result<(), Error> {
        tracing::info!("Stopping payment processors...");
        let mut seen_processors = Vec::new();
        let payment_processors = self.payment_processors.load_full();

        for (key, processor) in payment_processors.iter() {
            // Skip if we've already spawned a task for this processor instance
            if seen_processors.iter().any(|p| Arc::ptr_eq(p, processor)) {
                continue;
            }

            seen_processors.push(Arc::clone(processor));

            match processor.stop().await {
                Ok(()) => {
                    tracing::debug!("Successfully stopped payment processor for {:?}", key);
                }
                Err(e) => {
                    // Log the error but continue with other processors
                    tracing::error!("Failed to stop payment processor for {:?}: {}", key, e);
                }
            }
        }
        tracing::info!("Payment processor shutdown completed");
        Ok(())
    }

    /// Get all custom payment methods supported by registered payment processors
    ///
    /// This queries all payment processors for their supported custom methods
    /// and returns a deduplicated list.
    pub async fn get_custom_payment_methods(&self) -> Result<Vec<String>, Error> {
        use std::collections::HashSet;
        let mut custom_methods = HashSet::new();
        let mut seen_processors = Vec::new();
        let payment_processors = self.payment_processors.load_full();

        for processor in payment_processors.values() {
            // Skip if we've already queried this processor instance
            if seen_processors.iter().any(|p| Arc::ptr_eq(p, processor)) {
                continue;
            }
            seen_processors.push(Arc::clone(processor));

            match processor.get_settings().await {
                Ok(settings) => {
                    for (method, _) in settings.custom {
                        custom_methods.insert(method);
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to get settings from payment processor: {}", e);
                }
            }
        }

        Ok(custom_methods.into_iter().collect())
    }

    /// Get the payment processor for the given unit and payment method
    pub fn get_payment_processor(
        &self,
        unit: CurrencyUnit,
        payment_method: PaymentMethod,
    ) -> Result<DynMintPayment, Error> {
        let key = PaymentProcessorKey::new(unit.clone(), payment_method.clone());
        self.payment_processors
            .load()
            .get(&key)
            .cloned()
            .ok_or_else(|| {
                tracing::info!(
                    "No payment processor set for pair {}, {}",
                    unit,
                    payment_method
                );
                Error::UnsupportedUnit
            })
    }

    /// Registers a payment processor for `(unit, method)` on a running mint.
    ///
    /// Starts the processor if it isn't already running under another key,
    /// spawns its payment-event consumer if the mint is started, and updates
    /// stored mint info (NUT-04/NUT-05 method settings, and NUT-15 if the
    /// backend advertises MPP) from `limits` and the processor's reported
    /// settings. A duplicate `(unit, method)` already live in the
    /// in-memory map is rejected, mirroring
    /// [`MintBuilder::add_payment_processor`]; settings already persisted
    /// for the key from an earlier process are replaced rather than
    /// accumulated, so restarting with the same registration is idempotent.
    ///
    /// Updates the map first and the stored mint info last: the map entry is
    /// inert without the mint-info gate, so a crash between the two phases
    /// leaves the mint still refusing quotes for the unit (fail-closed).
    ///
    /// Hot-swapping the processor behind an already-registered key is not
    /// supported; deregister then register instead.
    pub async fn register_payment_processor(
        &self,
        unit: CurrencyUnit,
        method: PaymentMethod,
        limits: MintMeltLimits,
        processor: DynMintPayment,
    ) -> Result<(), Error> {
        let key = PaymentProcessorKey::new(unit.clone(), method.clone());
        let _guard = self.payment_processor_lock.lock().await;

        let current = self.payment_processors.load_full();
        if current.contains_key(&key) {
            return Err(Error::Custom(format!(
                "Duplicate payment processor for unit {} and method {}",
                unit, method
            )));
        }

        // Start the backend and (if running) notify the consumer supervisor
        // only for a genuinely new instance; one already live under another
        // key keeps running, exactly as the boot-time dedup does.
        let already_running = current.values().any(|p| Arc::ptr_eq(p, &processor));
        if !already_running {
            processor.start().await?;
        }

        // Phase 1 (first): the map entry.
        self.insert_payment_processor(key.clone(), processor.clone());

        if !already_running {
            let new_processor_tx = {
                let task_state = self.task_state.lock().await;
                task_state.new_processor_tx.clone()
            };
            if let Some(tx) = new_processor_tx {
                if tx.send(processor.clone()).await.is_err() {
                    tracing::debug!(
                        "No running consumer supervisor to notify for {:?}; \
                         the next start() will pick it up",
                        key
                    );
                }
            }
        }

        // Phase 2 (last): the quotability gate. Clear any entries already
        // persisted for this key first: the in-memory map is empty on every
        // fresh process, so it cannot tell a genuine re-register from the
        // settings an earlier process already wrote, and a bare push would
        // accumulate a duplicate on every restart.
        let settings = processor.get_settings().await?;
        let computed = builder::processor_method_settings(&unit, &method, &limits, &settings);
        let mut mint_info = self.mint_info().await?;
        Self::remove_quotability_settings(&mut mint_info, &key);
        if let Some(mpp_settings) = computed.mpp {
            mint_info.nuts.nut15.methods.push(mpp_settings);
        }
        if let Some(mint_settings) = computed.mint {
            mint_info.nuts.nut04.methods.push(mint_settings);
            mint_info.nuts.nut04.disabled = false;
        }
        if let Some(melt_settings) = computed.melt {
            mint_info.nuts.nut05.methods.push(melt_settings);
            mint_info.nuts.nut05.disabled = false;
        }
        self.set_mint_info(mint_info).await?;

        Ok(())
    }

    /// Removes a `(unit, method)` registration from a running mint: full
    /// teardown. Removes the NUT-04/NUT-05 (and matching NUT-15) entries
    /// first, so no new quote can be created for the pair, then removes the
    /// map entry too and stops the processor if no other key still
    /// references the same instance. A crash before the map entry is
    /// removed leaves it inert (closed gate, orphaned map entry), which is
    /// harmless. Deregistering a `(unit, method)` that was never registered
    /// is a no-op.
    ///
    /// Because the map entry is gone, a quote whose method re-consults the
    /// processor on every check (every method except bolt11, which only
    /// consults it before reaching `Paid`/`Issued`) can no longer be
    /// resolved: an already-paid, not-yet-issued quote of such a method
    /// becomes permanently unmintable once its processor is deregistered.
    /// Use [`retire_payment_processor`](Self::retire_payment_processor)
    /// instead when outstanding quotes for the pair must still resolve.
    pub async fn deregister_payment_processor(
        &self,
        unit: CurrencyUnit,
        method: PaymentMethod,
    ) -> Result<(), Error> {
        let key = PaymentProcessorKey::new(unit, method);
        let _guard = self.payment_processor_lock.lock().await;

        // Phase 1 (first): close the quotability gate.
        let mut mint_info = self.mint_info().await?;
        Self::remove_quotability_settings(&mut mint_info, &key);
        self.set_mint_info(mint_info).await?;

        // Phase 2 (last): the map entry, and the backend if orphaned.
        if let Some(processor) = self.remove_payment_processor(&key) {
            let still_referenced = self
                .payment_processors
                .load()
                .values()
                .any(|p| Arc::ptr_eq(p, &processor));
            if !still_referenced {
                if let Err(e) = processor.stop().await {
                    tracing::error!("Failed to stop payment processor for {:?}: {}", key, e);
                }
            }
        }

        Ok(())
    }

    /// Closes the quotability gate for `(unit, method)` without tearing
    /// down the processor: removes the NUT-04/NUT-05 (and matching NUT-15)
    /// entries, so no new quote can be created, but deliberately leaves the
    /// map entry in place. check_mint_quote_payments and the mint path keep
    /// resolving the processor through the (unchanged) map, so a quote
    /// already paid but not yet issued stays checkable and mintable for
    /// every payment method, including the ones deregister would strand.
    /// The processor itself is untouched: not stopped, and its
    /// start/stop reference count is not adjusted. Retiring a pair that is
    /// unregistered, or already retired, is a no-op.
    pub async fn retire_payment_processor(
        &self,
        unit: CurrencyUnit,
        method: PaymentMethod,
    ) -> Result<(), Error> {
        let key = PaymentProcessorKey::new(unit, method);
        let _guard = self.payment_processor_lock.lock().await;

        let mut mint_info = self.mint_info().await?;
        Self::remove_quotability_settings(&mut mint_info, &key);
        self.set_mint_info(mint_info).await?;

        Ok(())
    }

    /// Removes the NUT-04/NUT-05 (and matching NUT-15) entries for `key`
    /// from `mint_info`. Shared by `deregister_payment_processor` and
    /// `retire_payment_processor`, which differ only in what they then do
    /// with the map entry.
    fn remove_quotability_settings(mint_info: &mut MintInfo, key: &PaymentProcessorKey) {
        mint_info.nuts.nut04.remove_settings(&key.unit, &key.method);
        mint_info.nuts.nut05.remove_settings(&key.unit, &key.method);
        mint_info
            .nuts
            .nut15
            .methods
            .retain(|m| !(m.unit == key.unit && m.method == key.method));
    }

    /// Inserts or replaces one entry in the live payment-processor map.
    /// Caller must hold `payment_processor_lock`.
    fn insert_payment_processor(&self, key: PaymentProcessorKey, processor: DynMintPayment) {
        let mut updated = (*self.payment_processors.load_full()).clone();
        updated.insert(key, processor);
        self.payment_processors.store(Arc::new(updated));
    }

    /// Removes one entry from the live payment-processor map, if present.
    /// Caller must hold `payment_processor_lock`.
    fn remove_payment_processor(&self, key: &PaymentProcessorKey) -> Option<DynMintPayment> {
        let current = self.payment_processors.load_full();
        if !current.contains_key(key) {
            return None;
        }
        let mut updated = (*current).clone();
        let removed = updated.remove(key);
        self.payment_processors.store(Arc::new(updated));
        removed
    }

    /// Localstore
    #[inline]
    pub fn localstore(&self) -> DynMintDatabase {
        Arc::clone(&self.localstore)
    }

    /// Get the maximum number of inputs allowed per transaction
    #[inline]
    pub fn max_inputs(&self) -> usize {
        self.max_inputs
    }

    /// Pub Sub manager
    #[inline]
    pub fn pubsub_manager(&self) -> Arc<PubSubManager> {
        Arc::clone(&self.pubsub_manager)
    }

    /// Get mint info
    #[instrument(skip_all)]
    pub async fn mint_info(&self) -> Result<MintInfo, Error> {
        let mint_info = self
            .localstore
            .kv_read(
                CDK_MINT_PRIMARY_NAMESPACE,
                CDK_MINT_CONFIG_SECONDARY_NAMESPACE,
                CDK_MINT_CONFIG_KV_KEY,
            )
            .await?
            .ok_or(Error::CouldNotGetMintInfo)?;

        let mint_info: MintInfo = serde_json::from_slice(&mint_info)?;

        let mut mint_info = if let Some(auth_db) = self.auth_localstore.as_ref() {
            let mut mint_info = mint_info;
            let auth_endpoints = auth_db.get_auth_for_endpoints().await?;

            let mut clear_auth_endpoints: Vec<ProtectedEndpoint> = vec![];
            let mut blind_auth_endpoints: Vec<ProtectedEndpoint> = vec![];

            for (endpoint, auth) in auth_endpoints {
                match auth {
                    Some(AuthRequired::Clear) => {
                        clear_auth_endpoints.push(endpoint);
                    }
                    Some(AuthRequired::Blind) => {
                        blind_auth_endpoints.push(endpoint);
                    }
                    None => (),
                }
            }

            mint_info.nuts.nut21 = mint_info.nuts.nut21.map(|mut a| {
                a.protected_endpoints = clear_auth_endpoints;
                a
            });

            mint_info.nuts.nut22 = mint_info.nuts.nut22.map(|mut a| {
                a.protected_endpoints = blind_auth_endpoints;
                a
            });
            mint_info
        } else {
            mint_info
        };

        // Derived from the running limits rather than stored config, so it cannot drift from what
        // the endpoints actually enforce. One figure for both, so a wallet respecting it trips
        // neither cap.
        mint_info.max_array_length = Some(self.max_inputs.min(self.max_outputs) as u64);

        Ok(mint_info)
    }

    /// Set mint info
    #[instrument(skip_all)]
    pub async fn set_mint_info(&self, mint_info: MintInfo) -> Result<(), Error> {
        tracing::info!("Updating mint info");
        let mint_info_bytes = serde_json::to_vec(&mint_info)?;
        let mut tx = self.localstore.begin_transaction().await?;
        tx.kv_write(
            CDK_MINT_PRIMARY_NAMESPACE,
            CDK_MINT_CONFIG_SECONDARY_NAMESPACE,
            CDK_MINT_CONFIG_KV_KEY,
            &mint_info_bytes,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Set mint info and quote TTL atomically.
    #[instrument(skip_all)]
    pub async fn set_mint_info_and_quote_ttl(
        &self,
        mint_info: MintInfo,
        quote_ttl: QuoteTTL,
    ) -> Result<(), Error> {
        tracing::info!("Updating mint info and quote TTL");
        let mint_info_bytes = serde_json::to_vec(&mint_info)?;
        let quote_ttl_bytes = serde_json::to_vec(&quote_ttl)?;
        let mut tx = self.localstore.begin_transaction().await?;
        tx.kv_write(
            CDK_MINT_PRIMARY_NAMESPACE,
            CDK_MINT_CONFIG_SECONDARY_NAMESPACE,
            CDK_MINT_CONFIG_KV_KEY,
            &mint_info_bytes,
        )
        .await?;
        tx.kv_write(
            CDK_MINT_PRIMARY_NAMESPACE,
            CDK_MINT_CONFIG_SECONDARY_NAMESPACE,
            CDK_MINT_QUOTE_TTL_KV_KEY,
            &quote_ttl_bytes,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Get quote ttl
    #[instrument(skip_all)]
    pub async fn quote_ttl(&self) -> Result<QuoteTTL, Error> {
        let quote_ttl_bytes = self
            .localstore
            .kv_read(
                CDK_MINT_PRIMARY_NAMESPACE,
                CDK_MINT_CONFIG_SECONDARY_NAMESPACE,
                CDK_MINT_QUOTE_TTL_KV_KEY,
            )
            .await?;

        match quote_ttl_bytes {
            Some(bytes) => {
                let quote_ttl: QuoteTTL = serde_json::from_slice(&bytes)?;
                Ok(quote_ttl)
            }
            None => {
                // Return default if not found
                Ok(QuoteTTL::default())
            }
        }
    }

    /// Set quote ttl
    #[instrument(skip_all)]
    pub async fn set_quote_ttl(&self, quote_ttl: QuoteTTL) -> Result<(), Error> {
        let quote_ttl_bytes = serde_json::to_vec(&quote_ttl)?;
        let mut tx = self.localstore.begin_transaction().await?;
        tx.kv_write(
            CDK_MINT_PRIMARY_NAMESPACE,
            CDK_MINT_CONFIG_SECONDARY_NAMESPACE,
            CDK_MINT_QUOTE_TTL_KV_KEY,
            &quote_ttl_bytes,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// For each backend starts a task that waits for any invoice to be paid
    /// Once invoice is paid mint quote status is updated
    /// Returns true if a QuoteTTL is persisted in the database. This is used to avoid overwriting
    /// explicit configuration with defaults when the TTL has already been set by an operator.
    #[instrument(skip_all)]
    pub async fn quote_ttl_is_persisted(&self) -> Result<bool, Error> {
        let quote_ttl_bytes = self
            .localstore
            .kv_read(
                CDK_MINT_PRIMARY_NAMESPACE,
                CDK_MINT_CONFIG_SECONDARY_NAMESPACE,
                CDK_MINT_QUOTE_TTL_KV_KEY,
            )
            .await?;

        Ok(quote_ttl_bytes.is_some())
    }

    #[instrument(skip_all)]
    async fn wait_for_paid_invoices(
        mint: Arc<Mint>,
        payment_processors: Arc<HashMap<PaymentProcessorKey, DynMintPayment>>,
        mut new_processors: mpsc::Receiver<DynMintPayment>,
        localstore: DynMintDatabase,
        pubsub_manager: Arc<PubSubManager>,
        shutdown: Arc<Notify>,
    ) -> Result<(), Error> {
        let mut join_set = JoinSet::new();

        // Group processors by unique instance (using Arc pointer equality).
        // Carried through the select loop below so a later registration that
        // shares an instance already spawned here is never double-spawned.
        let mut seen_processors = Vec::new();
        for (key, processor) in payment_processors.iter() {
            // Skip if processor is already active
            if processor.is_payment_event_stream_active() {
                continue;
            }

            // Skip if we've already spawned a task for this processor instance
            if seen_processors.iter().any(|p| Arc::ptr_eq(p, processor)) {
                continue;
            }

            seen_processors.push(Arc::clone(processor));

            tracing::info!("Starting payment wait task for {:?}", key);

            let name = format!("payment processor {:?} {:?}", key.method, key.unit);
            Self::spawn_payment_waiter(
                &mut join_set,
                name,
                Arc::clone(&mint),
                Arc::clone(processor),
                Arc::clone(&localstore),
                Arc::clone(&pubsub_manager),
                Arc::clone(&shutdown),
            );
        }

        // Always run the full loop (rather than short-circuiting to just
        // awaiting shutdown when `join_set` starts empty): a mint can start
        // with zero processors and gain its first one later, and only this
        // loop listens on `new_processors`.
        let shutdown_future = shutdown.notified();
        tokio::pin!(shutdown_future);
        loop {
            tokio::select! {
                _ = &mut shutdown_future => {
                    tracing::info!("Shutting down payment processors");
                    break;
                }
                new_processor = new_processors.recv() => {
                    let Some(processor) = new_processor else {
                        // Sender dropped (mint stopping); the shutdown arm
                        // above will fire momentarily.
                        continue;
                    };

                    if processor.is_payment_event_stream_active()
                        || seen_processors.iter().any(|p| Arc::ptr_eq(p, &processor))
                    {
                        continue;
                    }
                    seen_processors.push(Arc::clone(&processor));

                    tracing::info!("Starting payment wait task for a runtime-registered processor");
                    Self::spawn_payment_waiter(
                        &mut join_set,
                        "payment processor (registered at runtime)".to_string(),
                        Arc::clone(&mint),
                        processor,
                        Arc::clone(&localstore),
                        Arc::clone(&pubsub_manager),
                        Arc::clone(&shutdown),
                    );
                }
                Some(result) = join_set.join_next(), if !join_set.is_empty() => {
                    if let Err(e) = result {
                        tracing::warn!("Task panicked: {:?}", e);
                    }
                }
            }
        }

        join_set.shutdown().await;
        Ok(())
    }

    /// Spawns one processor's payment-event consumer task into `join_set`.
    fn spawn_payment_waiter(
        join_set: &mut JoinSet<()>,
        name: String,
        mint: Arc<Mint>,
        processor: DynMintPayment,
        localstore: DynMintDatabase,
        pubsub_manager: Arc<PubSubManager>,
        shutdown: Arc<Notify>,
    ) {
        join_set.spawn(async move {
            let result = Self::wait_for_processor_payments(
                name,
                mint,
                processor,
                localstore,
                pubsub_manager,
                shutdown,
            )
            .await;

            if let Err(e) = result {
                tracing::error!("Payment processor task failed: {:?}", e);
            }
        });
    }

    /// Handles payment waiting for a single processor
    #[instrument(skip_all)]
    async fn wait_for_processor_payments(
        name: String,
        mint: Arc<Mint>,
        processor: DynMintPayment,
        localstore: DynMintDatabase,
        pubsub_manager: Arc<PubSubManager>,
        shutdown: Arc<Notify>,
    ) -> Result<(), Error> {
        let mut waiter = PaymentWaiter {
            name,
            mint,
            processor,
            localstore,
            pubsub_manager,
        };
        // `supervise` owns the reconnect/backoff/shutdown loop and runs
        // `on_shutdown` (which cancels the backend stream) on the way out.
        waiter.supervise(shutdown.notified()).await;
        Ok(())
    }

    #[instrument(skip_all)]
    pub(crate) async fn handle_successful_melt_payment_event(
        mint: &Arc<Mint>,
        localstore: &DynMintDatabase,
        pubsub_manager: &Arc<PubSubManager>,
        quote_id: &QuoteId,
        payment_response: cdk_common::payment::MakePaymentResponse,
    ) -> Result<(), Error> {
        let quote_lock = mint.melt_quote_lock(quote_id).await;
        let _quote_guard = quote_lock.lock_owned().await;

        let Some(mut quote) = localstore.get_melt_quote(quote_id).await? else {
            tracing::warn!("Outgoing payment event for unknown quote {}", quote_id);
            return Ok(());
        };

        let saga = localstore.get_melt_saga_by_quote_id(quote_id).await?;

        match quote.state {
            MeltQuoteState::Paid => {
                if saga.is_none() {
                    tracing::info!(
                        "Ignoring successful payment event for already finalized melt quote {}",
                        quote_id
                    );
                    return Ok(());
                }
            }
            MeltQuoteState::Unpaid | MeltQuoteState::Failed => {
                tracing::info!(
                    "Ignoring successful payment event for already resolved melt quote {} in state {}",
                    quote_id,
                    quote.state
                );
                return Ok(());
            }
            MeltQuoteState::Pending => {}
            MeltQuoteState::Unknown => {
                tracing::warn!(
                    "Ignoring outgoing payment event for melt quote {} in unexpected state {}",
                    quote_id,
                    quote.state
                );
                return Ok(());
            }
        }

        let Some(saga) = saga else {
            tracing::warn!(
                "Successful payment event for quote {} but finalization saga metadata is missing",
                quote_id
            );
            return Ok(());
        };

        saga_recovery::process_melt_saga_outcome(
            &saga,
            &mut quote,
            &payment_response,
            localstore,
            pubsub_manager,
            mint,
        )
        .await
    }

    #[instrument(skip_all)]
    pub(crate) async fn handle_failed_melt_payment_event(
        mint: &Arc<Mint>,
        localstore: &DynMintDatabase,
        pubsub_manager: &Arc<PubSubManager>,
        quote_id: &QuoteId,
    ) -> Result<(), Error> {
        let quote_lock = mint.melt_quote_lock(quote_id).await;
        let _quote_guard = quote_lock.lock_owned().await;

        let Some(mut quote) = localstore.get_melt_quote(quote_id).await? else {
            tracing::warn!("Outgoing payment event for unknown quote {}", quote_id);
            return Ok(());
        };

        match quote.state {
            MeltQuoteState::Pending => {}
            MeltQuoteState::Paid => {
                tracing::info!(
                    "Ignoring failed payment event for already paid melt quote {}",
                    quote_id
                );
                return Ok(());
            }
            MeltQuoteState::Unpaid | MeltQuoteState::Failed => {
                tracing::info!(
                    "Ignoring failed payment event for already resolved melt quote {} in state {}",
                    quote_id,
                    quote.state
                );
                return Ok(());
            }
            MeltQuoteState::Unknown => {
                tracing::warn!(
                    "Ignoring failed payment event for melt quote {} in unexpected state {}",
                    quote_id,
                    quote.state
                );
                return Ok(());
            }
        }

        let Some(saga) = localstore.get_melt_saga_by_quote_id(quote_id).await? else {
            tracing::warn!(
                "Failed payment event for quote {} but rollback saga metadata is missing",
                quote_id
            );
            return Ok(());
        };

        let payment_response = mint.check_melt_payment_status(&quote).await?;

        saga_recovery::process_melt_saga_failure_event(
            &saga,
            &mut quote,
            &payment_response,
            localstore,
            pubsub_manager,
            mint,
        )
        .await
    }

    /// Handle payment notification without needing full Mint instance
    /// This is a helper function that can be called with just the required components
    #[instrument(skip_all)]
    async fn handle_payment_notification(
        localstore: &DynMintDatabase,
        pubsub_manager: &Arc<PubSubManager>,
        wait_payment_response: WaitPaymentResponse,
    ) -> Result<(), Error> {
        if wait_payment_response.payment_amount.value() == 0 {
            tracing::warn!(
                "Received payment response with 0 amount with payment id {}.",
                wait_payment_response.payment_id
            );
            return Err(Error::AmountUndefined);
        }

        let mut tx = localstore.begin_transaction().await?;

        let should_notify = if let Ok(Some(mut mint_quote)) = tx
            .get_mint_quote_by_request_lookup_id(&wait_payment_response.payment_identifier)
            .await
        {
            let previous_state = mint_quote.state();
            let notify =
                Self::handle_mint_quote_payment(&mut tx, &mut mint_quote, wait_payment_response)
                    .await?;
            if notify {
                Some((mint_quote.clone(), mint_quote.amount_paid(), previous_state))
            } else {
                None
            }
        } else {
            tracing::warn!(
                "Could not get request for request lookup id {:?}",
                wait_payment_response.payment_identifier
            );
            None
        };

        tx.commit().await?;

        // Publish notification AFTER transaction commits so subscribers
        // see the committed state when they query.
        if let Some((quote, amount_paid, previous_state)) = should_notify {
            tracing::info!(
                quote_id = %quote.id,
                method = %quote.payment_method,
                unit = %quote.unit,
                request_lookup_id = %quote.request_lookup_id,
                previous_state = %previous_state,
                new_state = %quote.state(),
                amount_paid = %amount_paid,
                amount_issued = %quote.amount_issued(),
                amount_mintable = %quote.amount_mintable(),
                "mint quote payment notification committed",
            );
            pubsub_manager.mint_quote_payment(&quote, amount_paid);
        }

        Ok(())
    }

    /// Handle payment for a specific mint quote (extracted from pay_mint_quote)
    ///
    /// Returns `true` if a payment was recorded and a pubsub notification should
    /// be published **after** the enclosing transaction commits.
    #[instrument(skip_all)]
    async fn handle_mint_quote_payment(
        tx: &mut Box<dyn database::MintTransaction<database::Error> + Send + Sync>,
        mint_quote: &mut Acquired<MintQuote>,
        wait_payment_response: WaitPaymentResponse,
    ) -> Result<bool, Error> {
        tracing::debug!(
            "Received payment notification of {} {:?} for mint quote {} with payment id {}",
            wait_payment_response.payment_amount,
            wait_payment_response.unit(),
            mint_quote.id,
            wait_payment_response.payment_id.to_string()
        );

        let quote_state = mint_quote.state();
        if !mint_quote
            .payment_ids()
            .contains(&&wait_payment_response.payment_id)
        {
            if mint_quote.payment_method.is_bolt11()
                && (quote_state == MintQuoteState::Issued || quote_state == MintQuoteState::Paid)
            {
                tracing::debug!("Received payment notification for already issued quote.");
            } else {
                let payment_amount_quote_unit: Amount<CurrencyUnit> = wait_payment_response
                    .payment_amount
                    .convert_to(&mint_quote.unit)?;

                if payment_amount_quote_unit.value() == 0 {
                    tracing::error!("Zero amount payments should not be recorded.");
                    return Err(Error::AmountUndefined);
                }

                tracing::debug!(
                    "Payment received amount in quote unit {} {}",
                    mint_quote.unit,
                    payment_amount_quote_unit
                );

                match mint_quote.add_payment(
                    payment_amount_quote_unit,
                    wait_payment_response.payment_id.clone(),
                    None,
                ) {
                    Ok(()) => {
                        tx.update_mint_quote(mint_quote).await?;
                        return Ok(true);
                    }
                    Err(Error::DuplicatePaymentId) => {
                        tracing::debug!(
                            "Payment ID {} already processed (caught race condition)",
                            wait_payment_response.payment_id
                        );
                        // This is fine - another concurrent request already processed this payment
                    }
                    Err(e) => return Err(e),
                }
            }
        } else {
            tracing::debug!("Received payment notification for already seen payment.");
        }

        Ok(false)
    }

    /// Fee required for proof set
    #[instrument(skip_all)]
    pub async fn get_proofs_fee(
        &self,
        proofs: &Proofs,
    ) -> Result<crate::fees::ProofsFeeBreakdown, Error> {
        let mut proofs_per_keyset = HashMap::new();
        let mut fee_per_keyset = HashMap::new();

        for proof in proofs {
            if let std::collections::hash_map::Entry::Vacant(e) =
                fee_per_keyset.entry(proof.keyset_id)
            {
                let mint_keyset_info = self
                    .get_keyset_info(&proof.keyset_id)
                    .ok_or(Error::UnknownKeySet)?;
                e.insert(mint_keyset_info.input_fee_ppk);
            }

            proofs_per_keyset
                .entry(proof.keyset_id)
                .and_modify(|count| *count += 1)
                .or_insert(1);
        }

        let fee_breakdown = calculate_fee(&proofs_per_keyset, &fee_per_keyset)?;

        Ok(fee_breakdown)
    }

    /// Get active keysets
    pub fn get_active_keysets(&self) -> HashMap<CurrencyUnit, Id> {
        self.keysets
            .load()
            .iter()
            .filter_map(|keyset| {
                if keyset.active {
                    Some((keyset.unit.clone(), keyset.id))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get keyset info
    pub fn get_keyset_info(&self, id: &Id) -> Option<MintKeySetInfo> {
        self.keysets
            .load()
            .iter()
            .filter_map(|keyset| {
                if keyset.id == *id {
                    Some(keyset.into())
                } else {
                    None
                }
            })
            .next()
    }

    /// Blind Sign
    #[tracing::instrument(skip_all)]
    pub async fn blind_sign(
        &self,
        blinded_message: Vec<BlindedMessage>,
    ) -> Result<Vec<BlindSignature>, Error> {
        #[cfg(test)]
        {
            if crate::test_helpers::mint::should_fail_in_test() {
                return Err(Error::SignatureMissingOrInvalid);
            }
        }

        #[cfg(feature = "prometheus")]
        let metrics = MintMetricGuard::new("blind_sign");

        let result = self.signatory.blind_sign(blinded_message).await;

        #[cfg(feature = "prometheus")]
        {
            metrics.record(result.is_ok());
        }

        result
    }

    /// Verify [`Proof`] meets conditions and is signed
    #[tracing::instrument(skip_all)]
    pub async fn verify_proofs(&self, proofs: Proofs) -> Result<(), Error> {
        // This ignore P2PK and HTLC, as all NUT-10 spending conditions are
        // checked elsewhere.
        #[cfg(feature = "prometheus")]
        let metrics = MintMetricGuard::new("verify_proofs");

        let result = self.signatory.verify_proofs(proofs).await;

        #[cfg(feature = "prometheus")]
        {
            metrics.record(result.is_ok());
        }

        result
    }

    /// Restore
    #[instrument(skip_all)]
    pub async fn restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        #[cfg(feature = "prometheus")]
        let metrics = MintMetricGuard::new("restore");

        let result = async {
            let output_len = request.outputs.len();

            if output_len == 0 {
                return Ok(RestoreResponse {
                    outputs: Vec::new(),
                    signatures: Vec::new(),
                });
            }

            // Check max outputs limit
            if output_len > self.max_outputs {
                tracing::warn!(
                    "Restore request exceeds max outputs limit: {} > {}",
                    output_len,
                    self.max_outputs
                );
                return Err(Error::MaxOutputsExceeded {
                    actual: output_len,
                    max: self.max_outputs,
                });
            }

            let mut outputs = Vec::with_capacity(output_len);
            let mut signatures = Vec::with_capacity(output_len);

            // Build a position map to track original request order for verification
            let position_map: HashMap<PublicKey, usize> = request
                .outputs
                .iter()
                .enumerate()
                .map(|(idx, output)| (output.blinded_secret, idx))
                .collect();

            let blinded_message: Vec<PublicKey> =
                request.outputs.iter().map(|b| b.blinded_secret).collect();

            let blinded_signatures = self
                .localstore
                .get_blind_signatures(&blinded_message)
                .await?;

            if blinded_signatures.len() != output_len {
                return Err(Error::Internal);
            }

            for (blinded_message, blinded_signature) in
                request.outputs.into_iter().zip(blinded_signatures)
            {
                if let Some(blinded_signature) = blinded_signature {
                    if let Some(keyset_info) = self.get_keyset_info(&blinded_signature.keyset_id) {
                        if keyset_info.is_expired() {
                            tracing::debug!(
                                "Skipping restore for expired keyset {}",
                                blinded_signature.keyset_id
                            );
                            continue;
                        }
                    }
                    outputs.push(blinded_message);
                    signatures.push(blinded_signature);
                }
            }

            // Verify response outputs maintain the same relative order as the request
            // This ensures the NUT-09 spec requirement that outputs[i] corresponds to signatures[i]
            let mut last_position: Option<usize> = None;
            for output in &outputs {
                let current_position =
                    position_map.get(&output.blinded_secret).ok_or_else(|| {
                        tracing::error!("Restore response contains output not in original request");
                        Error::Internal
                    })?;

                if let Some(last_pos) = last_position {
                    if *current_position <= last_pos {
                        tracing::error!(
                            "Restore response outputs are out of order: position {} after {}",
                            current_position,
                            last_pos
                        );
                        return Err(Error::Internal);
                    }
                }
                last_position = Some(*current_position);
            }

            Ok(RestoreResponse {
                outputs,
                signatures,
            })
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            metrics.record(result.is_ok());
        }

        result
    }

    /// Get the total amount issed by keyset
    #[instrument(skip_all)]
    pub async fn total_issued(&self) -> Result<HashMap<Id, Amount>, Error> {
        #[cfg(feature = "prometheus")]
        let metrics = MintMetricGuard::new("total_issued");

        let result = async {
            let mut total_issued = self.localstore.get_total_issued().await?;
            for keyset in self.keysets().keysets {
                total_issued.entry(keyset.id).or_default();
            }
            Ok(total_issued)
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            metrics.record(result.is_ok());
        }

        result
    }

    /// Total redeemed for keyset
    #[instrument(skip_all)]
    pub async fn total_redeemed(&self) -> Result<HashMap<Id, Amount>, Error> {
        #[cfg(feature = "prometheus")]
        let metrics = MintMetricGuard::new("total_redeemed");

        let result = async {
            let mut total_redeemed = self.localstore.get_total_redeemed().await?;
            for keyset in self.keysets().keysets {
                total_redeemed.entry(keyset.id).or_default();
            }
            Ok(total_redeemed)
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            metrics.record(result.is_ok());
        }

        result
    }
}

#[cfg(test)]
mod tests {

    use std::collections::HashSet;
    use std::str::FromStr;
    use std::sync::Arc;

    use cdk_common::amount::SplitTarget;
    use cdk_common::melt::MeltQuoteRequest;
    use cdk_common::mint::{OperationKind, SagaStateEnum};
    use cdk_common::nut00::KnownMethod;
    use cdk_common::nuts::nut04::MintQuoteCustomRequest;
    use cdk_common::nuts::{MeltQuoteBolt11Request, PreMintSecrets};
    use cdk_common::payment::{MakePaymentResponse, PaymentIdentifier};
    use cdk_common::PaymentMethod;
    use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription, FakeWallet};
    use cdk_signatory::db_signatory::DbSignatory;
    use cdk_signatory::signatory::{RotateKeyArguments, SignatoryKeysets};
    use cdk_sqlite::mint::memory::new_with_state;
    use tokio::sync::watch;
    use tokio::time::{sleep, timeout};

    use super::*;
    use crate::mint::melt::melt_saga::{MeltSaga, PaymentOutcome};
    use crate::test_helpers::mint::{create_test_mint, mint_test_proofs};
    use crate::types::FeeReserve;

    #[derive(Default)]
    struct MintConfig<'a> {
        active_keysets: HashMap<CurrencyUnit, Id>,
        keysets: Vec<MintKeySetInfo>,
        mint_quotes: Vec<MintQuote>,
        melt_quotes: Vec<MeltQuote>,
        pending_proofs: Proofs,
        spent_proofs: Proofs,
        seed: &'a [u8],
        mint_info: MintInfo,
        supported_units: HashMap<CurrencyUnit, (u64, Vec<u64>)>,
    }

    /// A stand-in for the in-memory signatory whose keyset subscription is
    /// driven by the test. Injected snapshots are delivered verbatim through
    /// `subscribe_keysets`, so the mint's drain task can be exercised in
    /// isolation from `DbSignatory`'s rotation logic.
    struct MockSignatory {
        updates: watch::Sender<SignatoryKeysets>,
    }

    impl MockSignatory {
        fn new(initial: SignatoryKeysets) -> Self {
            let (updates, _) = watch::channel(initial);
            Self { updates }
        }

        /// Inject a new keyset snapshot through the subscription.
        fn push(&self, keysets: SignatoryKeysets) {
            self.updates.send_replace(keysets);
        }
    }

    #[async_trait::async_trait]
    impl Signatory for MockSignatory {
        fn name(&self) -> String {
            "mock".to_string()
        }

        async fn blind_sign(
            &self,
            _blinded_messages: Vec<BlindedMessage>,
        ) -> Result<Vec<BlindSignature>, Error> {
            Err(Error::Custom("unsupported in mock".to_string()))
        }

        async fn verify_proofs(&self, _proofs: Vec<cdk_common::Proof>) -> Result<(), Error> {
            Err(Error::Custom("unsupported in mock".to_string()))
        }

        async fn keysets(&self) -> Result<SignatoryKeysets, Error> {
            Ok(self.updates.borrow().clone())
        }

        async fn subscribe_keysets(&self) -> Result<watch::Receiver<SignatoryKeysets>, Error> {
            Ok(self.updates.subscribe())
        }

        async fn rotate_keyset(&self, _args: RotateKeyArguments) -> Result<SignatoryKeySet, Error> {
            Err(Error::Custom("unsupported in mock".to_string()))
        }
    }

    /// Produce a sequence of valid, distinct keyset snapshots by rotating a real
    /// `DbSignatory` `count` times and reading the full set after each rotation.
    /// Each rotation makes a new active Sat keyset, so the snapshots differ.
    async fn rotated_snapshots(count: usize) -> Vec<SignatoryKeysets> {
        let store = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("in-memory db"),
        );
        let signatory = DbSignatory::new(
            store,
            b"mock-signatory-seed",
            Default::default(),
            Default::default(),
        )
        .await
        .expect("DbSignatory::new");

        let amounts = vec![1, 2, 4, 8];
        let mut snapshots = Vec::with_capacity(count);
        for _ in 0..count {
            signatory
                .rotate_keyset(RotateKeyArguments {
                    unit: CurrencyUnit::Sat,
                    amounts: amounts.clone(),
                    input_fee_ppk: 0,
                    keyset_id_type: cdk_common::nut02::KeySetVersion::Version00,
                    final_expiry: None,
                })
                .await
                .expect("rotate_keyset");
            snapshots.push(signatory.keysets().await.expect("keysets"));
        }
        snapshots
    }

    /// The id of the active Sat keyset in a snapshot.
    fn active_sat_id(snapshot: &SignatoryKeysets) -> Id {
        snapshot
            .keysets
            .iter()
            .find(|k| k.active && k.unit == CurrencyUnit::Sat)
            .expect("snapshot should have an active Sat keyset")
            .id
    }

    /// Build a mint around an arbitrary signatory, with a fresh empty store.
    async fn create_mint_with_signatory(signatory: Arc<dyn Signatory + Send + Sync>) -> Mint {
        let localstore = Arc::new(
            new_with_state(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                MintInfo::default(),
            )
            .await
            .unwrap(),
        );

        Mint::new(
            MintInfo::default(),
            signatory,
            localstore,
            HashMap::new(),
            1000,
            1000,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn mock_injection_updates_mint_keysets() {
        let snaps = rotated_snapshots(2).await;
        let next = snaps[1].clone();
        let new_id = active_sat_id(&next);

        let mock = Arc::new(MockSignatory::new(snaps[0].clone()));
        let mint = create_mint_with_signatory(mock.clone()).await;
        mint.start().await.expect("mint should start");

        let before: Vec<Id> = mint.keysets.load().iter().map(|k| k.id).collect();
        assert!(
            !before.contains(&new_id),
            "injected keyset id must not exist before injection"
        );

        // Inject a new snapshot straight through the subscription.
        mock.push(next);

        let applied = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if mint.keysets.load().iter().any(|k| k.id == new_id) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        assert!(
            applied.is_ok(),
            "injected keyset did not reach the mint in time"
        );

        mint.stop().await.expect("mint should stop");
    }

    #[tokio::test]
    async fn mock_injection_latest_wins() {
        let snaps = rotated_snapshots(3).await;
        let b = snaps[1].clone();
        let c = snaps[2].clone();
        let c_id = active_sat_id(&c);
        let c_len = c.keysets.len();

        let mock = Arc::new(MockSignatory::new(snaps[0].clone()));
        let mint = create_mint_with_signatory(mock.clone()).await;
        mint.start().await.expect("mint should start");

        // Two injections back-to-back: the watch keeps only the latest, so the
        // mint must settle on C even if B is never observed.
        mock.push(b);
        mock.push(c);

        let converged = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if mint.keysets.load().iter().any(|k| k.id == c_id) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        assert!(
            converged.is_ok(),
            "mint did not converge to the latest snapshot"
        );
        assert_eq!(
            mint.keysets.load().len(),
            c_len,
            "mint should settle on the latest snapshot, not an earlier one"
        );

        mint.stop().await.expect("mint should stop");
    }

    #[tokio::test]
    async fn empty_snapshot_is_ignored() {
        let snaps = rotated_snapshots(2).await;
        let seed = snaps[0].clone();
        let next = snaps[1].clone();
        let next_id = active_sat_id(&next);
        let seed_pubkey = seed.pubkey;
        let seed_ids: Vec<Id> = seed.keysets.iter().map(|k| k.id).collect();

        let mock = Arc::new(MockSignatory::new(seed));
        let mint = create_mint_with_signatory(mock.clone()).await;
        mint.start().await.expect("mint should start");

        // An empty snapshot must be dropped by the drain guard: the mint
        // keysets stay untouched.
        mock.push(SignatoryKeysets {
            pubkey: seed_pubkey,
            keysets: vec![],
        });
        // Give the drain task time to observe and discard the empty snapshot.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let current: Vec<Id> = mint.keysets.load().iter().map(|k| k.id).collect();
        assert_eq!(
            current, seed_ids,
            "empty snapshot must not change the mint keysets"
        );

        // A valid snapshot after the empty one still propagates: the guard must
        // not wedge the drain loop.
        mock.push(next);
        let applied = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if mint.keysets.load().iter().any(|k| k.id == next_id) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(
            applied.is_ok(),
            "valid snapshot after an empty one should still propagate"
        );

        mint.stop().await.expect("mint should stop");
    }

    /// A mint may boot before any active keyset exists (for example a remote
    /// signatory still connecting). Construction and `start()` must succeed
    /// rather than failing with `NoActiveKeyset`, and keysets that arrive later
    /// over the subscription must install without a restart.
    #[tokio::test]
    async fn mint_starts_without_active_keysets_then_installs_them() {
        // A snapshot to install later; reuse its pubkey for the empty bootstrap
        // snapshot so both carry the same signatory identity.
        let snap = rotated_snapshots(1).await.remove(0);
        let new_id = active_sat_id(&snap);

        // Bootstrap from a snapshot with no keysets at all. This used to return
        // Error::NoActiveKeyset from the constructor.
        let mock = Arc::new(MockSignatory::new(SignatoryKeysets {
            pubkey: snap.pubkey,
            keysets: vec![],
        }));
        let mint = create_mint_with_signatory(mock.clone()).await;
        mint.start()
            .await
            .expect("mint should start with no active keysets");

        assert!(
            mint.keysets.load().is_empty(),
            "no keysets should be present before any arrive"
        );

        // Keysets arriving over the subscription install without a restart.
        mock.push(snap);
        let applied = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if mint
                    .keysets
                    .load()
                    .iter()
                    .any(|k| k.id == new_id && k.active)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(
            applied.is_ok(),
            "keysets that arrive after an empty start should install"
        );

        mint.stop().await.expect("mint should stop");
    }

    /// Regression test for the bootstrap/subscribe race: a rotation that lands
    /// between mint construction and `start()` must still reach the mint.
    ///
    /// The subscription is now both the bootstrap and the update source, so the
    /// snapshot pushed in that window is delivered by the drain task's first
    /// `changed()`. Under the old two-phase bootstrap (unary `keysets()` in the
    /// constructor, `subscribe_keysets()` later in `start()`) this snapshot was
    /// silently skipped until the next rotation, and this test would time out.
    #[tokio::test]
    async fn rotation_between_construction_and_start_is_not_missed() {
        let snaps = rotated_snapshots(2).await;
        let next = snaps[1].clone();
        let new_id = active_sat_id(&next);

        let mock = Arc::new(MockSignatory::new(snaps[0].clone()));
        let mint = create_mint_with_signatory(mock.clone()).await;

        // The mint bootstrapped snapshot A. Rotate to B *before* start() spawns
        // the drain task: this is exactly the window the old bootstrap missed.
        assert!(
            !mint.keysets.load().iter().any(|k| k.id == new_id),
            "bootstrapped snapshot must not already contain the rotated keyset"
        );
        mock.push(next);

        mint.start().await.expect("mint should start");

        // No push after start(): the mint must converge to B purely from the
        // snapshot that landed in the construction-to-start window.
        let applied = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if mint.keysets.load().iter().any(|k| k.id == new_id) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        assert!(
            applied.is_ok(),
            "snapshot pushed before start() did not reach the mint"
        );

        mint.stop().await.expect("mint should stop");
    }

    /// A restart after `stop()` re-subscribes (the retained receiver was
    /// consumed by the first `start()`) and must catch up on any snapshot that
    /// landed while the drain task was stopped.
    #[tokio::test]
    async fn restart_catches_up_on_missed_rotation() {
        let snaps = rotated_snapshots(2).await;
        let next = snaps[1].clone();
        let new_id = active_sat_id(&next);

        let mock = Arc::new(MockSignatory::new(snaps[0].clone()));
        let mint = create_mint_with_signatory(mock.clone()).await;

        mint.start().await.expect("mint should start");
        mint.stop().await.expect("mint should stop");

        // Rotate while stopped: the drain task is gone, so the ArcSwap is stale.
        mock.push(next);

        mint.start().await.expect("mint should restart");

        let applied = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if mint.keysets.load().iter().any(|k| k.id == new_id) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        assert!(
            applied.is_ok(),
            "restart did not catch up on the snapshot pushed while stopped"
        );

        mint.stop().await.expect("mint should stop");
    }

    /// A single armed pause point for a `GatedSignatory::keysets` call.
    struct Gate {
        reached: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
    }

    /// Wraps a real `DbSignatory`, delegating every call, but lets a test hold a
    /// single `keysets()` call open: it reads the current snapshot, signals the
    /// test, then blocks until released and returns the snapshot it read. This
    /// reproduces the window where `Mint::rotate_keyset` has read a snapshot but
    /// not yet stored it while another rotation lands, which without the shared
    /// store lock let a stale write clobber a newer one in the keyset ArcSwap.
    struct GatedSignatory {
        inner: Arc<DbSignatory>,
        gate: Mutex<Option<Gate>>,
    }

    impl GatedSignatory {
        fn new(inner: Arc<DbSignatory>) -> Self {
            Self {
                inner,
                gate: Mutex::new(None),
            }
        }

        /// Arm the next `keysets()` call to pause. Returns a receiver that fires
        /// once the call has read its snapshot, and a sender that releases it.
        async fn arm(
            &self,
        ) -> (
            tokio::sync::oneshot::Receiver<()>,
            tokio::sync::oneshot::Sender<()>,
        ) {
            let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = tokio::sync::oneshot::channel();
            *self.gate.lock().await = Some(Gate {
                reached: reached_tx,
                release: release_rx,
            });
            (reached_rx, release_tx)
        }
    }

    #[async_trait::async_trait]
    impl Signatory for GatedSignatory {
        fn name(&self) -> String {
            self.inner.name()
        }

        async fn blind_sign(
            &self,
            blinded_messages: Vec<BlindedMessage>,
        ) -> Result<Vec<BlindSignature>, Error> {
            self.inner.blind_sign(blinded_messages).await
        }

        async fn verify_proofs(&self, proofs: Vec<cdk_common::Proof>) -> Result<(), Error> {
            self.inner.verify_proofs(proofs).await
        }

        async fn keysets(&self) -> Result<SignatoryKeysets, Error> {
            let snapshot = self.inner.keysets().await?;
            let gate = self.gate.lock().await.take();
            if let Some(gate) = gate {
                let _ = gate.reached.send(());
                let _ = gate.release.await;
            }
            Ok(snapshot)
        }

        async fn subscribe_keysets(&self) -> Result<watch::Receiver<SignatoryKeysets>, Error> {
            self.inner.subscribe_keysets().await
        }

        async fn rotate_keyset(&self, args: RotateKeyArguments) -> Result<SignatoryKeySet, Error> {
            self.inner.rotate_keyset(args).await
        }
    }

    /// Regression test for the two-writer keyset race. A mint-initiated rotation
    /// reads the signatory snapshot and then stores it, while the subscription
    /// drain task also stores every published snapshot. If a newer rotation is
    /// drained between the mint rotation's read and its store, the stale store
    /// must not clobber the newer snapshot.
    ///
    /// The gate holds the mint rotation's `keysets()` read open while an
    /// out-of-band rotation on another unit lands and is drained. When the
    /// rotation resumes and stores its now-stale read, the shared store lock
    /// serializes it with the drain so the newest snapshot still wins. Without
    /// the lock the stale store sticks and this test times out.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rotate_store_does_not_clobber_newer_drained_snapshot() {
        let amounts: Vec<u64> = (0..4).map(|i| 2u64.pow(i)).collect();
        let mut supported_units = HashMap::new();
        supported_units.insert(CurrencyUnit::Sat, (0u64, amounts.clone()));
        supported_units.insert(CurrencyUnit::Msat, (0u64, amounts.clone()));

        let store = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("in-memory db"),
        );
        let inner = Arc::new(
            DbSignatory::new(
                store,
                b"gated-signatory-seed",
                supported_units.clone(),
                Default::default(),
            )
            .await
            .expect("DbSignatory::new"),
        );
        // Seed an active keyset for each unit.
        for (unit, (fee, amts)) in &supported_units {
            inner
                .rotate_keyset(RotateKeyArguments {
                    unit: unit.clone(),
                    amounts: amts.clone(),
                    input_fee_ppk: *fee,
                    keyset_id_type: cdk_common::nut02::KeySetVersion::Version00,
                    final_expiry: None,
                })
                .await
                .expect("seed rotate");
        }

        let gated = Arc::new(GatedSignatory::new(inner));
        let mint = create_mint_with_signatory(gated.clone()).await;
        mint.start().await.expect("mint should start");

        // Arm the gate, then start a mint-initiated Sat rotation. It commits the
        // new Sat keyset, then reads a snapshot that still predates the Msat
        // rotation below, and blocks before storing it.
        let (reached, release) = gated.arm().await;
        let mint_c = mint.clone();
        let amts = amounts.clone();
        let rotate = tokio::spawn(async move {
            mint_c
                .rotate_keyset(CurrencyUnit::Sat, amts, 0, true, None)
                .await
        });

        // Wait until the rotation has read its soon-to-be-stale snapshot.
        reached.await.expect("rotation reached the gate");

        // Land an out-of-band Msat rotation; the drain applies the newer
        // snapshot while the mint rotation is still parked at the gate.
        let rotated_msat = mint
            .signatory
            .rotate_keyset(RotateKeyArguments {
                unit: CurrencyUnit::Msat,
                amounts: amounts.clone(),
                input_fee_ppk: 0,
                keyset_id_type: cdk_common::nut02::KeySetVersion::Version00,
                final_expiry: None,
            })
            .await
            .expect("out-of-band msat rotate");

        // Release the parked rotation; its stale store must not win.
        release.send(()).expect("release the gate");
        let sat_info = rotate.await.expect("join").expect("mint rotate");

        // The cache must converge to hold BOTH the new Sat and the new Msat
        // keyset. A stale rotate store drops the drained Msat keyset and never
        // recovers, so this loop would time out.
        let converged = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let ids: Vec<Id> = mint.keysets.load().iter().map(|k| k.id).collect();
                if ids.contains(&sat_info.id) && ids.contains(&rotated_msat.id) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        assert!(
            converged.is_ok(),
            "stale rotate store clobbered the newer drained Msat keyset"
        );

        mint.stop().await.expect("mint should stop");
    }

    async fn create_mint(config: MintConfig<'_>) -> Mint {
        let localstore = Arc::new(
            new_with_state(
                config.active_keysets,
                config.keysets,
                config.mint_quotes,
                config.melt_quotes,
                config.pending_proofs,
                config.spent_proofs,
                config.mint_info,
            )
            .await
            .unwrap(),
        );

        let signatory = Arc::new(
            cdk_signatory::db_signatory::DbSignatory::new(
                localstore.clone(),
                config.seed,
                config.supported_units.clone(),
                HashMap::new(),
            )
            .await
            .expect("Failed to create signatory"),
        );

        for (unit, (fee, amounts)) in &config.supported_units {
            signatory
                .rotate_keyset(RotateKeyArguments {
                    unit: unit.clone(),
                    amounts: amounts.clone(),
                    input_fee_ppk: *fee,
                    keyset_id_type: cdk_common::nut02::KeySetVersion::Version00,
                    final_expiry: None,
                })
                .await
                .unwrap();
        }

        Mint::new(
            MintInfo::default(),
            signatory,
            localstore,
            HashMap::new(),
            1000,
            1000,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn mint_mod_new_mint() {
        let mut supported_units = HashMap::new();
        let amounts: Vec<u64> = (0..32).map(|i| 2u64.pow(i)).collect();
        supported_units.insert(CurrencyUnit::default(), (0, amounts));
        let config = MintConfig::<'_> {
            supported_units,
            ..Default::default()
        };
        let mint = create_mint(config).await;

        assert_eq!(
            mint.total_issued()
                .await
                .unwrap()
                .into_values()
                .collect::<Vec<_>>(),
            vec![Amount::default()]
        );

        assert_eq!(
            mint.total_issued()
                .await
                .unwrap()
                .into_values()
                .collect::<Vec<_>>(),
            vec![Amount::default()]
        );
    }

    #[tokio::test]
    async fn mint_mod_rotate_keyset() {
        let mut supported_units = HashMap::new();
        let amounts: Vec<u64> = (0..32).map(|i| 2u64.pow(i)).collect();
        supported_units.insert(CurrencyUnit::default(), (0, amounts));

        let config = MintConfig::<'_> {
            supported_units,
            ..Default::default()
        };
        let mint = create_mint(config).await;

        let keysets = mint.keysets();
        let first_keyset_id = keysets.keysets[0].id;

        // set the first keyset to inactive and generate a new keyset
        mint.rotate_keyset(CurrencyUnit::default(), vec![1], 1, true, None)
            .await
            .expect("test");

        let keysets = mint.keysets();

        assert_eq!(2, keysets.keysets.len());
        for keyset in &keysets.keysets {
            if keyset.id == first_keyset_id {
                assert!(!keyset.active);
            } else {
                assert!(keyset.active);
            }
        }
    }

    #[tokio::test]
    async fn mint_mod_rotate_keyset_with_expiry() {
        let mut supported_units = HashMap::new();
        let amounts: Vec<u64> = (0..32).map(|i| 2u64.pow(i)).collect();
        supported_units.insert(CurrencyUnit::default(), (0, amounts));

        let config = MintConfig::<'_> {
            supported_units,
            ..Default::default()
        };
        let mint = create_mint(config).await;

        let expiry: u64 = 1_000_000;
        let keyset_info = mint
            .rotate_keyset(CurrencyUnit::default(), vec![1], 0, true, Some(expiry))
            .await
            .expect("rotate with expiry");

        assert_eq!(
            keyset_info.final_expiry,
            Some(expiry),
            "final_expiry must be stored in the rotated keyset"
        );

        // Also verify it is retrievable through get_keyset_info
        let stored = mint
            .get_keyset_info(&keyset_info.id)
            .expect("keyset should be found");
        assert_eq!(stored.final_expiry, Some(expiry));
    }

    #[tokio::test]
    async fn restore_empty_outputs() {
        let mint = create_test_mint().await.unwrap();

        let response = mint
            .restore(RestoreRequest {
                outputs: Vec::new(),
            })
            .await
            .expect("empty restore should succeed");

        assert!(response.outputs.is_empty());
        assert!(response.signatures.is_empty());
    }

    #[tokio::test]
    async fn melt_quote_lock_prunes_expired_entries() {
        let mint = create_test_mint().await.unwrap();
        let expired_quote_id = QuoteId::new();
        let active_quote_id = QuoteId::new();

        let expired_lock = mint.melt_quote_lock(&expired_quote_id).await;
        let active_lock = mint.melt_quote_lock(&active_quote_id).await;
        drop(expired_lock);

        let active_lock_again = mint.melt_quote_lock(&active_quote_id).await;

        assert!(Arc::ptr_eq(&active_lock, &active_lock_again));

        let locks = mint.melt_quote_locks.lock().await;
        assert!(!locks.contains_key(&expired_quote_id));
        assert!(locks.contains_key(&active_quote_id));
    }

    #[tokio::test]
    async fn successful_payment_event_replays_cleanup_for_paid_quote_with_saga() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
        let melt_request = create_test_melt_request(&proofs, &quote);

        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            Arc::new(mint.clone()),
            mint.localstore(),
            mint.pubsub_manager(),
        );
        let setup_saga = saga
            .setup_melt(
                &melt_request,
                verification,
                PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .await
            .unwrap();

        let (payment_saga, decision) = setup_saga
            .attempt_internal_settlement(&melt_request)
            .await
            .unwrap();
        let operation_id = assert_single_melt_saga_operation_id(&mint).await;
        let PaymentOutcome::Confirmed(_confirmed_saga) =
            payment_saga.make_payment(decision).await.unwrap()
        else {
            panic!("Expected Confirmed outcome");
        };
        let payment_result = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId(quote.id.to_string()),
            payment_proof: None,
            status: MeltQuoteState::Paid,
            total_spent: quote.amount(),
        };

        let finalized_quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap();

        crate::mint::melt::shared::finalize_melt_quote(
            &mint,
            &mint.localstore,
            &mint.pubsub_manager,
            &finalized_quote,
            payment_result.total_spent.clone(),
            payment_result.payment_proof.clone(),
            &payment_result.payment_lookup_id,
            Some(operation_id),
        )
        .await
        .unwrap();

        Mint::handle_successful_melt_payment_event(
            &Arc::new(mint.clone()),
            &mint.localstore,
            &mint.pubsub_manager,
            &quote.id,
            payment_result,
        )
        .await
        .unwrap();

        let persisted_quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .expect("quote should still exist");
        assert_eq!(persisted_quote.state, MeltQuoteState::Paid);

        let sagas = mint
            .localstore
            .get_incomplete_sagas(OperationKind::Melt)
            .await
            .unwrap();
        assert!(
            !sagas.iter().any(|s| s.operation_id == operation_id),
            "saga should be deleted after replayed success finalization"
        );
    }

    #[tokio::test]
    async fn successful_payment_event_ignored_for_paid_quote_without_saga() {
        let mint = create_test_mint().await.unwrap();
        let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
        let payment_result = MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::CustomId(quote.id.to_string()),
            payment_proof: None,
            status: MeltQuoteState::Paid,
            total_spent: Amount::from(9_000).with_unit(CurrencyUnit::Sat),
        };
        let mut tx = mint.localstore.begin_transaction().await.unwrap();
        let mut stored_quote = tx.get_melt_quote(&quote.id).await.unwrap().unwrap();
        tx.update_melt_quote_state(&mut stored_quote, MeltQuoteState::Pending, None)
            .await
            .unwrap();
        tx.update_melt_quote_state(&mut stored_quote, MeltQuoteState::Paid, None)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        Mint::handle_successful_melt_payment_event(
            &Arc::new(mint.clone()),
            &mint.localstore,
            &mint.pubsub_manager,
            &quote.id,
            payment_result,
        )
        .await
        .unwrap();

        let sagas = mint
            .localstore
            .get_incomplete_sagas(OperationKind::Melt)
            .await
            .unwrap();
        assert!(sagas.is_empty());
    }

    #[tokio::test]
    async fn failed_payment_event_ignored_for_paid_quote() {
        let mint = create_test_mint().await.unwrap();
        let proofs = mint_test_proofs(&mint, Amount::from(10_000)).await.unwrap();
        let quote = create_test_melt_quote(&mint, Amount::from(9_000)).await;
        let melt_request = create_test_melt_request(&proofs, &quote);

        let verification = mint.verify_inputs(melt_request.inputs()).await.unwrap();
        let saga = MeltSaga::new(
            Arc::new(mint.clone()),
            mint.localstore(),
            mint.pubsub_manager(),
        );
        let _setup_saga = saga
            .setup_melt(
                &melt_request,
                verification,
                PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .await
            .unwrap();

        let operation_id = assert_single_melt_saga_operation_id(&mint).await;
        let mut tx = mint.localstore.begin_transaction().await.unwrap();
        let mut stored_quote = tx.get_melt_quote(&quote.id).await.unwrap().unwrap();
        tx.update_melt_quote_state(&mut stored_quote, MeltQuoteState::Paid, None)
            .await
            .unwrap();
        let mut saga = tx
            .get_saga_for_update(&operation_id)
            .await
            .unwrap()
            .expect("saga should exist");
        tx.update_acquired_saga(
            &mut saga,
            SagaStateEnum::Melt(cdk_common::mint::MeltSagaState::PaymentAttempted),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        Mint::handle_failed_melt_payment_event(
            &Arc::new(mint.clone()),
            &mint.localstore,
            &mint.pubsub_manager,
            &quote.id,
        )
        .await
        .unwrap();

        let persisted_quote = mint
            .localstore
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .expect("quote should still exist");
        assert_eq!(persisted_quote.state, MeltQuoteState::Paid);

        let sagas = mint
            .localstore
            .get_incomplete_sagas(OperationKind::Melt)
            .await
            .unwrap();
        assert!(sagas.iter().any(|s| s.operation_id == operation_id));
    }

    async fn create_test_melt_quote(mint: &crate::mint::Mint, amount: Amount) -> MeltQuote {
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Paid,
            check_payment_state: MeltQuoteState::Paid,
            pay_err: false,
            check_err: false,
        };

        let amount_msats: u64 = amount.into();
        let invoice = create_fake_invoice(
            amount_msats,
            serde_json::to_string(&fake_description).unwrap(),
        );

        let request = MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: invoice,
            unit: CurrencyUnit::Sat,
            options: None,
        });

        let quote_response = mint.get_melt_quote(request).await.unwrap();

        mint.localstore
            .get_melt_quote(quote_response.quote().expect("single-quote method"))
            .await
            .unwrap()
            .expect("quote should exist in database")
    }

    fn create_test_melt_request(
        proofs: &cdk_common::nuts::Proofs,
        quote: &MeltQuote,
    ) -> cdk_common::nuts::MeltRequest<cdk_common::QuoteId> {
        cdk_common::nuts::MeltRequest::new(quote.id.clone(), proofs.clone(), None)
    }

    async fn assert_single_melt_saga_operation_id(mint: &crate::mint::Mint) -> uuid::Uuid {
        let sagas = mint
            .localstore
            .get_incomplete_sagas(OperationKind::Melt)
            .await
            .unwrap();

        assert_eq!(sagas.len(), 1, "expected exactly one melt saga");
        sagas[0].operation_id
    }

    #[tokio::test]
    async fn test_mint_keyset_gen() {
        let seed = bip39::Mnemonic::from_str(
            "dismiss price public alone audit gallery ignore process swap dance crane furnace",
        )
        .unwrap();
        let mut supported_units = HashMap::new();
        let amounts: Vec<u64> = (0..32).map(|i| 2u64.pow(i)).collect();
        supported_units.insert(CurrencyUnit::default(), (0, amounts));

        let config = MintConfig::<'_> {
            seed: &seed.to_seed_normalized(""),
            supported_units,
            ..Default::default()
        };
        let mint = create_mint(config).await;

        let keys = mint.pubkeys();

        let expected_keys = r#"{"keysets":[{"id":"00214f2e37bdebad","unit":"sat","active":true,"keys":{"1":"024aebe0f8be04b1ba1d7d6b7fe454c9ae43e0fa22b2fdc88b172f3c5a0d19aaa4","1024":"02f050a80caa51a655a4adfcb4c9f6954d9d3b465d4535055319090948d4e89eb5","1048576":"0270b4f86ee9b5a3b3e4ee7f0067af466676c57265c65ce91fbecac7d07ee507d1","1073741824":"035f7fd86429f45f19463840cb90a6053197e808b38ca2e32e2e8690e269c6d820","128":"0214318e5f69e01babdfc0fe923801a58e6e4fd24efe13432f2fec03eae746a0ca","131072":"02aee4305555703649245920911f9da6bc86d59ca7a016b753925454288bf05758","134217728":"02647036cd6c6e93a520968a33f20599f147debc4dce1994e83dc86946ff8c0183","16":"03e41c8e640ad4bcba9d39a0553a8ac1059a0bfd65fd9c19efbe6db844d0d04440","16384":"026c140e10dbdf282f47d0889b427a18c18ef9dfc87888c4487aea7093d3a9dd2e","16777216":"0204d4854439b387fdee5ad46806e4f8a38dabc0158ddcc189912fdc76e2f13161","2":"02c36fe78839c9ccbf3ea3f43b0d38ebd0e25df5310975de879b6fe9b53a3f4038","2048":"03b80c919d38ae697f28af7b5812f38ab886072179616d2b1636708473cd351cf3","2097152":"023a0d0e3a76b085df6f02b1b454758062a097c52927e682be458d549781cbcc96","2147483648":"03e47890abf0fc06b178500dc642e01a648a3674b89634eefc0269c0561023b135","256":"0294de1d276c4b092b41eb50a6545c1d07c4fe173214fd299925774ae1dc190925","262144":"02069c8cbd7e26d7cf84d7a1e2080a1006f4541c6ec75628bf2f4f15dff31ca39a","268435456":"03667e9818b5c49d7fb9ce6ddbfee8933a6ad62b9f15297c84adb251d9cb0db4a2","32":"03d49648d7137553edb6d9d45a47e271fff7301b84e7c854844292dc67d3f39aa2","32768":"02ac41bdfbfbdbb6ebedeb2651c05a6c01e44c15f69cc728878e988eab0af98265","33554432":"03502a5360cb536d3e32abd0bddb214c2a51d9a0c6dc41b346b603206b607074ae","4":"03d8deb39a4d45d120980c973aeb2b4aa9ca1ffbf222a2b7b83f7e1bc68453e6fb","4096":"03d131e59268288cb6a9af37e6ad3f6702cefb36cd4b7fb21120355acf2563749f","4194304":"029b1766c75b5c8789c81194551a65c7902e23fc974ae39a1e2f8a69fe1a786a1f","512":"030552b9dba664409946d5f895cdda030bf7bad8dc7ce085ffb93dbfa54ef2dbfc","524288":"0276c4ca58705002ae7319b33c63e4fcd0aca83e4bf731a1a047c64d4ca3fedd8d","536870912":"028263d89b6ca5fa838a37db6ab35606b2c2955aa717956afc872b9ed3f31c48c5","64":"03966b40fb692370864afe4f161909247b589f4c572a5aa0895b0f297fe00dc894","65536":"03434e6c95715e2cce9f09a40125cbf1dec8247784c22fc5d2629092401b177835","67108864":"03c3ac346a9b0671caffcc3a16e18fbf679b024a4e5f85d7edebc2ca0152b97b7b","8":"0360bb9c61e60f998585768a8893f89963da5699d6ee76c938d148ec3815ed5419","8192":"022cc0be442980edb83fa46771dc3c4303cc572b03d3b174879ca2e10b071f2b4b","8388608":"032818f6e7d6d6dec4bd349df6e609f6c2a9555012e924356c911ee2dbfa8a3d98"},"input_fee_ppk":0}]}"#;
        println!("keys: {}", serde_json::to_string(&keys.clone()).unwrap());

        assert_eq!(expected_keys, serde_json::to_string(&keys.clone()).unwrap());
    }

    #[tokio::test]
    async fn test_start_stop_lifecycle() {
        let mut supported_units = HashMap::new();
        let amounts: Vec<u64> = (0..32).map(|i| 2u64.pow(i)).collect();
        supported_units.insert(CurrencyUnit::default(), (0, amounts));
        let config = MintConfig::<'_> {
            supported_units,
            ..Default::default()
        };
        let mint = create_mint(config).await;

        // Start should succeed (async)
        mint.start().await.expect("Failed to start mint");

        // Starting again should fail (already running)
        assert!(mint.start().await.is_err());

        // Stop should succeed (still async)
        mint.stop().await.expect("Failed to stop mint");

        // Stopping again should succeed (idempotent)
        mint.stop().await.expect("Second stop should be fine");

        // Should be able to start again after stopping
        mint.start().await.expect("Should be able to restart");
        mint.stop().await.expect("Final stop should work");
    }

    #[tokio::test]
    async fn mint_unit_string_collision() {
        let mut supported_units = HashMap::new();
        let amounts: Vec<u64> = (0..32).map(|i| 2_u64.pow(i as u32)).collect();
        supported_units.insert(CurrencyUnit::default(), (0, amounts.clone()));
        let config = MintConfig::<'_> {
            supported_units: supported_units.clone(),
            ..Default::default()
        };
        let mint = create_mint(config).await;

        let currency_unit = CurrencyUnit::custom("sW8W2A_hTH_gapj1_vj5suO3JI_");
        let rotate_argument = RotateKeyArguments {
            unit: currency_unit,
            amounts,
            input_fee_ppk: 100,
            keyset_id_type: cdk_common::nut02::KeySetVersion::Version00,
            final_expiry: None,
        };
        let rotation_result = mint.signatory.rotate_keyset(rotate_argument).await;

        assert!(rotation_result.is_err());

        assert!(matches!(
            rotation_result,
            Err(Error::UnitStringCollision(_currency_unit))
        ));
    }

    #[tokio::test]
    async fn signatory_rotation_propagates_to_mint() {
        let mut supported_units = HashMap::new();
        let amounts: Vec<u64> = (0..8).map(|i| 2u64.pow(i)).collect();
        supported_units.insert(CurrencyUnit::default(), (0, amounts.clone()));
        let config = MintConfig::<'_> {
            supported_units,
            ..Default::default()
        };
        let mint = create_mint(config).await;
        mint.start().await.expect("mint should start");

        let before: Vec<Id> = mint.keysets.load().iter().map(|k| k.id).collect();

        // Rotate directly on the signatory, out of band from the mint. Without
        // the keyset subscription the mint would never see this new keyset.
        let rotated = mint
            .signatory
            .rotate_keyset(RotateKeyArguments {
                unit: CurrencyUnit::default(),
                amounts,
                input_fee_ppk: 0,
                keyset_id_type: cdk_common::nut02::KeySetVersion::Version00,
                final_expiry: None,
            })
            .await
            .expect("rotate_keyset");

        assert!(
            !before.contains(&rotated.id),
            "rotated keyset should be new"
        );

        // The drain task should observe the pushed update and store it, so
        // polling the in-memory keysets eventually sees the rotated keyset.
        let applied = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if mint.keysets.load().iter().any(|k| k.id == rotated.id) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        assert!(
            applied.is_ok(),
            "signatory rotation did not propagate to the mint in time"
        );

        mint.stop().await.expect("mint should stop");
    }

    #[tokio::test]
    async fn verify_outputs_keyset_rejects_expired_keyset() {
        use cdk_common::nuts::SecretKey;
        use cdk_common::util::unix_time;

        let mut supported_units = HashMap::new();
        let amounts: Vec<u64> = (0..8).map(|i| 2u64.pow(i)).collect();
        supported_units.insert(CurrencyUnit::default(), (0, amounts));

        let config = MintConfig::<'_> {
            supported_units,
            ..Default::default()
        };
        let mint = create_mint(config).await;

        let current_keysets: Vec<SignatoryKeySet> = mint.keysets.load().as_ref().clone();
        let keyset_id = current_keysets[0].id;

        let expired_keysets: Vec<SignatoryKeySet> = current_keysets
            .into_iter()
            .map(|mut ks| {
                ks.final_expiry = Some(unix_time() - 1);
                ks
            })
            .collect();
        mint.keysets.store(Arc::new(expired_keysets));

        let blinded_secret = SecretKey::generate().public_key();
        let output = BlindedMessage::new(Amount::from(1), keyset_id, blinded_secret);

        let result = mint.verify_outputs_keyset(&[output]);

        assert!(
            matches!(result, Err(Error::ExpiredKeyset)),
            "expected ExpiredKeyset error, got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn verify_inputs_keyset_rejects_expired_keyset() {
        use cdk_common::nuts::{Proof, SecretKey};
        use cdk_common::secret::Secret;
        use cdk_common::util::unix_time;

        let mut supported_units = HashMap::new();
        let amounts: Vec<u64> = (0..8).map(|i| 2u64.pow(i)).collect();
        supported_units.insert(CurrencyUnit::default(), (0, amounts));

        let config = MintConfig::<'_> {
            supported_units,
            ..Default::default()
        };
        let mint = create_mint(config).await;

        let current_keysets: Vec<SignatoryKeySet> = mint.keysets.load().as_ref().clone();
        let keyset_id = current_keysets[0].id;

        let expired_keysets: Vec<SignatoryKeySet> = current_keysets
            .into_iter()
            .map(|mut ks| {
                ks.final_expiry = Some(unix_time() - 1);
                ks
            })
            .collect();
        mint.keysets.store(Arc::new(expired_keysets));

        // Expiry check runs before crypto, so an unsigned proof is fine here.
        let c = SecretKey::generate().public_key();
        let proof = Proof::new(Amount::from(1), keyset_id, Secret::generate(), c);

        let result = mint.verify_inputs_keyset(&vec![proof]).await;

        assert!(
            matches!(result, Err(Error::ExpiredKeyset)),
            "expected ExpiredKeyset error, got: {:?}",
            result
        );
    }

    // --- Dynamic payment processor registration -----------------------

    /// Full powers of 2 up to 2^31, 0 fee: matches `UnitConfig::default()`,
    /// so a brand-new unit's keyset can represent any test amount below.
    fn full_amounts() -> Vec<u64> {
        (0..32).map(|i| 2u64.pow(i)).collect()
    }

    fn fake_fee_reserve() -> FeeReserve {
        FeeReserve {
            min_fee_reserve: Amount::from(1),
            percent_fee_reserve: 1.0,
        }
    }

    /// A `FakeWallet` configured for `unit`, advertising `custom_method` as a
    /// supported custom payment method.
    fn custom_processor(unit: CurrencyUnit, custom_method: &str, payment_delay: u64) -> DynMintPayment {
        Arc::new(
            FakeWallet::new(
                fake_fee_reserve(),
                HashMap::default(),
                HashSet::default(),
                payment_delay,
                unit,
            )
            .with_custom_payment_methods(HashMap::from([(
                custom_method.to_string(),
                "{}".to_string(),
            )])),
        )
    }

    async fn create_custom_quote(
        mint: &Mint,
        unit: CurrencyUnit,
        method: PaymentMethod,
        amount: u64,
    ) -> Result<QuoteId, Error> {
        let request = MintQuoteRequest::Custom {
            method,
            request: MintQuoteCustomRequest {
                amount: Some(Amount::from(amount)),
                unit,
                description: None,
                pubkey: None,
                extra: serde_json::Value::Null,
            },
        };
        mint.get_mint_quote(request)
            .await
            .map(|response| response.quote().clone())
    }

    /// Polls the quote's stored state directly until `quote_id` reaches
    /// `Paid`, or panics after 5 seconds.
    ///
    /// Deliberately reads through `localstore()` rather than
    /// `check_mint_quotes`/`check_mint_quote_paid`: those also perform an
    /// active re-check against the processor map (unconditionally for
    /// non-bolt11 methods, and for bolt11 too while the quote is still
    /// `Unpaid`), which would make this helper itself depend on the
    /// processor still being registered. Reading the stored state instead
    /// observes only the push path (the payment-event consumer writing
    /// through `handle_payment_notification`), which is what an existing
    /// quote's "still gets paid after deregistration" guarantee rests on.
    async fn wait_until_quote_paid(mint: &Mint, quote_id: &QuoteId) {
        timeout(Duration::from_secs(5), async {
            loop {
                let quote = mint
                    .localstore()
                    .get_mint_quote(quote_id)
                    .await
                    .expect("read quote")
                    .expect("quote exists");
                if quote.state() == MintQuoteState::Paid {
                    return;
                }
                sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("quote did not reach Paid state in time");
    }

    /// Mints against `quote_id` in `unit` and returns the number of
    /// blind signatures returned.
    async fn mint_against_quote(mint: &Mint, unit: &CurrencyUnit, quote_id: QuoteId) -> usize {
        let keyset_id = *mint
            .get_active_keysets()
            .get(unit)
            .expect("active keyset for unit");

        let pre_mint = PreMintSecrets::random(
            keyset_id,
            Amount::from(32),
            &SplitTarget::default(),
            &(0, full_amounts()).into(),
        )
        .expect("premint secrets");
        let outputs = pre_mint.blinded_messages().to_vec();

        let request = MintRequest {
            quote: quote_id,
            outputs: outputs.clone(),
            signature: None,
        };

        let mint_res = mint
            .process_mint_request(MintInput::Single(request))
            .await
            .expect("mint against quote");

        assert_eq!(mint_res.signatures.len(), outputs.len());
        mint_res.signatures.len()
    }

    /// A thin `MintPayment` wrapper counting `start()` and
    /// `wait_payment_event()` calls, to verify the `Arc::ptr_eq` dedup: one
    /// processor instance registered under two keys must start and spawn a
    /// consumer exactly once.
    struct CountingProcessor {
        inner: FakeWallet,
        start_calls: Arc<std::sync::atomic::AtomicUsize>,
        wait_payment_event_calls: Arc<std::sync::atomic::AtomicUsize>,
        stop_calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl cdk_common::payment::MintPayment for CountingProcessor {
        type Err = cdk_common::payment::Error;

        async fn start(&self) -> Result<(), Self::Err> {
            self.start_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.start().await
        }

        async fn stop(&self) -> Result<(), Self::Err> {
            self.stop_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.stop().await
        }

        async fn get_settings(&self) -> Result<cdk_common::payment::SettingsResponse, Self::Err> {
            self.inner.get_settings().await
        }

        async fn create_incoming_payment_request(
            &self,
            options: cdk_common::payment::IncomingPaymentOptions,
        ) -> Result<cdk_common::payment::CreateIncomingPaymentResponse, Self::Err> {
            self.inner.create_incoming_payment_request(options).await
        }

        async fn get_payment_quote(
            &self,
            unit: &CurrencyUnit,
            options: cdk_common::payment::OutgoingPaymentOptions,
        ) -> Result<cdk_common::payment::PaymentQuoteResponse, Self::Err> {
            self.inner.get_payment_quote(unit, options).await
        }

        async fn make_payment(
            &self,
            unit: &CurrencyUnit,
            options: cdk_common::payment::OutgoingPaymentOptions,
        ) -> Result<MakePaymentResponse, Self::Err> {
            self.inner.make_payment(unit, options).await
        }

        async fn wait_payment_event(
            &self,
        ) -> Result<Pin<Box<dyn Stream<Item = cdk_common::payment::Event> + Send>>, Self::Err>
        {
            self.wait_payment_event_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.wait_payment_event().await
        }

        fn is_payment_event_stream_active(&self) -> bool {
            self.inner.is_payment_event_stream_active()
        }

        fn cancel_payment_event_stream(&self) {
            self.inner.cancel_payment_event_stream()
        }

        async fn check_incoming_payment_status(
            &self,
            payment_identifier: &PaymentIdentifier,
        ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
            self.inner
                .check_incoming_payment_status(payment_identifier)
                .await
        }

        async fn check_outgoing_payment(
            &self,
            payment_identifier: &PaymentIdentifier,
        ) -> Result<MakePaymentResponse, Self::Err> {
            self.inner.check_outgoing_payment(payment_identifier).await
        }
    }

    /// Spec case 1: register a custom-method processor on a started mint for
    /// a brand-new unit; create and pay a quote in that unit; mint against it.
    #[tokio::test]
    async fn register_payment_processor_custom_unit_mint_end_to_end() {
        let mint = create_test_mint().await.expect("test mint");

        let unit = CurrencyUnit::Custom("custom_unit".into());
        let method = PaymentMethod::Custom("custom_backend_method".to_string());

        // The signatory already creates keysets for brand-new units; callers
        // just need a non-empty amounts vector.
        mint.rotate_keyset(unit.clone(), full_amounts(), 0, true, None)
            .await
            .expect("rotate keyset for new unit");

        let processor = custom_processor(unit.clone(), method.as_str(), 0);
        mint.register_payment_processor(
            unit.clone(),
            method.clone(),
            MintMeltLimits::new(1, 10_000),
            processor,
        )
        .await
        .expect("register payment processor");

        let quote_id = create_custom_quote(&mint, unit.clone(), method.clone(), 32)
            .await
            .expect("create custom quote");
        wait_until_quote_paid(&mint, &quote_id).await;

        mint_against_quote(&mint, &unit, quote_id).await;
    }

    /// Spec case 2: deregister; new quote creation fails `UnsupportedUnit`;
    /// an existing unpaid quote can still be paid (via the still-running
    /// payment-event consumer) and minted. This holds for bolt11-method
    /// quotes because `check_mint_quote_payments` skips re-checking the
    /// backend once a bolt11 quote is `Paid`/`Issued` — see the
    /// `deregister_custom_method_existing_quote_cannot_be_minted` test below
    /// for the custom-method case, where that skip doesn't apply.
    #[tokio::test]
    async fn deregister_blocks_new_quotes_existing_quote_still_payable_and_mintable() {
        let mint = create_test_mint().await.expect("test mint");

        // Msat rather than a `Custom` unit: `FakeWallet`'s bolt11 path only
        // knows how to convert Sat/Msat/fiat amounts, and a `Custom` unit
        // can't be converted, which would make invoice creation fail for a
        // reason unrelated to what this test checks.
        let unit = CurrencyUnit::Msat;
        let method = PaymentMethod::Known(KnownMethod::Bolt11);

        mint.rotate_keyset(unit.clone(), full_amounts(), 0, true, None)
            .await
            .expect("rotate keyset for new unit");

        // Pays out after 1s: long enough to deregister first, so the
        // payment genuinely arrives after deregistration.
        let processor: DynMintPayment = Arc::new(FakeWallet::new(
            fake_fee_reserve(),
            HashMap::default(),
            HashSet::default(),
            1,
            unit.clone(),
        ));
        mint.register_payment_processor(
            unit.clone(),
            method.clone(),
            MintMeltLimits::new(1, 10_000),
            processor,
        )
        .await
        .expect("register payment processor");

        let quote_response = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(32),
                    unit: unit.clone(),
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .expect("create quote while registered");
        let quote_id = quote_response.quote().clone();

        mint.deregister_payment_processor(unit.clone(), method.clone())
            .await
            .expect("deregister");

        // No new quote can be created for the deregistered pair.
        let new_quote_result = mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(16),
                    unit: unit.clone(),
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await;
        assert!(
            matches!(new_quote_result, Err(Error::UnsupportedUnit)),
            "expected UnsupportedUnit, got {:?}",
            new_quote_result
        );

        // The pre-existing quote still gets paid (the consumer task spawned
        // at registration keeps running) and can still be minted.
        wait_until_quote_paid(&mint, &quote_id).await;
        mint_against_quote(&mint, &unit, quote_id).await;
    }

    /// Pins down `deregister_payment_processor`'s deliberate full-teardown
    /// behavior, not a latent bug: for a custom-method quote, deregistering
    /// its processor DOES strand an already-paid existing quote, unlike the
    /// bolt11 case above. `check_mint_quote_payments` only short-circuits
    /// the backend re-check for bolt11 (to support custom methods whose
    /// quotes can receive further incremental payments after reaching
    /// `Paid`); every other method keeps consulting the processor map on
    /// every check/mint call, so once the
    /// map entry is gone, `UnsupportedUnit` follows even for an
    /// already-fully-paid quote. This is exactly the case
    /// `retire_payment_processor` exists for; see
    /// `retire_lets_paid_custom_method_quote_be_minted` below.
    #[tokio::test]
    async fn deregister_custom_method_existing_quote_cannot_be_minted() {
        let mint = create_test_mint().await.expect("test mint");

        let unit = CurrencyUnit::Custom("custom_unit_gap".into());
        let method = PaymentMethod::Custom("custom_backend_method".to_string());

        mint.rotate_keyset(unit.clone(), full_amounts(), 0, true, None)
            .await
            .expect("rotate keyset for new unit");

        let processor = custom_processor(unit.clone(), method.as_str(), 0);
        mint.register_payment_processor(
            unit.clone(),
            method.clone(),
            MintMeltLimits::new(1, 10_000),
            processor,
        )
        .await
        .expect("register payment processor");

        let quote_id = create_custom_quote(&mint, unit.clone(), method.clone(), 32)
            .await
            .expect("create custom quote");
        wait_until_quote_paid(&mint, &quote_id).await;

        mint.deregister_payment_processor(unit.clone(), method.clone())
            .await
            .expect("deregister");

        let keyset_id = *mint
            .get_active_keysets()
            .get(&unit)
            .expect("active keyset for unit");
        let pre_mint = PreMintSecrets::random(
            keyset_id,
            Amount::from(32),
            &SplitTarget::default(),
            &(0, full_amounts()).into(),
        )
        .expect("premint secrets");
        let request = MintRequest {
            quote: quote_id,
            outputs: pre_mint.blinded_messages().to_vec(),
            signature: None,
        };

        let result = mint.process_mint_request(MintInput::Single(request)).await;
        assert!(
            matches!(result, Err(Error::UnsupportedUnit)),
            "expected the pre-existing custom-method recheck gap to surface as \
             UnsupportedUnit, got {:?}",
            result
        );
    }

    /// The case `retire_payment_processor` exists for: a custom-method quote
    /// that is already paid but not yet issued must still be mintable after
    /// retiring its processor, unlike after deregistering it (the test
    /// directly above this one). Retiring removes only the NUT-04/NUT-05
    /// gate and keeps the processor map entry, so the mint path still
    /// resolves the processor and genuinely signs against the quote.
    #[tokio::test]
    async fn retire_lets_paid_custom_method_quote_be_minted() {
        let mint = create_test_mint().await.expect("test mint");

        let unit = CurrencyUnit::Custom("custom_unit_retire_mint".into());
        let method = PaymentMethod::Custom("custom_backend_method".to_string());

        mint.rotate_keyset(unit.clone(), full_amounts(), 0, true, None)
            .await
            .expect("rotate keyset for new unit");

        let processor = custom_processor(unit.clone(), method.as_str(), 0);
        mint.register_payment_processor(
            unit.clone(),
            method.clone(),
            MintMeltLimits::new(1, 10_000),
            processor,
        )
        .await
        .expect("register payment processor");

        let quote_id = create_custom_quote(&mint, unit.clone(), method.clone(), 32)
            .await
            .expect("create custom quote");
        wait_until_quote_paid(&mint, &quote_id).await;

        mint.retire_payment_processor(unit.clone(), method.clone())
            .await
            .expect("retire");

        // The point of the test: minting against the already-paid quote
        // actually succeeds (real signatures, not just "no error") once the
        // processor is merely retired rather than fully deregistered.
        mint_against_quote(&mint, &unit, quote_id).await;
    }

    /// After retire, no new quote can be created for the retired pair.
    #[tokio::test]
    async fn retire_blocks_new_quote_creation() {
        let mint = create_test_mint().await.expect("test mint");

        let unit = CurrencyUnit::Custom("custom_unit_retire_block".into());
        let method = PaymentMethod::Custom("custom_backend_method".to_string());

        mint.rotate_keyset(unit.clone(), full_amounts(), 0, true, None)
            .await
            .expect("rotate keyset for new unit");

        mint.register_payment_processor(
            unit.clone(),
            method.clone(),
            MintMeltLimits::new(1, 10_000),
            custom_processor(unit.clone(), method.as_str(), 0),
        )
        .await
        .expect("register payment processor");

        mint.retire_payment_processor(unit.clone(), method.clone())
            .await
            .expect("retire");

        let result = create_custom_quote(&mint, unit.clone(), method.clone(), 10).await;
        assert!(
            matches!(result, Err(Error::UnsupportedUnit)),
            "expected UnsupportedUnit for a retired pair, got {:?}",
            result
        );
    }

    /// Retiring one of two keys sharing a processor instance leaves the
    /// other key fully functional and never stops the processor (no
    /// start/stop refcounting is touched by retire at all).
    #[tokio::test]
    async fn retire_leaves_shared_instance_functional_and_not_stopped() {
        let mint = create_test_mint().await.expect("test mint");

        let unit_a = CurrencyUnit::Custom("custom_unit_retire_shared_a".into());
        let unit_b = CurrencyUnit::Custom("custom_unit_retire_shared_b".into());
        let method = PaymentMethod::Custom("custom_backend_method".to_string());

        for unit in [&unit_a, &unit_b] {
            mint.rotate_keyset(unit.clone(), full_amounts(), 0, true, None)
                .await
                .expect("rotate keyset");
        }

        let inner = FakeWallet::new(
            fake_fee_reserve(),
            HashMap::default(),
            HashSet::default(),
            0,
            unit_a.clone(),
        )
        .with_custom_payment_methods(HashMap::from([(
            method.as_str().to_string(),
            "{}".to_string(),
        )]));
        let stop_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let processor: DynMintPayment = Arc::new(CountingProcessor {
            inner,
            start_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            wait_payment_event_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            stop_calls: stop_calls.clone(),
        });

        mint.register_payment_processor(
            unit_a.clone(),
            method.clone(),
            MintMeltLimits::new(1, 10_000),
            processor.clone(),
        )
        .await
        .expect("register under unit_a");
        mint.register_payment_processor(
            unit_b.clone(),
            method.clone(),
            MintMeltLimits::new(1, 10_000),
            processor.clone(),
        )
        .await
        .expect("register under unit_b");

        mint.retire_payment_processor(unit_a.clone(), method.clone())
            .await
            .expect("retire unit_a");

        assert_eq!(
            stop_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "retire must never stop the processor"
        );

        let unit_a_result = create_custom_quote(&mint, unit_a.clone(), method.clone(), 10).await;
        assert!(
            matches!(unit_a_result, Err(Error::UnsupportedUnit)),
            "expected UnsupportedUnit for retired unit_a, got {:?}",
            unit_a_result
        );

        create_custom_quote(&mint, unit_b.clone(), method.clone(), 10)
            .await
            .expect("unit_b still functional after unit_a's retirement");
    }

    /// Retiring an unregistered `(unit, method)` is a no-op.
    #[tokio::test]
    async fn retire_payment_processor_unregistered_is_noop() {
        let mint = create_test_mint().await.expect("test mint");
        let unit = CurrencyUnit::Custom("custom_unit_retire_never_registered".into());
        let method = PaymentMethod::Custom("custom_backend_method".to_string());

        mint.retire_payment_processor(unit, method)
            .await
            .expect("retiring an unknown pair is a no-op");
    }

    /// Spec case 3: the same processor instance registered under a second
    /// key starts and spawns a consumer exactly once; deregistering one key
    /// leaves the other functional.
    #[tokio::test]
    async fn register_shared_instance_single_start_independent_deregister() {
        let mint = create_test_mint().await.expect("test mint");

        let unit_a = CurrencyUnit::Custom("custom_unit_shared_a".into());
        let unit_b = CurrencyUnit::Custom("custom_unit_shared_b".into());
        let method = PaymentMethod::Custom("custom_backend_method".to_string());

        for unit in [&unit_a, &unit_b] {
            mint.rotate_keyset(unit.clone(), full_amounts(), 0, true, None)
                .await
                .expect("rotate keyset");
        }

        // `CountingProcessor` needs the concrete `FakeWallet` to delegate
        // to, so build it directly rather than through `custom_processor`
        // (which returns an already type-erased `DynMintPayment`).
        let inner = FakeWallet::new(
            fake_fee_reserve(),
            HashMap::default(),
            HashSet::default(),
            0,
            unit_a.clone(),
        )
        .with_custom_payment_methods(HashMap::from([(
            method.as_str().to_string(),
            "{}".to_string(),
        )]));
        let start_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let wait_payment_event_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let processor: DynMintPayment = Arc::new(CountingProcessor {
            inner,
            start_calls: start_calls.clone(),
            wait_payment_event_calls: wait_payment_event_calls.clone(),
            stop_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });

        mint.register_payment_processor(
            unit_a.clone(),
            method.clone(),
            MintMeltLimits::new(1, 10_000),
            processor.clone(),
        )
        .await
        .expect("register under unit_a");
        mint.register_payment_processor(
            unit_b.clone(),
            method.clone(),
            MintMeltLimits::new(1, 10_000),
            processor.clone(),
        )
        .await
        .expect("register under unit_b");

        // Give the consumer-spawn channel a moment to be drained.
        sleep(std::time::Duration::from_millis(200)).await;

        assert_eq!(
            start_calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a shared instance must be started exactly once"
        );
        assert_eq!(
            wait_payment_event_calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a shared instance must get exactly one payment-event consumer"
        );

        // Both units are quotable off the single shared instance.
        create_custom_quote(&mint, unit_a.clone(), method.clone(), 10)
            .await
            .expect("quote on unit_a");
        create_custom_quote(&mint, unit_b.clone(), method.clone(), 10)
            .await
            .expect("quote on unit_b");

        // Deregistering unit_a leaves unit_b functional.
        mint.deregister_payment_processor(unit_a.clone(), method.clone())
            .await
            .expect("deregister unit_a");

        let unit_a_result =
            create_custom_quote(&mint, unit_a.clone(), method.clone(), 10).await;
        assert!(
            matches!(unit_a_result, Err(Error::UnsupportedUnit)),
            "expected UnsupportedUnit for deregistered unit_a, got {:?}",
            unit_a_result
        );

        create_custom_quote(&mint, unit_b.clone(), method.clone(), 10)
            .await
            .expect("unit_b still functional after unit_a's deregistration");
    }

    /// Spec case 4 (crash-shaped): simulates a crash between phase 1 (map
    /// insert) and phase 2 (mint-info update) of `register_payment_processor`
    /// by performing only the map insert directly. The mint must still
    /// refuse quote creation for the unit: an inert map entry alone never
    /// makes a unit quotable.
    #[tokio::test]
    async fn register_interrupted_between_phases_leaves_mint_refusing_quotes() {
        let mint = create_test_mint().await.expect("test mint");

        let unit = CurrencyUnit::Custom("custom_unit_crash".into());
        let method = PaymentMethod::Custom("custom_backend_method".to_string());
        let key = PaymentProcessorKey::new(unit.clone(), method.clone());

        mint.rotate_keyset(unit.clone(), full_amounts(), 0, true, None)
            .await
            .expect("rotate keyset for new unit");

        let processor = custom_processor(unit.clone(), method.as_str(), 0);

        // Phase 1 only: mirrors `register_payment_processor`'s map update,
        // skipping phase 2 (the mint-info gate) to stand in for a crash
        // between the two phases.
        {
            let _guard = mint.payment_processor_lock.lock().await;
            mint.insert_payment_processor(key, processor);
        }

        let result = create_custom_quote(&mint, unit.clone(), method.clone(), 10).await;
        assert!(
            matches!(result, Err(Error::UnsupportedUnit)),
            "an inert map entry without the mint-info gate must still refuse quotes, got {:?}",
            result
        );
    }

    /// Duplicate `(unit, method)` registration is rejected, mirroring
    /// `MintBuilder::add_payment_processor`.
    #[tokio::test]
    async fn register_payment_processor_rejects_duplicate() {
        let mint = create_test_mint().await.expect("test mint");

        let unit = CurrencyUnit::Custom("custom_unit_dup".into());
        let method = PaymentMethod::Custom("custom_backend_method".to_string());

        mint.rotate_keyset(unit.clone(), full_amounts(), 0, true, None)
            .await
            .expect("rotate keyset for new unit");

        mint.register_payment_processor(
            unit.clone(),
            method.clone(),
            MintMeltLimits::new(1, 10_000),
            custom_processor(unit.clone(), method.as_str(), 0),
        )
        .await
        .expect("first registration succeeds");

        let result = mint
            .register_payment_processor(
                unit.clone(),
                method.clone(),
                MintMeltLimits::new(1, 10_000),
                custom_processor(unit.clone(), method.as_str(), 0),
            )
            .await;
        assert!(
            matches!(result, Err(Error::Custom(_))),
            "expected duplicate registration to be rejected, got {:?}",
            result
        );
    }

    /// Deregistering a `(unit, method)` that was never registered is a no-op.
    #[tokio::test]
    async fn deregister_payment_processor_unregistered_is_noop() {
        let mint = create_test_mint().await.expect("test mint");
        let unit = CurrencyUnit::Custom("custom_unit_never_registered".into());
        let method = PaymentMethod::Custom("custom_backend_method".to_string());

        mint.deregister_payment_processor(unit, method)
            .await
            .expect("deregistering an unknown pair is a no-op");
    }

    /// Simulates a restart: the in-memory processor map is fresh (as it is
    /// on every new process) but `mint_info` already carries the NUT-04/
    /// NUT-05 entries a previous process persisted for this key. A
    /// re-register must replace those entries, not add to them.
    #[tokio::test]
    async fn register_payment_processor_idempotent_against_persisted_settings() {
        let mint = create_test_mint().await.expect("test mint");

        let unit = CurrencyUnit::Custom("custom_restart".into());
        let method = PaymentMethod::Custom("custom_method".to_string());
        let key = PaymentProcessorKey::new(unit.clone(), method.clone());

        mint.rotate_keyset(unit.clone(), full_amounts(), 0, true, None)
            .await
            .expect("rotate keyset for new unit");

        mint.register_payment_processor(
            unit.clone(),
            method.clone(),
            MintMeltLimits::new(1, 10_000),
            custom_processor(unit.clone(), method.as_str(), 0),
        )
        .await
        .expect("first registration persists settings");

        // Drop the map entry directly, bypassing deregister (which would
        // also clear the persisted settings): this leaves mint_info exactly
        // as a previous process would have left it on disk, with the
        // in-memory map empty for this key, as it is on every fresh start.
        {
            let _guard = mint.payment_processor_lock.lock().await;
            mint.remove_payment_processor(&key);
        }

        mint.register_payment_processor(
            unit.clone(),
            method.clone(),
            MintMeltLimits::new(1, 10_000),
            custom_processor(unit.clone(), method.as_str(), 0),
        )
        .await
        .expect("re-registration after simulated restart succeeds");

        let mint_info = mint.mint_info().await.expect("mint info");
        let nut04_count = mint_info
            .nuts
            .nut04
            .methods
            .iter()
            .filter(|m| m.unit == unit && m.method == method)
            .count();
        let nut05_count = mint_info
            .nuts
            .nut05
            .methods
            .iter()
            .filter(|m| m.unit == unit && m.method == method)
            .count();
        assert_eq!(
            nut04_count, 1,
            "expected exactly one persisted NUT-04 entry after re-register, got {}",
            nut04_count
        );
        assert_eq!(
            nut05_count, 1,
            "expected exactly one persisted NUT-05 entry after re-register, got {}",
            nut05_count
        );
    }
}
