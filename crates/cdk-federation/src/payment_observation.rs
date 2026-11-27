use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use cdk::cdk_payment::Event;
#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use cdk::federation::{
    FederationAddressAllocationScope, FederationAddressKeychain, FederationAllocatedAddressRef,
    FederationChainSourceDescriptor, FederationChainSourceScope, FederationDepositConfirmation,
    FederationDepositObservation, FederationDepositReorgObservation, FederationWalletUtxoState,
};
use cdk::federation::{
    FederationChainObservation, FederationOperation, FederationOperationStatus,
    FederationPaymentObservation, FederationPaymentObservationReport,
    FederationPaymentObservationReportSummary,
};
use cdk::mint::QuoteId;
use cdk::nuts::{MeltQuoteState, MintQuoteState};
use cdk_common::database::mint::FederationPaymentObservationSelection;
use futures::stream::{FuturesUnordered, StreamExt as _};
use futures::FutureExt as _;

use super::payment_schedule::{
    FederationPaymentAdvancement, FederationPaymentObservationPermit,
    FederationPaymentObservationSchedule, FederationPaymentObservationTrigger,
    FederationPaymentQuote, FederationPaymentQuoteKind, FederationPaymentScheduleLane,
};
use crate::FederationNode;

const FEDERATION_STALE_PAYMENT_OBSERVATION_SECS: u64 = 15 * 60;
const FEDERATION_ONCHAIN_OBSERVER_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
const FEDERATION_PAYMENT_REPAIR_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
const FEDERATION_PAYMENT_IDLE_DEADLINE: std::time::Duration =
    std::time::Duration::from_secs(24 * 60 * 60);
const FEDERATION_PAYMENT_MAX_ADVANCEMENTS_PER_WAKE: usize = 8;
const FEDERATION_PAYMENT_SCAN_LIMIT_PER_KIND: usize = 64;
const FEDERATION_PAYMENT_DUE_BATCH_LIMIT: usize = 64;
const FEDERATION_PAYMENT_WAKE_CAPACITY: usize = 512;
const FEDERATION_PAYMENT_WAKE_DRAIN_LIMIT: usize = 32;
/// Concurrent task permits for unpaid Bark incoming invoice waits.
const FEDERATION_PAYMENT_CONCURRENT_INCOMING_WAIT_TASKS: usize = 64;
/// Concurrent task permits for paid/advancing Bark incoming claims.
const FEDERATION_PAYMENT_CONCURRENT_CLAIM_TASKS: usize = 16;
/// Concurrent task permits for Bark outgoing melts.
const FEDERATION_PAYMENT_CONCURRENT_MELT_TASKS: usize = 16;
/// Concurrent task permits for generic `MintPayment` / non-Bark quote probes.
const FEDERATION_PAYMENT_CONCURRENT_DIRECT_QUOTE_TASKS: usize = 16;
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
    /// Every quote task permit in the quote's lane is busy; the quote stays due and unleased.
    CapacityReached,
    /// The quote is not due, typically because an attempt already holds it.
    NotDue,
    /// The quote's attempt lease is held and a task permit is reserved for it.
    Claimed {
        task_permit: tokio::sync::OwnedSemaphorePermit,
        attempt: FederationPaymentObservationPermit,
    },
}

/// Wake emitted when a consensus operation touching a payment quote is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FederationPaymentWake {
    quote: FederationPaymentQuote,
    /// True when the operation carries payment evidence or advances an active
    /// claim/settlement rather than creating a fresh unpaid quote.
    priority: bool,
}

/// Active quote paired with its bounded execution lane.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ClassifiedPaymentQuote {
    quote: FederationPaymentQuote,
    lane: FederationPaymentScheduleLane,
}

/// Separate bounded permit pools per observation lane so unpaid incoming
/// invoice long-polls cannot starve ready claims, outgoing melts, or direct
/// `MintPayment` probes.
#[derive(Debug, Clone)]
struct FederationPaymentTaskPool {
    incoming_wait: std::sync::Arc<tokio::sync::Semaphore>,
    incoming_claim: std::sync::Arc<tokio::sync::Semaphore>,
    outgoing_melt: std::sync::Arc<tokio::sync::Semaphore>,
    direct: std::sync::Arc<tokio::sync::Semaphore>,
}

impl Default for FederationPaymentTaskPool {
    fn default() -> Self {
        Self {
            incoming_wait: std::sync::Arc::new(tokio::sync::Semaphore::new(
                FEDERATION_PAYMENT_CONCURRENT_INCOMING_WAIT_TASKS,
            )),
            incoming_claim: std::sync::Arc::new(tokio::sync::Semaphore::new(
                FEDERATION_PAYMENT_CONCURRENT_CLAIM_TASKS,
            )),
            outgoing_melt: std::sync::Arc::new(tokio::sync::Semaphore::new(
                FEDERATION_PAYMENT_CONCURRENT_MELT_TASKS,
            )),
            direct: std::sync::Arc::new(tokio::sync::Semaphore::new(
                FEDERATION_PAYMENT_CONCURRENT_DIRECT_QUOTE_TASKS,
            )),
        }
    }
}

impl FederationPaymentTaskPool {
    fn semaphore(
        &self,
        lane: FederationPaymentScheduleLane,
    ) -> &std::sync::Arc<tokio::sync::Semaphore> {
        match lane {
            FederationPaymentScheduleLane::IncomingWait => &self.incoming_wait,
            FederationPaymentScheduleLane::IncomingClaim => &self.incoming_claim,
            FederationPaymentScheduleLane::OutgoingMelt => &self.outgoing_melt,
            FederationPaymentScheduleLane::Direct => &self.direct,
        }
    }

    fn available_permits(&self, lane: FederationPaymentScheduleLane) -> usize {
        self.semaphore(lane).available_permits()
    }

    fn available_lanes(&self) -> Vec<FederationPaymentScheduleLane> {
        FederationPaymentScheduleLane::ALL
            .into_iter()
            .filter(|lane| self.available_permits(*lane) > 0)
            .collect()
    }

    fn saturated_lanes(&self) -> Vec<FederationPaymentScheduleLane> {
        FederationPaymentScheduleLane::ALL
            .into_iter()
            .filter(|lane| self.available_permits(*lane) == 0)
            .collect()
    }

    /// Wait until at least one lane in `lanes` releases a permit, then release
    /// it immediately so the observer loop can dispatch the next due quote.
    async fn wait_for_any_lane(&self, lanes: &[FederationPaymentScheduleLane]) {
        if lanes.is_empty() {
            return;
        }
        let waiters = lanes
            .iter()
            .map(|lane| Box::pin(self.semaphore(*lane).clone().acquire_owned()))
            .collect::<Vec<_>>();
        let (result, _, _) = futures::future::select_all(waiters).await;
        if let Ok(permit) = result {
            drop(permit);
        }
    }
}

#[derive(Debug, Clone)]
pub struct FederationPaymentObserver {
    schedule: FederationPaymentObservationSchedule,
    accepted_operation_wakes: tokio::sync::broadcast::Sender<(FederationPaymentWake, u64, Instant)>,
    task_pool: FederationPaymentTaskPool,
}

impl Default for FederationPaymentObserver {
    fn default() -> Self {
        let (accepted_operation_wakes, _) =
            tokio::sync::broadcast::channel(FEDERATION_PAYMENT_WAKE_CAPACITY);
        Self {
            schedule: FederationPaymentObservationSchedule::default(),
            accepted_operation_wakes,
            task_pool: FederationPaymentTaskPool::default(),
        }
    }
}

impl FederationPaymentObserver {
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
        let mut submitted = 0_usize;
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
                let observation =
                    FederationChainObservation::Deposit(FederationDepositObservation {
                        allocation: FederationAllocatedAddressRef {
                            application_id: scope.application_id,
                            application_key_epoch: scope.application_key_epoch,
                            quote: allocation.quote.clone(),
                            keychain,
                            derivation_index: allocation.derivation_index,
                        },
                        outpoint: output.outpoint,
                        txout: output.txout.clone(),
                        confirmation,
                        observation_epoch,
                        observer,
                        source: source.clone(),
                    });
                state
                    .submit_federation_chain_observation(observation)
                    .await?;
                tracing::debug!(
                    quote = %allocation.quote,
                    outpoint = %output.outpoint,
                    confirmed = confirmation.is_some(),
                    observation_epoch,
                    "Federation payment observer submitted deposit observation"
                );
                submitted = submitted.saturating_add(1);
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
                state
                    .submit_federation_chain_observation(FederationChainObservation::Reorg(
                        FederationDepositReorgObservation {
                            outpoint: utxo.outpoint,
                            disconnected_block_hash: block_hash,
                            disconnected_block_height: block_height,
                            observation_epoch: utxo.observation_epoch,
                            observer,
                            source: source.clone(),
                        },
                    ))
                    .await?;
                tracing::debug!(
                    quote = %utxo.allocation.quote,
                    outpoint = %utxo.outpoint,
                    "Federation payment observer submitted deposit reorg observation"
                );
                submitted = submitted.saturating_add(1);
            }
        }
        Ok(submitted)
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
        let classified = self
            .classify_and_filter_active_quotes(state, quotes, trigger, false)
            .await?;
        let mut direct = Vec::with_capacity(classified.len());
        let mut background = Vec::new();
        for item in classified {
            if item.lane == FederationPaymentScheduleLane::Direct {
                direct.push(item.quote);
            } else {
                background.push(item);
            }
        }

        let submitted = Box::pin(self.drive_active_direct_quotes(state, direct, trigger)).await?;
        // A dispatched quote has only started an attempt; its task reports
        // its own progress. Counting it here made every dispatch look like a
        // submitted observation.
        self.dispatch_classified_quotes(state, background, trigger, queued_at)
            .await?;
        Ok(submitted)
    }

    /// Filter out inactive quotes (removing their schedules) and assign each
    /// active quote its bounded execution lane in one DB read per quote.
    async fn classify_and_filter_active_quotes(
        &self,
        state: &FederationNode,
        quotes: Vec<FederationPaymentQuote>,
        trigger: FederationPaymentObservationTrigger,
        priority_hint: bool,
    ) -> Result<Vec<ClassifiedPaymentQuote>, cdk::Error> {
        let db = state.mint.localstore();
        #[cfg(feature = "federation-bark")]
        let has_bark = state.federation.bark_runtime().is_some();
        #[cfg(not(feature = "federation-bark"))]
        let has_bark = false;

        let mut active = Vec::with_capacity(quotes.len());
        for quote in quotes {
            let active_is_bark = match quote.kind {
                FederationPaymentQuoteKind::Mint => match db.get_mint_quote(&quote.id).await? {
                    Some(mint_quote)
                        if !matches!(
                            mint_quote.state(),
                            MintQuoteState::Paid | MintQuoteState::Issued
                        ) =>
                    {
                        Some(has_bark && mint_quote.payment_method.is_bolt11())
                    }
                    _ => None,
                },
                FederationPaymentQuoteKind::Melt => match db.get_melt_quote(&quote.id).await? {
                    Some(melt_quote) if melt_quote.state == MeltQuoteState::Pending => {
                        Some(has_bark && melt_quote.payment_method.is_bolt11())
                    }
                    _ => None,
                },
            };
            let Some(uses_bark) = active_is_bark else {
                self.schedule.remove(&db, &quote).await?;
                continue;
            };
            let lane = if !uses_bark {
                FederationPaymentScheduleLane::Direct
            } else {
                self.bark_lane_for_quote(&db, &quote, trigger, priority_hint)
                    .await?
            };
            active.push(ClassifiedPaymentQuote { quote, lane });
        }
        Ok(active)
    }

    async fn bark_lane_for_quote(
        &self,
        db: &cdk_common::database::DynMintDatabase,
        quote: &FederationPaymentQuote,
        trigger: FederationPaymentObservationTrigger,
        priority_hint: bool,
    ) -> Result<FederationPaymentScheduleLane, cdk::Error> {
        match quote.kind {
            FederationPaymentQuoteKind::Melt => Ok(FederationPaymentScheduleLane::OutgoingMelt),
            FederationPaymentQuoteKind::Mint => {
                if priority_hint
                    || trigger == FederationPaymentObservationTrigger::BackendNotified
                    || self.schedule.has_payment_activity(db, quote).await?
                {
                    Ok(FederationPaymentScheduleLane::IncomingClaim)
                } else {
                    Ok(FederationPaymentScheduleLane::IncomingWait)
                }
            }
        }
    }

    /// Hand each due quote to its own background task in its lane.
    ///
    /// The attempt lease is claimed here, before the task is spawned, so the
    /// quote stops being due at once: a scheduled round that runs again before
    /// the task starts neither dispatches it a second time nor finds it due.
    /// Saturation in one lane skips only further quotes in that lane so unpaid
    /// incoming waits never block ready claims, outgoing melts, or direct probes.
    async fn dispatch_classified_quotes(
        &self,
        state: &FederationNode,
        quotes: Vec<ClassifiedPaymentQuote>,
        trigger: FederationPaymentObservationTrigger,
        queued_at: Instant,
    ) -> Result<usize, cdk::Error> {
        let Some(federation_config) = state.mint.federation_config() else {
            return Ok(0);
        };
        if quotes.is_empty() {
            return Ok(0);
        }
        let member_id = federation_config.member_id();
        let db = state.mint.localstore();
        let mut dispatched = 0_usize;
        let mut saturated_lanes = [false; 4];
        let event_driven = matches!(
            trigger,
            FederationPaymentObservationTrigger::BackendNotified
                | FederationPaymentObservationTrigger::OperationAccepted(_)
        );

        for ClassifiedPaymentQuote { quote, lane } in quotes {
            if saturated_lanes[lane.index()] {
                if event_driven {
                    self.schedule
                        .mark_due_now(&db, &quote, lane, trigger, current_unix_time_ms())
                        .await?;
                } else {
                    self.schedule.reconcile_lane(&db, &quote, lane).await?;
                }
                continue;
            }
            let (task_permit, attempt) = match self
                .claim_quote_task_in_lane(&db, &quote, trigger, lane, current_unix_time_ms())
                .await?
            {
                FederationPaymentQuoteTaskClaim::Claimed {
                    task_permit,
                    attempt,
                } => (task_permit, attempt),
                FederationPaymentQuoteTaskClaim::NotDue => continue,
                FederationPaymentQuoteTaskClaim::CapacityReached => {
                    saturated_lanes[lane.index()] = true;
                    if event_driven {
                        self.schedule
                            .mark_due_now(&db, &quote, lane, trigger, current_unix_time_ms())
                            .await?;
                    } else {
                        self.schedule.reconcile_lane(&db, &quote, lane).await?;
                    }
                    tracing::debug!(
                        quote = %quote.id,
                        quote_kind = quote.kind.as_str(),
                        "Federation payment quote task lane capacity reached; durable schedule will retry"
                    );
                    continue;
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
            let backend = match lane {
                FederationPaymentScheduleLane::Direct => {
                    super::latency::FederationLatencyBackend::Cashu
                }
                FederationPaymentScheduleLane::IncomingWait
                | FederationPaymentScheduleLane::IncomingClaim
                | FederationPaymentScheduleLane::OutgoingMelt => {
                    super::latency::FederationLatencyBackend::Bark
                }
            };
            let tasks = state.federation.tasks.clone();
            tasks.spawn("payment quote", false, async move {
                let _task_permit = task_permit;
                super::latency::record_duration(
                    super::latency::FederationLatencyCorrelation::Quote,
                    &correlation_id,
                    backend,
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

    /// Reserve a quote task permit in `lane` and the quote's attempt lease together.
    ///
    /// The task permit is taken first so a quote is never leased without a
    /// task to drive it; a quote that is not due releases the permit again.
    async fn claim_quote_task_in_lane(
        &self,
        db: &cdk_common::database::DynMintDatabase,
        quote: &FederationPaymentQuote,
        trigger: FederationPaymentObservationTrigger,
        lane: FederationPaymentScheduleLane,
        now_ms: u64,
    ) -> Result<FederationPaymentQuoteTaskClaim, cdk::Error> {
        let Ok(task_permit) = self.task_pool.semaphore(lane).clone().try_acquire_owned() else {
            return Ok(FederationPaymentQuoteTaskClaim::CapacityReached);
        };
        match self
            .schedule
            .claim_in_lane(db, quote, trigger, lane, now_ms)
            .await?
        {
            Some(attempt) => Ok(FederationPaymentQuoteTaskClaim::Claimed {
                task_permit,
                attempt,
            }),
            None => Ok(FederationPaymentQuoteTaskClaim::NotDue),
        }
    }

    async fn drive_active_direct_quotes(
        &self,
        state: &FederationNode,
        active_quotes: Vec<FederationPaymentQuote>,
        trigger: FederationPaymentObservationTrigger,
    ) -> Result<usize, cdk::Error> {
        let Some(federation_config) = state.mint.federation_config() else {
            return Ok(0);
        };
        if active_quotes.is_empty() {
            return Ok(0);
        }
        let member_id = federation_config.member_id();
        let db = state.mint.localstore();
        let mut schedule_changes = self.schedule.subscribe_changes();
        let candidates = self
            .schedule
            .candidates(&db, &active_quotes, trigger, current_unix_time_ms())
            .await?;
        let direct_semaphore = self
            .task_pool
            .semaphore(FederationPaymentScheduleLane::Direct)
            .clone();
        let db_ref = &db;
        let submitted = run_bounded_direct_tasks(
            candidates,
            direct_semaphore,
            FEDERATION_PAYMENT_CONCURRENT_DIRECT_QUOTE_TASKS,
            |quote| async move {
                let Some(attempt) = self
                    .schedule
                    .claim_in_lane(
                        db_ref,
                        &quote,
                        trigger,
                        FederationPaymentScheduleLane::Direct,
                        current_unix_time_ms(),
                    )
                    .await?
                else {
                    return Ok(0_usize);
                };
                Box::pin(self.drive_claimed_quote(state, member_id, &quote, attempt, trigger)).await
            },
        )
        .await?;
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

    async fn resolve_payment_event_quote(
        state: &FederationNode,
        event: Event,
    ) -> Result<Option<FederationPaymentQuote>, cdk::Error> {
        match event {
            Event::PaymentReceived(payment) => Ok(state
                .mint
                .localstore()
                .get_mint_quote_by_request_lookup_id(&payment.payment_identifier)
                .await?
                .map(|quote| FederationPaymentQuote::mint(quote.id))),
            Event::PaymentSuccessful { quote_id, .. } | Event::PaymentFailed { quote_id, .. } => {
                Ok(Some(FederationPaymentQuote::melt(quote_id)))
            }
        }
    }

    pub async fn observe_event(
        &self,
        state: &FederationNode,
        event: Event,
    ) -> Result<usize, cdk::Error> {
        state
            .ensure_federation_ready_for_authoritative_read()
            .await?;
        let Some(quote) = Self::resolve_payment_event_quote(state, event).await? else {
            return Ok(0);
        };
        Box::pin(self.drive_or_dispatch_quotes(
            state,
            vec![quote],
            FederationPaymentObservationTrigger::BackendNotified,
            Instant::now(),
        ))
        .await
    }

    /// Dispatch a burst of backend payment events asynchronously across their
    /// bounded lanes without head-of-line blocking the observer worker loop.
    async fn dispatch_payment_events(
        &self,
        state: &FederationNode,
        events: Vec<Event>,
    ) -> Result<usize, cdk::Error> {
        state
            .ensure_federation_ready_for_authoritative_read()
            .await?;
        let mut quotes = Vec::with_capacity(events.len());
        let mut seen = HashSet::with_capacity(events.len());
        for event in events {
            if let Some(quote) = Self::resolve_payment_event_quote(state, event).await? {
                if seen.insert(quote.clone()) {
                    quotes.push(quote);
                }
            }
        }
        if quotes.is_empty() {
            return Ok(0);
        }
        let classified = self
            .classify_and_filter_active_quotes(
                state,
                quotes,
                FederationPaymentObservationTrigger::BackendNotified,
                true,
            )
            .await?;
        self.dispatch_classified_quotes(
            state,
            classified,
            FederationPaymentObservationTrigger::BackendNotified,
            Instant::now(),
        )
        .await
    }

    async fn dispatch_accepted_wake(
        &self,
        state: &FederationNode,
        wake: FederationPaymentWake,
        operation_index: u64,
        queued_at: Instant,
    ) -> Result<usize, cdk::Error> {
        let trigger = FederationPaymentObservationTrigger::OperationAccepted(operation_index);
        let classified = self
            .classify_and_filter_active_quotes(state, vec![wake.quote], trigger, wake.priority)
            .await?;
        self.dispatch_classified_quotes(state, classified, trigger, queued_at)
            .await
    }

    /// Fast-path scheduled wakeup driven directly from the in-memory priority
    /// queue for lanes that currently have available task permits.
    async fn scheduled_due_quotes(&self, state: &FederationNode) -> Result<usize, cdk::Error> {
        if state.mint.federation_config().is_none() {
            return Ok(0);
        }
        state
            .ensure_federation_ready_for_authoritative_read()
            .await?;

        let available_lanes = self.task_pool.available_lanes();
        if available_lanes.is_empty() {
            return Ok(0);
        }
        let db = state.mint.localstore();
        let due = self
            .schedule
            .due_quotes_for_lanes(
                &db,
                &available_lanes,
                current_unix_time_ms(),
                FEDERATION_PAYMENT_DUE_BATCH_LIMIT,
            )
            .await?;
        if due.is_empty() {
            // Legacy KV rows without stored quote metadata still surface their
            // deadlines in the heap; fall back to one bounded DB scan to
            // discover and backfill them.
            return self.scheduled_quotes(state).await;
        }
        let classified = self
            .classify_and_filter_active_quotes(
                state,
                due,
                FederationPaymentObservationTrigger::Scheduled,
                false,
            )
            .await?;
        self.dispatch_classified_quotes(
            state,
            classified,
            FederationPaymentObservationTrigger::Scheduled,
            Instant::now(),
        )
        .await
    }

    async fn scheduled_quotes(&self, state: &FederationNode) -> Result<usize, cdk::Error> {
        if state.mint.federation_config().is_none() {
            return Ok(0);
        }
        state
            .ensure_federation_ready_for_authoritative_read()
            .await?;

        let db = state.mint.localstore();
        let available_lanes = self.task_pool.available_lanes();
        let due_from_queue = self
            .schedule
            .due_quotes_for_lanes(
                &db,
                &available_lanes,
                current_unix_time_ms(),
                FEDERATION_PAYMENT_DUE_BATCH_LIMIT,
            )
            .await?;
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

        let mut batch = Vec::with_capacity(due_from_queue.len() + mint_ids.len() + melt_ids.len());
        let mut seen = HashSet::with_capacity(batch.capacity());
        for quote in due_from_queue {
            if seen.insert(quote.clone()) {
                batch.push(quote);
            }
        }
        let scan_len = mint_ids.len().max(melt_ids.len());
        for index in 0..scan_len {
            if let Some(id) = melt_ids.get(index) {
                let quote = FederationPaymentQuote::melt(id.clone());
                if seen.insert(quote.clone()) {
                    batch.push(quote);
                }
            }
            if let Some(id) = mint_ids.get(index) {
                let quote = FederationPaymentQuote::mint(id.clone());
                if seen.insert(quote.clone()) {
                    batch.push(quote);
                }
            }
        }

        let submitted = Box::pin(self.drive_or_dispatch_quotes(
            state,
            batch,
            FederationPaymentObservationTrigger::Scheduled,
            Instant::now(),
        ))
        .await?;

        let next_mint_cursor = match mint_reaches_end {
            true => None,
            false => mint_ids.last().cloned(),
        };
        let next_melt_cursor = match melt_reaches_end {
            true => None,
            false => melt_ids.last().cloned(),
        };
        if mint_ids.is_empty() && mint_cursor.is_some() || !mint_ids.is_empty() {
            self.schedule
                .set_scan_cursor(&db, FederationPaymentQuoteKind::Mint, next_mint_cursor)
                .await?;
        }
        if melt_ids.is_empty() && melt_cursor.is_some() || !melt_ids.is_empty() {
            self.schedule
                .set_scan_cursor(&db, FederationPaymentQuoteKind::Melt, next_melt_cursor)
                .await?;
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
        for wake in payment_wakes_for_entry(entry) {
            let _ = self
                .accepted_operation_wakes
                .send((wake, entry.index, Instant::now()));
        }
    }

    #[cfg(test)]
    async fn next_schedule_delay(
        &self,
        db: &cdk_common::database::DynMintDatabase,
    ) -> Result<std::time::Duration, cdk::Error> {
        self.next_schedule_delay_for_lanes(db, &FederationPaymentScheduleLane::ALL)
            .await
    }

    async fn next_schedule_delay_for_lanes(
        &self,
        db: &cdk_common::database::DynMintDatabase,
        lanes: &[FederationPaymentScheduleLane],
    ) -> Result<std::time::Duration, cdk::Error> {
        let Some(deadline_ms) = self
            .schedule
            .next_attempt_at_ms_for_lanes(db, lanes)
            .await?
        else {
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
            if let Err(error) = self.schedule.hydrate(&state.mint.localstore()).await {
                tracing::warn!(
                    error = %error,
                    "Could not hydrate federation payment schedule at startup; will retry lazily"
                );
            }

            let mut repair_interval = tokio::time::interval(FEDERATION_PAYMENT_REPAIR_INTERVAL);
            repair_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut onchain_interval = tokio::time::interval(FEDERATION_ONCHAIN_OBSERVER_INTERVAL);
            onchain_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut failure_backoff = FederationPaymentWorkerFailureBackoff::default();

            loop {
                if stop.is_cancelled() {
                    break;
                }
                let available_lanes = self.task_pool.available_lanes();
                let saturated_lanes = self.task_pool.saturated_lanes();
                let schedule_delay = match self
                    .next_schedule_delay_for_lanes(&state.mint.localstore(), &available_lanes)
                    .await
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
                // while its task runs. A due quote in a saturated lane stays
                // unleased and due; excluding saturated lanes from the schedule
                // timer prevents spinning on that lane while still dispatching
                // due quotes in any other lane with free capacity.
                let scheduled_round_ready = scheduled_round_can_dispatch(&available_lanes)
                    && schedule_delay < FEDERATION_PAYMENT_IDLE_DEADLINE;
                let has_saturated_lane = !saturated_lanes.is_empty();
                // One panicking round, event, or backend future must not kill
                // the only observation worker; it is logged and the loop
                // continues so remaining quotes keep being repaired.
                let (result, closed) = match std::panic::AssertUnwindSafe(async {
                    tokio::select! {
                        _ = stop.cancelled() => (Ok(0), true),
                        _ = tokio::time::sleep(schedule_delay), if scheduled_round_ready => {
                            (self.scheduled_due_quotes(&state).await, false)
                        }
                        _ = self.task_pool.wait_for_any_lane(&saturated_lanes), if has_saturated_lane => {
                            (Ok(0), false)
                        }
                        _ = repair_interval.tick() => (self.run_round(&state).await, false),
                        _ = onchain_interval.tick() => {
                            (self.observe_onchain_wallets(&state).await, false)
                        }
                        changed = schedule_changes.changed() => {
                            (changed.map(|_| 0).map_err(|_| cdk::Error::Internal), false)
                        }
                        wake = accepted_operation_wakes.recv() => match wake {
                            Ok((first_wake, first_index, first_queued_at)) => {
                                let mut wakes = Vec::with_capacity(1);
                                wakes.push((first_wake, first_index, first_queued_at));
                                let mut lagged = false;
                                while wakes.len() < FEDERATION_PAYMENT_WAKE_DRAIN_LIMIT {
                                    match accepted_operation_wakes.try_recv() {
                                        Ok(item) => wakes.push(item),
                                        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                                            break;
                                        }
                                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(skipped)) => {
                                            tracing::warn!(
                                                skipped,
                                                "Federation payment journal wake lagged; running repair scan"
                                            );
                                            lagged = true;
                                            break;
                                        }
                                        Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                                            break;
                                        }
                                    }
                                }
                                let mut outcome = Ok(0_usize);
                                for (wake, operation_index, queued_at) in wakes {
                                    if let Err(error) = self
                                        .dispatch_accepted_wake(
                                            &state,
                                            wake,
                                            operation_index,
                                            queued_at,
                                        )
                                        .await
                                    {
                                        outcome = Err(error);
                                        break;
                                    }
                                }
                                let outcome = if lagged {
                                    match outcome {
                                        Ok(_) => self.run_round(&state).await,
                                        Err(error) => Err(error),
                                    }
                                } else {
                                    outcome
                                };
                                (outcome, false)
                            }
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
                            Ok(first_event) => {
                                let mut events = Vec::with_capacity(1);
                                events.push(first_event);
                                let mut lagged = false;
                                while events.len() < FEDERATION_PAYMENT_WAKE_DRAIN_LIMIT {
                                    match payment_events.try_recv() {
                                        Ok(event) => events.push(event),
                                        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                                            break;
                                        }
                                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(skipped)) => {
                                            tracing::warn!(
                                                skipped,
                                                "Federation payment observer lagged; running repair scan"
                                            );
                                            lagged = true;
                                            break;
                                        }
                                        Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                                            break;
                                        }
                                    }
                                }
                                let dispatched = self.dispatch_payment_events(&state, events).await;
                                let outcome = if lagged {
                                    match dispatched {
                                        Ok(_) => self.run_round(&state).await,
                                        Err(error) => Err(error),
                                    }
                                } else {
                                    dispatched
                                };
                                (outcome, false)
                            }
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

/// Drive `items` concurrently up to `max_in_flight` while holding permits from
/// `semaphore`. Already-started futures are always drained before returning an
/// error so no claimed quote lease is ever dropped mid-flight.
async fn run_bounded_direct_tasks<T, F, Fut>(
    items: Vec<T>,
    semaphore: std::sync::Arc<tokio::sync::Semaphore>,
    max_in_flight: usize,
    mut work: F,
) -> Result<usize, cdk::Error>
where
    F: FnMut(T) -> Fut,
    Fut: std::future::Future<Output = Result<usize, cdk::Error>>,
{
    if items.is_empty() {
        return Ok(0);
    }
    let concurrency_limit = max_in_flight.max(1);
    let mut in_flight = FuturesUnordered::new();
    let mut pending = items.into_iter();
    let mut submitted = 0_usize;
    let mut first_error = None::<cdk::Error>;

    loop {
        while first_error.is_none() && in_flight.len() < concurrency_limit {
            let Some(item) = pending.next() else {
                break;
            };
            let semaphore = semaphore.clone();
            let future = work(item);
            in_flight.push(async move {
                let _permit = semaphore
                    .acquire_owned()
                    .await
                    .map_err(|_| cdk::Error::Internal)?;
                future.await
            });
        }

        let Some(outcome) = in_flight.next().await else {
            break;
        };
        match outcome {
            Ok(advanced) => submitted = submitted.saturating_add(advanced),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }

    match first_error {
        Some(error) => Err(error),
        None => Ok(submitted),
    }
}

/// Whether the worker may start a scheduled round now.
///
/// With no lane holding a free quote-task permit, every due quote is left due
/// without being driven, so a scheduled round would repeat without delay.
fn scheduled_round_can_dispatch(available_lanes: &[FederationPaymentScheduleLane]) -> bool {
    !available_lanes.is_empty()
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
) -> Vec<FederationPaymentWake> {
    if entry.status() != FederationOperationStatus::Applied {
        return Vec::new();
    }

    let parse = |quote: &str| quote.parse::<QuoteId>().ok();
    let wake =
        |quote: FederationPaymentQuote, priority: bool| FederationPaymentWake { quote, priority };
    match &entry.envelope.operation {
        FederationOperation::AddressAllocation(operation) => parse(&operation.quote)
            .map(|id| wake(FederationPaymentQuote::mint(id), false))
            .into_iter()
            .collect(),
        FederationOperation::ChainObservation(FederationChainObservation::Deposit(observation)) => {
            parse(&observation.allocation.quote)
                .map(|id| wake(FederationPaymentQuote::mint(id), true))
                .into_iter()
                .collect()
        }
        FederationOperation::MintQuote(operation) => parse(&operation.quote)
            .map(|id| wake(FederationPaymentQuote::mint(id), false))
            .into_iter()
            .collect(),
        FederationOperation::MintQuotePayment(observation) => parse(&observation.quote)
            .map(|id| wake(FederationPaymentQuote::mint(id), true))
            .into_iter()
            .collect(),
        FederationOperation::Mint(operation) => operation
            .request
            .quote_ids()
            .into_iter()
            .filter_map(parse)
            .map(|id| wake(FederationPaymentQuote::mint(id), true))
            .collect(),
        FederationOperation::MeltQuote(operation) => parse(&operation.quote)
            .map(|id| wake(FederationPaymentQuote::melt(id), false))
            .into_iter()
            .collect(),
        FederationOperation::MeltQuotePayment(observation) => parse(&observation.quote)
            .map(|id| wake(FederationPaymentQuote::melt(id), true))
            .into_iter()
            .collect(),
        FederationOperation::Melt(operation) => parse(operation.request.quote())
            .map(|id| wake(FederationPaymentQuote::melt(id), true))
            .into_iter()
            .collect(),
        #[cfg(feature = "federation-bark")]
        FederationOperation::Application(_) => {
            let Ok(operation) =
                cdk::federation::validate_federation_bark_operation(&entry.envelope.operation)
            else {
                return Vec::new();
            };
            let Some(quote_id) = operation.quote().and_then(parse) else {
                return Vec::new();
            };
            match operation {
                cdk::federation::FederationBarkOperation::ReceiveIntent(_)
                | cdk::federation::FederationBarkOperation::ReceivePrepared(_) => {
                    vec![wake(FederationPaymentQuote::mint(quote_id), false)]
                }
                cdk::federation::FederationBarkOperation::ActionPlan(plan) => match plan.kind {
                    cdk::federation::FederationBarkActionKind::IncomingClaim => {
                        vec![wake(FederationPaymentQuote::mint(quote_id), true)]
                    }
                    cdk::federation::FederationBarkActionKind::LightningSend
                    | cdk::federation::FederationBarkActionKind::FailedSendRevocation => {
                        vec![wake(FederationPaymentQuote::melt(quote_id), true)]
                    }
                    _ => vec![
                        wake(FederationPaymentQuote::mint(quote_id.clone()), true),
                        wake(FederationPaymentQuote::melt(quote_id), true),
                    ],
                },
                cdk::federation::FederationBarkOperation::ActionTransition(_)
                | cdk::federation::FederationBarkOperation::MemberObservation(_)
                | cdk::federation::FederationBarkOperation::EvidenceCertificate(_) => vec![
                    wake(FederationPaymentQuote::mint(quote_id.clone()), true),
                    wake(FederationPaymentQuote::melt(quote_id), true),
                ],
            }
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

async fn submit_observation(
    state: &FederationNode,
    candidate: FederationPaymentObservation,
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
        assert!(!scheduled_round_can_dispatch(&[]));
        assert!(scheduled_round_can_dispatch(&[
            FederationPaymentScheduleLane::OutgoingMelt
        ]));
        assert!(scheduled_round_can_dispatch(
            &FederationPaymentScheduleLane::ALL
        ));
    }

    #[tokio::test]
    async fn worker_wakes_when_a_quote_task_permit_is_released() {
        let observer = FederationPaymentObserver::default();
        let mut held = Vec::new();
        for (lane, count) in [
            (
                FederationPaymentScheduleLane::IncomingWait,
                FEDERATION_PAYMENT_CONCURRENT_INCOMING_WAIT_TASKS,
            ),
            (
                FederationPaymentScheduleLane::IncomingClaim,
                FEDERATION_PAYMENT_CONCURRENT_CLAIM_TASKS,
            ),
            (
                FederationPaymentScheduleLane::OutgoingMelt,
                FEDERATION_PAYMENT_CONCURRENT_MELT_TASKS,
            ),
            (
                FederationPaymentScheduleLane::Direct,
                FEDERATION_PAYMENT_CONCURRENT_DIRECT_QUOTE_TASKS,
            ),
        ] {
            held.push(
                observer
                    .task_pool
                    .semaphore(lane)
                    .clone()
                    .acquire_many_owned(u32::try_from(count).expect("permit count fits in u32"))
                    .await
                    .expect("semaphore is open"),
            );
        }
        assert!(!scheduled_round_can_dispatch(
            &observer.task_pool.available_lanes()
        ));
        let saturated = observer.task_pool.saturated_lanes();
        assert_eq!(saturated.len(), FederationPaymentScheduleLane::ALL.len());

        let waiter = tokio::spawn({
            let task_pool = observer.task_pool.clone();
            async move { task_pool.wait_for_any_lane(&saturated).await }
        });
        tokio::task::yield_now().await;
        assert!(
            !waiter.is_finished(),
            "the worker must block, not spin, while every permit is held"
        );

        held.pop();
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("released permit wakes the worker")
            .expect("waiter task completes");
        assert!(scheduled_round_can_dispatch(
            &observer.task_pool.available_lanes()
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
        lane: FederationPaymentScheduleLane,
        now_ms: u64,
    ) {
        let attempt = observer
            .schedule
            .claim_in_lane(
                db,
                quote,
                FederationPaymentObservationTrigger::Scheduled,
                lane,
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
        let lane = FederationPaymentScheduleLane::IncomingWait;
        let now_ms = current_unix_time_ms();
        schedule_due_quote(&observer, &db, &quote, lane, now_ms).await;
        assert_eq!(
            observer.next_schedule_delay(&db).await.expect("delay"),
            std::time::Duration::ZERO,
            "the quote is due before it is dispatched"
        );

        let FederationPaymentQuoteTaskClaim::Claimed {
            task_permit,
            attempt,
        } = observer
            .claim_quote_task_in_lane(
                &db,
                &quote,
                FederationPaymentObservationTrigger::Scheduled,
                lane,
                now_ms,
            )
            .await
            .expect("first dispatch")
        else {
            panic!("a due quote is dispatched");
        };

        // Scheduled rounds that run again before the task starts driving the
        // quote must neither dispatch it again nor take more task permits.
        for _ in 0..FEDERATION_PAYMENT_CONCURRENT_INCOMING_WAIT_TASKS * 2 {
            let repeated = observer
                .claim_quote_task_in_lane(
                    &db,
                    &quote,
                    FederationPaymentObservationTrigger::Scheduled,
                    lane,
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
            observer.task_pool.available_permits(lane),
            FEDERATION_PAYMENT_CONCURRENT_INCOMING_WAIT_TASKS - 1
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
                .claim_quote_task_in_lane(
                    &db,
                    &quote,
                    FederationPaymentObservationTrigger::Scheduled,
                    lane,
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
            (
                FederationPaymentQuote::mint(QuoteId::new()),
                FederationPaymentScheduleLane::IncomingWait,
            ),
            (
                FederationPaymentQuote::melt(QuoteId::new()),
                FederationPaymentScheduleLane::OutgoingMelt,
            ),
        ];
        let now_ms = current_unix_time_ms();
        let mut attempts = Vec::new();
        for (quote, lane) in &quotes {
            schedule_due_quote(&observer, &db, quote, *lane, now_ms).await;
            attempts.push(
                observer
                    .claim_quote_task_in_lane(
                        &db,
                        quote,
                        FederationPaymentObservationTrigger::Scheduled,
                        *lane,
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
        let lane = FederationPaymentScheduleLane::OutgoingMelt;
        let now_ms = current_unix_time_ms();
        schedule_due_quote(&observer, &db, &quote, lane, now_ms).await;
        let held = observer
            .task_pool
            .semaphore(lane)
            .clone()
            .acquire_many_owned(
                u32::try_from(FEDERATION_PAYMENT_CONCURRENT_MELT_TASKS)
                    .expect("permit count fits in u32"),
            )
            .await
            .expect("semaphore is open");

        assert!(matches!(
            observer
                .claim_quote_task_in_lane(
                    &db,
                    &quote,
                    FederationPaymentObservationTrigger::Scheduled,
                    lane,
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
                .claim_quote_task_in_lane(
                    &db,
                    &quote,
                    FederationPaymentObservationTrigger::Scheduled,
                    lane,
                    now_ms,
                )
                .await
                .expect("dispatch after release"),
            FederationPaymentQuoteTaskClaim::Claimed { .. }
        ));
    }

    #[tokio::test]
    async fn unpaid_incoming_quotes_do_not_starve_outgoing_melts_or_ready_claims() {
        let db = memory_db().await;
        let observer = FederationPaymentObserver::default();
        let now_ms = current_unix_time_ms();

        // Occupy every single permit in IncomingWait (far more than the old 16-permit cap).
        let mut held_unpaid = Vec::new();
        for _ in 0..FEDERATION_PAYMENT_CONCURRENT_INCOMING_WAIT_TASKS {
            let unpaid = FederationPaymentQuote::mint(QuoteId::new());
            schedule_due_quote(
                &observer,
                &db,
                &unpaid,
                FederationPaymentScheduleLane::IncomingWait,
                now_ms,
            )
            .await;
            let claim = observer
                .claim_quote_task_in_lane(
                    &db,
                    &unpaid,
                    FederationPaymentObservationTrigger::Scheduled,
                    FederationPaymentScheduleLane::IncomingWait,
                    now_ms,
                )
                .await
                .expect("claim unpaid");
            assert!(matches!(
                claim,
                FederationPaymentQuoteTaskClaim::Claimed { .. }
            ));
            held_unpaid.push(claim);
        }

        // One more unpaid quote hits CapacityReached and stays unleased.
        let overflow_unpaid = FederationPaymentQuote::mint(QuoteId::new());
        schedule_due_quote(
            &observer,
            &db,
            &overflow_unpaid,
            FederationPaymentScheduleLane::IncomingWait,
            now_ms,
        )
        .await;
        assert!(matches!(
            observer
                .claim_quote_task_in_lane(
                    &db,
                    &overflow_unpaid,
                    FederationPaymentObservationTrigger::Scheduled,
                    FederationPaymentScheduleLane::IncomingWait,
                    now_ms,
                )
                .await
                .expect("overflow unpaid"),
            FederationPaymentQuoteTaskClaim::CapacityReached
        ));

        // With IncomingWait saturated and no due work in other lanes, the
        // worker ignores the saturated lane so it does NOT busy-spin.
        let available_lanes = observer.task_pool.available_lanes();
        assert!(!available_lanes.contains(&FederationPaymentScheduleLane::IncomingWait));
        assert_eq!(
            observer
                .next_schedule_delay_for_lanes(&db, &available_lanes)
                .await
                .expect("idle delay"),
            FEDERATION_PAYMENT_IDLE_DEADLINE
        );

        // Now an outgoing melt and a ready incoming claim become due:
        // both must wake immediately and claim permits in their own lanes
        // while all 64 IncomingWait permits are still held.
        let ready_claim = FederationPaymentQuote::mint(QuoteId::new());
        let outgoing_melt = FederationPaymentQuote::melt(QuoteId::new());
        schedule_due_quote(
            &observer,
            &db,
            &ready_claim,
            FederationPaymentScheduleLane::IncomingClaim,
            now_ms,
        )
        .await;
        schedule_due_quote(
            &observer,
            &db,
            &outgoing_melt,
            FederationPaymentScheduleLane::OutgoingMelt,
            now_ms,
        )
        .await;

        assert_eq!(
            observer
                .next_schedule_delay_for_lanes(&db, &available_lanes)
                .await
                .expect("ready delay"),
            std::time::Duration::ZERO
        );
        assert!(matches!(
            observer
                .claim_quote_task_in_lane(
                    &db,
                    &ready_claim,
                    FederationPaymentObservationTrigger::Scheduled,
                    FederationPaymentScheduleLane::IncomingClaim,
                    now_ms,
                )
                .await
                .expect("claim ready incoming"),
            FederationPaymentQuoteTaskClaim::Claimed { .. }
        ));
        assert!(matches!(
            observer
                .claim_quote_task_in_lane(
                    &db,
                    &outgoing_melt,
                    FederationPaymentObservationTrigger::Scheduled,
                    FederationPaymentScheduleLane::OutgoingMelt,
                    now_ms,
                )
                .await
                .expect("claim outgoing melt"),
            FederationPaymentQuoteTaskClaim::Claimed { .. }
        ));

        drop(held_unpaid);
    }

    #[tokio::test]
    async fn direct_quote_probes_run_concurrently_within_bound_and_drain_on_error() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let bound = 4_usize;
        let semaphore = Arc::new(tokio::sync::Semaphore::new(bound));
        let barrier = Arc::new(tokio::sync::Barrier::new(bound));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak_in_flight = Arc::new(AtomicUsize::new(0));

        // All 4 tasks in the first wave must enter concurrently to clear the
        // barrier; a sequential loop would time out and fail the test.
        let total = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            run_bounded_direct_tasks((0..12_usize).collect(), semaphore.clone(), bound, |index| {
                let barrier = Arc::clone(&barrier);
                let in_flight = Arc::clone(&in_flight);
                let peak_in_flight = Arc::clone(&peak_in_flight);
                async move {
                    let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    peak_in_flight.fetch_max(current, Ordering::SeqCst);
                    if index < bound {
                        barrier.wait().await;
                    } else {
                        tokio::task::yield_now().await;
                    }
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    Ok(1)
                }
            }),
        )
        .await
        .expect("concurrent probes must not serialize and deadlock on barrier")
        .expect("all probes succeed");

        assert_eq!(total, 12);
        assert_eq!(peak_in_flight.load(Ordering::SeqCst), bound);
        assert_eq!(semaphore.available_permits(), bound);

        // Verify that an error in one task still drains all already-started
        // sibling tasks cleanly so leases are never dropped mid-flight.
        let completed_siblings = Arc::new(AtomicUsize::new(0));
        let err_barrier = Arc::new(tokio::sync::Barrier::new(bound));
        let result =
            run_bounded_direct_tasks((0..8_usize).collect(), semaphore.clone(), bound, |index| {
                let err_barrier = Arc::clone(&err_barrier);
                let completed_siblings = Arc::clone(&completed_siblings);
                async move {
                    if index < bound {
                        err_barrier.wait().await;
                    }
                    if index == 0 {
                        return Err(cdk::Error::Internal);
                    }
                    tokio::task::yield_now().await;
                    completed_siblings.fetch_add(1, Ordering::SeqCst);
                    Ok(1)
                }
            })
            .await;
        assert!(result.is_err());
        assert_eq!(
            completed_siblings.load(Ordering::SeqCst),
            bound - 1,
            "every already-started sibling must drain before returning error"
        );
        assert_eq!(semaphore.available_permits(), bound);
    }

    #[tokio::test]
    async fn lane_mismatch_on_saturated_target_lane_reconciles_without_spinning() {
        let db = memory_db().await;
        let observer = FederationPaymentObserver::default();
        let quote = FederationPaymentQuote::mint(QuoteId::new());
        let now_ms = current_unix_time_ms();

        // Simulate a pre-upgrade entry indexed in IncomingWait whose classified
        // target lane is Direct.
        schedule_due_quote(
            &observer,
            &db,
            &quote,
            FederationPaymentScheduleLane::IncomingWait,
            now_ms,
        )
        .await;

        // Saturate Direct while IncomingWait still has free permits.
        let held_direct = observer
            .task_pool
            .semaphore(FederationPaymentScheduleLane::Direct)
            .clone()
            .acquire_many_owned(
                u32::try_from(FEDERATION_PAYMENT_CONCURRENT_DIRECT_QUOTE_TASKS)
                    .expect("permit count fits in u32"),
            )
            .await
            .expect("direct semaphore open");

        assert!(matches!(
            observer
                .claim_quote_task_in_lane(
                    &db,
                    &quote,
                    FederationPaymentObservationTrigger::Scheduled,
                    FederationPaymentScheduleLane::Direct,
                    now_ms,
                )
                .await
                .expect("capacity reached on direct"),
            FederationPaymentQuoteTaskClaim::CapacityReached
        ));

        observer
            .schedule
            .reconcile_lane(&db, &quote, FederationPaymentScheduleLane::Direct)
            .await
            .expect("reconcile lane");

        // After reconciliation, the quote sits unleased in Direct, so available
        // lanes (including IncomingWait) no longer wake with Duration::ZERO.
        let available_lanes = observer.task_pool.available_lanes();
        assert!(available_lanes.contains(&FederationPaymentScheduleLane::IncomingWait));
        assert!(!available_lanes.contains(&FederationPaymentScheduleLane::Direct));
        assert_eq!(
            observer
                .next_schedule_delay_for_lanes(&db, &available_lanes)
                .await
                .expect("available lanes idle"),
            FEDERATION_PAYMENT_IDLE_DEADLINE
        );

        // Once a Direct permit is released, the quote is immediately due again.
        drop(held_direct);
        let available_lanes = observer.task_pool.available_lanes();
        assert_eq!(
            observer
                .next_schedule_delay_for_lanes(&db, &available_lanes)
                .await
                .expect("direct ready"),
            std::time::Duration::ZERO
        );
        assert!(matches!(
            observer
                .claim_quote_task_in_lane(
                    &db,
                    &quote,
                    FederationPaymentObservationTrigger::Scheduled,
                    FederationPaymentScheduleLane::Direct,
                    now_ms,
                )
                .await
                .expect("claim direct after release"),
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
            vec![FederationPaymentWake {
                quote: FederationPaymentQuote::mint(quote),
                priority: true,
            }]
        );
        let other_quote = FederationPaymentQuote::mint(other);
        assert!(!payment_wakes_for_entry(&entry)
            .iter()
            .any(|wake| wake.quote == other_quote));
    }
}
