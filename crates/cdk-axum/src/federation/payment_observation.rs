use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use cdk::cdk_payment::Event;
use cdk::federation::{
    FederationAddressAllocationScope, FederationAddressKeychain, FederationAllocatedAddressRef,
    FederationChainObservation, FederationChainSourceDescriptor, FederationChainSourceScope,
    FederationDepositConfirmation, FederationDepositObservation, FederationDepositReorgObservation,
    FederationPaymentObservation, FederationPaymentObservationRecord,
    FederationPaymentObservationReport, FederationPaymentObservationReportSummary,
    FederationWalletUtxoState,
};
use cdk::mint::QuoteId;
use cdk::nuts::{MeltQuoteState, MintQuoteState};
use futures::FutureExt as _;

use crate::MintState;

const FEDERATION_STALE_PAYMENT_OBSERVATION_SECS: u64 = 15 * 60;
const FEDERATION_PAYMENT_OBSERVER_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
const FEDERATION_PAYMENT_OBSERVER_BATCH_SIZE: usize = 100;
/// Per-quote backend probe deadline.
///
/// One hung or slow backend call must never occupy the sequential observer
/// worker. A deadline expiry leaves the quote pending/unknown and it is
/// polled again next round through its durable idempotency handle; it is
/// never translated into a terminal failure.
const FEDERATION_PAYMENT_OBSERVER_PROBE_DEADLINE: std::time::Duration =
    std::time::Duration::from_secs(10);

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

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FederationPaymentObserver;

impl FederationPaymentObserver {
    pub(crate) async fn observe_onchain_wallets(
        &self,
        state: &MintState,
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

        for wallet in state.federation.onchain_wallets() {
            if wallet.ensure_ready(0).await.is_err() {
                continue;
            }
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
            let application = &wallet_config.cdk_onchain_application;
            if application.application_id != scope.application_id
                || application.application_key_epoch != scope.application_key_epoch
            {
                return Err(cdk::Error::Internal.into());
            }
            let source = FederationChainSourceDescriptor::new(
                match wallet.observation_source().scope {
                    cdk_bdk::FederatedChainSourceScope::Independent => {
                        FederationChainSourceScope::Independent
                    }
                    cdk_bdk::FederatedChainSourceScope::Shared => {
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
                    cdk_bdk::FederatedWalletKeychain::External => {
                        FederationAddressKeychain::External
                    }
                    cdk_bdk::FederatedWalletKeychain::Internal => {
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
                                .min(application.confirmation_policy.minimum_confirmations),
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

    pub(crate) async fn observe_mint_quotes(
        &self,
        state: &MintState,
        quote_ids: &[QuoteId],
    ) -> Result<usize, cdk::Error> {
        let mut submitted = 0usize;
        for quote_id in quote_ids {
            let quote = state
                .mint
                .localstore()
                .get_mint_quote(quote_id)
                .await?
                .ok_or(cdk::Error::UnknownQuote)?;
            if quote.state() == MintQuoteState::Issued {
                continue;
            }
            #[cfg(feature = "federation-bark")]
            if state.federation.bark_runtime().is_some() {
                match super::bark_incoming::advance_federated_bark_mint_quote(state, quote_id).await
                {
                    Ok(advanced) => submitted = submitted.saturating_add(advanced),
                    Err(error) => tracing::debug!(
                        quote = %quote_id,
                        error = %error,
                        "federated Bark receive advancement left quote pending"
                    ),
                }
            }
            let Some(observation) = ({
                let probe = std::panic::AssertUnwindSafe(async {
                    state
                        .mint
                        .probe_federation_mint_quote_payment(quote_id)
                        .await
                })
                .catch_unwind();
                match tokio::time::timeout(FEDERATION_PAYMENT_OBSERVER_PROBE_DEADLINE, probe).await
                {
                    Ok(Ok(Ok(observation))) => observation,
                    Ok(Ok(Err(err))) => {
                        tracing::debug!(
                            quote = %quote_id,
                            error = %err,
                            "mint quote payment probe failed; quote left pending"
                        );
                        continue;
                    }
                    Ok(Err(_panic)) => {
                        tracing::warn!(
                            quote = %quote_id,
                            "mint quote payment probe panicked; quote left pending"
                        );
                        continue;
                    }
                    Err(_) => {
                        tracing::debug!(
                            quote = %quote_id,
                            "mint quote payment probe timed out; quote left pending"
                        );
                        continue;
                    }
                }
            }) else {
                continue;
            };
            if submit_observation(state, FederationPaymentObservation::MintQuote(observation))
                .await?
            {
                submitted = submitted.saturating_add(1);
            }
        }
        Ok(submitted)
    }

    pub(crate) async fn observe_melt_quote(
        &self,
        state: &MintState,
        quote_id: &QuoteId,
    ) -> Result<bool, cdk::Error> {
        #[cfg(feature = "federation-bark")]
        let planned = if state.federation.bark_runtime().is_some() {
            match super::bark_outgoing::advance_federated_bark_melt_quote(state, quote_id).await {
                Ok(advanced) => advanced > 0,
                Err(error) => {
                    tracing::debug!(
                        quote = %quote_id,
                        error = %error,
                        "federated Bark send planning left quote pending"
                    );
                    false
                }
            }
        } else {
            false
        };
        #[cfg(not(feature = "federation-bark"))]
        let planned = false;
        let Some(observation) = ({
            let probe = std::panic::AssertUnwindSafe(async {
                state
                    .mint
                    .probe_federation_melt_quote_payment(quote_id)
                    .await
            })
            .catch_unwind();
            match tokio::time::timeout(FEDERATION_PAYMENT_OBSERVER_PROBE_DEADLINE, probe).await {
                Ok(Ok(Ok(observation))) => observation,
                Ok(Ok(Err(err))) => {
                    tracing::debug!(
                        quote = %quote_id,
                        error = %err,
                        "melt quote payment probe failed; quote left pending"
                    );
                    return Ok(planned);
                }
                Ok(Err(_panic)) => {
                    tracing::warn!(
                        quote = %quote_id,
                        "melt quote payment probe panicked; quote left pending"
                    );
                    return Ok(planned);
                }
                Err(_) => {
                    tracing::debug!(
                        quote = %quote_id,
                        "melt quote payment probe timed out; quote left pending"
                    );
                    return Ok(planned);
                }
            }
        }) else {
            return Ok(planned);
        };
        Ok(
            submit_observation(state, FederationPaymentObservation::MeltQuote(observation)).await?
                || planned,
        )
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
                self.observe_mint_quotes(state, std::slice::from_ref(&quote.id))
                    .await
            }
            Event::PaymentSuccessful { quote_id, .. } | Event::PaymentFailed { quote_id, .. } => {
                Ok(usize::from(
                    self.observe_melt_quote(state, &quote_id).await?,
                ))
            }
        }
    }

    pub(crate) async fn run_round(&self, state: &MintState) -> Result<usize, cdk::Error> {
        if state.mint.federation_config().is_none() {
            return Ok(0);
        }
        state
            .ensure_federation_ready_for_authoritative_read()
            .await?;

        let mut submitted = self.observe_onchain_wallets(state).await?;
        let has_frost_wallet = state
            .mint
            .federation_config()
            .is_some_and(|runtime| runtime.public_config().frost_wallet.is_some());
        let mint_quote_ids = state
            .mint
            .localstore()
            .get_mint_quotes()
            .await?
            .into_iter()
            .filter(|quote| {
                quote.state() != MintQuoteState::Issued
                    || (has_frost_wallet && quote.payment_method.is_onchain())
            })
            .map(|quote| quote.id)
            .collect::<Vec<_>>();
        for quote_ids in mint_quote_ids.chunks(FEDERATION_PAYMENT_OBSERVER_BATCH_SIZE) {
            submitted = submitted.saturating_add(self.observe_mint_quotes(state, quote_ids).await?);
        }

        let melt_quote_ids = state
            .mint
            .localstore()
            .get_melt_quotes()
            .await?
            .into_iter()
            .filter(|quote| quote.state == MeltQuoteState::Pending)
            .map(|quote| quote.id)
            .collect::<Vec<_>>();
        for quote_id in melt_quote_ids {
            if self.observe_melt_quote(state, &quote_id).await? {
                submitted = submitted.saturating_add(1);
            }
        }

        Ok(submitted)
    }

    pub(crate) fn spawn(self, state: MintState) {
        if state.mint.federation_config().is_none() {
            return;
        }

        let mut payment_events = state.mint.subscribe_federation_payment_events();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(FEDERATION_PAYMENT_OBSERVER_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;

            loop {
                // One panicking round, event, or backend future must not kill
                // the only observation worker; it is logged and the loop
                // continues so remaining quotes keep being repaired.
                let (result, closed) = match std::panic::AssertUnwindSafe(async {
                    tokio::select! {
                        _ = interval.tick() => (self.run_round(&state).await, false),
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
                        continue;
                    }
                };
                if closed {
                    break;
                }
                match result {
                    Ok(submitted) if submitted > 0 => {
                        tracing::debug!(
                            submitted,
                            "Federation payment observer submitted observations"
                        );
                    }
                    Ok(_) => {}
                    Err(err) => tracing::warn!("Federation payment observer failed: {}", err),
                }
            }
        });
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

    let envelope = match candidate {
        FederationPaymentObservation::MintQuote(observation) => config
            .mint_quote_payment_observation_with_metadata(
                observation.quote,
                observation.method,
                observation.state,
                observation.amount_paid,
                observation.amount_issued,
                observation.payment_proof,
                observation.payment_id,
            ),
        FederationPaymentObservation::MeltQuote(observation) => config
            .melt_quote_payment_observation_with_metadata(
                observation.quote,
                observation.method,
                observation.state,
                observation.amount,
                observation.fee_reserve,
                observation.payment_proof,
                observation.payment_id,
            ),
    };
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
    let entries = db.get_federation_journal_entries().await?;
    let mut report_keys = BTreeSet::new();

    for entry in &entries {
        if let Some(record) = FederationPaymentObservationRecord::from_journal_entry(entry) {
            report_keys.insert((
                record.observation.context(),
                record.observation.quote().to_string(),
            ));
        }
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

#[cfg(test)]
mod tests {
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
}
