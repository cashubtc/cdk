//! Cashu Mint

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use cdk_common::amount::to_unit;
use cdk_common::common::{PaymentProcessorKey, QuoteTTL};
#[cfg(feature = "auth")]
use cdk_common::database::DynMintAuthDatabase;
use cdk_common::database::{self, Acquired, DynMintDatabase};
use cdk_common::nuts::{BlindSignature, BlindedMessage, CurrencyUnit, Id};
use cdk_common::payment::{DynMintPayment, WaitPaymentResponse};
pub use cdk_common::quote_id::QuoteId;
#[cfg(feature = "prometheus")]
use cdk_prometheus::global;
use cdk_signatory::signatory::{Signatory, SignatoryKeySet};
use futures::StreamExt;
#[cfg(feature = "auth")]
use nut21::ProtectedEndpoint;
use subscription::PubSubManager;
use tokio::sync::{Mutex, Notify};
use tokio::task::{JoinHandle, JoinSet};
use tracing::instrument;

use crate::error::Error;
use crate::fees::calculate_fee;
use crate::nuts::*;
use crate::Amount;
#[cfg(feature = "auth")]
use crate::OidcClient;

#[cfg(feature = "auth")]
pub(crate) mod auth;
mod builder;
mod check_spendable;
mod issue;
mod keysets;
mod ln;
mod melt;
mod start_up_check;
mod subscription;
mod swap;
mod verification;

pub use builder::{MintBuilder, MintMeltLimits};
pub use cdk_common::mint::{MeltQuote, MintKeySetInfo, MintQuote};
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
    #[cfg(feature = "auth")]
    auth_localstore: Option<DynMintAuthDatabase>,
    /// Payment processors for mint
    payment_processors: Arc<HashMap<PaymentProcessorKey, DynMintPayment>>,
    /// Subscription manager
    pubsub_manager: Arc<PubSubManager>,
    #[cfg(feature = "auth")]
    oidc_client: Option<OidcClient>,
    /// In-memory keyset
    keysets: Arc<ArcSwap<Vec<SignatoryKeySet>>>,
    /// Background task management
    task_state: Arc<Mutex<TaskState>>,
}

/// State for managing background tasks
#[derive(Default)]
struct TaskState {
    /// Shutdown signal for all background tasks
    shutdown_notify: Option<Arc<Notify>>,
    /// Handle to the main supervisor task
    supervisor_handle: Option<JoinHandle<Result<(), Error>>>,
}

impl Mint {
    /// Create new [`Mint`] without authentication
    pub async fn new(
        mint_info: MintInfo,
        signatory: Arc<dyn Signatory + Send + Sync>,
        localstore: DynMintDatabase,
        payment_processors: HashMap<PaymentProcessorKey, DynMintPayment>,
    ) -> Result<Self, Error> {
        Self::new_internal(
            mint_info,
            signatory,
            localstore,
            #[cfg(feature = "auth")]
            None,
            payment_processors,
        )
        .await
    }

    /// Create new [`Mint`] with authentication support
    #[cfg(feature = "auth")]
    pub async fn new_with_auth(
        mint_info: MintInfo,
        signatory: Arc<dyn Signatory + Send + Sync>,
        localstore: DynMintDatabase,
        auth_localstore: DynMintAuthDatabase,
        payment_processors: HashMap<PaymentProcessorKey, DynMintPayment>,
    ) -> Result<Self, Error> {
        Self::new_internal(
            mint_info,
            signatory,
            localstore,
            Some(auth_localstore),
            payment_processors,
        )
        .await
    }

    /// Internal function to create a new [`Mint`] with shared logic
    #[inline]
    async fn new_internal(
        mint_info: MintInfo,
        signatory: Arc<dyn Signatory + Send + Sync>,
        localstore: DynMintDatabase,
        #[cfg(feature = "auth")] auth_localstore: Option<DynMintAuthDatabase>,
        payment_processors: HashMap<PaymentProcessorKey, DynMintPayment>,
    ) -> Result<Self, Error> {
        let keysets = signatory.keysets().await?;
        if !keysets
            .keysets
            .iter()
            .any(|keyset| keyset.active && keyset.unit != CurrencyUnit::Auth)
        {
            return Err(Error::NoActiveKeyset);
        }

        tracing::info!(
            "Using Signatory {} with {} active keys",
            signatory.name(),
            keysets
                .keysets
                .iter()
                .filter(|keyset| keyset.active && keyset.unit != CurrencyUnit::Auth)
                .count()
        );

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

        let payment_processors = Arc::new(payment_processors);

        Ok(Self {
            signatory,
            pubsub_manager: PubSubManager::new((localstore.clone(), payment_processors.clone())),
            localstore,
            #[cfg(feature = "auth")]
            oidc_client: computed_info.nuts.nut21.as_ref().map(|nut21| {
                OidcClient::new(
                    nut21.openid_discovery.clone(),
                    Some(nut21.client_id.clone()),
                )
            }),
            payment_processors,
            #[cfg(feature = "auth")]
            auth_localstore,
            keysets: Arc::new(ArcSwap::new(keysets.keysets.into())),
            task_state: Arc::new(Mutex::new(TaskState::default())),
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
        // Now includes checking payment status with LN backend to determine
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
        let mut seen_processors = Vec::new();
        for (key, processor) in self.payment_processors.iter() {
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

        // Clone required components for the background task
        let payment_processors = self.payment_processors.clone();
        let localstore = Arc::clone(&self.localstore);
        let pubsub_manager = Arc::clone(&self.pubsub_manager);
        let shutdown_clone = shutdown_notify.clone();

        // Spawn the supervisor task
        let supervisor_handle = tokio::spawn(async move {
            Self::wait_for_paid_invoices(
                &payment_processors,
                localstore,
                pubsub_manager,
                shutdown_clone,
            )
            .await
        });

        // Store the handles
        task_state.shutdown_notify = Some(shutdown_notify);
        task_state.supervisor_handle = Some(supervisor_handle);

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

        // Stop all payment processors
        self.stop_payment_processors().await?;

        result
    }

    /// Stop all payment processors
    async fn stop_payment_processors(&self) -> Result<(), Error> {
        tracing::info!("Stopping payment processors...");
        let mut seen_processors = Vec::new();

        for (key, processor) in self.payment_processors.iter() {
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

    /// Get the payment processor for the given unit and payment method
    pub fn get_payment_processor(
        &self,
        unit: CurrencyUnit,
        payment_method: PaymentMethod,
    ) -> Result<DynMintPayment, Error> {
        let key = PaymentProcessorKey::new(unit.clone(), payment_method.clone());
        self.payment_processors.get(&key).cloned().ok_or_else(|| {
            tracing::info!(
                "No payment processor set for pair {}, {}",
                unit,
                payment_method
            );
            Error::UnsupportedUnit
        })
    }

    /// Localstore
    pub fn localstore(&self) -> DynMintDatabase {
        Arc::clone(&self.localstore)
    }

    /// Pub Sub manager
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

        #[cfg(feature = "auth")]
        let mint_info = if let Some(auth_db) = self.auth_localstore.as_ref() {
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
        payment_processors: &HashMap<PaymentProcessorKey, DynMintPayment>,
        localstore: DynMintDatabase,
        pubsub_manager: Arc<PubSubManager>,
        shutdown: Arc<Notify>,
    ) -> Result<(), Error> {
        let mut join_set = JoinSet::new();

        // Group processors by unique instance (using Arc pointer equality)
        let mut seen_processors = Vec::new();
        for (key, processor) in payment_processors {
            // Skip if processor is already active
            if processor.is_wait_invoice_active() {
                continue;
            }

            // Skip if we've already spawned a task for this processor instance
            if seen_processors.iter().any(|p| Arc::ptr_eq(p, processor)) {
                continue;
            }

            seen_processors.push(Arc::clone(processor));

            tracing::info!("Starting payment wait task for {:?}", key);

            // Clone for the spawned task
            let processor = Arc::clone(processor);
            let localstore = Arc::clone(&localstore);
            let pubsub_manager = Arc::clone(&pubsub_manager);
            let shutdown = Arc::clone(&shutdown);

            join_set.spawn(async move {
                let result = Self::wait_for_processor_payments(
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

        // If no payment processors, just wait for shutdown
        if join_set.is_empty() {
            shutdown.notified().await;
        } else {
            // Wait for shutdown or all tasks to complete
            loop {
                tokio::select! {
                    _ = shutdown.notified() => {
                        tracing::info!("Shutting down payment processors");
                        break;
                    }
                    Some(result) = join_set.join_next() => {
                        if let Err(e) = result {
                            tracing::warn!("Task panicked: {:?}", e);
                        }
                    }
                    else => break, // All tasks completed
                }
            }
        }

        join_set.shutdown().await;
        Ok(())
    }

    /// Handles payment waiting for a single processor
    #[instrument(skip_all)]
    async fn wait_for_processor_payments(
        processor: DynMintPayment,
        localstore: DynMintDatabase,
        pubsub_manager: Arc<PubSubManager>,
        shutdown: Arc<Notify>,
    ) -> Result<(), Error> {
        loop {
            tokio::select! {
                _ = shutdown.notified() => {
                    processor.cancel_wait_invoice();
                    break;
                }
                result = processor.wait_payment_event() => {
                    match result {
                        Ok(mut stream) => {
                            while let Some(event) = stream.next().await {
                                match event {
                                    cdk_common::payment::Event::PaymentReceived(wait_payment_response) => {
                                        if let Err(e) = Self::handle_payment_notification(
                                            &localstore,
                                            &pubsub_manager,
                                            wait_payment_response,
                                        ).await {
                                            tracing::warn!("Payment notification error: {:?}", e);
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to get payment stream: {}", e);
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Handle payment notification without needing full Mint instance
    /// This is a helper function that can be called with just the required components
    #[instrument(skip_all)]
    async fn handle_payment_notification(
        localstore: &DynMintDatabase,
        pubsub_manager: &Arc<PubSubManager>,
        wait_payment_response: WaitPaymentResponse,
    ) -> Result<(), Error> {
        if wait_payment_response.payment_amount == Amount::ZERO {
            tracing::warn!(
                "Received payment response with 0 amount with payment id {}.",
                wait_payment_response.payment_id
            );
            return Err(Error::AmountUndefined);
        }

        let mut tx = localstore.begin_transaction().await?;

        if let Ok(Some(mut mint_quote)) = tx
            .get_mint_quote_by_request_lookup_id(&wait_payment_response.payment_identifier)
            .await
        {
            Self::handle_mint_quote_payment(
                &mut tx,
                &mut mint_quote,
                wait_payment_response,
                pubsub_manager,
            )
            .await?;
        } else {
            tracing::warn!(
                "Could not get request for request lookup id {:?}",
                wait_payment_response.payment_identifier
            );
        }

        tx.commit().await?;
        Ok(())
    }

    /// Handle payment for a specific mint quote (extracted from pay_mint_quote)
    #[instrument(skip_all)]
    async fn handle_mint_quote_payment(
        tx: &mut Box<dyn database::MintTransaction<database::Error> + Send + Sync>,
        mint_quote: &mut Acquired<MintQuote>,
        wait_payment_response: WaitPaymentResponse,
        pubsub_manager: &Arc<PubSubManager>,
    ) -> Result<(), Error> {
        tracing::debug!(
            "Received payment notification of {} {} for mint quote {} with payment id {}",
            wait_payment_response.payment_amount,
            wait_payment_response.unit,
            mint_quote.id,
            wait_payment_response.payment_id.to_string()
        );

        let quote_state = mint_quote.state();
        if !mint_quote
            .payment_ids()
            .contains(&&wait_payment_response.payment_id)
        {
            if mint_quote.payment_method == PaymentMethod::Bolt11
                && (quote_state == MintQuoteState::Issued || quote_state == MintQuoteState::Paid)
            {
                tracing::info!("Received payment notification for already issued quote.");
            } else {
                let payment_amount_quote_unit = to_unit(
                    wait_payment_response.payment_amount,
                    &wait_payment_response.unit,
                    &mint_quote.unit,
                )?;

                if payment_amount_quote_unit == Amount::ZERO {
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
                        pubsub_manager.mint_quote_payment(mint_quote, mint_quote.amount_paid());
                    }
                    Err(Error::DuplicatePaymentId) => {
                        tracing::info!(
                            "Payment ID {} already processed (caught race condition)",
                            wait_payment_response.payment_id
                        );
                        // This is fine - another concurrent request already processed this payment
                    }
                    Err(e) => return Err(e),
                }
            }
        } else {
            tracing::info!("Received payment notification for already seen payment.");
        }

        Ok(())
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
        global::inc_in_flight_requests("blind_sign");

        let result = self.signatory.blind_sign(blinded_message).await;

        #[cfg(feature = "prometheus")]
        {
            global::dec_in_flight_requests("blind_sign");
            global::record_mint_operation("blind_sign", result.is_ok());
        }

        result
    }

    /// Verify [`Proof`] meets conditions and is signed
    #[tracing::instrument(skip_all)]
    pub async fn verify_proofs(&self, proofs: Proofs) -> Result<(), Error> {
        // This ignore P2PK and HTLC, as all NUT-10 spending conditions are
        // checked elsewhere.
        #[cfg(feature = "prometheus")]
        global::inc_in_flight_requests("verify_proofs");

        let result = self.signatory.verify_proofs(proofs).await;

        #[cfg(feature = "prometheus")]
        {
            global::dec_in_flight_requests("verify_proofs");
            global::record_mint_operation("verify_proofs", result.is_ok());
        }

        result
    }

    /// Restore
    #[instrument(skip_all)]
    pub async fn restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        #[cfg(feature = "prometheus")]
        global::inc_in_flight_requests("restore");

        let result = async {
            let output_len = request.outputs.len();

            let mut outputs = Vec::with_capacity(output_len);
            let mut signatures = Vec::with_capacity(output_len);

            let blinded_message: Vec<PublicKey> =
                request.outputs.iter().map(|b| b.blinded_secret).collect();

            let blinded_signatures = self
                .localstore
                .get_blind_signatures(&blinded_message)
                .await?;

            assert_eq!(blinded_signatures.len(), output_len);

            for (blinded_message, blinded_signature) in
                request.outputs.into_iter().zip(blinded_signatures)
            {
                if let Some(blinded_signature) = blinded_signature {
                    outputs.push(blinded_message);
                    signatures.push(blinded_signature);
                }
            }

            Ok(RestoreResponse {
                outputs,
                signatures: signatures.clone(),
                promises: Some(signatures),
            })
        }
        .await;

        #[cfg(feature = "prometheus")]
        {
            global::dec_in_flight_requests("restore");
            global::record_mint_operation("restore", result.is_ok());
        }

        result
    }

    /// Get the total amount issed by keyset
    #[instrument(skip_all)]
    pub async fn total_issued(&self) -> Result<HashMap<Id, Amount>, Error> {
        #[cfg(feature = "prometheus")]
        global::inc_in_flight_requests("total_issued");

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
            global::dec_in_flight_requests("total_issued");
            global::record_mint_operation("total_issued", result.is_ok());
        }

        result
    }

    /// Total redeemed for keyset
    #[instrument(skip_all)]
    pub async fn total_redeemed(&self) -> Result<HashMap<Id, Amount>, Error> {
        #[cfg(feature = "prometheus")]
        global::inc_in_flight_requests("total_redeemed");

        let total_redeemed = async {
            let mut total_redeemed = self.localstore.get_total_redeemed().await?;
            for keyset in self.keysets().keysets {
                total_redeemed.entry(keyset.id).or_default();
            }
            Ok(total_redeemed)
        }
        .await;

        #[cfg(feature = "prometheus")]
        global::dec_in_flight_requests("total_redeemed");

        total_redeemed
    }
}

#[cfg(test)]
mod tests {

    use std::str::FromStr;

    use cdk_signatory::signatory::RotateKeyArguments;
    use cdk_sqlite::mint::memory::new_with_state;

    use super::*;

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
        supported_units: HashMap<CurrencyUnit, (u64, u8)>,
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
                config.supported_units,
                HashMap::new(),
            )
            .await
            .expect("Failed to create signatory"),
        );

        Mint::new(MintInfo::default(), signatory, localstore, HashMap::new())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn mint_mod_new_mint() {
        let mut supported_units = HashMap::new();
        supported_units.insert(CurrencyUnit::default(), (0, 32));
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
        supported_units.insert(CurrencyUnit::default(), (0, 32));

        let config = MintConfig::<'_> {
            supported_units,
            ..Default::default()
        };
        let mint = create_mint(config).await;

        let keysets = mint.keysets();
        let first_keyset_id = keysets.keysets[0].id;

        // set the first keyset to inactive and generate a new keyset
        mint.rotate_keyset(CurrencyUnit::default(), vec![1], 1)
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
    async fn test_mint_keyset_gen() {
        let seed = bip39::Mnemonic::from_str(
            "dismiss price public alone audit gallery ignore process swap dance crane furnace",
        )
        .unwrap();
        let mut supported_units = HashMap::new();
        supported_units.insert(CurrencyUnit::default(), (0, 32));

        let config = MintConfig::<'_> {
            seed: &seed.to_seed_normalized(""),
            supported_units,
            ..Default::default()
        };
        let mint = create_mint(config).await;

        let keys = mint.pubkeys();

        assert_eq!("00417bedcff77ec8", keys.keysets[0].id.to_string());

        assert_eq!(
            "0294c8092579e6e7ab1e07f4e82b3da6337c9f2fdd6a64e1a5e8a58fd327db359d",
            keys.keysets[0].keys.get(&Amount::from(1)).unwrap().to_hex()
        );
    }

    #[tokio::test]
    async fn test_start_stop_lifecycle() {
        let mut supported_units = HashMap::new();
        supported_units.insert(CurrencyUnit::default(), (0, 32));
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
        supported_units.insert(CurrencyUnit::default(), (0, 32));
        let config = MintConfig::<'_> {
            supported_units,
            ..Default::default()
        };
        let mint = create_mint(config).await;

        let currency_unit = CurrencyUnit::custom("sW8W2A_hTH_gapj1_vj5suO3JI_");
        let rotate_argument = RotateKeyArguments {
            unit: currency_unit,
            amounts: Default::default(),
            input_fee_ppk: 100,
            final_expiry: None,
        };
        let rotation_result = mint.signatory.rotate_keyset(rotate_argument).await;

        assert!(rotation_result.is_err());

        assert!(matches!(
            rotation_result,
            Err(Error::UnitStringCollision(_currency_unit))
        ));
    }
}
