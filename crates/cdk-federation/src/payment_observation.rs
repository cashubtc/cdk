use std::collections::BTreeMap;
#[cfg(any(test, feature = "federation-bdk", feature = "federation-bark"))]
use std::collections::{BTreeSet, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cdk::cdk_payment::Event;
use cdk::federation::{
    FederationAcceptedTransactionState, FederationChainObservation,
    FederationOnchainPaymentIntentState, FederationOperation, FederationOperationStatus,
    FederationPaymentObservation, FederationPaymentObservationReport,
    FederationPaymentObservationReportSummary, FederationWalletUtxoState,
    FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS,
};
#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use cdk::federation::{
    FederationAddressAllocationScope, FederationAddressKeychain, FederationAllocatedAddressRef,
    FederationChainSourceDescriptor, FederationChainSourceScope, FederationDepositConfirmation,
    FederationDepositObservation, FederationDepositReorgObservation,
};
#[cfg(any(test, feature = "federation-bdk", feature = "federation-bark"))]
use cdk::federation::{
    FederationAlephBftError, FederationConsensusItem, FederationError,
    FederationJournalAppendOutcome, FederationOperationConflictKey, FederationOperationEnvelope,
    FederationOperationId, FederationSubmitOperationResponse,
    FEDERATION_MAX_ALEPH_BFT_UNIT_BODY_BYTES,
};
use cdk::mint::QuoteId;
use cdk::nuts::{MeltQuoteState, MintQuoteState};
use cdk_common::database::mint::FederationPaymentObservationSelection;
use futures::FutureExt as _;

use super::payment_schedule::{
    FederationPaymentAdvancement, FederationPaymentObservationPermit,
    FederationPaymentObservationSchedule, FederationPaymentObservationTrigger,
    FederationPaymentQuote, FederationPaymentQuoteKind,
};
use crate::FederationNode;

const FEDERATION_STALE_PAYMENT_OBSERVATION_SECS: u64 = 15 * 60;
/// Active on-chain observation cadence when unpaid on-chain mint quotes or
/// pending/unconfirmed on-chain transactions are present.
pub const FEDERATION_ONCHAIN_OBSERVER_ACTIVE_INTERVAL: Duration = Duration::from_secs(1);
/// Idle on-chain observation cadence when no active on-chain work is pending.
pub const FEDERATION_ONCHAIN_OBSERVER_IDLE_INTERVAL: Duration = Duration::from_secs(5);
const FEDERATION_ONCHAIN_ACTIVITY_MINT_QUOTE_SCAN_LIMIT: usize = 64;
const FEDERATION_PAYMENT_REPAIR_INTERVAL: Duration = Duration::from_secs(60);
const FEDERATION_PAYMENT_IDLE_DEADLINE: std::time::Duration =
    std::time::Duration::from_secs(24 * 60 * 60);
const FEDERATION_PAYMENT_MAX_ADVANCEMENTS_PER_WAKE: usize = 8;
const FEDERATION_PAYMENT_SCAN_LIMIT_PER_KIND: usize = 4;
const FEDERATION_PAYMENT_WAKE_CAPACITY: usize = 256;
const FEDERATION_PAYMENT_CONCURRENT_QUOTE_TASKS: usize = 16;
/// Per-quote backend probe deadline.
///
/// One hung or slow backend call must never occupy the sequential observer
/// worker. A deadline expiry leaves the quote pending/unknown and it is
/// polled again next round through its durable idempotency handle; it is
/// never translated into a terminal failure.
const FEDERATION_PAYMENT_OBSERVER_PROBE_DEADLINE: std::time::Duration =
    std::time::Duration::from_secs(10);
const FEDERATION_PAYMENT_WORKER_FAILURE_BACKOFF_INITIAL: std::time::Duration =
    std::time::Duration::from_secs(1);
const FEDERATION_PAYMENT_WORKER_FAILURE_BACKOFF_MAX: std::time::Duration =
    std::time::Duration::from_secs(30);

type FederationPaymentProbeFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<Option<FederationPaymentObservation>, cdk::Error>>
            + Send
            + 'a,
    >,
>;

#[derive(Debug, Default)]
struct FederationPaymentWorkerFailureBackoff {
    consecutive_failures: u32,
}

impl FederationPaymentWorkerFailureBackoff {
    fn next_delay(&mut self) -> std::time::Duration {
        let multiplier = 1_u32 << self.consecutive_failures.min(5);
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        std::cmp::min(
            FEDERATION_PAYMENT_WORKER_FAILURE_BACKOFF_INITIAL.saturating_mul(multiplier),
            FEDERATION_PAYMENT_WORKER_FAILURE_BACKOFF_MAX,
        )
    }

    fn reset(&mut self) {
        self.consecutive_failures = 0;
    }
}

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
fn confirmation_matches_known_anchor(
    block: Option<(cdk_common::bitcoin::BlockHash, u32)>,
    confirmation: Option<FederationDepositConfirmation>,
) -> bool {
    match (block, confirmation) {
        (Some(block), Some(confirmation)) => {
            block == (confirmation.block_hash, confirmation.block_height)
        }
        _ => true,
    }
}

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
fn reorg_disconnection_is_proven(
    still_confirmed_in_block: bool,
    membership: cdk_bitcoin_wallet::FederatedBlockMembership,
) -> bool {
    !still_confirmed_in_block
        && membership == cdk_bitcoin_wallet::FederatedBlockMembership::Disconnected
}

#[cfg(feature = "federation-bark")]
fn advance_bark_mint_quote<'a>(
    state: &'a FederationNode,
    quote_id: &'a QuoteId,
    observer_fallback_round: u32,
    wait_for_backend_update: bool,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<FederationPaymentAdvancement, cdk::Error>>
            + Send
            + 'a,
    >,
> {
    Box::pin(cdk_bark::advance_federated_bark_mint_quote(
        state,
        quote_id,
        observer_fallback_round,
        wait_for_backend_update,
    ))
}

#[cfg(feature = "federation-bark")]
fn advance_bark_melt_quote<'a>(
    state: &'a FederationNode,
    quote_id: &'a QuoteId,
    observer_fallback_round: u32,
    wait_for_backend_update: bool,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<FederationPaymentAdvancement, cdk::Error>>
            + Send
            + 'a,
    >,
> {
    Box::pin(cdk_bark::advance_federated_bark_melt_quote(
        state,
        quote_id,
        observer_fallback_round,
        wait_for_backend_update,
    ))
}

/// Outcome of reserving a background task for one quote attempt.
#[derive(Debug)]
enum FederationPaymentQuoteTaskClaim {
    /// Every quote task is busy; the quote stays due and unleased.
    CapacityReached,
    /// The quote is not due, typically because an attempt already holds it.
    NotDue,
    /// The quote's attempt lease is held and a task permit is reserved for it.
    Claimed {
        task_permit: tokio::sync::OwnedSemaphorePermit,
        attempt: FederationPaymentObservationPermit,
    },
}

#[cfg(any(test, feature = "federation-bdk", feature = "federation-bark"))]
#[derive(Debug, Clone)]
struct ObservationEnvelopeChunkItem {
    index: usize,
    envelope: FederationOperationEnvelope,
}

#[cfg(any(test, feature = "federation-bdk", feature = "federation-bark"))]
#[derive(Debug, Default, PartialEq, Eq)]
struct ObservationBatchSubmitResult {
    applied_indices: Vec<usize>,
    newly_submitted: usize,
}

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
struct PendingChainObservation {
    envelope: FederationOperationEnvelope,
    quote: String,
    outpoint: cdk_common::bitcoin::OutPoint,
    is_reorg: bool,
    confirmed_external_mint_quote: Option<QuoteId>,
    confirmed: bool,
    observation_epoch: u64,
}

#[cfg(any(test, feature = "federation-bdk", feature = "federation-bark"))]
fn chunk_operation_envelopes(
    envelopes: Vec<FederationOperationEnvelope>,
    max_items: usize,
    max_body_bytes: usize,
) -> Result<Vec<Vec<ObservationEnvelopeChunkItem>>, cdk::Error> {
    let max_items = max_items.clamp(1, FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS);
    let mut chunks = Vec::new();
    let mut current_chunk = Vec::new();
    let mut current_bytes = 0_usize;
    let mut seen_operation_ids = BTreeSet::<FederationOperationId>::new();
    let mut current_conflict_keys = HashSet::<FederationOperationConflictKey>::new();

    for (index, envelope) in envelopes.into_iter().enumerate() {
        let operation_id = envelope.operation_id()?;
        if !seen_operation_ids.insert(operation_id) {
            continue;
        }
        let item_bytes = FederationConsensusItem::operation(envelope.clone()).canonical_size()?;
        let conflict_keys = envelope.operation.conflict_keys();
        let conflicts_with_current = conflict_keys
            .iter()
            .any(|key| current_conflict_keys.contains(key));
        let would_exceed_items = current_chunk.len() >= max_items;
        let would_exceed_bytes =
            !current_chunk.is_empty() && current_bytes.saturating_add(item_bytes) > max_body_bytes;

        if !current_chunk.is_empty()
            && (would_exceed_items || would_exceed_bytes || conflicts_with_current)
        {
            chunks.push(std::mem::take(&mut current_chunk));
            current_bytes = 0;
            current_conflict_keys.clear();
        }

        current_bytes = current_bytes.saturating_add(item_bytes);
        current_conflict_keys.extend(conflict_keys);
        current_chunk.push(ObservationEnvelopeChunkItem { index, envelope });
    }

    if !current_chunk.is_empty() {
        chunks.push(current_chunk);
    }
    Ok(chunks)
}

#[cfg(any(test, feature = "federation-bdk", feature = "federation-bark"))]
fn is_recoverable_observation_batch_error(error: &cdk::Error) -> bool {
    matches!(
        error,
        cdk::Error::Federation(
            FederationError::ConflictingOperation { .. }
                | FederationError::RejectedOperation { .. }
                | FederationError::OperationStateInvalid { .. }
                | FederationError::ChainObservationInvalid { .. }
                | FederationError::ChainObservationStateInvalid { .. }
                | FederationError::ConsensusItemTooLarge { .. }
                | FederationError::OperationEnvelopeTooLarge { .. }
                | FederationError::AlephBft(
                    FederationAlephBftError::OrderedGroupInvalid { .. }
                        | FederationAlephBftError::OrderedGroupTooManyItems { .. }
                        | FederationAlephBftError::OrderedGroupBodyTooLarge { .. }
                )
        )
    )
}

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
fn expected_deposit_script(
    runtime: &cdk::mint::FederationRuntimeConfig,
    wallet_config: &cdk::federation::FederationFrostWalletConfig,
    allocation: &FederationAllocatedAddressRef,
) -> Result<cdk_common::bitcoin::ScriptBuf, cdk::Error> {
    let expected_script = wallet_config
        .cdk_onchain_application
        .as_ref()
        .filter(|application| {
            application.application_id == allocation.application_id
                && application.application_key_epoch == allocation.application_key_epoch
        })
        .map(|_| {
            wallet_config.derive_cdk_onchain_script(
                runtime.public_config(),
                allocation.application_id,
                allocation.application_key_epoch,
                allocation.keychain,
                allocation.derivation_index,
            )
        })
        .transpose()?;
    #[cfg(feature = "federation-bark")]
    let expected_script = match expected_script {
        Some(script) => Some(script),
        None => cdk_federation_bark::federation_bark_recovery_application(runtime.public_config())?
            .filter(|application| {
                application.application_id() == allocation.application_id
                    && application.application_key_epoch() == allocation.application_key_epoch
            })
            .map(|application| {
                application.derive_script(
                    runtime.public_config(),
                    &wallet_config.root,
                    allocation.keychain == FederationAddressKeychain::Internal,
                    allocation.derivation_index,
                )
            })
            .transpose()?,
    };
    expected_script.ok_or_else(|| {
        cdk::Error::Federation(FederationError::ChainObservationInvalid {
            reason: "deposit scope is not a configured descriptor application",
        })
    })
}

/// Coordinates federation payment and on-chain wallet observation.
#[derive(Debug, Clone)]
pub struct FederationPaymentObserver {
    schedule: FederationPaymentObservationSchedule,
    accepted_operation_wakes:
        tokio::sync::broadcast::Sender<(FederationPaymentQuote, u64, Instant)>,
    onchain_activity_wake: Arc<tokio::sync::Notify>,
    quote_tasks: Arc<tokio::sync::Semaphore>,
    onchain_active_interval: Duration,
    onchain_idle_interval: Duration,
    onchain_max_batch_items: usize,
}

impl Default for FederationPaymentObserver {
    fn default() -> Self {
        let (accepted_operation_wakes, _) =
            tokio::sync::broadcast::channel(FEDERATION_PAYMENT_WAKE_CAPACITY);
        Self {
            schedule: FederationPaymentObservationSchedule::default(),
            accepted_operation_wakes,
            onchain_activity_wake: Arc::new(tokio::sync::Notify::new()),
            quote_tasks: Arc::new(tokio::sync::Semaphore::new(
                FEDERATION_PAYMENT_CONCURRENT_QUOTE_TASKS,
            )),
            onchain_active_interval: FEDERATION_ONCHAIN_OBSERVER_ACTIVE_INTERVAL,
            onchain_idle_interval: FEDERATION_ONCHAIN_OBSERVER_IDLE_INTERVAL,
            onchain_max_batch_items: FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS,
        }
    }
}

impl FederationPaymentObserver {
    /// Configure the active and idle polling intervals for on-chain wallet observation.
    ///
    /// Zero durations are clamped to 1 millisecond, and `idle_interval` is at
    /// least `active_interval`.
    #[must_use]
    pub fn with_onchain_intervals(
        mut self,
        active_interval: Duration,
        idle_interval: Duration,
    ) -> Self {
        let min_interval = Duration::from_millis(1);
        let active = active_interval.max(min_interval);
        let idle = idle_interval.max(active);
        self.onchain_active_interval = active;
        self.onchain_idle_interval = idle;
        self
    }

    /// Return the configured active on-chain observation interval.
    #[must_use]
    pub fn onchain_active_interval(&self) -> Duration {
        self.onchain_active_interval
    }

    /// Return the configured idle on-chain observation interval.
    #[must_use]
    pub fn onchain_idle_interval(&self) -> Duration {
        self.onchain_idle_interval
    }

    /// Configure the maximum number of observation envelopes submitted in one consensus batch.
    ///
    /// Values are clamped to `1..=FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS`.
    #[must_use]
    pub fn with_onchain_max_batch_items(mut self, max_batch_items: usize) -> Self {
        self.onchain_max_batch_items =
            max_batch_items.clamp(1, FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS);
        self
    }

    /// Whether the federation currently has active on-chain work that benefits
    /// from the faster on-chain observation cadence.
    pub async fn has_active_onchain_work(state: &FederationNode) -> Result<bool, cdk::Error> {
        let Some(runtime) = state.mint.federation_config() else {
            return Ok(false);
        };
        if runtime.public_config().frost_wallet.is_none() {
            return Ok(false);
        }
        let transaction_state = state.mint.federation_transaction_state().await?;
        if transaction_state
            .intents()
            .values()
            .any(|record| record.state == FederationOnchainPaymentIntentState::Pending)
        {
            return Ok(true);
        }
        if transaction_state
            .accepted_transactions()
            .values()
            .any(|accepted| {
                matches!(
                    accepted.state,
                    FederationAcceptedTransactionState::Accepted
                        | FederationAcceptedTransactionState::Signed
                        | FederationAcceptedTransactionState::BroadcastPending
                        | FederationAcceptedTransactionState::Broadcast
                )
            })
        {
            return Ok(true);
        }

        let chain_state = state.mint.federation_chain_observation_state().await?;
        if chain_state.utxos().any(|utxo| {
            matches!(
                utxo.state,
                FederationWalletUtxoState::Observed | FederationWalletUtxoState::Confirmed
            )
        }) {
            return Ok(true);
        }

        let db = state.mint.localstore();
        let mint_ids = db
            .get_federation_mint_quote_ids_for_payment_observation(
                None,
                FEDERATION_ONCHAIN_ACTIVITY_MINT_QUOTE_SCAN_LIMIT,
            )
            .await?;
        for quote_id in mint_ids {
            if let Some(quote) = db.get_mint_quote(&quote_id).await? {
                if quote.payment_method.is_onchain()
                    && !matches!(quote.state(), MintQuoteState::Paid | MintQuoteState::Issued)
                {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    /// Select the next on-chain observation polling interval based on current
    /// on-chain quote, UTXO, and transaction activity.
    pub async fn next_onchain_observer_interval(
        &self,
        state: &FederationNode,
    ) -> Result<Duration, cdk::Error> {
        if Self::has_active_onchain_work(state).await? {
            Ok(self.onchain_active_interval)
        } else {
            Ok(self.onchain_idle_interval)
        }
    }

    #[cfg(any(test, feature = "federation-bdk", feature = "federation-bark"))]
    async fn submit_observation_envelope_batch(
        &self,
        state: &FederationNode,
        envelopes: Vec<FederationOperationEnvelope>,
    ) -> Result<ObservationBatchSubmitResult, cdk::Error> {
        if envelopes.is_empty() {
            return Ok(ObservationBatchSubmitResult::default());
        }
        let chunks = chunk_operation_envelopes(
            envelopes,
            self.onchain_max_batch_items,
            FEDERATION_MAX_ALEPH_BFT_UNIT_BODY_BYTES,
        )?;
        let mut result = ObservationBatchSubmitResult::default();

        for chunk in chunks {
            let mut admitted = Vec::with_capacity(chunk.len());
            for item in chunk {
                match state
                    .validate_federation_envelope_admission(&item.envelope)
                    .await
                {
                    Ok(()) => admitted.push(item),
                    Err(error) if is_recoverable_observation_batch_error(&error) => {
                        tracing::debug!(
                            error = %error,
                            "Skipping federation observation that failed local admission"
                        );
                    }
                    Err(error) => return Err(error),
                }
            }
            if admitted.is_empty() {
                continue;
            }
            state.ensure_federation_ready_for_public_submit().await?;
            if admitted.len() == 1 {
                let item = admitted.pop().expect("single admitted observation");
                Self::submit_single_observation_item(state, item, &mut result).await?;
                continue;
            }

            let expected_ids = admitted
                .iter()
                .map(|item| item.envelope.operation_id())
                .collect::<Result<Vec<_>, _>>()?;
            let group_envelopes = admitted
                .iter()
                .map(|item| item.envelope.clone())
                .collect::<Vec<_>>();
            match state
                .federation
                .submit_envelope_group(group_envelopes)
                .await
            {
                Ok(responses) => {
                    if responses.len() != admitted.len() {
                        return Err(FederationError::AlephBft(
                            FederationAlephBftError::OrderedGroupInvalid {
                                reason:
                                    "ordered group response count did not match submitted batch",
                            },
                        )
                        .into());
                    }
                    for ((item, expected_id), response) in
                        admitted.into_iter().zip(expected_ids).zip(responses)
                    {
                        Self::record_observation_response(
                            item.index,
                            expected_id,
                            &response,
                            &mut result,
                        )?;
                    }
                }
                Err(error) if is_recoverable_observation_batch_error(&error) => {
                    tracing::debug!(
                        chunk_len = admitted.len(),
                        error = %error,
                        "Batched federation observation group failed; falling back to per-item submission"
                    );
                    for item in admitted {
                        Self::submit_single_observation_item(state, item, &mut result).await?;
                    }
                }
                Err(error) => return Err(error),
            }
        }

        Ok(result)
    }

    #[cfg(any(test, feature = "federation-bdk", feature = "federation-bark"))]
    async fn submit_single_observation_item(
        state: &FederationNode,
        item: ObservationEnvelopeChunkItem,
        result: &mut ObservationBatchSubmitResult,
    ) -> Result<(), cdk::Error> {
        let expected_id = item.envelope.operation_id()?;
        match state.federation.submit_envelope(item.envelope).await {
            Ok(response) => {
                Self::record_observation_response(item.index, expected_id, &response, result)
            }
            Err(error) if is_recoverable_observation_batch_error(&error) => {
                tracing::debug!(
                    operation_id = %expected_id,
                    error = %error,
                    "Skipping conflicting or rejected federation observation during per-item submission"
                );
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    #[cfg(any(test, feature = "federation-bdk", feature = "federation-bark"))]
    fn record_observation_response(
        index: usize,
        expected_id: FederationOperationId,
        response: &FederationSubmitOperationResponse,
        result: &mut ObservationBatchSubmitResult,
    ) -> Result<(), cdk::Error> {
        let entry = response.outcome.entry();
        if entry.operation_id != expected_id {
            return Err(FederationError::SubmitOperationResponseMismatch {
                expected: expected_id,
                found: entry.operation_id,
            }
            .into());
        }
        match entry.status() {
            FederationOperationStatus::Applied => {
                result.applied_indices.push(index);
                if matches!(
                    response.outcome,
                    FederationJournalAppendOutcome::Appended(_)
                ) {
                    result.newly_submitted = result.newly_submitted.saturating_add(1);
                }
                Ok(())
            }
            FederationOperationStatus::Rejected => {
                tracing::debug!(
                    operation_id = %expected_id,
                    "Federation observation finalized as rejected; skipping"
                );
                Ok(())
            }
            status => Err(FederationError::RejectedOperation {
                operation_id: expected_id,
                status,
            }
            .into()),
        }
    }

    #[cfg(any(test, feature = "federation-bdk", feature = "federation-bark"))]
    async fn submit_confirmed_mint_quote_payments(
        &self,
        state: &FederationNode,
        quote_ids: &BTreeSet<QuoteId>,
    ) -> Result<usize, cdk::Error> {
        if quote_ids.is_empty() {
            return Ok(0);
        }
        let Some(config) = state.mint.federation_config() else {
            return Ok(0);
        };
        let mut envelopes = Vec::new();
        for quote_id in quote_ids {
            let Some(quote) = state.mint.localstore().get_mint_quote(quote_id).await? else {
                continue;
            };
            if !quote.payment_method.is_onchain()
                || matches!(quote.state(), MintQuoteState::Paid | MintQuoteState::Issued)
            {
                continue;
            }
            let observation = match state
                .mint
                .probe_federation_mint_quote_payment(quote_id)
                .await
            {
                Ok(Some(observation)) => FederationPaymentObservation::MintQuote(observation),
                Ok(None) => continue,
                Err(error) => {
                    tracing::debug!(
                        quote = %quote_id,
                        error = %error,
                        "Federation on-chain mint quote probe failed after deposit batch; will retry"
                    );
                    continue;
                }
            };
            if !payment_observation_needs_submission(state, &observation).await? {
                continue;
            }
            if let FederationPaymentObservation::MintQuote(mint_observation) = observation {
                envelopes.push(
                    config.operation_envelope(FederationOperation::MintQuotePayment(
                        mint_observation,
                    )),
                );
            }
        }
        if envelopes.is_empty() {
            return Ok(0);
        }
        let batch_result = self
            .submit_observation_envelope_batch(state, envelopes)
            .await?;
        if batch_result.newly_submitted > 0 {
            tracing::debug!(
                submitted = batch_result.newly_submitted,
                "Federation payment observer submitted batched on-chain mint quote payment observations"
            );
        }
        Ok(batch_result.newly_submitted)
    }

    #[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
    pub async fn observe_onchain_wallets(
        &self,
        state: &FederationNode,
    ) -> Result<usize, cdk::Error> {
        let correlation_id = state
            .mint
            .federation_config()
            .map(|runtime| runtime.member_id().to_string())
            .unwrap_or_else(|| "disabled".to_string());
        let timer = super::latency::FederationLatencyTimer::start(
            super::latency::FederationLatencyCorrelation::Wallet,
            correlation_id,
            super::latency::FederationLatencyBackend::Bdk,
            super::latency::FederationLatencyDirection::Incoming,
            super::latency::FederationLatencyPhase::BdkWalletScan,
        );
        let result = self.observe_onchain_wallets_inner(state).await;
        timer.finish(super::latency::FederationLatencyOutcome::from_success(
            result.is_ok(),
        ));
        result
    }

    #[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
    async fn observe_onchain_wallets_inner(
        &self,
        state: &FederationNode,
    ) -> Result<usize, cdk::Error> {
        let Some(runtime) = state.mint.federation_config() else {
            return Ok(0);
        };
        let Some(wallet_config) = runtime.public_config().frost_wallet.as_ref() else {
            return Ok(0);
        };
        let allocations = state.mint.federation_address_allocation_state().await?;
        let canonical = state.mint.federation_chain_observation_state().await?;
        let observer = runtime.member_id();
        let mut pending_observations = Vec::<PendingChainObservation>::new();
        let mut wallets = Vec::<(cdk_bitcoin_wallet::FederatedDescriptorWallet, bool)>::new();

        #[cfg(feature = "federation-bdk")]
        wallets.extend(
            state
                .federation
                .onchain_wallets()
                .iter()
                .cloned()
                .map(|wallet| (wallet, false)),
        );
        #[cfg(feature = "federation-bark")]
        if let Some(bark) = state.federation.bark_runtime() {
            wallets.push((bark.recovery_wallet().as_ref().clone(), true));
        }

        for (wallet, synchronize_locally) in wallets {
            let wallet_scope = wallet.scope();
            let application_id = cdk::federation::FederationFrostApplicationId::from_bytes(
                wallet_scope.application_id(),
            )?;
            let scope = FederationAddressAllocationScope {
                application_id,
                application_key_epoch: wallet_scope.application_key_epoch(),
            };
            let Some(allocation_wallet) = allocations.wallet(scope) else {
                continue;
            };
            wallet
                .reconcile_revealed_counters(
                    allocation_wallet.external_next_index,
                    allocation_wallet.internal_next_index,
                )
                .await
                .map_err(|_| cdk::Error::Internal)?;
            if synchronize_locally {
                let sync_timer = super::latency::FederationLatencyTimer::start(
                    super::latency::FederationLatencyCorrelation::Wallet,
                    cdk::federation::FederationOperationId::from_bytes(
                        scope.application_id.to_bytes(),
                    )
                    .to_string(),
                    super::latency::FederationLatencyBackend::Bark,
                    super::latency::FederationLatencyDirection::Internal,
                    super::latency::FederationLatencyPhase::OnchainWalletSync,
                );
                let sync_result = wallet.sync_once().await;
                sync_timer.finish(super::latency::FederationLatencyOutcome::from_success(
                    sync_result.is_ok(),
                ));
                if sync_result.is_err() {
                    continue;
                }
            }
            if wallet.ensure_ready(0).await.is_err() {
                continue;
            }
            let minimum_confirmations = wallet_config
                .cdk_onchain_application
                .as_ref()
                .filter(|application| {
                    application.application_id == scope.application_id
                        && application.application_key_epoch == scope.application_key_epoch
                })
                .map(|application| application.confirmation_policy.minimum_confirmations);
            #[cfg(feature = "federation-bark")]
            let minimum_confirmations = match minimum_confirmations {
                Some(value) => Some(value),
                None => cdk_federation_bark::federation_bark_recovery_application(
                    runtime.public_config(),
                )
                .map_err(|_| cdk::Error::Internal)?
                .filter(|application| {
                    application.application_id() == scope.application_id
                        && application.application_key_epoch() == scope.application_key_epoch
                })
                .map(|application| application.minimum_confirmations()),
            };
            let Some(minimum_confirmations) = minimum_confirmations else {
                return Err(cdk::Error::Internal);
            };
            let source = FederationChainSourceDescriptor::new(
                match wallet.observation_source().scope {
                    cdk_bitcoin_wallet::FederatedChainSourceScope::Independent => {
                        FederationChainSourceScope::Independent
                    }
                    cdk_bitcoin_wallet::FederatedChainSourceScope::Shared => {
                        FederationChainSourceScope::Shared
                    }
                },
                wallet.observation_source().source_id.clone(),
            )?;
            let outputs = wallet.observed_outputs().await;
            let outputs_by_outpoint = outputs
                .iter()
                .map(|output| (output.outpoint, output))
                .collect::<BTreeMap<_, _>>();

            for output in &outputs {
                let keychain = match output.keychain {
                    cdk_bitcoin_wallet::FederatedWalletKeychain::External => {
                        FederationAddressKeychain::External
                    }
                    cdk_bitcoin_wallet::FederatedWalletKeychain::Internal => {
                        FederationAddressKeychain::Internal
                    }
                };
                let Some(allocation) = allocation_wallet.allocations.values().find(|allocation| {
                    allocation.keychain == keychain
                        && allocation.derivation_index == output.derivation_index
                }) else {
                    continue;
                };
                let confirmation =
                    output
                        .confirmation
                        .map(|confirmation| FederationDepositConfirmation {
                            block_hash: confirmation.block_hash,
                            block_height: confirmation.block_height,
                            confirmation_depth: confirmation
                                .confirmation_depth
                                .min(minimum_confirmations),
                        });
                let canonical_utxo = canonical.utxo(output.outpoint);
                let observation_epoch = match canonical_utxo {
                    Some(utxo) => match utxo.next_deposit_observation_epoch()? {
                        Some(observation_epoch) => observation_epoch,
                        None => continue,
                    },
                    None => 0,
                };
                if canonical_utxo.is_some_and(|utxo| {
                    !confirmation_matches_known_anchor(utxo.block, confirmation)
                }) {
                    continue;
                }
                if canonical_utxo.is_some_and(|utxo| {
                    utxo.state == FederationWalletUtxoState::Reorged
                        && !confirmation.is_some_and(|confirmation| {
                            confirmation.confirmation_depth >= utxo.minimum_confirmations
                                && utxo.block
                                    == Some((confirmation.block_hash, confirmation.block_height))
                        })
                }) {
                    continue;
                }
                if canonical_utxo.is_some_and(|utxo| {
                    utxo.observations.get(&observer).is_some_and(|evidence| {
                        matches!(
                            &evidence.observation,
                            FederationChainObservation::Deposit(existing)
                                if existing.allocation.quote == allocation.quote
                                    && existing.txout == output.txout
                                    && existing.observation_epoch == observation_epoch
                                    && existing.confirmation.map(|value| {
                                        (value.block_hash, value.block_height)
                                    }) == confirmation.map(|value| {
                                        (value.block_hash, value.block_height)
                                    })
                                    && existing.confirmation.map(|value| value.confirmation_depth)
                                        >= confirmation.map(|value| value.confirmation_depth)
                        )
                    })
                }) {
                    continue;
                }
                let allocation_ref = FederationAllocatedAddressRef {
                    application_id: scope.application_id,
                    application_key_epoch: scope.application_key_epoch,
                    quote: allocation.quote.clone(),
                    keychain,
                    derivation_index: allocation.derivation_index,
                };
                allocation_ref.validate_allocation(allocation)?;
                let expected_script =
                    expected_deposit_script(runtime, wallet_config, &allocation_ref)?;
                if output.txout.script_pubkey != expected_script {
                    tracing::warn!(
                        quote = %allocation.quote,
                        outpoint = %output.outpoint,
                        "Skipping federation deposit observation whose script does not match accepted allocation"
                    );
                    continue;
                }
                let required_height = confirmation.map_or(0, |confirmation| {
                    confirmation
                        .block_height
                        .saturating_add(confirmation.confirmation_depth.saturating_sub(1))
                });
                if wallet.ensure_ready(required_height).await.is_err() {
                    continue;
                }
                let confirmed_external_mint_quote = match keychain {
                    FederationAddressKeychain::External
                        if confirmation.is_some_and(|value| {
                            value.confirmation_depth >= minimum_confirmations
                        }) =>
                    {
                        allocation.quote.parse::<QuoteId>().ok()
                    }
                    _ => None,
                };
                let observation =
                    FederationChainObservation::Deposit(FederationDepositObservation {
                        allocation: allocation_ref,
                        outpoint: output.outpoint,
                        txout: output.txout.clone(),
                        confirmation,
                        observation_epoch,
                        observer,
                        source: source.clone(),
                    });
                let envelope = runtime.chain_observation_operation(observation)?;
                pending_observations.push(PendingChainObservation {
                    envelope,
                    quote: allocation.quote.clone(),
                    outpoint: output.outpoint,
                    is_reorg: false,
                    confirmed_external_mint_quote,
                    confirmed: confirmation.is_some(),
                    observation_epoch,
                });
            }

            for utxo in canonical.utxos().filter(|utxo| {
                utxo.allocation.scope() == scope
                    && utxo.state != FederationWalletUtxoState::Quarantined
                    && utxo.observations.get(&observer).is_some_and(|evidence| {
                        matches!(
                            &evidence.observation,
                            FederationChainObservation::Deposit(observation)
                                if observation.observation_epoch == utxo.observation_epoch
                        )
                    })
                    && !utxo
                        .reorg_observations
                        .get(&observer)
                        .is_some_and(|evidence| {
                            matches!(
                                &evidence.observation,
                                FederationChainObservation::Reorg(observation)
                                    if observation.observation_epoch == utxo.observation_epoch
                            )
                        })
            }) {
                let Some((block_hash, block_height)) = utxo.block else {
                    continue;
                };
                let still_confirmed_in_block = outputs_by_outpoint
                    .get(&utxo.outpoint)
                    .and_then(|output| output.confirmation)
                    .is_some_and(|confirmation| {
                        confirmation.block_hash == block_hash
                            && confirmation.block_height == block_height
                    });
                if still_confirmed_in_block {
                    continue;
                }
                let membership = wallet.block_membership(block_hash, block_height).await;
                if !reorg_disconnection_is_proven(still_confirmed_in_block, membership) {
                    continue;
                }
                let observation =
                    FederationChainObservation::Reorg(FederationDepositReorgObservation {
                        outpoint: utxo.outpoint,
                        disconnected_block_hash: block_hash,
                        disconnected_block_height: block_height,
                        observation_epoch: utxo.observation_epoch,
                        observer,
                        source: source.clone(),
                    });
                let envelope = runtime.chain_observation_operation(observation)?;
                pending_observations.push(PendingChainObservation {
                    envelope,
                    quote: utxo.allocation.quote.clone(),
                    outpoint: utxo.outpoint,
                    is_reorg: true,
                    confirmed_external_mint_quote: None,
                    confirmed: false,
                    observation_epoch: utxo.observation_epoch,
                });
            }
        }

        if pending_observations.is_empty() {
            return Ok(0);
        }

        let envelopes = pending_observations
            .iter()
            .map(|pending| pending.envelope.clone())
            .collect::<Vec<_>>();
        let batch_result = self
            .submit_observation_envelope_batch(state, envelopes)
            .await?;

        let mut confirmed_mint_quotes = BTreeSet::new();
        for index in &batch_result.applied_indices {
            if let Some(pending) = pending_observations.get(*index) {
                if pending.is_reorg {
                    tracing::debug!(
                        quote = %pending.quote,
                        outpoint = %pending.outpoint,
                        "Federation payment observer submitted deposit reorg observation"
                    );
                } else {
                    tracing::debug!(
                        quote = %pending.quote,
                        outpoint = %pending.outpoint,
                        confirmed = pending.confirmed,
                        observation_epoch = pending.observation_epoch,
                        "Federation payment observer submitted deposit observation"
                    );
                    if let Some(quote_id) = &pending.confirmed_external_mint_quote {
                        confirmed_mint_quotes.insert(quote_id.clone());
                    }
                }
            }
        }

        let payment_submitted = self
            .submit_confirmed_mint_quote_payments(state, &confirmed_mint_quotes)
            .await?;

        if batch_result.newly_submitted > 0 {
            Ok(batch_result.newly_submitted)
        } else {
            Ok(payment_submitted)
        }
    }

    #[cfg(not(any(feature = "federation-bdk", feature = "federation-bark")))]
    async fn observe_onchain_wallets(&self, _state: &FederationNode) -> Result<usize, cdk::Error> {
        Ok(0)
    }

    pub async fn observe_mint_quotes(
        &self,
        state: &FederationNode,
        quote_ids: &[QuoteId],
    ) -> Result<usize, cdk::Error> {
        let quotes = quote_ids
            .iter()
            .cloned()
            .map(FederationPaymentQuote::mint)
            .collect::<Vec<_>>();
        Box::pin(self.drive_or_dispatch_quotes(
            state,
            quotes,
            FederationPaymentObservationTrigger::Requested,
            Instant::now(),
        ))
        .await
    }

    async fn drive_or_dispatch_quotes(
        &self,
        state: &FederationNode,
        quotes: Vec<FederationPaymentQuote>,
        trigger: FederationPaymentObservationTrigger,
        queued_at: Instant,
    ) -> Result<usize, cdk::Error> {
        let mut direct = Vec::with_capacity(quotes.len());
        let mut background = Vec::new();
        for quote in quotes {
            if self.quote_uses_bark(state, &quote).await? {
                background.push(quote);
            } else {
                direct.push(quote);
            }
        }

        let submitted = Box::pin(self.drive_quotes(state, &direct, trigger)).await?;
        // A dispatched quote has only started an attempt; its task reports
        // its own progress. Counting it here made every dispatch look like a
        // submitted observation.
        self.dispatch_quotes(state, background, trigger, queued_at)
            .await?;
        Ok(submitted)
    }

    async fn quote_uses_bark(
        &self,
        state: &FederationNode,
        quote: &FederationPaymentQuote,
    ) -> Result<bool, cdk::Error> {
        #[cfg(feature = "federation-bark")]
        {
            if state.federation.bark_runtime().is_none() {
                return Ok(false);
            }
            let uses_bark = match quote.kind {
                FederationPaymentQuoteKind::Mint => state
                    .mint
                    .localstore()
                    .get_mint_quote(&quote.id)
                    .await?
                    .is_some_and(|quote| quote.payment_method.is_bolt11()),
                FederationPaymentQuoteKind::Melt => state
                    .mint
                    .localstore()
                    .get_melt_quote(&quote.id)
                    .await?
                    .is_some_and(|quote| quote.payment_method.is_bolt11()),
            };
            Ok(uses_bark)
        }
        #[cfg(not(feature = "federation-bark"))]
        {
            let _ = (state, quote);
            Ok(false)
        }
    }

    /// Hand each due quote to its own background task.
    ///
    /// The attempt lease is claimed here, before the task is spawned, so the
    /// quote stops being due at once: a scheduled round that runs again before
    /// the task starts neither dispatches it a second time nor finds it due.
    async fn dispatch_quotes(
        &self,
        state: &FederationNode,
        quotes: Vec<FederationPaymentQuote>,
        trigger: FederationPaymentObservationTrigger,
        queued_at: Instant,
    ) -> Result<usize, cdk::Error> {
        let Some(federation_config) = state.mint.federation_config() else {
            return Ok(0);
        };
        let member_id = federation_config.member_id();
        let db = state.mint.localstore();
        let active_quotes = self.active_quotes(&db, &quotes).await?;
        let mut dispatched = 0_usize;
        for quote in active_quotes {
            let (task_permit, attempt) = match self
                .claim_quote_task(&db, &quote, trigger, current_unix_time_ms())
                .await?
            {
                FederationPaymentQuoteTaskClaim::Claimed {
                    task_permit,
                    attempt,
                } => (task_permit, attempt),
                FederationPaymentQuoteTaskClaim::NotDue => continue,
                FederationPaymentQuoteTaskClaim::CapacityReached => {
                    tracing::debug!(
                        quote = %quote.id,
                        quote_kind = quote.kind.as_str(),
                        "Federation payment quote task capacity reached; durable schedule will retry"
                    );
                    break;
                }
            };
            tracing::trace!(
                quote = %quote.id,
                quote_kind = quote.kind.as_str(),
                "Federation payment quote dispatched to a background task"
            );
            let observer = self.clone();
            let state = state.clone();
            let correlation_id = quote.id.to_string();
            let direction = match quote.kind {
                FederationPaymentQuoteKind::Mint => {
                    super::latency::FederationLatencyDirection::Incoming
                }
                FederationPaymentQuoteKind::Melt => {
                    super::latency::FederationLatencyDirection::Outgoing
                }
            };
            let tasks = state.federation.tasks.clone();
            tasks.spawn("payment quote", false, async move {
                let _task_permit = task_permit;
                super::latency::record_duration(
                    super::latency::FederationLatencyCorrelation::Quote,
                    &correlation_id,
                    super::latency::FederationLatencyBackend::Bark,
                    direction,
                    super::latency::FederationLatencyPhase::PaymentCoordinatorDispatch,
                    super::latency::FederationLatencyOutcome::Success,
                    queued_at.elapsed(),
                );
                let work = Box::pin(
                    observer.drive_claimed_quote(&state, member_id, &quote, attempt, trigger),
                );
                let result = std::panic::AssertUnwindSafe(work).catch_unwind().await;
                match result {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => tracing::warn!(
                        quote = %quote.id,
                        quote_kind = quote.kind.as_str(),
                        error = %error,
                        "Federation payment quote task failed; durable schedule will retry"
                    ),
                    Err(_) => tracing::error!(
                        quote = %quote.id,
                        quote_kind = quote.kind.as_str(),
                        "Federation payment quote task panicked; durable schedule will retry"
                    ),
                }
            });
            dispatched = dispatched.saturating_add(1);
        }
        Ok(dispatched)
    }

    /// Reserve a quote task and the quote's attempt lease together.
    ///
    /// The task permit is taken first so a quote is never leased without a
    /// task to drive it; a quote that is not due releases the permit again.
    async fn claim_quote_task(
        &self,
        db: &cdk_common::database::DynMintDatabase,
        quote: &FederationPaymentQuote,
        trigger: FederationPaymentObservationTrigger,
        now_ms: u64,
    ) -> Result<FederationPaymentQuoteTaskClaim, cdk::Error> {
        let Ok(task_permit) = self.quote_tasks.clone().try_acquire_owned() else {
            return Ok(FederationPaymentQuoteTaskClaim::CapacityReached);
        };
        match self.schedule.claim(db, quote, trigger, now_ms).await? {
            Some(attempt) => Ok(FederationPaymentQuoteTaskClaim::Claimed {
                task_permit,
                attempt,
            }),
            None => Ok(FederationPaymentQuoteTaskClaim::NotDue),
        }
    }

    /// Keep the quotes that still await payment evidence and drop the
    /// schedules of the rest.
    async fn active_quotes(
        &self,
        db: &cdk_common::database::DynMintDatabase,
        quotes: &[FederationPaymentQuote],
    ) -> Result<Vec<FederationPaymentQuote>, cdk::Error> {
        let mut active_quotes = Vec::with_capacity(quotes.len());
        for quote in quotes {
            let active = match quote.kind {
                FederationPaymentQuoteKind::Mint => {
                    db.get_mint_quote(&quote.id).await?.is_some_and(|quote| {
                        !matches!(quote.state(), MintQuoteState::Paid | MintQuoteState::Issued)
                    })
                }
                FederationPaymentQuoteKind::Melt => db
                    .get_melt_quote(&quote.id)
                    .await?
                    .is_some_and(|quote| quote.state == MeltQuoteState::Pending),
            };
            if active {
                active_quotes.push(quote.clone());
            } else {
                self.schedule.remove(db, quote).await?;
            }
        }
        Ok(active_quotes)
    }

    async fn drive_quotes(
        &self,
        state: &FederationNode,
        quotes: &[FederationPaymentQuote],
        trigger: FederationPaymentObservationTrigger,
    ) -> Result<usize, cdk::Error> {
        let Some(federation_config) = state.mint.federation_config() else {
            return Ok(0);
        };
        let member_id = federation_config.member_id();
        let db = state.mint.localstore();
        let mut schedule_changes = self.schedule.subscribe_changes();
        let active_quotes = self.active_quotes(&db, quotes).await?;
        let quotes = self
            .schedule
            .candidates(&db, &active_quotes, trigger, current_unix_time_ms())
            .await?;
        let mut submitted = 0usize;
        for quote in &quotes {
            let Some(attempt) = self
                .schedule
                .claim(&db, quote, trigger, current_unix_time_ms())
                .await?
            else {
                continue;
            };
            let advanced = self
                .drive_claimed_quote(state, member_id, quote, attempt, trigger)
                .await?;
            submitted = submitted.saturating_add(advanced);
        }
        if trigger == FederationPaymentObservationTrigger::Requested {
            self.wait_for_active_attempts(&db, &active_quotes, &mut schedule_changes)
                .await?;
        }
        Ok(submitted)
    }

    /// Run one bounded attempt for a quote whose lease is already held, then
    /// record its outcome in the durable schedule.
    async fn drive_claimed_quote(
        &self,
        state: &FederationNode,
        member_id: cdk::federation::MemberId,
        quote: &FederationPaymentQuote,
        attempt: FederationPaymentObservationPermit,
        trigger: FederationPaymentObservationTrigger,
    ) -> Result<usize, cdk::Error> {
        let db = state.mint.localstore();
        let mut submitted = 0usize;
        let mut made_progress = false;
        let mut advancement = FederationPaymentAdvancement::Progressed;
        let mut observer_fallback_round = attempt.observer_fallback_round();
        let mut last_processed_operation_index =
            db.get_federation_journal_tip().await?.map(|tip| tip.index);
        let direction = match quote.kind {
            FederationPaymentQuoteKind::Mint => {
                super::latency::FederationLatencyDirection::Incoming
            }
            FederationPaymentQuoteKind::Melt => {
                super::latency::FederationLatencyDirection::Outgoing
            }
        };
        let round_timer = super::latency::FederationLatencyTimer::start(
            super::latency::FederationLatencyCorrelation::Quote,
            quote.id.to_string(),
            super::latency::FederationLatencyBackend::Cashu,
            direction,
            super::latency::FederationLatencyPhase::PaymentObservationRound,
        );
        for completed_steps in 0..FEDERATION_PAYMENT_MAX_ADVANCEMENTS_PER_WAKE {
            let before_operation_index = last_processed_operation_index;
            advancement = self
                .advance_one_quote(
                    state,
                    quote,
                    observer_fallback_round,
                    trigger.wait_for_backend_update(),
                )
                .await?;
            last_processed_operation_index =
                db.get_federation_journal_tip().await?.map(|tip| tip.index);
            if advancement == FederationPaymentAdvancement::Progressed {
                made_progress = true;
                submitted = submitted.saturating_add(1);
                observer_fallback_round = 0;
            }
            if !continue_payment_pump(
                completed_steps,
                advancement,
                before_operation_index,
                last_processed_operation_index,
            ) {
                break;
            }
        }
        self.schedule
            .finish(
                &db,
                attempt,
                member_id,
                super::payment_schedule::FederationPaymentObservationProgress {
                    advancement,
                    made_progress,
                    last_processed_operation_index,
                },
                current_unix_time_ms(),
            )
            .await?;
        if made_progress {
            tracing::debug!(
                quote = %quote.id,
                quote_kind = quote.kind.as_str(),
                submitted,
                "Federation payment observer advanced quote"
            );
        }
        let round_outcome = match advancement {
            FederationPaymentAdvancement::Complete => {
                super::latency::FederationLatencyOutcome::Success
            }
            FederationPaymentAdvancement::Progressed => {
                super::latency::FederationLatencyOutcome::Success
            }
            FederationPaymentAdvancement::AwaitingExternalEvidence
            | FederationPaymentAdvancement::AwaitingBackendNotification
            | FederationPaymentAdvancement::AwaitingFederationEvidence
            | FederationPaymentAdvancement::AwaitingObserverFallback => {
                super::latency::FederationLatencyOutcome::Pending
            }
        };
        round_timer.finish(round_outcome);
        Ok(submitted)
    }

    async fn wait_for_active_attempts(
        &self,
        db: &cdk_common::database::DynMintDatabase,
        quotes: &[FederationPaymentQuote],
        schedule_changes: &mut tokio::sync::watch::Receiver<u64>,
    ) -> Result<(), cdk::Error> {
        loop {
            let now_ms = current_unix_time_ms();
            let Some(lease_until_ms) = self
                .schedule
                .active_lease_until_ms(db, quotes, now_ms)
                .await?
            else {
                return Ok(());
            };
            let remaining = std::time::Duration::from_millis(lease_until_ms.saturating_sub(now_ms));
            match tokio::time::timeout(remaining, schedule_changes.changed()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) | Err(_) => return Ok(()),
            }
        }
    }

    fn advance_one_quote<'a>(
        &'a self,
        state: &'a FederationNode,
        quote: &'a FederationPaymentQuote,
        observer_fallback_round: u32,
        wait_for_backend_update: bool,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<FederationPaymentAdvancement, cdk::Error>>
                + Send
                + 'a,
        >,
    > {
        match quote.kind {
            FederationPaymentQuoteKind::Mint => Box::pin(self.advance_one_mint_quote(
                state,
                &quote.id,
                observer_fallback_round,
                wait_for_backend_update,
            )),
            FederationPaymentQuoteKind::Melt => Box::pin(self.advance_one_melt_quote(
                state,
                &quote.id,
                observer_fallback_round,
                wait_for_backend_update,
            )),
        }
    }

    async fn advance_one_mint_quote(
        &self,
        state: &FederationNode,
        quote_id: &QuoteId,
        observer_fallback_round: u32,
        wait_for_backend_update: bool,
    ) -> Result<FederationPaymentAdvancement, cdk::Error> {
        #[cfg(not(feature = "federation-bark"))]
        let _ = (observer_fallback_round, wait_for_backend_update);
        let Some(quote) = state.mint.localstore().get_mint_quote(quote_id).await? else {
            return Ok(FederationPaymentAdvancement::Complete);
        };
        if matches!(quote.state(), MintQuoteState::Paid | MintQuoteState::Issued) {
            return Ok(FederationPaymentAdvancement::Complete);
        }

        #[cfg(feature = "federation-bark")]
        if quote.payment_method.is_bolt11() && state.federation.bark_runtime().is_some() {
            return match advance_bark_mint_quote(
                state,
                quote_id,
                observer_fallback_round,
                wait_for_backend_update,
            )
            .await
            {
                Ok(advancement) => Ok(advancement),
                Err(error) => {
                    tracing::debug!(
                        quote = %quote_id,
                        error = %error,
                        "federated Bark receive advancement left quote pending"
                    );
                    Ok(FederationPaymentAdvancement::AwaitingExternalEvidence)
                }
            };
        }

        let probe_backend = if quote.payment_method.is_onchain() {
            super::latency::FederationLatencyBackend::Bdk
        } else {
            super::latency::FederationLatencyBackend::Cashu
        };
        let probe = Box::pin(async move {
            state
                .mint
                .probe_federation_mint_quote_payment(quote_id)
                .await
                .map(|observation| observation.map(FederationPaymentObservation::MintQuote))
        });
        Self::probe_and_submit_quote(
            state,
            quote_id,
            probe_backend,
            super::latency::FederationLatencyDirection::Incoming,
            "mint",
            probe,
        )
        .await
    }

    pub async fn observe_melt_quote(
        &self,
        state: &FederationNode,
        quote_id: &QuoteId,
    ) -> Result<bool, cdk::Error> {
        let submitted = Box::pin(self.drive_or_dispatch_quotes(
            state,
            vec![FederationPaymentQuote::melt(quote_id.clone())],
            FederationPaymentObservationTrigger::Requested,
            Instant::now(),
        ))
        .await?;
        Ok(submitted > 0)
    }

    async fn advance_one_melt_quote(
        &self,
        state: &FederationNode,
        quote_id: &QuoteId,
        observer_fallback_round: u32,
        wait_for_backend_update: bool,
    ) -> Result<FederationPaymentAdvancement, cdk::Error> {
        #[cfg(not(feature = "federation-bark"))]
        let _ = (observer_fallback_round, wait_for_backend_update);
        let Some(quote) = state.mint.localstore().get_melt_quote(quote_id).await? else {
            return Ok(FederationPaymentAdvancement::Complete);
        };
        if quote.state != MeltQuoteState::Pending {
            return Ok(FederationPaymentAdvancement::Complete);
        }

        #[cfg(feature = "federation-bark")]
        if quote.payment_method.is_bolt11() && state.federation.bark_runtime().is_some() {
            return match advance_bark_melt_quote(
                state,
                quote_id,
                observer_fallback_round,
                wait_for_backend_update,
            )
            .await
            {
                Ok(advancement) => Ok(advancement),
                Err(error) => {
                    tracing::debug!(
                        quote = %quote_id,
                        error = %error,
                        "federated Bark send planning left quote pending"
                    );
                    Ok(FederationPaymentAdvancement::AwaitingExternalEvidence)
                }
            };
        }
        let probe_backend = if quote.payment_method.is_onchain() {
            super::latency::FederationLatencyBackend::Bdk
        } else {
            super::latency::FederationLatencyBackend::Cashu
        };
        let probe = Box::pin(async move {
            state
                .mint
                .probe_federation_melt_quote_payment(quote_id)
                .await
                .map(|observation| observation.map(FederationPaymentObservation::MeltQuote))
        });
        Self::probe_and_submit_quote(
            state,
            quote_id,
            probe_backend,
            super::latency::FederationLatencyDirection::Outgoing,
            "melt",
            probe,
        )
        .await
    }

    fn probe_and_submit_quote<'a>(
        state: &'a FederationNode,
        quote_id: &'a QuoteId,
        probe_backend: super::latency::FederationLatencyBackend,
        direction: super::latency::FederationLatencyDirection,
        quote_kind: &'static str,
        probe: FederationPaymentProbeFuture<'a>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<FederationPaymentAdvancement, cdk::Error>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let probe_timer = super::latency::FederationLatencyTimer::start(
                super::latency::FederationLatencyCorrelation::Quote,
                quote_id.to_string(),
                probe_backend,
                direction,
                super::latency::FederationLatencyPhase::BackendPaymentProbe,
            );
            let probe = Box::pin(std::panic::AssertUnwindSafe(probe).catch_unwind());
            let observation =
                match tokio::time::timeout(FEDERATION_PAYMENT_OBSERVER_PROBE_DEADLINE, probe).await
                {
                    Ok(Ok(Ok(Some(observation)))) => {
                        probe_timer.finish(super::latency::FederationLatencyOutcome::Success);
                        observation
                    }
                    Ok(Ok(Ok(None))) => {
                        probe_timer.finish(super::latency::FederationLatencyOutcome::Success);
                        return Ok(FederationPaymentAdvancement::AwaitingExternalEvidence);
                    }
                    Ok(Ok(Err(error))) => {
                        probe_timer.finish(super::latency::FederationLatencyOutcome::Error);
                        tracing::debug!(
                            quote = %quote_id,
                            quote_kind,
                            error = %error,
                            "federation payment probe failed; quote left pending"
                        );
                        return Ok(FederationPaymentAdvancement::AwaitingExternalEvidence);
                    }
                    Ok(Err(_panic)) => {
                        probe_timer.finish(super::latency::FederationLatencyOutcome::Error);
                        tracing::warn!(
                            quote = %quote_id,
                            quote_kind,
                            "federation payment probe panicked; quote left pending"
                        );
                        return Ok(FederationPaymentAdvancement::AwaitingExternalEvidence);
                    }
                    Err(_) => {
                        probe_timer.finish(super::latency::FederationLatencyOutcome::Timeout);
                        tracing::debug!(
                            quote = %quote_id,
                            quote_kind,
                            "federation payment probe timed out; quote left pending"
                        );
                        return Ok(FederationPaymentAdvancement::AwaitingExternalEvidence);
                    }
                };
            if submit_observation(state, observation).await? {
                Ok(FederationPaymentAdvancement::Progressed)
            } else {
                Ok(FederationPaymentAdvancement::AwaitingFederationEvidence)
            }
        })
    }

    pub async fn observe_event(
        &self,
        state: &FederationNode,
        event: Event,
    ) -> Result<usize, cdk::Error> {
        state
            .ensure_federation_ready_for_authoritative_read()
            .await?;
        match event {
            Event::PaymentReceived(payment) => {
                let Some(quote) = state
                    .mint
                    .localstore()
                    .get_mint_quote_by_request_lookup_id(&payment.payment_identifier)
                    .await?
                else {
                    return Ok(0);
                };
                Box::pin(self.drive_or_dispatch_quotes(
                    state,
                    vec![FederationPaymentQuote::mint(quote.id)],
                    FederationPaymentObservationTrigger::BackendNotified,
                    Instant::now(),
                ))
                .await
            }
            Event::PaymentSuccessful { quote_id, .. } | Event::PaymentFailed { quote_id, .. } => {
                Box::pin(self.drive_or_dispatch_quotes(
                    state,
                    vec![FederationPaymentQuote::melt(quote_id)],
                    FederationPaymentObservationTrigger::BackendNotified,
                    Instant::now(),
                ))
                .await
            }
        }
    }

    async fn scheduled_quotes(&self, state: &FederationNode) -> Result<usize, cdk::Error> {
        if state.mint.federation_config().is_none() {
            return Ok(0);
        }
        state
            .ensure_federation_ready_for_authoritative_read()
            .await?;

        let db = state.mint.localstore();
        let mint_cursor = self
            .schedule
            .scan_cursor(&db, FederationPaymentQuoteKind::Mint)
            .await?;
        let melt_cursor = self
            .schedule
            .scan_cursor(&db, FederationPaymentQuoteKind::Melt)
            .await?;
        let mint_ids = db
            .get_federation_mint_quote_ids_for_payment_observation(
                mint_cursor.as_ref(),
                FEDERATION_PAYMENT_SCAN_LIMIT_PER_KIND,
            )
            .await?;
        let melt_ids = db
            .get_federation_melt_quote_ids_for_payment_observation(
                melt_cursor.as_ref(),
                FEDERATION_PAYMENT_SCAN_LIMIT_PER_KIND,
            )
            .await?;
        let mint_reaches_end = mint_ids.len() < FEDERATION_PAYMENT_SCAN_LIMIT_PER_KIND;
        let melt_reaches_end = melt_ids.len() < FEDERATION_PAYMENT_SCAN_LIMIT_PER_KIND;
        if mint_ids.is_empty() && mint_cursor.is_some() {
            self.schedule
                .set_scan_cursor(&db, FederationPaymentQuoteKind::Mint, None)
                .await?;
        }
        if melt_ids.is_empty() && melt_cursor.is_some() {
            self.schedule
                .set_scan_cursor(&db, FederationPaymentQuoteKind::Melt, None)
                .await?;
        }

        let mut submitted = 0usize;
        let batch_len = mint_ids.len().max(melt_ids.len());
        for index in 0..batch_len {
            for (kind, ids, reaches_end) in [
                (
                    FederationPaymentQuoteKind::Mint,
                    &mint_ids,
                    mint_reaches_end,
                ),
                (
                    FederationPaymentQuoteKind::Melt,
                    &melt_ids,
                    melt_reaches_end,
                ),
            ] {
                let Some(id) = ids.get(index) else {
                    continue;
                };
                let quote = match kind {
                    FederationPaymentQuoteKind::Mint => FederationPaymentQuote::mint(id.clone()),
                    FederationPaymentQuoteKind::Melt => FederationPaymentQuote::melt(id.clone()),
                };
                submitted = submitted.saturating_add(
                    Box::pin(self.drive_or_dispatch_quotes(
                        state,
                        vec![quote],
                        FederationPaymentObservationTrigger::Scheduled,
                        Instant::now(),
                    ))
                    .await?,
                );
                let next_cursor = match reaches_end && index.saturating_add(1) == ids.len() {
                    true => None,
                    false => Some(id.clone()),
                };
                self.schedule
                    .set_scan_cursor(&db, kind, next_cursor)
                    .await?;
            }
        }

        Ok(submitted)
    }

    pub async fn run_round(&self, state: &FederationNode) -> Result<usize, cdk::Error> {
        #[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
        let submitted = self.observe_onchain_wallets(state).await?;
        #[cfg(not(any(feature = "federation-bdk", feature = "federation-bark")))]
        let submitted = 0_usize;
        let submitted = submitted.saturating_add(self.scheduled_quotes(state).await?);

        Ok(submitted)
    }

    pub fn notify_accepted_operation(&self, entry: &cdk::federation::FederationJournalEntry) {
        if entry.status() == FederationOperationStatus::Applied
            && matches!(
                entry.envelope.operation,
                FederationOperation::AddressAllocation(_)
                    | FederationOperation::MintQuote(_)
                    | FederationOperation::ChainObservation(_)
                    | FederationOperation::TransactionProposal(_)
                    | FederationOperation::TransactionSigned(_)
                    | FederationOperation::TransactionBroadcastIntent(_)
            )
        {
            self.onchain_activity_wake.notify_one();
        }
        for quote in payment_wakes_for_entry(entry) {
            let _ = self
                .accepted_operation_wakes
                .send((quote, entry.index, Instant::now()));
        }
    }

    async fn next_schedule_delay(
        &self,
        db: &cdk_common::database::DynMintDatabase,
    ) -> Result<std::time::Duration, cdk::Error> {
        let Some(deadline_ms) = self.schedule.next_attempt_at_ms(db).await? else {
            return Ok(FEDERATION_PAYMENT_IDLE_DEADLINE);
        };
        Ok(std::time::Duration::from_millis(
            deadline_ms.saturating_sub(current_unix_time_ms()),
        ))
    }

    pub fn spawn(self, state: FederationNode) {
        if state.mint.federation_config().is_none() {
            return;
        }

        let mut payment_events = state.mint.subscribe_federation_payment_events();
        let mut accepted_operation_wakes = self.accepted_operation_wakes.subscribe();
        let mut schedule_changes = self.schedule.subscribe_changes();
        let tasks = state.federation.tasks.clone();
        let stop = tasks.stop_token();
        tasks.spawn("payment observer", true, async move {
            let mut repair_interval = tokio::time::interval(FEDERATION_PAYMENT_REPAIR_INTERVAL);
            repair_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut next_onchain_at = tokio::time::Instant::now();
            let mut failure_backoff = FederationPaymentWorkerFailureBackoff::default();

            loop {
                if stop.is_cancelled() {
                    break;
                }
                let schedule_delay = match self.next_schedule_delay(&state.mint.localstore()).await
                {
                    Ok(delay) => delay,
                    Err(error) => {
                        tracing::warn!(
                            error = %error,
                            "Could not load federation payment schedule; using repair deadline"
                        );
                        FEDERATION_PAYMENT_REPAIR_INTERVAL
                    }
                };
                // A dispatched quote holds its attempt lease, so it is not due
                // while its task runs. A due quote that cannot get a task
                // permit is left unleased and stays due, so a scheduled round
                // started while every permit is held would find it due again
                // immediately and spin. Wait for a running quote task to
                // finish instead.
                let scheduled_round_ready =
                    scheduled_round_can_dispatch(self.quote_tasks.available_permits());
                // One panicking round, event, or backend future must not kill
                // the only observation worker; it is logged and the loop
                // continues so remaining quotes keep being repaired.
                let (result, closed) = match std::panic::AssertUnwindSafe(async {
                    tokio::select! {
                        _ = stop.cancelled() => (Ok(0), true),
                        _ = tokio::time::sleep(schedule_delay), if scheduled_round_ready => {
                            (self.scheduled_quotes(&state).await, false)
                        }
                        permit = self.quote_tasks.clone().acquire_owned(), if !scheduled_round_ready => {
                            // Release it at once: the next round dispatches
                            // through `dispatch_quotes`, which takes its own.
                            drop(permit);
                            (Ok(0), false)
                        }
                        _ = repair_interval.tick() => (self.run_round(&state).await, false),
                        _ = tokio::time::sleep_until(next_onchain_at) => {
                            let observe_result = self.observe_onchain_wallets(&state).await;
                            let delay = match self.next_onchain_observer_interval(&state).await {
                                Ok(delay) => delay,
                                Err(error) => {
                                    tracing::warn!(
                                        error = %error,
                                        "Could not inspect federation on-chain activity; using idle interval"
                                    );
                                    self.onchain_idle_interval
                                }
                            };
                            next_onchain_at = tokio::time::Instant::now() + delay;
                            (observe_result, false)
                        }
                        _ = self.onchain_activity_wake.notified() => {
                            let active_deadline =
                                tokio::time::Instant::now() + self.onchain_active_interval;
                            if active_deadline < next_onchain_at {
                                next_onchain_at = active_deadline;
                            }
                            (Ok(0), false)
                        }
                        changed = schedule_changes.changed() => {
                            (changed.map(|_| 0).map_err(|_| cdk::Error::Internal), false)
                        }
                        wake = accepted_operation_wakes.recv() => match wake {
                            Ok((quote, operation_index, queued_at)) => (
                                Box::pin(self.drive_or_dispatch_quotes(
                                    &state,
                                    vec![quote],
                                    FederationPaymentObservationTrigger::OperationAccepted(
                                        operation_index,
                                    ),
                                    queued_at,
                                )).await,
                                false,
                            ),
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                tracing::warn!(
                                    skipped,
                                    "Federation payment journal wake lagged; running repair scan"
                                );
                                (self.run_round(&state).await, false)
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => (Ok(0), true),
                        },
                        event = payment_events.recv() => match event {
                            Ok(event) => (self.observe_event(&state, event).await, false),
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                tracing::warn!(
                                    skipped,
                                    "Federation payment observer lagged; running repair scan"
                                );
                                (self.run_round(&state).await, false)
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => (Ok(0), true),
                        },
                    }
                })
                .catch_unwind()
                .await
                {
                    Ok(inner) => inner,
                    Err(_) => {
                        tracing::error!(
                            "Federation payment observer round panicked; continuing observation"
                        );
                        (Err(cdk::Error::Internal), false)
                    }
                };
                if closed {
                    break;
                }
                match result {
                    Ok(submitted) if submitted > 0 => {
                        failure_backoff.reset();
                        tracing::debug!(
                            submitted,
                            "Federation payment observer submitted observations"
                        );
                    }
                    Ok(_) => failure_backoff.reset(),
                    Err(err) => {
                        let retry_delay = failure_backoff.next_delay();
                        tracing::warn!(
                            error = %err,
                            consecutive_failures = failure_backoff.consecutive_failures,
                            retry_delay_ms = u64::try_from(retry_delay.as_millis())
                                .unwrap_or(u64::MAX),
                            "Federation payment observer failed; backing off worker"
                        );
                        tokio::select! {
                            _ = stop.cancelled() => break,
                            _ = tokio::time::sleep(retry_delay) => {}
                        }
                    }
                }
            }
        });
    }
}

/// Whether the worker may start a scheduled round now.
///
/// With no free quote-task permit, every due Bark quote is left due without
/// being driven, so the round would be repeated without delay.
fn scheduled_round_can_dispatch(available_quote_permits: usize) -> bool {
    available_quote_permits > 0
}

fn continue_payment_pump(
    completed_steps: usize,
    advancement: FederationPaymentAdvancement,
    before_operation_index: Option<u64>,
    after_operation_index: Option<u64>,
) -> bool {
    completed_steps.saturating_add(1) < FEDERATION_PAYMENT_MAX_ADVANCEMENTS_PER_WAKE
        && (advancement == FederationPaymentAdvancement::Progressed
            || before_operation_index != after_operation_index)
}

fn payment_wakes_for_entry(
    entry: &cdk::federation::FederationJournalEntry,
) -> Vec<FederationPaymentQuote> {
    if entry.status() != FederationOperationStatus::Applied {
        return Vec::new();
    }

    let parse = |quote: &str| quote.parse::<QuoteId>().ok();
    match &entry.envelope.operation {
        FederationOperation::AddressAllocation(operation) => parse(&operation.quote)
            .map(FederationPaymentQuote::mint)
            .into_iter()
            .collect(),
        FederationOperation::ChainObservation(FederationChainObservation::Deposit(observation)) => {
            parse(&observation.allocation.quote)
                .map(FederationPaymentQuote::mint)
                .into_iter()
                .collect()
        }
        FederationOperation::MintQuote(operation) => parse(&operation.quote)
            .map(FederationPaymentQuote::mint)
            .into_iter()
            .collect(),
        FederationOperation::MintQuotePayment(observation) => parse(&observation.quote)
            .map(FederationPaymentQuote::mint)
            .into_iter()
            .collect(),
        FederationOperation::Mint(operation) => operation
            .request
            .quote_ids()
            .into_iter()
            .filter_map(parse)
            .map(FederationPaymentQuote::mint)
            .collect(),
        FederationOperation::MeltQuote(operation) => parse(&operation.quote)
            .map(FederationPaymentQuote::melt)
            .into_iter()
            .collect(),
        FederationOperation::MeltQuotePayment(observation) => parse(&observation.quote)
            .map(FederationPaymentQuote::melt)
            .into_iter()
            .collect(),
        FederationOperation::Melt(operation) => parse(operation.request.quote())
            .map(FederationPaymentQuote::melt)
            .into_iter()
            .collect(),
        #[cfg(feature = "federation-bark")]
        FederationOperation::Application(_) => {
            cdk::federation::validate_federation_bark_operation(&entry.envelope.operation)
                .ok()
                .and_then(|operation| operation.quote().map(str::to_owned))
                .and_then(|quote| parse(&quote))
                .into_iter()
                .flat_map(|quote| {
                    [
                        FederationPaymentQuote::mint(quote.clone()),
                        FederationPaymentQuote::melt(quote),
                    ]
                })
                .collect()
        }
        #[cfg(not(feature = "federation-bark"))]
        FederationOperation::Application(_) => Vec::new(),
        FederationOperation::ChainObservation(FederationChainObservation::Reorg(_))
        | FederationOperation::Swap(_)
        | FederationOperation::KeysetRotation(_)
        | FederationOperation::TransactionProposal(_)
        | FederationOperation::TransactionSigned(_)
        | FederationOperation::TransactionBroadcastIntent(_)
        | FederationOperation::TransactionBroadcastObservation(_) => Vec::new(),
    }
}

async fn payment_observation_needs_submission(
    state: &FederationNode,
    candidate: &FederationPaymentObservation,
) -> Result<bool, cdk::Error> {
    let Some(config) = state.mint.federation_config() else {
        return Ok(false);
    };
    let records = state
        .mint
        .localstore()
        .get_federation_payment_observations(FederationPaymentObservationSelection::Live {
            context: candidate.context(),
            quote: candidate.quote(),
        })
        .await?
        .into_iter()
        .map(|observation| observation.record)
        .collect::<Vec<_>>();
    if records.iter().any(|record| {
        record.observation.observer() == candidate.observer()
            && candidate.matches_payment_evidence(&record.observation)
    }) {
        return Ok(false);
    }

    let observations = records
        .iter()
        .map(|record| &record.observation)
        .collect::<Vec<_>>();
    let policy = *config
        .public_config()
        .payment_verification_policy(candidate.context());
    if let Some(base) = observations
        .iter()
        .copied()
        .find(|observation| candidate.matches_payment_evidence(observation))
    {
        let quorum = base.quorum(
            observations.iter().copied(),
            policy.required_matching_observations(),
        );
        if policy.evaluate_quorum(&quorum).is_ok() {
            return Ok(false);
        }
    }

    Ok(true)
}

async fn submit_observation(
    state: &FederationNode,
    candidate: FederationPaymentObservation,
) -> Result<bool, cdk::Error> {
    let Some(config) = state.mint.federation_config() else {
        return Ok(false);
    };
    if !payment_observation_needs_submission(state, &candidate).await? {
        return Ok(false);
    }

    let operation = match candidate {
        FederationPaymentObservation::MintQuote(observation) => {
            FederationOperation::MintQuotePayment(observation)
        }
        FederationPaymentObservation::MeltQuote(observation) => {
            FederationOperation::MeltQuotePayment(observation)
        }
    };
    let envelope = config.operation_envelope(operation);
    state
        .submit_federation_payment_observation_envelope(envelope)
        .await?;
    Ok(true)
}

pub async fn payment_observation_report_summaries(
    state: &FederationNode,
) -> Result<Vec<FederationPaymentObservationReportSummary>, cdk::Error> {
    let Some(runtime_config) = state.mint.federation_config() else {
        return Ok(Vec::new());
    };
    // One read of the live observations, grouped by quote in consensus order.
    let mut observations_by_quote = BTreeMap::<_, Vec<_>>::new();
    for observation in state
        .mint
        .localstore()
        .get_federation_payment_observations(FederationPaymentObservationSelection::AllLive)
        .await?
    {
        observations_by_quote
            .entry((
                observation.record.observation.context(),
                observation.record.observation.quote().to_string(),
            ))
            .or_default()
            .push(observation.record);
    }

    let mut reports = Vec::with_capacity(observations_by_quote.len());
    let now = current_unix_time();
    for ((context, quote), observations) in observations_by_quote {
        let required = runtime_config
            .public_config()
            .payment_verification_policy(context)
            .required_matching_observations();
        let report = FederationPaymentObservationReport::from_records(
            context,
            &quote,
            required,
            &observations,
        );
        let quote_age_secs =
            payment_observation_quote_age_secs(state, context, &quote, now).await?;
        reports.push(
            report
                .summary_with_quote_age(quote_age_secs, FEDERATION_STALE_PAYMENT_OBSERVATION_SECS),
        );
    }

    Ok(reports)
}

async fn payment_observation_quote_age_secs(
    state: &FederationNode,
    context: cdk::federation::FederationPaymentObservationContext,
    quote: &str,
    now: u64,
) -> Result<Option<u64>, cdk::Error> {
    let Ok(quote_id) = cdk::mint::QuoteId::from_str(quote) else {
        return Ok(None);
    };
    let db = state.mint.localstore();
    match context {
        cdk::federation::FederationPaymentObservationContext::MintQuote => Ok(db
            .get_mint_quote(&quote_id)
            .await?
            .map(|quote| now.saturating_sub(quote.created_time))),
        cdk::federation::FederationPaymentObservationContext::MeltQuote => Ok(db
            .get_melt_quote(&quote_id)
            .await?
            .map(|quote| now.saturating_sub(quote.created_time))),
    }
}

fn current_unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn current_unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use cdk::federation::{
        FederationApplicationState, FederationId, FederationMintQuotePaymentObservation,
        FederationOperationEnvelope, InMemoryFederationJournal, MemberId,
    };
    use cdk::nuts::PaymentMethod;
    use cdk::Amount;
    #[cfg(feature = "federation-bdk")]
    use cdk_common::bitcoin::{hashes::Hash, BlockHash};

    use super::*;

    #[test]
    #[cfg(feature = "federation-bdk")]
    fn automatic_recovery_rejects_a_different_block_anchor() {
        let original_block = (BlockHash::from_byte_array([1; 32]), 100);
        let same_block = FederationDepositConfirmation {
            block_hash: original_block.0,
            block_height: original_block.1,
            confirmation_depth: 1,
        };
        let different_block = FederationDepositConfirmation {
            block_hash: BlockHash::from_byte_array([2; 32]),
            ..same_block
        };

        assert!(confirmation_matches_known_anchor(
            Some(original_block),
            Some(same_block)
        ));
        assert!(!confirmation_matches_known_anchor(
            Some(original_block),
            Some(different_block)
        ));
    }

    #[test]
    #[cfg(feature = "federation-bdk")]
    fn reorg_requires_positive_local_chain_disconnection_evidence() {
        for membership in [
            cdk_bitcoin_wallet::FederatedBlockMembership::Connected,
            cdk_bitcoin_wallet::FederatedBlockMembership::Unknown,
        ] {
            assert!(
                !reorg_disconnection_is_proven(false, membership),
                "missing or unconfirmed output evidence must remain pending"
            );
        }
        assert!(reorg_disconnection_is_proven(
            false,
            cdk_bitcoin_wallet::FederatedBlockMembership::Disconnected
        ));
        assert!(!reorg_disconnection_is_proven(
            true,
            cdk_bitcoin_wallet::FederatedBlockMembership::Disconnected
        ));
    }

    #[test]
    fn scheduled_round_waits_while_every_quote_task_permit_is_held() {
        assert!(!scheduled_round_can_dispatch(0));
        assert!(scheduled_round_can_dispatch(1));
        assert!(scheduled_round_can_dispatch(
            FEDERATION_PAYMENT_CONCURRENT_QUOTE_TASKS
        ));
    }

    #[tokio::test]
    async fn worker_wakes_when_a_quote_task_permit_is_released() {
        let observer = FederationPaymentObserver::default();
        let held = observer
            .quote_tasks
            .clone()
            .acquire_many_owned(
                u32::try_from(FEDERATION_PAYMENT_CONCURRENT_QUOTE_TASKS)
                    .expect("permit count fits in u32"),
            )
            .await
            .expect("semaphore is open");
        assert!(!scheduled_round_can_dispatch(
            observer.quote_tasks.available_permits()
        ));

        let waiter = tokio::spawn({
            let quote_tasks = observer.quote_tasks.clone();
            async move { quote_tasks.acquire_owned().await.map(drop) }
        });
        tokio::task::yield_now().await;
        assert!(
            !waiter.is_finished(),
            "the worker must block, not spin, while every permit is held"
        );

        drop(held);
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("released permit wakes the worker")
            .expect("waiter task completes")
            .expect("semaphore is open");
        assert!(scheduled_round_can_dispatch(
            observer.quote_tasks.available_permits()
        ));
    }

    async fn memory_db() -> cdk_common::database::DynMintDatabase {
        std::sync::Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        )
    }

    /// Persist a finished attempt whose next scheduled attempt is already due.
    async fn schedule_due_quote(
        observer: &FederationPaymentObserver,
        db: &cdk_common::database::DynMintDatabase,
        quote: &FederationPaymentQuote,
        now_ms: u64,
    ) {
        let attempt = observer
            .schedule
            .claim(
                db,
                quote,
                FederationPaymentObservationTrigger::Scheduled,
                now_ms.saturating_sub(2_000),
            )
            .await
            .expect("claim earlier attempt")
            .expect("earlier attempt permit");
        observer
            .schedule
            .finish(
                db,
                attempt,
                MemberId::new(1).expect("member"),
                crate::payment_schedule::FederationPaymentObservationProgress {
                    advancement: FederationPaymentAdvancement::AwaitingBackendNotification,
                    made_progress: false,
                    last_processed_operation_index: None,
                },
                now_ms.saturating_sub(1_000),
            )
            .await
            .expect("finish earlier attempt");
    }

    #[tokio::test]
    async fn due_quote_is_dispatched_once_until_its_attempt_finishes() {
        let db = memory_db().await;
        let observer = FederationPaymentObserver::default();
        let quote = FederationPaymentQuote::mint(QuoteId::new());
        let now_ms = current_unix_time_ms();
        schedule_due_quote(&observer, &db, &quote, now_ms).await;
        assert_eq!(
            observer.next_schedule_delay(&db).await.expect("delay"),
            std::time::Duration::ZERO,
            "the quote is due before it is dispatched"
        );

        let FederationPaymentQuoteTaskClaim::Claimed {
            task_permit,
            attempt,
        } = observer
            .claim_quote_task(
                &db,
                &quote,
                FederationPaymentObservationTrigger::Scheduled,
                now_ms,
            )
            .await
            .expect("first dispatch")
        else {
            panic!("a due quote is dispatched");
        };

        // Scheduled rounds that run again before the task starts driving the
        // quote must neither dispatch it again nor take more task permits.
        for _ in 0..FEDERATION_PAYMENT_CONCURRENT_QUOTE_TASKS * 2 {
            let repeated = observer
                .claim_quote_task(
                    &db,
                    &quote,
                    FederationPaymentObservationTrigger::Scheduled,
                    now_ms,
                )
                .await
                .expect("repeated round");
            assert!(
                matches!(repeated, FederationPaymentQuoteTaskClaim::NotDue),
                "a dispatched quote is not dispatched again"
            );
        }
        assert_eq!(
            observer.quote_tasks.available_permits(),
            FEDERATION_PAYMENT_CONCURRENT_QUOTE_TASKS - 1
        );

        drop(task_permit);
        observer
            .schedule
            .finish(
                &db,
                attempt,
                MemberId::new(1).expect("member"),
                crate::payment_schedule::FederationPaymentObservationProgress {
                    advancement: FederationPaymentAdvancement::AwaitingBackendNotification,
                    made_progress: false,
                    last_processed_operation_index: None,
                },
                now_ms,
            )
            .await
            .expect("finish attempt");
        assert!(matches!(
            observer
                .claim_quote_task(
                    &db,
                    &quote,
                    FederationPaymentObservationTrigger::Scheduled,
                    now_ms.saturating_add(1_000),
                )
                .await
                .expect("next attempt"),
            FederationPaymentQuoteTaskClaim::Claimed { .. }
        ));
    }

    #[tokio::test]
    async fn worker_waits_while_every_due_quote_is_being_driven() {
        let db = memory_db().await;
        let observer = FederationPaymentObserver::default();
        let quotes = [
            FederationPaymentQuote::mint(QuoteId::new()),
            FederationPaymentQuote::melt(QuoteId::new()),
        ];
        let now_ms = current_unix_time_ms();
        let mut attempts = Vec::new();
        for quote in &quotes {
            schedule_due_quote(&observer, &db, quote, now_ms).await;
            attempts.push(
                observer
                    .claim_quote_task(
                        &db,
                        quote,
                        FederationPaymentObservationTrigger::Scheduled,
                        now_ms,
                    )
                    .await
                    .expect("dispatch"),
            );
        }
        assert!(attempts
            .iter()
            .all(|attempt| matches!(attempt, FederationPaymentQuoteTaskClaim::Claimed { .. })));

        // With every due quote leased to a running task, the next scheduled
        // round waits for the lease instead of running again at once.
        assert!(
            observer.next_schedule_delay(&db).await.expect("delay")
                >= std::time::Duration::from_secs(10),
            "the worker must not busy-loop on quotes that are already being driven"
        );
    }

    #[tokio::test]
    async fn quote_stays_due_and_unleased_when_every_task_is_busy() {
        let db = memory_db().await;
        let observer = FederationPaymentObserver::default();
        let quote = FederationPaymentQuote::melt(QuoteId::new());
        let now_ms = current_unix_time_ms();
        schedule_due_quote(&observer, &db, &quote, now_ms).await;
        let held = observer
            .quote_tasks
            .clone()
            .acquire_many_owned(
                u32::try_from(FEDERATION_PAYMENT_CONCURRENT_QUOTE_TASKS)
                    .expect("permit count fits in u32"),
            )
            .await
            .expect("semaphore is open");

        assert!(matches!(
            observer
                .claim_quote_task(
                    &db,
                    &quote,
                    FederationPaymentObservationTrigger::Scheduled,
                    now_ms,
                )
                .await
                .expect("dispatch at capacity"),
            FederationPaymentQuoteTaskClaim::CapacityReached
        ));
        assert_eq!(
            observer.next_schedule_delay(&db).await.expect("delay"),
            std::time::Duration::ZERO,
            "a quote without a task must not be leased"
        );

        drop(held);
        assert!(matches!(
            observer
                .claim_quote_task(
                    &db,
                    &quote,
                    FederationPaymentObservationTrigger::Scheduled,
                    now_ms,
                )
                .await
                .expect("dispatch after release"),
            FederationPaymentQuoteTaskClaim::Claimed { .. }
        ));
    }

    #[test]
    fn progress_pump_runs_immediately_but_yields_after_eight_steps() {
        for completed_steps in 0..FEDERATION_PAYMENT_MAX_ADVANCEMENTS_PER_WAKE - 1 {
            assert!(continue_payment_pump(
                completed_steps,
                FederationPaymentAdvancement::Progressed,
                Some(10),
                Some(11),
            ));
        }
        assert!(!continue_payment_pump(
            FEDERATION_PAYMENT_MAX_ADVANCEMENTS_PER_WAKE - 1,
            FederationPaymentAdvancement::Progressed,
            Some(10),
            Some(11),
        ));
    }

    #[test]
    fn journal_progress_rechecks_a_waiting_quote_without_a_timer() {
        assert!(continue_payment_pump(
            0,
            FederationPaymentAdvancement::AwaitingFederationEvidence,
            Some(10),
            Some(11),
        ));
        assert!(!continue_payment_pump(
            0,
            FederationPaymentAdvancement::AwaitingFederationEvidence,
            Some(11),
            Some(11),
        ));
    }

    #[test]
    fn worker_failure_backoff_is_bounded_and_resets_after_success() {
        let mut backoff = FederationPaymentWorkerFailureBackoff::default();
        let delays = (0..7)
            .map(|_| backoff.next_delay().as_secs())
            .collect::<Vec<_>>();

        assert_eq!(delays, vec![1, 2, 4, 8, 16, 30, 30]);
        backoff.reset();
        assert_eq!(backoff.next_delay(), std::time::Duration::from_secs(1));
    }

    #[test]
    fn accepted_payment_wake_is_scoped_to_the_exact_quote() {
        let quote = QuoteId::new();
        let other = QuoteId::new();
        let operation =
            FederationOperation::MintQuotePayment(FederationMintQuotePaymentObservation {
                quote: quote.to_string(),
                observer: MemberId::new(1).expect("member"),
                method: PaymentMethod::BOLT11,
                state: MintQuoteState::Paid,
                amount_paid: Amount::from(1),
                amount_issued: Amount::ZERO,
                payment_id: None,
                payment_proof: None,
            });
        let mut journal = InMemoryFederationJournal::new();
        let mut entry = journal
            .append(FederationOperationEnvelope::new(
                FederationId::from_bytes([7; 32]),
                operation,
            ))
            .expect("append")
            .entry()
            .clone();

        assert!(payment_wakes_for_entry(&entry).is_empty());
        entry.application = FederationApplicationState::Applied {};
        assert_eq!(
            payment_wakes_for_entry(&entry),
            vec![FederationPaymentQuote::mint(quote)]
        );
        assert!(!payment_wakes_for_entry(&entry).contains(&FederationPaymentQuote::mint(other)));
    }

    #[test]
    fn onchain_observer_intervals_default_and_clamp() {
        let default_observer = FederationPaymentObserver::default();
        assert_eq!(
            default_observer.onchain_active_interval(),
            Duration::from_secs(1)
        );
        assert_eq!(
            default_observer.onchain_idle_interval(),
            Duration::from_secs(5)
        );
        assert_eq!(
            default_observer.onchain_max_batch_items,
            FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS
        );

        let custom = FederationPaymentObserver::default()
            .with_onchain_intervals(Duration::from_millis(250), Duration::from_secs(3))
            .with_onchain_max_batch_items(8);
        assert_eq!(custom.onchain_active_interval(), Duration::from_millis(250));
        assert_eq!(custom.onchain_idle_interval(), Duration::from_secs(3));
        assert_eq!(custom.onchain_max_batch_items, 8);

        let clamped_zero = FederationPaymentObserver::default()
            .with_onchain_intervals(Duration::ZERO, Duration::ZERO)
            .with_onchain_max_batch_items(0);
        assert_eq!(
            clamped_zero.onchain_active_interval(),
            Duration::from_millis(1)
        );
        assert_eq!(
            clamped_zero.onchain_idle_interval(),
            Duration::from_millis(1)
        );
        assert_eq!(clamped_zero.onchain_max_batch_items, 1);

        let clamped_inverted = FederationPaymentObserver::default()
            .with_onchain_intervals(Duration::from_secs(4), Duration::from_secs(2))
            .with_onchain_max_batch_items(FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS + 100);
        assert_eq!(
            clamped_inverted.onchain_active_interval(),
            Duration::from_secs(4)
        );
        assert_eq!(
            clamped_inverted.onchain_idle_interval(),
            Duration::from_secs(4)
        );
        assert_eq!(
            clamped_inverted.onchain_max_batch_items,
            FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS
        );
    }

    const TEST_APP_KEY_EPOCH: u64 = 3;

    async fn local_onchain_test_node() -> (
        FederationNode,
        cdk::mint::FederationRuntimeConfig,
        cdk::federation::FederationFrostApplicationId,
    ) {
        use std::collections::{HashMap, HashSet};

        use cdk::federation::{
            generate_trusted_dealer_configs, FederationConsensusRecoveryBase,
            FederationConsensusRuntime, FederationDkgCeremonyId, FederationFrostApplicationConfig,
            FederationFrostRootPublicConfig, FederationFrostWalletConfig,
            FederationOnchainConfirmationPolicy, FederationPaymentObservationPolicy,
            FederationRestSequenceTracker, FederationSuffixVerifier, ThresholdParams,
            TrustedDealerKeysetSpec, TrustedDealerMember,
        };
        use cdk::mint::{FederationRuntimeConfig, MintBuilder, MintMeltLimits};
        use cdk_common::common::FeeReserve;
        use cdk_common::mint_url::MintUrl;
        use cdk_common::nuts::nut00::KnownMethod;
        use cdk_common::nuts::nut01::BlsSecretKey;
        use cdk_common::{CurrencyUnit, SecretKey};

        let members = (1..=2)
            .map(|id| {
                TrustedDealerMember::new(
                    MemberId::new(id).expect("member id"),
                    MintUrl::from_str(&format!("https://mint{id}.example.com"))
                        .expect("public mint url"),
                    MintUrl::from_str(&format!("https://fed{id}.example.com"))
                        .expect("federation api url"),
                    SecretKey::from_slice(&[id as u8; 32]).expect("identity secret"),
                )
            })
            .collect::<Vec<_>>();
        let keyset = TrustedDealerKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            BTreeMap::from([(
                Amount::from(1),
                vec![
                    BlsSecretKey::from_reduced_bytes(&[2; 32]),
                    BlsSecretKey::from_reduced_bytes(&[3; 32]),
                ],
            )]),
        );
        let mut bundle = generate_trusted_dealer_configs(
            ThresholdParams::with_bft_consensus(2, 2).expect("params"),
            members,
            vec![keyset],
        )
        .expect("trusted dealer bundle");
        bundle.public_config.payment_observation_policy =
            FederationPaymentObservationPolicy::development_single_observation_for_testing();

        let verification_share = [
            2, 121, 190, 102, 126, 249, 220, 187, 172, 85, 160, 98, 149, 206, 135, 11, 7, 2, 155,
            252, 219, 45, 206, 40, 217, 89, 242, 129, 91, 22, 248, 23, 152,
        ];
        let participant_mapping = bundle
            .public_config
            .members
            .keys()
            .map(|member| (*member, member.get()))
            .collect::<BTreeMap<_, _>>();
        let verification_shares = bundle
            .public_config
            .members
            .keys()
            .map(|member| (*member, verification_share))
            .collect::<BTreeMap<_, _>>();
        let mut canonical_public_package = b"CDKFRT01".to_vec();
        canonical_public_package.extend_from_slice(&1_u64.to_be_bytes());
        canonical_public_package.extend_from_slice(&1_u16.to_be_bytes());
        canonical_public_package.extend_from_slice(&1_u16.to_be_bytes());
        canonical_public_package.extend_from_slice(
            &bundle
                .public_config
                .threshold_params
                .signature_threshold
                .to_be_bytes(),
        );
        canonical_public_package.extend_from_slice(
            &u16::try_from(verification_shares.len())
                .expect("test FROST roster fits u16")
                .to_be_bytes(),
        );
        for (member, share) in &verification_shares {
            canonical_public_package.extend_from_slice(&member.get().to_be_bytes());
            canonical_public_package.extend_from_slice(share);
        }
        canonical_public_package.extend_from_slice(&verification_share);
        canonical_public_package.extend_from_slice(&[5; 32]);

        let root = FederationFrostRootPublicConfig {
            version: 2,
            federation_id: bundle.public_config.federation_id,
            setup_authorization: bundle.public_config.setup_authorization,
            ceremony_id: FederationDkgCeremonyId::from_bytes([6; 32]),
            root_key_epoch: 1,
            dkg_protocol_version: 1,
            ciphersuite_version: 1,
            threshold: bundle.public_config.threshold_params.signature_threshold,
            participant_mapping,
            verification_shares,
            untweaked_group_key: verification_share,
            transcript_binding: [5; 32],
            canonical_public_package,
        };
        let application = FederationFrostApplicationConfig::new_cdk_onchain(
            &bundle.public_config,
            &root,
            TEST_APP_KEY_EPOCH,
            cdk_common::bitcoin::Network::Regtest,
            FederationOnchainConfirmationPolicy::new(2).expect("confirmation policy"),
            1,
        )
        .expect("valid test CDK on-chain application");
        let application_id = application.application_id;
        bundle.public_config.frost_wallet = Some(
            FederationFrostWalletConfig::new(
                &bundle.public_config,
                root,
                Some(application),
                BTreeMap::new(),
            )
            .expect("valid test FROST wallet"),
        );

        let private_config = bundle
            .private_configs
            .get(&MemberId::new(1).expect("member 1"))
            .expect("member 1 private config")
            .clone();
        let runtime_config =
            FederationRuntimeConfig::new(bundle.public_config, private_config).expect("runtime");

        let localstore = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("sqlite database"),
        );
        let fake_wallet = cdk_fake_wallet::FakeWallet::new(
            FeeReserve {
                min_fee_reserve: Amount::from(1),
                percent_fee_reserve: 1.0,
            },
            HashMap::new(),
            HashSet::new(),
            0,
            CurrencyUnit::Sat,
        );
        let mut builder =
            MintBuilder::new(localstore.clone()).with_federation_config(runtime_config.clone());
        builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                Arc::new(fake_wallet),
            )
            .await
            .expect("payment processor");
        let mint = Arc::new(
            builder
                .build_with_seed(localstore, &[9; 64])
                .await
                .expect("mint"),
        );

        let frontier = mint
            .localstore()
            .get_federation_checkpoint_frontier()
            .await
            .expect("checkpoint frontier");
        let verifier = FederationSuffixVerifier::new_for_validated_config(
            runtime_config.public_config(),
            runtime_config.member_id(),
            0,
            FederationConsensusRecoveryBase::Genesis,
        )
        .expect("suffix verifier");
        let consensus = FederationConsensusRuntime::recover_verified_suffix(
            verifier
                .finish_for_durable_frontier(&frontier)
                .expect("verified empty suffix"),
        );
        let member = crate::FederationMemberContext::new(
            Arc::clone(&mint),
            Arc::new(tokio::sync::Mutex::new(consensus)),
            Arc::new(tokio::sync::Mutex::new(
                FederationRestSequenceTracker::default(),
            )),
            crate::FederationPeerSyncConfig::default(),
        );
        let mut node = FederationNode {
            mint,
            federation: crate::FederationNodeRuntime::new(
                crate::default_federation_transport(),
                crate::FederationMembership::configured(member),
                #[cfg(feature = "federation-bdk")]
                None,
                crate::runtime_components::FederationRuntimeComponents::new(
                    Some(Arc::new(tokio::sync::Mutex::new(
                        crate::FederationSyncState::default(),
                    ))),
                    None,
                    None,
                    #[cfg(feature = "federation-bdk")]
                    Vec::new(),
                    None,
                    #[cfg(feature = "federation-bark")]
                    None,
                ),
            ),
        };
        node.federation.finality = crate::FederationFinalityMode::Local;
        (node, runtime_config, application_id)
    }

    async fn allocate_and_create_onchain_mint_quote(
        node: &FederationNode,
        runtime: &cdk::mint::FederationRuntimeConfig,
        application_id: cdk::federation::FederationFrostApplicationId,
        key_seed: u8,
    ) -> (QuoteId, cdk::federation::FederationAddressAllocation) {
        use cdk::federation::{
            FederationAddressAllocationScope, FederationAddressKeychain,
            FederationMintQuoteOperation,
        };
        use cdk_common::nuts::nut00::KnownMethod;
        use cdk_common::nuts::nut30::{MintQuoteOnchainRequest, MintQuoteOnchainResponse};
        use cdk_common::payment::PaymentIdentifier;
        use cdk_common::{CurrencyUnit, MintQuoteRequest, MintQuoteResponse, SecretKey};

        let quote_id = QuoteId::new();
        let alloc_envelope = runtime
            .address_allocation_operation(
                quote_id.to_string(),
                application_id,
                TEST_APP_KEY_EPOCH,
                FederationAddressKeychain::External,
            )
            .expect("allocation envelope");
        node.submit_federation_envelope(alloc_envelope)
            .await
            .expect("submit address allocation");
        let scope = FederationAddressAllocationScope {
            application_id,
            application_key_epoch: TEST_APP_KEY_EPOCH,
        };
        let allocation = node
            .mint
            .federation_address_allocation(scope, &quote_id.to_string())
            .await
            .expect("allocation lookup")
            .expect("accepted allocation");
        let wallet_config = runtime
            .public_config()
            .frost_wallet
            .as_ref()
            .expect("wallet config");
        let script = wallet_config
            .derive_cdk_onchain_script(
                runtime.public_config(),
                application_id,
                TEST_APP_KEY_EPOCH,
                allocation.keychain,
                allocation.derivation_index,
            )
            .expect("derived script");
        let address = cdk_common::bitcoin::Address::from_script(
            script.as_script(),
            cdk_common::bitcoin::Network::Regtest,
        )
        .expect("address")
        .to_string();
        let wallet_key = SecretKey::from_slice(&[key_seed; 32]).expect("wallet key");
        let quote_envelope = runtime.operation_envelope(FederationOperation::MintQuote(
            FederationMintQuoteOperation {
                quote: quote_id.to_string(),
                request_lookup_id: PaymentIdentifier::QuoteId(quote_id.clone()),
                request: MintQuoteRequest::Onchain(MintQuoteOnchainRequest {
                    unit: CurrencyUnit::Sat,
                    pubkey: wallet_key.public_key(),
                }),
                response: MintQuoteResponse::Onchain(MintQuoteOnchainResponse {
                    quote: quote_id.to_string(),
                    request: address,
                    unit: CurrencyUnit::Sat,
                    method: PaymentMethod::Known(KnownMethod::Onchain),
                    expiry: None,
                    pubkey: wallet_key.public_key(),
                    amount_paid: Amount::ZERO,
                    amount_issued: Amount::ZERO,
                    updated_at: 0,
                }),
            },
        ));
        node.submit_federation_envelope(quote_envelope)
            .await
            .expect("submit mint quote");
        (quote_id, allocation)
    }

    fn deposit_envelope_for_allocation(
        runtime: &cdk::mint::FederationRuntimeConfig,
        application_id: cdk::federation::FederationFrostApplicationId,
        allocation: &cdk::federation::FederationAddressAllocation,
        tx_seed: u8,
        vout: u32,
        amount_sat: u64,
        confirmation: Option<cdk::federation::FederationDepositConfirmation>,
    ) -> (FederationOperationEnvelope, cdk_common::bitcoin::OutPoint) {
        use cdk::federation::{
            FederationAllocatedAddressRef, FederationChainSourceDescriptor,
            FederationChainSourceScope, FederationDepositObservation,
        };
        use cdk_common::bitcoin::hashes::Hash as _;
        use cdk_common::bitcoin::{Amount as BitcoinAmount, OutPoint, TxOut, Txid};

        let wallet_config = runtime
            .public_config()
            .frost_wallet
            .as_ref()
            .expect("wallet config");
        let script = wallet_config
            .derive_cdk_onchain_script(
                runtime.public_config(),
                application_id,
                TEST_APP_KEY_EPOCH,
                allocation.keychain,
                allocation.derivation_index,
            )
            .expect("derived script");
        let outpoint = OutPoint::new(Txid::from_byte_array([tx_seed; 32]), vout);
        let observation = FederationChainObservation::Deposit(FederationDepositObservation {
            allocation: FederationAllocatedAddressRef {
                application_id,
                application_key_epoch: TEST_APP_KEY_EPOCH,
                quote: allocation.quote.clone(),
                keychain: allocation.keychain,
                derivation_index: allocation.derivation_index,
            },
            outpoint,
            txout: TxOut {
                value: BitcoinAmount::from_sat(amount_sat),
                script_pubkey: script,
            },
            confirmation,
            observation_epoch: 0,
            observer: runtime.member_id(),
            source: FederationChainSourceDescriptor::new(
                FederationChainSourceScope::Independent,
                "test-regtest-source",
            )
            .expect("source"),
        });
        let envelope = runtime
            .chain_observation_operation(observation)
            .expect("chain observation envelope");
        (envelope, outpoint)
    }

    #[tokio::test]
    async fn chunk_operation_envelopes_enforces_item_limit_byte_limit_dedup_and_conflicts() {
        use cdk::federation::FederationDepositConfirmation;
        use cdk_common::bitcoin::hashes::Hash as _;
        use cdk_common::bitcoin::BlockHash;

        let (node, runtime, application_id) = local_onchain_test_node().await;
        let mut envelopes = Vec::new();
        let mut first_allocation = None;
        for index in 0..5_u8 {
            let (_quote_id, allocation) =
                allocate_and_create_onchain_mint_quote(&node, &runtime, application_id, 20 + index)
                    .await;
            if first_allocation.is_none() {
                first_allocation = Some(allocation.clone());
            }
            let (envelope, _) = deposit_envelope_for_allocation(
                &runtime,
                application_id,
                &allocation,
                40 + index,
                0,
                10_000,
                Some(FederationDepositConfirmation {
                    block_hash: BlockHash::from_byte_array([77; 32]),
                    block_height: 100,
                    confirmation_depth: 2,
                }),
            );
            envelopes.push(envelope);
        }

        // Item count cap splits 5 items with cap 2 into [2, 2, 1] preserving original indices.
        let by_items = chunk_operation_envelopes(
            envelopes.clone(),
            2,
            FEDERATION_MAX_ALEPH_BFT_UNIT_BODY_BYTES,
        )
        .expect("chunk by item cap");
        assert_eq!(
            by_items
                .iter()
                .map(|chunk| chunk.iter().map(|item| item.index).collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            vec![vec![0, 1], vec![2, 3], vec![4]]
        );

        // Byte cap splits chunks when adding the next item would exceed max_body_bytes.
        let single_bytes = FederationConsensusItem::operation(envelopes[0].clone())
            .canonical_size()
            .expect("canonical size");
        let by_bytes = chunk_operation_envelopes(
            envelopes.clone(),
            FEDERATION_MAX_ALEPH_BFT_UNIT_ITEMS,
            single_bytes.saturating_mul(2).saturating_add(16),
        )
        .expect("chunk by byte cap");
        assert!(by_bytes.len() >= 3);
        assert!(by_bytes.iter().all(|chunk| chunk.len() <= 2));

        // Exact duplicates by operation_id are deduplicated across chunks.
        let deduped = chunk_operation_envelopes(
            vec![
                envelopes[0].clone(),
                envelopes[0].clone(),
                envelopes[1].clone(),
            ],
            10,
            FEDERATION_MAX_ALEPH_BFT_UNIT_BODY_BYTES,
        )
        .expect("dedup chunks");
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].len(), 2);

        // Distinct envelopes sharing a conflict key (same outpoint/observer/epoch/depth,
        // differing block hash) are placed in separate chunks so one group never carries
        // internal conflicts.
        let (conflicting_anchor, _) = deposit_envelope_for_allocation(
            &runtime,
            application_id,
            first_allocation.as_ref().expect("first allocation"),
            40,
            0,
            10_000,
            Some(FederationDepositConfirmation {
                block_hash: BlockHash::from_byte_array([78; 32]),
                block_height: 100,
                confirmation_depth: 2,
            }),
        );
        let split_conflicts = chunk_operation_envelopes(
            vec![envelopes[0].clone(), conflicting_anchor],
            10,
            FEDERATION_MAX_ALEPH_BFT_UNIT_BODY_BYTES,
        )
        .expect("conflict split");
        assert_eq!(split_conflicts.len(), 2);
        assert_eq!(split_conflicts[0].len(), 1);
        assert_eq!(split_conflicts[1].len(), 1);
    }

    #[tokio::test]
    async fn batched_chain_observations_finalize_in_one_unit_and_pay_quotes() {
        use cdk::federation::FederationDepositConfirmation;
        use cdk_common::bitcoin::hashes::Hash as _;
        use cdk_common::bitcoin::BlockHash;
        use cdk_common::database::mint::FederationJournalEntryKey;

        let (node, runtime, application_id) = local_onchain_test_node().await;
        let observer = FederationPaymentObserver::default();
        let confirmation = Some(FederationDepositConfirmation {
            block_hash: BlockHash::from_byte_array([88; 32]),
            block_height: 150,
            confirmation_depth: 2,
        });

        let mut quote_ids = BTreeSet::new();
        let mut envelopes = Vec::new();
        let mut outpoints = Vec::new();
        for index in 0..3_u8 {
            let (quote_id, allocation) =
                allocate_and_create_onchain_mint_quote(&node, &runtime, application_id, 30 + index)
                    .await;
            let (envelope, outpoint) = deposit_envelope_for_allocation(
                &runtime,
                application_id,
                &allocation,
                50 + index,
                u32::from(index),
                15_000,
                confirmation,
            );
            quote_ids.insert(quote_id);
            envelopes.push(envelope);
            outpoints.push(outpoint);
        }

        let op_ids = envelopes
            .iter()
            .map(|envelope| envelope.operation_id().expect("op id"))
            .collect::<Vec<_>>();
        let batch_result = observer
            .submit_observation_envelope_batch(&node, envelopes)
            .await
            .expect("batched chain observation submit");
        assert_eq!(batch_result.newly_submitted, 3);
        assert_eq!(batch_result.applied_indices, vec![0, 1, 2]);

        // All 3 UTXO observations landed in one single AlephBFT unit consecutively.
        let mut unit_indices = BTreeSet::new();
        let mut item_indices = Vec::new();
        for op_id in &op_ids {
            let entry = node
                .mint
                .localstore()
                .get_federation_journal_entry(FederationJournalEntryKey::Operation(*op_id), None)
                .await
                .expect("journal lookup")
                .expect("persisted entry");
            assert_eq!(entry.status(), FederationOperationStatus::Applied);
            let evidence = entry.consensus_evidence.expect("consensus evidence");
            unit_indices.insert(evidence.unit_index);
            item_indices.push(evidence.item_index);
        }
        assert_eq!(
            unit_indices.len(),
            1,
            "all 3 deposit observations must share one consensus unit"
        );
        let first_item_index = item_indices[0];
        assert_eq!(
            item_indices,
            vec![first_item_index, first_item_index + 1, first_item_index + 2]
        );

        let chain_state = node
            .mint
            .federation_chain_observation_state()
            .await
            .expect("chain state");
        for outpoint in &outpoints {
            assert_eq!(
                chain_state.utxo(*outpoint).expect("utxo").state,
                FederationWalletUtxoState::Spendable
            );
        }

        // Same-pass MintQuotePayment batching marks all 3 quotes Paid immediately in one unit.
        let payment_submitted = observer
            .submit_confirmed_mint_quote_payments(&node, &quote_ids)
            .await
            .expect("same-pass mint quote payments");
        assert_eq!(payment_submitted, 3);

        let mut payment_unit_indices = BTreeSet::new();
        for quote_id in &quote_ids {
            let quote = node
                .mint
                .localstore()
                .get_mint_quote(quote_id)
                .await
                .expect("quote lookup")
                .expect("quote");
            assert_eq!(quote.state(), MintQuoteState::Paid);
            assert_eq!(quote.amount_paid().value(), 15_000);
            let records = node
                .mint
                .localstore()
                .get_federation_payment_observations(FederationPaymentObservationSelection::Live {
                    context: cdk::federation::FederationPaymentObservationContext::MintQuote,
                    quote: &quote_id.to_string(),
                })
                .await
                .expect("payment observations");
            assert_eq!(records.len(), 1);
            let entry = node
                .mint
                .localstore()
                .get_federation_journal_entry(
                    FederationJournalEntryKey::Operation(records[0].record.operation_id),
                    None,
                )
                .await
                .expect("payment journal lookup")
                .expect("payment entry");
            payment_unit_indices.insert(entry.consensus_evidence.expect("evidence").unit_index);
        }
        assert_eq!(
            payment_unit_indices.len(),
            1,
            "all 3 same-pass MintQuotePayment observations must share one consensus unit"
        );
    }

    #[tokio::test]
    async fn batched_chain_observations_chunk_when_exceeding_group_cap() {
        use cdk::federation::FederationDepositConfirmation;
        use cdk_common::bitcoin::hashes::Hash as _;
        use cdk_common::bitcoin::BlockHash;
        use cdk_common::database::mint::FederationJournalEntryKey;

        let (node, runtime, application_id) = local_onchain_test_node().await;
        let observer = FederationPaymentObserver::default().with_onchain_max_batch_items(2);
        let confirmation = Some(FederationDepositConfirmation {
            block_hash: BlockHash::from_byte_array([91; 32]),
            block_height: 160,
            confirmation_depth: 2,
        });

        let mut envelopes = Vec::new();
        let mut outpoints = Vec::new();
        for index in 0..5_u8 {
            let (_quote_id, allocation) =
                allocate_and_create_onchain_mint_quote(&node, &runtime, application_id, 60 + index)
                    .await;
            let (envelope, outpoint) = deposit_envelope_for_allocation(
                &runtime,
                application_id,
                &allocation,
                70 + index,
                0,
                12_000,
                confirmation,
            );
            envelopes.push(envelope);
            outpoints.push(outpoint);
        }
        let op_ids = envelopes
            .iter()
            .map(|envelope| envelope.operation_id().expect("op id"))
            .collect::<Vec<_>>();

        let batch_result = observer
            .submit_observation_envelope_batch(&node, envelopes)
            .await
            .expect("chunked submit");
        assert_eq!(batch_result.newly_submitted, 5);
        assert_eq!(batch_result.applied_indices, vec![0, 1, 2, 3, 4]);

        let mut counts_by_unit = BTreeMap::<u64, usize>::new();
        for op_id in &op_ids {
            let entry = node
                .mint
                .localstore()
                .get_federation_journal_entry(FederationJournalEntryKey::Operation(*op_id), None)
                .await
                .expect("journal lookup")
                .expect("entry");
            let unit_index = entry.consensus_evidence.expect("evidence").unit_index;
            *counts_by_unit.entry(unit_index).or_default() += 1;
        }
        assert_eq!(
            counts_by_unit.into_values().collect::<Vec<_>>(),
            vec![2, 2, 1]
        );

        let chain_state = node
            .mint
            .federation_chain_observation_state()
            .await
            .expect("chain state");
        for outpoint in &outpoints {
            assert!(chain_state.utxo(*outpoint).is_some());
        }
    }

    #[tokio::test]
    async fn batched_chain_observations_fall_back_on_partial_conflict() {
        use cdk::federation::FederationDepositConfirmation;
        use cdk_common::bitcoin::hashes::Hash as _;
        use cdk_common::bitcoin::BlockHash;

        let (node, runtime, application_id) = local_onchain_test_node().await;
        let observer = FederationPaymentObserver::default();
        let confirmation = Some(FederationDepositConfirmation {
            block_hash: BlockHash::from_byte_array([95; 32]),
            block_height: 170,
            confirmation_depth: 2,
        });

        let mut envelopes = Vec::new();
        let mut outpoints = Vec::new();
        for index in 0..3_u8 {
            let (_quote_id, allocation) =
                allocate_and_create_onchain_mint_quote(&node, &runtime, application_id, 80 + index)
                    .await;
            let (envelope, outpoint) = deposit_envelope_for_allocation(
                &runtime,
                application_id,
                &allocation,
                90 + index,
                0,
                20_000,
                confirmation,
            );
            envelopes.push(envelope);
            outpoints.push(outpoint);
        }

        // Pre-submit observation 0 so the 3-item group is partially present in the journal
        // and `submit_envelope_group` rejects with OrderedGroupInvalid.
        node.submit_federation_envelope(envelopes[0].clone())
            .await
            .expect("pre-submit first observation");

        let batch_result = observer
            .submit_observation_envelope_batch(&node, envelopes)
            .await
            .expect("fallback should preserve valid UTXOs");
        assert_eq!(batch_result.newly_submitted, 2);
        assert_eq!(batch_result.applied_indices, vec![0, 1, 2]);

        let chain_state = node
            .mint
            .federation_chain_observation_state()
            .await
            .expect("chain state");
        for outpoint in &outpoints {
            assert_eq!(
                chain_state.utxo(*outpoint).expect("utxo").state,
                FederationWalletUtxoState::Spendable
            );
        }

        // A deterministically rejected observation (wrong script) in a batch must
        // never drop a valid neighbor observation.
        let (_bad_quote, bad_alloc) =
            allocate_and_create_onchain_mint_quote(&node, &runtime, application_id, 100).await;
        let (_good_quote, good_alloc) =
            allocate_and_create_onchain_mint_quote(&node, &runtime, application_id, 101).await;
        let (mut bad_envelope, bad_outpoint) = deposit_envelope_for_allocation(
            &runtime,
            application_id,
            &bad_alloc,
            110,
            0,
            5_000,
            confirmation,
        );
        if let FederationOperation::ChainObservation(FederationChainObservation::Deposit(
            ref mut deposit,
        )) = bad_envelope.operation
        {
            deposit.txout.script_pubkey = cdk_common::bitcoin::ScriptBuf::from_bytes(vec![0x51]);
            bad_envelope = runtime
                .chain_observation_operation(FederationChainObservation::Deposit(deposit.clone()))
                .expect("re-signed invalid deposit");
        }
        let (good_envelope, good_outpoint) = deposit_envelope_for_allocation(
            &runtime,
            application_id,
            &good_alloc,
            111,
            0,
            5_000,
            confirmation,
        );
        let mixed_result = observer
            .submit_observation_envelope_batch(&node, vec![bad_envelope, good_envelope])
            .await
            .expect("mixed batch should land valid observation");
        assert_eq!(mixed_result.newly_submitted, 1);
        assert_eq!(mixed_result.applied_indices, vec![1]);

        let updated_state = node
            .mint
            .federation_chain_observation_state()
            .await
            .expect("updated chain state");
        assert!(updated_state.utxo(bad_outpoint).is_none());
        assert_eq!(
            updated_state.utxo(good_outpoint).expect("good utxo").state,
            FederationWalletUtxoState::Spendable
        );
    }

    #[tokio::test]
    async fn adaptive_onchain_polling_switches_between_active_and_idle_intervals() {
        use cdk::federation::FederationDepositConfirmation;
        use cdk_common::bitcoin::hashes::Hash as _;
        use cdk_common::bitcoin::BlockHash;

        let (node, runtime, application_id) = local_onchain_test_node().await;
        let active = Duration::from_millis(200);
        let idle = Duration::from_secs(5);
        let observer = FederationPaymentObserver::default().with_onchain_intervals(active, idle);

        // Idle when there are no unpaid on-chain quotes, unconfirmed UTXOs, or pending txs.
        assert!(!FederationPaymentObserver::has_active_onchain_work(&node)
            .await
            .expect("idle check"));
        assert_eq!(
            observer
                .next_onchain_observer_interval(&node)
                .await
                .expect("idle interval"),
            idle
        );

        // Unpaid on-chain mint quote switches to the faster active interval.
        let (quote_id, allocation) =
            allocate_and_create_onchain_mint_quote(&node, &runtime, application_id, 120).await;
        assert!(FederationPaymentObserver::has_active_onchain_work(&node)
            .await
            .expect("active quote check"));
        assert_eq!(
            observer
                .next_onchain_observer_interval(&node)
                .await
                .expect("active interval"),
            active
        );

        // Unconfirmed (Observed) deposit keeps the active interval.
        let (unconfirmed_envelope, _) = deposit_envelope_for_allocation(
            &runtime,
            application_id,
            &allocation,
            121,
            0,
            10_000,
            None,
        );
        observer
            .submit_observation_envelope_batch(&node, vec![unconfirmed_envelope])
            .await
            .expect("unconfirmed deposit");
        assert_eq!(
            observer
                .next_onchain_observer_interval(&node)
                .await
                .expect("active unconfirmed interval"),
            active
        );

        // Confirming the deposit to Spendable and settling the MintQuotePayment returns to idle.
        let (confirmed_envelope, _) = deposit_envelope_for_allocation(
            &runtime,
            application_id,
            &allocation,
            121,
            0,
            10_000,
            Some(FederationDepositConfirmation {
                block_hash: BlockHash::from_byte_array([122; 32]),
                block_height: 180,
                confirmation_depth: 2,
            }),
        );
        observer
            .submit_observation_envelope_batch(&node, vec![confirmed_envelope])
            .await
            .expect("confirmed deposit");
        let quote_set = BTreeSet::from([quote_id]);
        assert_eq!(
            observer
                .submit_confirmed_mint_quote_payments(&node, &quote_set)
                .await
                .expect("settle quote"),
            1
        );
        assert!(!FederationPaymentObserver::has_active_onchain_work(&node)
            .await
            .expect("returned to idle"));
        assert_eq!(
            observer
                .next_onchain_observer_interval(&node)
                .await
                .expect("idle interval after settlement"),
            idle
        );

        // A second unconfirmed UTXO alone (even for an already-paid quote) re-activates active cadence.
        let (extra_unconfirmed, _) = deposit_envelope_for_allocation(
            &runtime,
            application_id,
            &allocation,
            123,
            1,
            5_000,
            None,
        );
        observer
            .submit_observation_envelope_batch(&node, vec![extra_unconfirmed])
            .await
            .expect("extra unconfirmed deposit");
        assert_eq!(
            observer
                .next_onchain_observer_interval(&node)
                .await
                .expect("active interval for unconfirmed utxo"),
            active
        );
    }
}
