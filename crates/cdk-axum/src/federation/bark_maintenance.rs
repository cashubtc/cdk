//! Bounded consensus-stepped Bark refresh and emergency-exit maintenance.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::Arc;

use cdk::federation::{
    federation_bark_exit_allocation_quote, FederationAddressAllocationScope,
    FederationAddressKeychain, FederationAllocatedAddressRef, FederationBarkActionId,
    FederationBarkActionKind, FederationBarkDigest, FederationBarkEvidencePhase,
    FederationBarkObservationStatus, FederationBarkOperation, FederationBarkProjectionSnapshot,
    FederationBarkSemanticPhase, FederationOperation, MemberId,
};
use cdk::mint::QuoteId;
use cdk::nuts::MeltQuoteState;

use crate::MintState;

const BARK_MAINTENANCE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
const BARK_MAINTENANCE_RETRY_MAX: std::time::Duration = std::time::Duration::from_secs(300);
const BARK_ROUTINE_MAINTENANCE_PAYMENT_QUIET_PERIOD: std::time::Duration =
    std::time::Duration::from_secs(30);
const BARK_RECENT_PAYMENT_JOURNAL_LIMIT: usize = 256;

pub(crate) fn spawn_federated_bark_maintenance(state: MintState) {
    if state.federation.bark_runtime().is_none() {
        return;
    }
    tokio::spawn(async move {
        let mut consecutive_no_progress = 0_u32;
        let mut next_delay = BARK_MAINTENANCE_INTERVAL;
        loop {
            tokio::time::sleep(next_delay).await;
            match advance_federated_bark_maintenance(state.clone()).await {
                Ok(advanced) if advanced > 0 => {
                    consecutive_no_progress = 0;
                    next_delay = BARK_MAINTENANCE_INTERVAL;
                    tracing::debug!(advanced, "advanced federated Bark maintenance");
                }
                Ok(_) => {
                    consecutive_no_progress = consecutive_no_progress.saturating_add(1);
                    next_delay = bark_maintenance_retry_delay(consecutive_no_progress);
                }
                Err(error) => {
                    consecutive_no_progress = consecutive_no_progress.saturating_add(1);
                    next_delay = bark_maintenance_retry_delay(consecutive_no_progress);
                    tracing::warn!(
                        error = %cdk::federation::redact_federation_operator_message(&error.to_string()),
                        consecutive_no_progress,
                        retry_delay_secs = next_delay.as_secs(),
                        "federated Bark maintenance round failed"
                    );
                }
            }
        }
    });
}

fn bark_maintenance_retry_delay(consecutive_failures: u32) -> std::time::Duration {
    let exponent = consecutive_failures.saturating_sub(1).min(6);
    let seconds = BARK_MAINTENANCE_INTERVAL
        .as_secs()
        .saturating_mul(1_u64 << exponent)
        .min(BARK_MAINTENANCE_RETRY_MAX.as_secs());
    std::time::Duration::from_secs(seconds)
}

/// Advance at most one consensus-visible Bark maintenance step.
pub(super) async fn advance_federated_bark_maintenance(
    state: MintState,
) -> Result<usize, cdk::Error> {
    advance_federated_bark_maintenance_inner(state, None).await
}

/// Retry at most one consensus-visible step for one exact accepted action.
pub(super) async fn retry_federated_bark_maintenance_action(
    state: MintState,
    action_id: FederationBarkActionId,
) -> Result<usize, cdk::Error> {
    advance_federated_bark_maintenance_inner(state, Some(action_id)).await
}

async fn advance_federated_bark_maintenance_inner(
    state: MintState,
    target_action_id: Option<FederationBarkActionId>,
) -> Result<usize, cdk::Error> {
    let maintenance_lock = Arc::clone(state.federation.bark_maintenance_lock());
    let _maintenance_guard = maintenance_lock.lock().await;
    let federation = state
        .mint
        .federation_config()
        .cloned()
        .ok_or(cdk::Error::Internal)?;
    let runtime = match state.federation.bark_runtime() {
        Some(runtime) => runtime.clone(),
        None => return Ok(0),
    };
    let controls = runtime.maintenance_controls();
    if target_action_id.is_none()
        && !controls.automatic_refresh
        && !controls.automatic_emergency_exit
    {
        return Ok(0);
    }
    let projection = crate::federation_bark_ready_projection(&state.mint, &runtime)
        .await
        .map_err(|_| cdk::Error::Internal)?;
    let active = projection
        .actions
        .iter()
        .find(|action| {
            target_action_id.is_none_or(|target| action.action_id == target)
                && matches!(
                    action.phase,
                    FederationBarkSemanticPhase::RefreshPlanned
                        | FederationBarkSemanticPhase::ExitPlanned
                        | FederationBarkSemanticPhase::ExitRegistered
                        | FederationBarkSemanticPhase::ExitPublished
                        | FederationBarkSemanticPhase::SigningAttemptAdvanced
                )
        })
        .cloned();

    let Some(action) = active else {
        if target_action_id.is_some() {
            return Ok(0);
        }
        if !controls.automatic_refresh {
            return Ok(0);
        }
        if routine_payment_work_blocks_maintenance(&state, &projection).await? {
            return Ok(0);
        }
        let actor = federation
            .public_config()
            .members
            .keys()
            .next()
            .copied()
            .ok_or(cdk::Error::Internal)?;
        if !should_publish_maintenance_plan(actor, federation.member_id()) {
            return Ok(0);
        }
        let Some(plan) = runtime
            .propose_refresh(&projection)
            .await
            .map_err(|_| cdk::Error::Internal)?
        else {
            return Ok(0);
        };
        if plan.actor != actor {
            return Err(cdk::Error::Internal);
        }
        if routine_payment_work_blocks_maintenance(&state, &projection).await? {
            return Ok(0);
        }
        return submit_bark_operation(
            state.clone(),
            federation.operation_envelope(FederationOperation::Bark(
                FederationBarkOperation::ActionPlan(plan),
            )),
        )
        .await;
    };

    let effective_phase = action.signing_retry_phase.unwrap_or(action.phase);
    match effective_phase {
        FederationBarkSemanticPhase::RefreshPlanned => {
            let plan_entry = maintenance_entry(
                state.clone(),
                projection.clone(),
                action.action_id,
                FederationBarkSemanticPhase::RefreshPlanned,
            )
            .await?;
            let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
                &plan_entry.envelope.operation
            else {
                return Err(cdk::Error::Internal);
            };
            runtime
                .apply_refresh_plan(&projection, plan)
                .await
                .map_err(|_| cdk::Error::Internal)?;

            if action.phase == FederationBarkSemanticPhase::RefreshPlanned
                && controls.automatic_emergency_exit
                && runtime
                    .emergency_exit_due(&projection, &plan_entry)
                    .await
                    .map_err(|_| cdk::Error::Internal)?
            {
                return propose_exit(
                    state.clone(),
                    runtime.clone(),
                    projection.clone(),
                    plan_entry.clone(),
                )
                .await;
            }

            if submit_trigger_observation(
                state.clone(),
                runtime.clone(),
                projection.clone(),
                plan_entry.clone(),
            )
            .await?
            {
                return Ok(1);
            }
            if plan.actor != federation.member_id()
                || maintenance_trigger_observers(&projection, plan).len()
                    < usize::from(
                        federation
                            .public_config()
                            .threshold_params
                            .signature_threshold,
                    )
            {
                return Ok(0);
            }
            let authorization_entry =
                super::bark_custody_attempt::accepted_bark_custody_authorization(
                    &state.mint,
                    &projection,
                    &action,
                    &plan_entry,
                )
                .await?;
            match runtime
                .execute_refresh(&projection, &plan_entry, &authorization_entry)
                .await
            {
                Ok(cdk_bark::FederatedBarkRefreshOutcome::Pending) => Ok(0),
                Ok(cdk_bark::FederatedBarkRefreshOutcome::Completed(transition)) => {
                    submit_bark_operation(
                        state.clone(),
                        federation.operation_envelope(FederationOperation::Bark(
                            FederationBarkOperation::ActionTransition(*transition),
                        )),
                    )
                    .await
                }
                Err(cdk_bark::Error::CustodySignerUnavailable) => {
                    let retry = runtime
                        .advance_signing_attempt(&action, &plan_entry)
                        .map_err(|_| cdk::Error::Internal)?;
                    submit_bark_operation(
                        state.clone(),
                        federation.operation_envelope(FederationOperation::Bark(
                            FederationBarkOperation::ActionTransition(retry),
                        )),
                    )
                    .await
                }
                Err(_) => Err(cdk::Error::Internal),
            }
        }
        FederationBarkSemanticPhase::ExitPlanned => {
            let plan_entry = maintenance_entry(
                state.clone(),
                projection.clone(),
                action.action_id,
                FederationBarkSemanticPhase::ExitPlanned,
            )
            .await?;
            let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
                &plan_entry.envelope.operation
            else {
                return Err(cdk::Error::Internal);
            };
            runtime
                .apply_emergency_exit_plan(&projection, plan)
                .await
                .map_err(|_| cdk::Error::Internal)?;
            if submit_trigger_observation(
                state.clone(),
                runtime.clone(),
                projection.clone(),
                plan_entry.clone(),
            )
            .await?
            {
                return Ok(1);
            }
            if plan.actor != federation.member_id()
                || maintenance_trigger_observers(&projection, plan).len()
                    < usize::from(
                        federation
                            .public_config()
                            .threshold_params
                            .signature_threshold,
                    )
            {
                return Ok(0);
            }
            let transition = runtime
                .register_emergency_exit(&projection, &plan_entry)
                .await
                .map_err(|_| cdk::Error::Internal)?;
            submit_bark_operation(
                state.clone(),
                federation.operation_envelope(FederationOperation::Bark(
                    FederationBarkOperation::ActionTransition(transition),
                )),
            )
            .await
        }
        FederationBarkSemanticPhase::ExitRegistered => {
            let (plan_entry, registered_entry) =
                exit_entries(state.clone(), projection.clone(), action.action_id).await?;
            let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
                &plan_entry.envelope.operation
            else {
                return Err(cdk::Error::Internal);
            };
            runtime
                .apply_emergency_exit_registered(&projection, &plan_entry, &registered_entry)
                .await
                .map_err(|_| cdk::Error::Internal)?;
            if plan.actor != federation.member_id() {
                return Ok(0);
            }
            let authorization_entry =
                super::bark_custody_attempt::accepted_bark_custody_authorization(
                    &state.mint,
                    &projection,
                    &action,
                    &plan_entry,
                )
                .await?;
            match runtime
                .publish_emergency_exit(
                    &projection,
                    &plan_entry,
                    &registered_entry,
                    &authorization_entry,
                )
                .await
            {
                Ok(cdk_bark::FederatedBarkExitOutcome::Pending) => Ok(0),
                Ok(cdk_bark::FederatedBarkExitOutcome::Published(transition)) => {
                    submit_bark_operation(
                        state.clone(),
                        federation.operation_envelope(FederationOperation::Bark(
                            FederationBarkOperation::ActionTransition(*transition),
                        )),
                    )
                    .await
                }
                Err(cdk_bark::Error::CustodySignerUnavailable) => {
                    let retry = runtime
                        .advance_signing_attempt(&action, &plan_entry)
                        .map_err(|_| cdk::Error::Internal)?;
                    submit_bark_operation(
                        state.clone(),
                        federation.operation_envelope(FederationOperation::Bark(
                            FederationBarkOperation::ActionTransition(retry),
                        )),
                    )
                    .await
                }
                Err(_) => Err(cdk::Error::Internal),
            }
        }
        FederationBarkSemanticPhase::ExitPublished => {
            let (plan_entry, registered_entry) =
                exit_entries(state.clone(), projection.clone(), action.action_id).await?;
            let published_entry = maintenance_entry(
                state.clone(),
                projection.clone(),
                action.action_id,
                FederationBarkSemanticPhase::ExitPublished,
            )
            .await?;
            if submit_confirmation_observation(
                state.clone(),
                runtime.clone(),
                projection.clone(),
                plan_entry.clone(),
                registered_entry.clone(),
                published_entry.clone(),
            )
            .await?
            {
                return Ok(1);
            }
            let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
                &plan_entry.envelope.operation
            else {
                return Err(cdk::Error::Internal);
            };
            if plan.actor != federation.member_id()
                || !has_confirmation_quorum(
                    &projection,
                    plan.action_id,
                    federation
                        .public_config()
                        .threshold_params
                        .signature_threshold,
                )
            {
                return Ok(0);
            }
            let Some(transition) = runtime
                .confirm_emergency_exit(
                    &projection,
                    &plan_entry,
                    &registered_entry,
                    &published_entry,
                )
                .await
                .map_err(|_| cdk::Error::Internal)?
            else {
                return Ok(0);
            };
            submit_bark_operation(
                state.clone(),
                federation.operation_envelope(FederationOperation::Bark(
                    FederationBarkOperation::ActionTransition(transition),
                )),
            )
            .await
        }
        _ => Ok(0),
    }
}

async fn routine_payment_work_blocks_maintenance(
    state: &MintState,
    projection: &FederationBarkProjectionSnapshot,
) -> Result<bool, cdk::Error> {
    if projection
        .actions
        .iter()
        .any(|action| payment_action_blocks_routine_maintenance(projection, action))
    {
        return Ok(true);
    }

    let now = current_unix_time_seconds();
    if recent_receive_accounting_blocks_routine_maintenance(state, projection, now).await? {
        return Ok(true);
    }

    recent_bolt11_melt_blocks_routine_maintenance(state, now).await
}

async fn recent_receive_accounting_blocks_routine_maintenance(
    state: &MintState,
    projection: &FederationBarkProjectionSnapshot,
    now: u64,
) -> Result<bool, cdk::Error> {
    let store = state.mint.localstore();
    let Some(tip) = store.get_federation_journal_tip().await? else {
        return Ok(false);
    };
    let journal_floor = recent_payment_journal_floor(tip.index);
    let quote_ids = projection
        .actions
        .iter()
        .filter(|action| {
            action.action_kind == Some(FederationBarkActionKind::IncomingClaim)
                && action.latest_operation_index >= journal_floor
        })
        .filter_map(|action| action.quote.as_deref())
        .map(QuoteId::from_str)
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|_| cdk::Error::Internal)?
        .into_iter()
        .collect::<Vec<_>>();
    if quote_ids.is_empty() {
        return Ok(false);
    }

    let quotes = store.get_mint_quotes_by_ids(&quote_ids).await?;
    Ok(quotes
        .into_iter()
        .flatten()
        .any(|quote| payment_timestamp_is_quiet(quote.updated_at(), now)))
}

async fn recent_bolt11_melt_blocks_routine_maintenance(
    state: &MintState,
    now: u64,
) -> Result<bool, cdk::Error> {
    let store = state.mint.localstore();
    let Some(tip) = store.get_federation_journal_tip().await? else {
        return Ok(false);
    };
    let next_index = recent_payment_journal_floor(tip.index);
    let entries = store
        .get_federation_journal_entries_range(
            next_index,
            tip.index.saturating_add(1),
            BARK_RECENT_PAYMENT_JOURNAL_LIMIT,
        )
        .await?;
    let recent_quotes = entries
        .iter()
        .filter_map(|entry| match &entry.envelope.operation {
            FederationOperation::MeltQuote(operation)
                if matches!(operation.request, cdk::MeltQuoteRequest::Bolt11(_)) =>
            {
                Some((operation.quote.as_str(), operation.metadata.created_time))
            }
            _ => None,
        })
        .map(|(quote, created_time)| {
            QuoteId::from_str(quote)
                .map(|quote| (quote, created_time))
                .map_err(|_| cdk::Error::Internal)
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    if recent_quotes
        .values()
        .any(|created_time| payment_timestamp_is_quiet(*created_time, now))
    {
        return Ok(true);
    }
    if recent_quotes.is_empty() {
        return Ok(false);
    }

    let quote_ids = recent_quotes.keys().cloned().collect::<Vec<_>>();
    let quotes = store.get_federation_melt_quotes(&quote_ids).await?;
    Ok(quotes
        .iter()
        .any(|quote| melt_quote_blocks_routine_maintenance(quote.state, quote.paid_time, now)))
}

fn melt_quote_blocks_routine_maintenance(
    state: MeltQuoteState,
    paid_time: Option<u64>,
    now: u64,
) -> bool {
    state == MeltQuoteState::Pending
        || (state == MeltQuoteState::Paid
            && paid_time.is_some_and(|paid_time| payment_timestamp_is_quiet(paid_time, now)))
}

fn payment_action_blocks_routine_maintenance(
    projection: &FederationBarkProjectionSnapshot,
    action: &cdk::federation::FederationBarkActionRecord,
) -> bool {
    if action.quote.is_none() {
        return false;
    }
    let expires_at = match action.phase {
        FederationBarkSemanticPhase::ReceiveIntent
        | FederationBarkSemanticPhase::ReceivePrepared => match &action.operation {
            FederationBarkOperation::ReceiveIntent(intent) => Some(intent.expires_at),
            FederationBarkOperation::ReceivePrepared(prepared) => Some(prepared.expires_at),
            _ => None,
        },
        _ => None,
    };
    let accepted_evidence = projection.observations.iter().any(|record| {
        matches!(
            &record.operation,
            FederationBarkOperation::MemberObservation(observation)
                if observation.action_id == action.action_id
                    && observation.status == FederationBarkObservationStatus::Accepted
        )
    });
    let claimed_result_certified = projection.observations.iter().any(|record| {
        matches!(
            &record.operation,
            FederationBarkOperation::EvidenceCertificate(certificate)
                if certificate.binding.action_id == action.action_id
                    && certificate.binding.phase == FederationBarkEvidencePhase::ReceiveClaimed
        )
    });
    payment_state_blocks_routine_maintenance(
        action.phase,
        expires_at,
        accepted_evidence,
        claimed_result_certified,
        current_unix_time_seconds(),
    )
}

fn payment_state_blocks_routine_maintenance(
    phase: FederationBarkSemanticPhase,
    expires_at: Option<u64>,
    accepted_evidence_present: bool,
    claimed_result_certified: bool,
    now: u64,
) -> bool {
    if phase == FederationBarkSemanticPhase::ReceiveClaimed {
        return !claimed_result_certified;
    }
    if payment_phase_is_terminal(phase) {
        return false;
    }
    match phase {
        FederationBarkSemanticPhase::ReceiveIntent
        | FederationBarkSemanticPhase::ReceivePrepared => expires_at.is_none_or(|expires_at| {
            !super::bark_incoming::receive_is_automatically_terminal(
                expires_at,
                accepted_evidence_present,
                now,
            )
        }),
        _ => true,
    }
}

fn payment_phase_is_terminal(phase: FederationBarkSemanticPhase) -> bool {
    matches!(
        phase,
        FederationBarkSemanticPhase::PaymentSucceeded
            | FederationBarkSemanticPhase::RevocationCompleted
    )
}

fn payment_timestamp_is_quiet(timestamp: u64, now: u64) -> bool {
    timestamp <= now
        && now.saturating_sub(timestamp) < BARK_ROUTINE_MAINTENANCE_PAYMENT_QUIET_PERIOD.as_secs()
}

fn recent_payment_journal_floor(tip_index: u64) -> u64 {
    let journal_limit = u64::try_from(BARK_RECENT_PAYMENT_JOURNAL_LIMIT).unwrap_or(u64::MAX);
    tip_index.saturating_sub(journal_limit.saturating_sub(1))
}

fn current_unix_time_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn should_publish_maintenance_plan(actor: MemberId, local_member: MemberId) -> bool {
    actor == local_member
}

async fn propose_exit(
    state: MintState,
    runtime: Arc<cdk_bark::FederatedBarkRuntime>,
    projection: FederationBarkProjectionSnapshot,
    refresh_plan_entry: cdk::federation::FederationJournalEntry,
) -> Result<usize, cdk::Error> {
    let federation = state
        .mint
        .federation_config()
        .cloned()
        .ok_or(cdk::Error::Internal)?;
    let FederationOperation::Bark(FederationBarkOperation::ActionPlan(refresh)) =
        &refresh_plan_entry.envelope.operation
    else {
        return Err(cdk::Error::Internal);
    };
    let wallet = federation
        .public_config()
        .frost_wallet
        .as_ref()
        .cloned()
        .ok_or(cdk::Error::Internal)?;
    let onchain = wallet.cdk_onchain_application.clone();
    let quote = federation_bark_exit_allocation_quote(refresh.action_id);
    let scope = FederationAddressAllocationScope {
        application_id: onchain.application_id,
        application_key_epoch: onchain.application_key_epoch,
    };
    let allocation = match state
        .mint
        .federation_address_allocation(scope, &quote)
        .await?
    {
        Some(allocation) => allocation,
        None => {
            let envelope = federation.address_allocation_operation(
                &quote,
                onchain.application_id,
                onchain.application_key_epoch,
                FederationAddressKeychain::Internal,
            )?;
            return submit_bark_operation(state, envelope).await;
        }
    };
    let destination_script = wallet.derive_cdk_onchain_script(
        federation.public_config(),
        onchain.application_id,
        onchain.application_key_epoch,
        FederationAddressKeychain::Internal,
        allocation.derivation_index,
    )?;
    let plan = runtime
        .propose_emergency_exit(
            &projection,
            &refresh_plan_entry,
            FederationAllocatedAddressRef {
                application_id: onchain.application_id,
                application_key_epoch: onchain.application_key_epoch,
                quote,
                keychain: FederationAddressKeychain::Internal,
                derivation_index: allocation.derivation_index,
            },
            destination_script.into_bytes(),
            onchain.confirmation_policy.minimum_confirmations,
        )
        .map_err(|_| cdk::Error::Internal)?;
    submit_bark_operation(
        state,
        federation.operation_envelope(FederationOperation::Bark(
            FederationBarkOperation::ActionPlan(plan),
        )),
    )
    .await
}

async fn submit_trigger_observation(
    state: MintState,
    runtime: Arc<cdk_bark::FederatedBarkRuntime>,
    projection: FederationBarkProjectionSnapshot,
    plan_entry: cdk::federation::FederationJournalEntry,
) -> Result<bool, cdk::Error> {
    let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
        &plan_entry.envelope.operation
    else {
        return Err(cdk::Error::Internal);
    };
    if maintenance_trigger_observers(&projection, plan).contains(&runtime.member_id()) {
        return Ok(false);
    }
    let epoch = next_observation_epoch(&projection, plan.action_id, runtime.member_id())?;
    let Some(observation) = runtime
        .observe_maintenance_trigger(&projection, &plan_entry, epoch)
        .await
        .map_err(|_| cdk::Error::Internal)?
    else {
        return Ok(false);
    };
    let federation = state
        .mint
        .federation_config()
        .cloned()
        .ok_or(cdk::Error::Internal)?;
    submit_bark_operation(
        state,
        federation.operation_envelope(FederationOperation::Bark(
            FederationBarkOperation::MemberObservation(observation),
        )),
    )
    .await?;
    Ok(true)
}

async fn submit_confirmation_observation(
    state: MintState,
    runtime: Arc<cdk_bark::FederatedBarkRuntime>,
    projection: FederationBarkProjectionSnapshot,
    plan_entry: cdk::federation::FederationJournalEntry,
    registered_entry: cdk::federation::FederationJournalEntry,
    published_entry: cdk::federation::FederationJournalEntry,
) -> Result<bool, cdk::Error> {
    let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
        &plan_entry.envelope.operation
    else {
        return Err(cdk::Error::Internal);
    };
    if projection.observations.iter().any(|record| {
        matches!(
            &record.operation,
            FederationBarkOperation::MemberObservation(observation)
                if observation.action_id == plan.action_id
                    && observation.observer == runtime.member_id()
                    && observation.status == FederationBarkObservationStatus::Succeeded
        )
    }) {
        return Ok(false);
    }
    let epoch = next_observation_epoch(&projection, plan.action_id, runtime.member_id())?;
    let Some(observation) = runtime
        .observe_emergency_exit_confirmation(
            &projection,
            &plan_entry,
            &registered_entry,
            &published_entry,
            epoch,
        )
        .await
        .map_err(|_| cdk::Error::Internal)?
    else {
        return Ok(false);
    };
    let federation = state
        .mint
        .federation_config()
        .cloned()
        .ok_or(cdk::Error::Internal)?;
    submit_bark_operation(
        state,
        federation.operation_envelope(FederationOperation::Bark(
            FederationBarkOperation::MemberObservation(observation),
        )),
    )
    .await?;
    Ok(true)
}

fn maintenance_trigger_observers(
    projection: &FederationBarkProjectionSnapshot,
    plan: &cdk::federation::FederationBarkActionPlan,
) -> BTreeSet<MemberId> {
    let commitment = FederationBarkOperation::ActionPlan(plan.clone())
        .commitment()
        .ok();
    projection
        .observations
        .iter()
        .filter_map(|record| match &record.operation {
            FederationBarkOperation::MemberObservation(observation)
                if observation.action_id == plan.action_id
                    && observation.status == FederationBarkObservationStatus::Accepted
                    && Some(observation.result_hash) == commitment =>
            {
                Some(observation.observer)
            }
            _ => None,
        })
        .collect()
}

fn has_confirmation_quorum(
    projection: &FederationBarkProjectionSnapshot,
    action_id: cdk::federation::FederationBarkActionId,
    threshold: u16,
) -> bool {
    let mut by_result = BTreeMap::<FederationBarkDigest, BTreeSet<MemberId>>::new();
    for record in &projection.observations {
        let FederationBarkOperation::MemberObservation(observation) = &record.operation else {
            continue;
        };
        if observation.action_id == action_id
            && observation.status == FederationBarkObservationStatus::Succeeded
        {
            by_result
                .entry(observation.result_hash)
                .or_default()
                .insert(observation.observer);
        }
    }
    by_result
        .values()
        .any(|observers| observers.len() >= usize::from(threshold))
}

fn next_observation_epoch(
    projection: &FederationBarkProjectionSnapshot,
    action_id: cdk::federation::FederationBarkActionId,
    member_id: MemberId,
) -> Result<u64, cdk::Error> {
    projection
        .observations
        .iter()
        .filter_map(|record| match &record.operation {
            FederationBarkOperation::MemberObservation(observation)
                if observation.action_id == action_id && observation.observer == member_id =>
            {
                Some(observation.observation_epoch)
            }
            _ => None,
        })
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(cdk::Error::Internal)
}

async fn exit_entries(
    state: MintState,
    projection: FederationBarkProjectionSnapshot,
    action_id: cdk::federation::FederationBarkActionId,
) -> Result<
    (
        cdk::federation::FederationJournalEntry,
        cdk::federation::FederationJournalEntry,
    ),
    cdk::Error,
> {
    Ok((
        maintenance_entry(
            state.clone(),
            projection.clone(),
            action_id,
            FederationBarkSemanticPhase::ExitPlanned,
        )
        .await?,
        maintenance_entry(
            state,
            projection,
            action_id,
            FederationBarkSemanticPhase::ExitRegistered,
        )
        .await?,
    ))
}

async fn maintenance_entry(
    state: MintState,
    projection: FederationBarkProjectionSnapshot,
    action_id: cdk::federation::FederationBarkActionId,
    phase: FederationBarkSemanticPhase,
) -> Result<cdk::federation::FederationJournalEntry, cdk::Error> {
    let operation_id = projection
        .transitions
        .iter()
        .find(|record| record.action_id == action_id && record.phase == phase)
        .map(|record| record.operation_id)
        .ok_or(cdk::Error::Internal)?;
    state
        .mint
        .localstore()
        .get_federation_journal_entry_by_operation_id(operation_id)
        .await?
        .ok_or(cdk::Error::Internal)
}

async fn submit_bark_operation(
    state: MintState,
    envelope: cdk::federation::FederationOperationEnvelope,
) -> Result<usize, cdk::Error> {
    state
        .submit_federation_envelope(envelope)
        .await?
        .ensure_accepted()?;
    Ok(1)
}

#[cfg(test)]
mod tests {
    use cdk::federation::{FederationBarkSemanticPhase, MemberId};
    use cdk::nuts::MeltQuoteState;

    use super::{
        bark_maintenance_retry_delay, melt_quote_blocks_routine_maintenance,
        payment_phase_is_terminal, payment_state_blocks_routine_maintenance,
        payment_timestamp_is_quiet, recent_payment_journal_floor, should_publish_maintenance_plan,
        BARK_MAINTENANCE_INTERVAL, BARK_RECENT_PAYMENT_JOURNAL_LIMIT,
        BARK_ROUTINE_MAINTENANCE_PAYMENT_QUIET_PERIOD,
    };

    #[test]
    fn maintenance_failures_back_off_to_five_minutes() {
        let seconds = (1..=8)
            .map(|failure| bark_maintenance_retry_delay(failure).as_secs())
            .collect::<Vec<_>>();

        assert_eq!(seconds, [5, 10, 20, 40, 80, 160, 300, 300]);
    }

    #[test]
    fn idle_maintenance_scans_drop_below_one_per_minute() {
        let mut elapsed = BARK_MAINTENANCE_INTERVAL;
        let mut scans = 0_u32;
        let mut no_progress = 0_u32;
        while elapsed <= std::time::Duration::from_secs(5 * 60) {
            scans = scans.saturating_add(1);
            no_progress = no_progress.saturating_add(1);
            elapsed = elapsed.saturating_add(bark_maintenance_retry_delay(no_progress));
        }

        assert_eq!(scans, 6);
        assert!(scans < 60);
    }

    #[test]
    fn only_terminal_payment_phases_release_routine_maintenance() {
        for phase in [
            FederationBarkSemanticPhase::PaymentSucceeded,
            FederationBarkSemanticPhase::RevocationCompleted,
        ] {
            assert!(payment_phase_is_terminal(phase));
        }
        for phase in [
            FederationBarkSemanticPhase::ReceivePrepared,
            FederationBarkSemanticPhase::ReceiveClaimed,
            FederationBarkSemanticPhase::PaymentFailed,
            FederationBarkSemanticPhase::RevocationPlanned,
        ] {
            assert!(!payment_phase_is_terminal(phase));
        }
    }

    #[test]
    fn active_or_evidenced_payment_blocks_routine_maintenance() {
        let expires_at = 1_000;

        assert!(payment_state_blocks_routine_maintenance(
            FederationBarkSemanticPhase::ReceivePrepared,
            Some(expires_at),
            false,
            false,
            expires_at,
        ));
        assert!(!payment_state_blocks_routine_maintenance(
            FederationBarkSemanticPhase::ReceivePrepared,
            Some(expires_at),
            false,
            false,
            expires_at + 901,
        ));
        assert!(payment_state_blocks_routine_maintenance(
            FederationBarkSemanticPhase::ReceivePrepared,
            Some(expires_at),
            true,
            false,
            expires_at + 901,
        ));
        assert!(payment_state_blocks_routine_maintenance(
            FederationBarkSemanticPhase::PaymentFailed,
            None,
            false,
            false,
            expires_at + 901,
        ));
        assert!(!payment_state_blocks_routine_maintenance(
            FederationBarkSemanticPhase::PaymentSucceeded,
            None,
            false,
            false,
            expires_at,
        ));
    }

    #[test]
    fn claimed_receive_blocks_until_result_certificate() {
        assert!(payment_state_blocks_routine_maintenance(
            FederationBarkSemanticPhase::ReceiveClaimed,
            None,
            true,
            false,
            1_000,
        ));
        assert!(!payment_state_blocks_routine_maintenance(
            FederationBarkSemanticPhase::ReceiveClaimed,
            None,
            true,
            true,
            1_000,
        ));
    }

    #[test]
    fn payment_quiet_period_is_bounded_and_ignores_future_timestamps() {
        let now = 1_000;
        let quiet = BARK_ROUTINE_MAINTENANCE_PAYMENT_QUIET_PERIOD.as_secs();

        assert!(payment_timestamp_is_quiet(now, now));
        assert!(payment_timestamp_is_quiet(now - quiet + 1, now));
        assert!(!payment_timestamp_is_quiet(now - quiet, now));
        assert!(!payment_timestamp_is_quiet(now + 1, now));
    }

    #[test]
    fn terminal_melt_restarts_payment_quiet_period() {
        let now = 1_000;
        let quiet = BARK_ROUTINE_MAINTENANCE_PAYMENT_QUIET_PERIOD.as_secs();
        let payment_started_before_quiet_period = now - quiet;

        assert!(!payment_timestamp_is_quiet(
            payment_started_before_quiet_period,
            now,
        ));
        assert!(melt_quote_blocks_routine_maintenance(
            MeltQuoteState::Pending,
            None,
            now,
        ));
        assert!(melt_quote_blocks_routine_maintenance(
            MeltQuoteState::Paid,
            Some(now),
            now,
        ));
        assert!(melt_quote_blocks_routine_maintenance(
            MeltQuoteState::Paid,
            Some(now - quiet + 1),
            now,
        ));
        assert!(!melt_quote_blocks_routine_maintenance(
            MeltQuoteState::Paid,
            Some(now - quiet),
            now,
        ));
        assert!(!melt_quote_blocks_routine_maintenance(
            MeltQuoteState::Paid,
            None,
            now,
        ));
        assert!(!melt_quote_blocks_routine_maintenance(
            MeltQuoteState::Paid,
            Some(now + 1),
            now,
        ));
        assert!(!melt_quote_blocks_routine_maintenance(
            MeltQuoteState::Unpaid,
            Some(now),
            now,
        ));
    }

    #[test]
    fn recent_payment_scan_is_bounded_by_the_journal_tip() {
        let limit = u64::try_from(BARK_RECENT_PAYMENT_JOURNAL_LIMIT).expect("journal limit");

        assert_eq!(recent_payment_journal_floor(0), 0);
        assert_eq!(recent_payment_journal_floor(limit - 1), 0);
        assert_eq!(recent_payment_journal_floor(limit), 1);
        assert_eq!(recent_payment_journal_floor(u64::MAX), u64::MAX - limit + 1);
    }

    #[test]
    fn only_the_deterministic_actor_publishes_a_maintenance_plan() {
        let actor = MemberId::new(1).expect("actor");

        assert!(should_publish_maintenance_plan(actor, actor));
        for member in 2..=5 {
            assert!(!should_publish_maintenance_plan(
                actor,
                MemberId::new(member).expect("member"),
            ));
        }
    }
}
