#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use std::collections::BTreeMap;
use std::collections::BTreeSet;
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
use futures::FutureExt as _;

use super::payment_schedule::{
    FederationPaymentAdvancement, FederationPaymentObservationSchedule,
    FederationPaymentObservationTrigger, FederationPaymentQuote, FederationPaymentQuoteKind,
};
use crate::MintState;

const FEDERATION_STALE_PAYMENT_OBSERVATION_SECS: u64 = 15 * 60;
const FEDERATION_ONCHAIN_OBSERVER_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
const FEDERATION_PAYMENT_REPAIR_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
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
    state: &'a MintState,
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
    Box::pin(super::bark_incoming::advance_federated_bark_mint_quote(
        state,
        quote_id,
        observer_fallback_round,
        wait_for_backend_update,
    ))
}

#[cfg(feature = "federation-bark")]
fn advance_bark_melt_quote<'a>(
    state: &'a MintState,
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
    Box::pin(super::bark_outgoing::advance_federated_bark_melt_quote(
        state,
        quote_id,
        observer_fallback_round,
        wait_for_backend_update,
    ))
}

#[derive(Debug, Clone)]
pub(crate) struct FederationPaymentObserver {
    schedule: FederationPaymentObservationSchedule,
    accepted_operation_wakes:
        tokio::sync::broadcast::Sender<(FederationPaymentQuote, u64, Instant)>,
    quote_tasks: std::sync::Arc<tokio::sync::Semaphore>,
}

impl Default for FederationPaymentObserver {
    fn default() -> Self {
        let (accepted_operation_wakes, _) =
            tokio::sync::broadcast::channel(FEDERATION_PAYMENT_WAKE_CAPACITY);
        Self {
            schedule: FederationPaymentObservationSchedule::default(),
            accepted_operation_wakes,
            quote_tasks: std::sync::Arc::new(tokio::sync::Semaphore::new(
                FEDERATION_PAYMENT_CONCURRENT_QUOTE_TASKS,
            )),
        }
    }
}

impl FederationPaymentObserver {
    #[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
    pub(crate) async fn observe_onchain_wallets(
        &self,
        state: &MintState,
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
    async fn observe_onchain_wallets_inner(&self, state: &MintState) -> Result<usize, cdk::Error> {
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
                submitted = submitted.saturating_add(1);
            }
        }
        Ok(submitted)
    }

    #[cfg(not(any(feature = "federation-bdk", feature = "federation-bark")))]
    async fn observe_onchain_wallets(&self, _state: &MintState) -> Result<usize, cdk::Error> {
        Ok(0)
    }

    pub(crate) async fn observe_mint_quotes(
        &self,
        state: &MintState,
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
        state: &MintState,
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
        Ok(submitted.saturating_add(self.dispatch_quotes(state, background, trigger, queued_at)))
    }

    async fn quote_uses_bark(
        &self,
        state: &MintState,
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

    fn dispatch_quotes(
        &self,
        state: &MintState,
        quotes: Vec<FederationPaymentQuote>,
        trigger: FederationPaymentObservationTrigger,
        queued_at: Instant,
    ) -> usize {
        let mut dispatched = 0_usize;
        for quote in quotes {
            let Ok(permit) = self.quote_tasks.clone().try_acquire_owned() else {
                tracing::debug!(
                    "Federation payment quote task capacity reached; durable schedule will retry"
                );
                break;
            };
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
                let _permit = permit;
                super::latency::record_duration(
                    super::latency::FederationLatencyCorrelation::Quote,
                    &correlation_id,
                    super::latency::FederationLatencyBackend::Bark,
                    direction,
                    super::latency::FederationLatencyPhase::PaymentCoordinatorDispatch,
                    super::latency::FederationLatencyOutcome::Success,
                    queued_at.elapsed(),
                );
                let quotes = [quote];
                let work = Box::pin(observer.drive_quotes(&state, &quotes, trigger));
                let result = std::panic::AssertUnwindSafe(work).catch_unwind().await;
                match result {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => tracing::warn!(
                        error = %error,
                        "Federation payment quote task failed; durable schedule will retry"
                    ),
                    Err(_) => tracing::error!(
                        "Federation payment quote task panicked; durable schedule will retry"
                    ),
                }
            });
            dispatched = dispatched.saturating_add(1);
        }
        dispatched
    }

    async fn drive_quotes(
        &self,
        state: &MintState,
        quotes: &[FederationPaymentQuote],
        trigger: FederationPaymentObservationTrigger,
    ) -> Result<usize, cdk::Error> {
        let Some(federation_config) = state.mint.federation_config() else {
            return Ok(0);
        };
        let member_id = federation_config.member_id();
        let db = state.mint.localstore();
        let mut schedule_changes = self.schedule.subscribe_changes();
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
                self.schedule.remove(&db, quote).await?;
            }
        }
        let quotes = self
            .schedule
            .candidates(&db, &active_quotes, trigger, current_unix_time_ms())
            .await?;
        let mut submitted = 0usize;
        for quote in &quotes {
            let Some(permit) = self
                .schedule
                .claim(&db, quote, trigger, current_unix_time_ms())
                .await?
            else {
                continue;
            };

            let mut made_progress = false;
            let mut advancement = FederationPaymentAdvancement::Progressed;
            let mut observer_fallback_round = permit.observer_fallback_round();
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
                    permit,
                    member_id,
                    super::payment_schedule::FederationPaymentObservationProgress {
                        advancement,
                        made_progress,
                        last_processed_operation_index,
                    },
                    current_unix_time_ms(),
                )
                .await?;
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
        }
        if trigger == FederationPaymentObservationTrigger::Requested {
            self.wait_for_active_attempts(&db, &active_quotes, &mut schedule_changes)
                .await?;
        }
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
        state: &'a MintState,
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
        state: &MintState,
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

    pub(crate) async fn observe_melt_quote(
        &self,
        state: &MintState,
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
        state: &MintState,
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
        state: &'a MintState,
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

    pub(crate) async fn observe_event(
        &self,
        state: &MintState,
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

    async fn scheduled_quotes(&self, state: &MintState) -> Result<usize, cdk::Error> {
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

    pub(crate) async fn run_round(&self, state: &MintState) -> Result<usize, cdk::Error> {
        #[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
        let submitted = self.observe_onchain_wallets(state).await?;
        #[cfg(not(any(feature = "federation-bdk", feature = "federation-bark")))]
        let submitted = 0_usize;
        let submitted = submitted.saturating_add(self.scheduled_quotes(state).await?);

        Ok(submitted)
    }

    pub(crate) fn notify_accepted_operation(
        &self,
        entry: &cdk::federation::FederationJournalEntry,
    ) {
        for quote in payment_wakes_for_entry(entry) {
            let _ = self
                .accepted_operation_wakes
                .send((quote, entry.index, Instant::now()));
        }
    }

    async fn next_schedule_delay(
        &self,
        state: &MintState,
    ) -> Result<std::time::Duration, cdk::Error> {
        let Some(deadline_ms) = self
            .schedule
            .next_attempt_at_ms(&state.mint.localstore())
            .await?
        else {
            return Ok(FEDERATION_PAYMENT_IDLE_DEADLINE);
        };
        Ok(std::time::Duration::from_millis(
            deadline_ms.saturating_sub(current_unix_time_ms()),
        ))
    }

    pub(crate) fn spawn(self, state: MintState) {
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
            let mut onchain_interval = tokio::time::interval(FEDERATION_ONCHAIN_OBSERVER_INTERVAL);
            onchain_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut failure_backoff = FederationPaymentWorkerFailureBackoff::default();

            loop {
                if stop.is_cancelled() {
                    break;
                }
                let schedule_delay = match self.next_schedule_delay(&state).await {
                    Ok(delay) => delay,
                    Err(error) => {
                        tracing::warn!(
                            error = %error,
                            "Could not load federation payment schedule; using repair deadline"
                        );
                        FEDERATION_PAYMENT_REPAIR_INTERVAL
                    }
                };
                // One panicking round, event, or backend future must not kill
                // the only observation worker; it is logged and the loop
                // continues so remaining quotes keep being repaired.
                let (result, closed) = match std::panic::AssertUnwindSafe(async {
                    tokio::select! {
                        _ = stop.cancelled() => (Ok(0), true),
                        _ = tokio::time::sleep(schedule_delay) => {
                            (self.scheduled_quotes(&state).await, false)
                        }
                        _ = repair_interval.tick() => (self.run_round(&state).await, false),
                        _ = onchain_interval.tick() => {
                            (self.observe_onchain_wallets(&state).await, false)
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
        FederationOperation::Application(_) | FederationOperation::ExternalApplication(_) => {
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
        FederationOperation::Application(_) | FederationOperation::ExternalApplication(_) => {
            Vec::new()
        }
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
    state: &MintState,
    candidate: FederationPaymentObservation,
) -> Result<bool, cdk::Error> {
    let Some(config) = state.mint.federation_config() else {
        return Ok(false);
    };
    let records = state
        .mint
        .localstore()
        .get_federation_payment_observations(candidate.context(), candidate.quote())
        .await?;
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

pub(super) async fn payment_observation_report_summaries(
    state: &MintState,
) -> Result<Vec<FederationPaymentObservationReportSummary>, cdk::Error> {
    let Some(runtime_config) = state.mint.federation_config() else {
        return Ok(Vec::new());
    };
    let db = state.mint.localstore();
    let mut report_keys = BTreeSet::new();
    for record in db.get_all_federation_payment_observations().await? {
        report_keys.insert((
            record.observation.context(),
            record.observation.quote().to_string(),
        ));
    }

    let mut reports = Vec::with_capacity(report_keys.len());
    let now = current_unix_time();
    for (context, quote) in report_keys {
        let required = runtime_config
            .public_config()
            .payment_verification_policy(context)
            .required_matching_observations();
        let observations = db
            .get_federation_payment_observations(context, &quote)
            .await?;
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
    state: &MintState,
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
    use cdk_common::bitcoin::hashes::Hash;
    use cdk_common::bitcoin::BlockHash;

    use super::*;

    #[test]
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
}
