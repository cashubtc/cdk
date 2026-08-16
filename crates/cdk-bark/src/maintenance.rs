//! Backend-owned Bark refresh and emergency-exit effect orchestration.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use bitcoin::consensus::deserialize as deserialize_transaction;
use bitcoin::{ScriptBuf, Transaction};
use cdk_bitcoin_wallet::FederatedDescriptorWalletKeychain;
use cdk_common::federation::{
    FederationAddressAllocation, FederationAddressAllocationScope, FederationAddressKeychain,
    FederationAllocatedAddressRef, FederationChainObservationState, FederationJournalEntry,
    FederationOperationId, FederationOperationStatus, MemberId,
};
use cdk_federation_bark::{
    federation_bark_cpfp_allocation_quote, federation_bark_exit_allocation_quote,
    federation_bark_refresh_requires_recovery, validate_federation_bark_operation,
    FederationBarkActionId, FederationBarkActionPlan, FederationBarkActionRecord,
    FederationBarkArtifactKind, FederationBarkDigest, FederationBarkObservationStatus,
    FederationBarkOperation, FederationBarkProjectionSnapshot, FederationBarkSemanticPhase,
};
use thiserror::Error as ThisError;

use crate::{
    Error, FederatedBarkDegradedReason, FederatedBarkExitOutcome, FederatedBarkRefreshOutcome,
    FederatedBarkRuntime,
};

/// One side-effect boundary selected from the accepted Bark projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FederatedBarkRecoveryStep {
    /// No active recovery action exists; routine refresh planning may run.
    PlanRefresh,
    /// Apply or execute an accepted refresh plan.
    RefreshPlanned {
        /// Accepted action selected for this exact step.
        action_id: FederationBarkActionId,
    },
    /// Prepare and register an accepted emergency exit.
    ExitPlanned {
        /// Accepted action selected for this exact step.
        action_id: FederationBarkActionId,
    },
    /// Sign/provide accepted CPFPs and progress an accepted exit.
    ExitRegistered {
        /// Accepted action selected for this exact step.
        action_id: FederationBarkActionId,
    },
    /// Observe and finalize an accepted exit publication.
    ExitPublished {
        /// Accepted action selected for this exact step.
        action_id: FederationBarkActionId,
    },
}

/// Failure from either Bark's recovery state machine or its narrow host port.
#[derive(Debug, ThisError)]
pub enum FederatedBarkRecoveryDriverError<E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    /// A host database or consensus operation failed.
    #[error("federated Bark recovery host operation failed")]
    Host(#[source] E),
    /// A backend-owned Bark operation failed.
    #[error("federated Bark recovery backend operation failed")]
    Backend(#[source] Error),
    /// Accepted state did not contain the exact operation required by its projection.
    #[error("federated Bark recovery accepted state is invalid")]
    InvalidAcceptedState,
}

impl<E> FederatedBarkRecoveryDriverError<E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn host(error: E) -> Self {
        Self::Host(error)
    }

    fn backend(error: Error) -> Self {
        Self::Backend(error)
    }
}

/// Narrow integration port for accepted reads and consensus submission.
///
/// The host deliberately exposes no Bark phase-specific callback. Phase
/// selection, replay, external effects, retry, and transition construction
/// remain owned by this crate.
#[async_trait]
pub trait FederatedBarkRecoveryHost: Send + Sync + 'static {
    /// Host error type retained at the integration boundary.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Read one authoritative accepted Bark projection.
    async fn recovery_projection(
        &self,
        runtime: &Arc<FederatedBarkRuntime>,
    ) -> Result<FederationBarkProjectionSnapshot, Self::Error>;

    /// Check whether ordinary payment work currently blocks routine maintenance.
    async fn routine_payment_work_blocks(
        &self,
        projection: &FederationBarkProjectionSnapshot,
    ) -> Result<bool, Self::Error>;

    /// Load one accepted or rejected journal entry by exact operation id.
    async fn journal_entry(
        &self,
        operation_id: FederationOperationId,
    ) -> Result<Option<FederationJournalEntry>, Self::Error>;

    /// Return the known consensus status of one backend operation.
    async fn operation_status(
        &self,
        operation: &FederationBarkOperation,
    ) -> Result<Option<FederationOperationStatus>, Self::Error>;

    /// Submit one typed Bark operation and wait for its terminal acceptance.
    async fn submit_operation(
        &self,
        operation: FederationBarkOperation,
    ) -> Result<usize, Self::Error>;

    /// Read an exact consensus address allocation.
    async fn address_allocation(
        &self,
        scope: FederationAddressAllocationScope,
        quote: &str,
    ) -> Result<Option<FederationAddressAllocation>, Self::Error>;

    /// Submit one neutral address-allocation intent.
    async fn submit_address_allocation(
        &self,
        scope: FederationAddressAllocationScope,
        quote: String,
        keychain: FederationAddressKeychain,
    ) -> Result<usize, Self::Error>;

    /// Read the authoritative accepted chain-observation projection.
    async fn chain_observation_state(&self)
        -> Result<FederationChainObservationState, Self::Error>;
}

pub(crate) async fn execute_recovery_step<H>(
    host: &H,
    runtime: &Arc<FederatedBarkRuntime>,
    projection: FederationBarkProjectionSnapshot,
    step: FederatedBarkRecoveryStep,
    operator_requested: bool,
) -> Result<usize, FederatedBarkRecoveryDriverError<H::Error>>
where
    H: FederatedBarkRecoveryHost,
{
    let _advance_guard = runtime.recovery_advance.lock().await;
    let controls = runtime.maintenance_controls();
    if step == FederatedBarkRecoveryStep::PlanRefresh {
        if host
            .routine_payment_work_blocks(&projection)
            .await
            .map_err(FederatedBarkRecoveryDriverError::host)?
        {
            return Ok(0);
        }
        let actor = runtime
            .maintenance_actor()
            .map_err(FederatedBarkRecoveryDriverError::backend)?;
        if actor != runtime.member_id() {
            return Ok(0);
        }
        let Some(plan) = runtime
            .propose_refresh(&projection)
            .await
            .map_err(FederatedBarkRecoveryDriverError::backend)?
        else {
            return Ok(0);
        };
        if plan.actor != actor {
            return Err(FederatedBarkRecoveryDriverError::InvalidAcceptedState);
        }
        if host
            .routine_payment_work_blocks(&projection)
            .await
            .map_err(FederatedBarkRecoveryDriverError::host)?
        {
            return Ok(0);
        }
        return host
            .submit_operation(FederationBarkOperation::ActionPlan(plan))
            .await
            .map_err(FederatedBarkRecoveryDriverError::host);
    }

    let action_id = step
        .action_id()
        .ok_or(FederatedBarkRecoveryDriverError::InvalidAcceptedState)?;
    let action = projection
        .actions
        .iter()
        .find(|action| action.action_id == action_id)
        .cloned()
        .ok_or(FederatedBarkRecoveryDriverError::InvalidAcceptedState)?;

    // An authenticated operator retry may transfer an unchanged action to the
    // next canonical member. Ordinary automatic work never races the accepted
    // actor merely because a local timer elapsed.
    if operator_requested && action.actor != runtime.member_id() {
        let plan_operation_id = action
            .plan_operation_id
            .ok_or(FederatedBarkRecoveryDriverError::InvalidAcceptedState)?;
        let plan_entry = host
            .journal_entry(plan_operation_id)
            .await
            .map_err(FederatedBarkRecoveryDriverError::host)?
            .filter(|entry| entry.status() == FederationOperationStatus::Applied)
            .ok_or(FederatedBarkRecoveryDriverError::InvalidAcceptedState)?;
        if let Ok(retry) = runtime.advance_actor_attempt(&action, &plan_entry) {
            return host
                .submit_operation(FederationBarkOperation::ActionTransition(retry))
                .await
                .map_err(FederatedBarkRecoveryDriverError::host);
        }
    }

    match step {
        FederatedBarkRecoveryStep::PlanRefresh => Ok(0),
        FederatedBarkRecoveryStep::RefreshPlanned { .. } => {
            execute_refresh(
                host,
                runtime,
                projection,
                action,
                operator_requested,
                controls,
            )
            .await
        }
        FederatedBarkRecoveryStep::ExitPlanned { .. } => {
            execute_exit_plan(host, runtime, projection, action).await
        }
        FederatedBarkRecoveryStep::ExitRegistered { .. } => {
            execute_registered_exit(host, runtime, projection, action).await
        }
        FederatedBarkRecoveryStep::ExitPublished { .. } => {
            execute_published_exit(host, runtime, projection, action).await
        }
    }
}

impl FederatedBarkRecoveryStep {
    const fn action_id(self) -> Option<FederationBarkActionId> {
        match self {
            Self::PlanRefresh => None,
            Self::RefreshPlanned { action_id }
            | Self::ExitPlanned { action_id }
            | Self::ExitRegistered { action_id }
            | Self::ExitPublished { action_id } => Some(action_id),
        }
    }
}

async fn execute_refresh<H>(
    host: &H,
    runtime: &Arc<FederatedBarkRuntime>,
    projection: FederationBarkProjectionSnapshot,
    action: FederationBarkActionRecord,
    operator_requested: bool,
    controls: crate::FederatedBarkMaintenanceControls,
) -> Result<usize, FederatedBarkRecoveryDriverError<H::Error>>
where
    H: FederatedBarkRecoveryHost,
{
    let plan_entry = maintenance_entry(
        host,
        &projection,
        action.action_id,
        FederationBarkSemanticPhase::RefreshPlanned,
    )
    .await?;
    let plan = maintenance_plan(&plan_entry)?;
    if federation_bark_refresh_requires_recovery(&plan) {
        if controls.automatic_emergency_exit || operator_requested {
            return propose_exit(
                host,
                runtime,
                &projection,
                &action,
                &plan_entry,
                operator_requested,
            )
            .await;
        }
        runtime
            .mark_degraded(FederatedBarkDegradedReason::RecoveryRequired)
            .await;
        tracing::warn!(
            action_id = %plan.action_id,
            "accepted Bark refresh exceeds its signing budget and remains reserved for operator-triggered emergency exit"
        );
        return Ok(0);
    }
    runtime
        .apply_refresh_plan(&projection, &plan)
        .await
        .map_err(FederatedBarkRecoveryDriverError::backend)?;

    if action.phase == FederationBarkSemanticPhase::RefreshPlanned
        && controls.automatic_emergency_exit
        && runtime
            .emergency_exit_due(&projection, &plan_entry)
            .await
            .map_err(FederatedBarkRecoveryDriverError::backend)?
    {
        return propose_exit(
            host,
            runtime,
            &projection,
            &action,
            &plan_entry,
            operator_requested,
        )
        .await;
    }

    if submit_trigger_observation(host, runtime, &projection, &plan_entry).await? {
        return Ok(1);
    }
    if action.actor != runtime.member_id()
        || maintenance_trigger_observers(&projection, &plan).len()
            < usize::from(runtime.signature_threshold())
    {
        return Ok(0);
    }
    let authorization_entry =
        accepted_custody_authorization(host, &projection, &action, &plan_entry).await?;
    match runtime
        .execute_refresh(&projection, &plan_entry, &authorization_entry)
        .await
    {
        Ok(FederatedBarkRefreshOutcome::Pending) => Ok(0),
        Ok(FederatedBarkRefreshOutcome::Completed(transition)) => host
            .submit_operation(FederationBarkOperation::ActionTransition(*transition))
            .await
            .map_err(FederatedBarkRecoveryDriverError::host),
        Err(Error::CustodySignerUnavailable) => {
            submit_signing_retry(host, runtime, &action, &plan_entry).await
        }
        Err(error) => Err(FederatedBarkRecoveryDriverError::backend(error)),
    }
}

async fn execute_exit_plan<H>(
    host: &H,
    runtime: &Arc<FederatedBarkRuntime>,
    projection: FederationBarkProjectionSnapshot,
    action: FederationBarkActionRecord,
) -> Result<usize, FederatedBarkRecoveryDriverError<H::Error>>
where
    H: FederatedBarkRecoveryHost,
{
    let plan_entry = maintenance_entry(
        host,
        &projection,
        action.action_id,
        FederationBarkSemanticPhase::ExitPlanned,
    )
    .await?;
    let plan = maintenance_plan(&plan_entry)?;
    runtime
        .apply_emergency_exit_plan(&projection, &plan)
        .await
        .map_err(FederatedBarkRecoveryDriverError::backend)?;
    if submit_trigger_observation(host, runtime, &projection, &plan_entry).await? {
        return Ok(1);
    }
    if action.actor != runtime.member_id()
        || maintenance_trigger_observers(&projection, &plan).len()
            < usize::from(runtime.signature_threshold())
    {
        return Ok(0);
    }

    let scope = runtime
        .recovery_address_scope()
        .map_err(FederatedBarkRecoveryDriverError::backend)?;
    let mut parent_txids = plan
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Transaction)
        .filter_map(|artifact| deserialize_transaction::<Transaction>(&artifact.bytes).ok())
        .filter(|transaction| {
            transaction
                .output
                .iter()
                .filter(|output| output.script_pubkey == ScriptBuf::new_p2a())
                .count()
                == 1
        })
        .map(|transaction| transaction.compute_txid())
        .collect::<Vec<_>>();
    parent_txids.sort();
    parent_txids.dedup();
    if parent_txids.is_empty() {
        return Err(FederatedBarkRecoveryDriverError::InvalidAcceptedState);
    }

    let mut change_allocations = Vec::with_capacity(parent_txids.len());
    for parent_txid in parent_txids {
        let quote = federation_bark_cpfp_allocation_quote(plan.action_id, parent_txid);
        let allocation = match host
            .address_allocation(scope, &quote)
            .await
            .map_err(FederatedBarkRecoveryDriverError::host)?
        {
            Some(allocation) => allocation,
            None => {
                return host
                    .submit_address_allocation(scope, quote, FederationAddressKeychain::Internal)
                    .await
                    .map_err(FederatedBarkRecoveryDriverError::host);
            }
        };
        let expected_script = runtime
            .derive_recovery_script(true, allocation.derivation_index)
            .map_err(FederatedBarkRecoveryDriverError::backend)?;
        let local_script = runtime
            .recovery_wallet()
            .peek_federated_script(
                FederatedDescriptorWalletKeychain::Internal,
                allocation.derivation_index,
            )
            .await;
        if local_script != expected_script {
            return Err(FederatedBarkRecoveryDriverError::InvalidAcceptedState);
        }
        change_allocations.push(FederationAllocatedAddressRef {
            application_id: scope.application_id,
            application_key_epoch: scope.application_key_epoch,
            quote,
            keychain: FederationAddressKeychain::Internal,
            derivation_index: allocation.derivation_index,
        });
    }
    let chain_state = host
        .chain_observation_state()
        .await
        .map_err(FederatedBarkRecoveryDriverError::host)?;
    let package_set = match runtime
        .prepare_registered_cpfp_packages(&action, &plan_entry, &chain_state, &change_allocations)
        .await
    {
        Ok(package_set) => package_set,
        Err(Error::RecoveryFeeReserveInsufficient) => {
            runtime
                .mark_degraded(FederatedBarkDegradedReason::RecoveryFeeReserveInsufficient)
                .await;
            tracing::warn!(
                action_id = %plan.action_id,
                "Bark recovery requires a spendable fee-reserve output in its recovery application"
            );
            return Ok(0);
        }
        Err(error) => return Err(FederatedBarkRecoveryDriverError::backend(error)),
    };
    runtime
        .clear_degraded(FederatedBarkDegradedReason::RecoveryFeeReserveInsufficient)
        .await;
    let transition = runtime
        .register_emergency_exit(&projection, &action, &plan_entry, &package_set)
        .await
        .map_err(FederatedBarkRecoveryDriverError::backend)?;
    host.submit_operation(FederationBarkOperation::ActionTransition(transition))
        .await
        .map_err(FederatedBarkRecoveryDriverError::host)
}

async fn execute_registered_exit<H>(
    host: &H,
    runtime: &Arc<FederatedBarkRuntime>,
    projection: FederationBarkProjectionSnapshot,
    action: FederationBarkActionRecord,
) -> Result<usize, FederatedBarkRecoveryDriverError<H::Error>>
where
    H: FederatedBarkRecoveryHost,
{
    let (plan_entry, registered_entry) = exit_entries(host, &projection, action.action_id).await?;
    runtime
        .apply_emergency_exit_registered(&projection, &plan_entry, &registered_entry)
        .await
        .map_err(FederatedBarkRecoveryDriverError::backend)?;
    if action.actor != runtime.member_id() {
        return Ok(0);
    }
    let registered = maintenance_transition(
        &registered_entry,
        FederationBarkSemanticPhase::ExitRegistered,
    )?;
    let package_sets = registered
        .artifacts
        .iter()
        .filter_map(|artifact| crate::FederationBarkCpfpPackageSet::from_artifact(artifact).ok())
        .collect::<Vec<_>>();
    let [package_set] = package_sets.as_slice() else {
        return Err(FederatedBarkRecoveryDriverError::InvalidAcceptedState);
    };
    let publication = match runtime
        .sign_registered_cpfp_packages(&action, &registered_entry)
        .await
    {
        Ok(publication) => publication,
        Err(Error::CustodySignerUnavailable) => {
            return submit_signing_retry(host, runtime, &action, &plan_entry).await;
        }
        Err(error) => return Err(FederatedBarkRecoveryDriverError::backend(error)),
    };
    let authorization_entry =
        accepted_custody_authorization(host, &projection, &action, &plan_entry).await?;
    match runtime
        .publish_emergency_exit(
            &projection,
            &plan_entry,
            &registered_entry,
            &authorization_entry,
            package_set,
            &publication,
        )
        .await
    {
        Ok(FederatedBarkExitOutcome::Pending) => Ok(0),
        Ok(FederatedBarkExitOutcome::Published(transition)) => host
            .submit_operation(FederationBarkOperation::ActionTransition(*transition))
            .await
            .map_err(FederatedBarkRecoveryDriverError::host),
        Err(Error::CustodySignerUnavailable) => {
            submit_signing_retry(host, runtime, &action, &plan_entry).await
        }
        Err(error) => Err(FederatedBarkRecoveryDriverError::backend(error)),
    }
}

async fn execute_published_exit<H>(
    host: &H,
    runtime: &Arc<FederatedBarkRuntime>,
    projection: FederationBarkProjectionSnapshot,
    action: FederationBarkActionRecord,
) -> Result<usize, FederatedBarkRecoveryDriverError<H::Error>>
where
    H: FederatedBarkRecoveryHost,
{
    let (plan_entry, registered_entry) = exit_entries(host, &projection, action.action_id).await?;
    let published_entry = maintenance_entry(
        host,
        &projection,
        action.action_id,
        FederationBarkSemanticPhase::ExitPublished,
    )
    .await?;
    if submit_confirmation_observation(
        host,
        runtime,
        &projection,
        &plan_entry,
        &registered_entry,
        &published_entry,
    )
    .await?
    {
        return Ok(1);
    }
    let plan = maintenance_plan(&plan_entry)?;
    if action.actor != runtime.member_id()
        || !has_confirmation_quorum(&projection, plan.action_id, runtime.signature_threshold())
    {
        return Ok(0);
    }
    let Some(transition) = runtime
        .confirm_emergency_exit(
            &projection,
            &action,
            &plan_entry,
            &registered_entry,
            &published_entry,
        )
        .await
        .map_err(FederatedBarkRecoveryDriverError::backend)?
    else {
        return Ok(0);
    };
    host.submit_operation(FederationBarkOperation::ActionTransition(transition))
        .await
        .map_err(FederatedBarkRecoveryDriverError::host)
}

async fn propose_exit<H>(
    host: &H,
    runtime: &Arc<FederatedBarkRuntime>,
    projection: &FederationBarkProjectionSnapshot,
    action: &FederationBarkActionRecord,
    refresh_plan_entry: &FederationJournalEntry,
    operator_requested: bool,
) -> Result<usize, FederatedBarkRecoveryDriverError<H::Error>>
where
    H: FederatedBarkRecoveryHost,
{
    let refresh = maintenance_plan(refresh_plan_entry)?;
    let scope = runtime
        .recovery_address_scope()
        .map_err(FederatedBarkRecoveryDriverError::backend)?;
    let quote = federation_bark_exit_allocation_quote(refresh.action_id);
    let allocation = match host
        .address_allocation(scope, &quote)
        .await
        .map_err(FederatedBarkRecoveryDriverError::host)?
    {
        Some(allocation) => allocation,
        None => {
            return host
                .submit_address_allocation(scope, quote, FederationAddressKeychain::Internal)
                .await
                .map_err(FederatedBarkRecoveryDriverError::host);
        }
    };
    let destination_script = runtime
        .derive_recovery_script(true, allocation.derivation_index)
        .map_err(FederatedBarkRecoveryDriverError::backend)?;
    let plan = runtime
        .propose_emergency_exit(
            projection,
            action,
            refresh_plan_entry,
            FederationAllocatedAddressRef {
                application_id: scope.application_id,
                application_key_epoch: scope.application_key_epoch,
                quote,
                keychain: FederationAddressKeychain::Internal,
                derivation_index: allocation.derivation_index,
            },
            destination_script.into_bytes(),
            runtime
                .recovery_minimum_confirmations()
                .map_err(FederatedBarkRecoveryDriverError::backend)?,
        )
        .map_err(FederatedBarkRecoveryDriverError::backend)?;
    let operation = FederationBarkOperation::ActionPlan(plan);
    let rejected = host
        .operation_status(&operation)
        .await
        .map_err(FederatedBarkRecoveryDriverError::host)?
        == Some(FederationOperationStatus::Rejected);
    if rejected {
        if !operator_requested {
            runtime
                .mark_degraded(FederatedBarkDegradedReason::RecoveryRequired)
                .await;
            tracing::warn!(
                action_id = %action.action_id,
                attempt = action.attempt,
                "rejected Bark emergency exit requires an explicit operator retry"
            );
            return Ok(0);
        }
        return submit_signing_retry(host, runtime, action, refresh_plan_entry).await;
    }
    host.submit_operation(operation)
        .await
        .map_err(FederatedBarkRecoveryDriverError::host)
}

async fn submit_signing_retry<H>(
    host: &H,
    runtime: &Arc<FederatedBarkRuntime>,
    action: &FederationBarkActionRecord,
    plan_entry: &FederationJournalEntry,
) -> Result<usize, FederatedBarkRecoveryDriverError<H::Error>>
where
    H: FederatedBarkRecoveryHost,
{
    let retry = runtime
        .advance_signing_attempt(action, plan_entry)
        .map_err(FederatedBarkRecoveryDriverError::backend)?;
    host.submit_operation(FederationBarkOperation::ActionTransition(retry))
        .await
        .map_err(FederatedBarkRecoveryDriverError::host)
}

async fn submit_trigger_observation<H>(
    host: &H,
    runtime: &Arc<FederatedBarkRuntime>,
    projection: &FederationBarkProjectionSnapshot,
    plan_entry: &FederationJournalEntry,
) -> Result<bool, FederatedBarkRecoveryDriverError<H::Error>>
where
    H: FederatedBarkRecoveryHost,
{
    let plan = maintenance_plan(plan_entry)?;
    if maintenance_trigger_observers(projection, &plan).contains(&runtime.member_id()) {
        return Ok(false);
    }
    let epoch = next_observation_epoch(projection, plan.action_id, runtime.member_id())?;
    let Some(observation) = runtime
        .observe_maintenance_trigger(projection, plan_entry, epoch)
        .await
        .map_err(FederatedBarkRecoveryDriverError::backend)?
    else {
        return Ok(false);
    };
    host.submit_operation(FederationBarkOperation::MemberObservation(observation))
        .await
        .map_err(FederatedBarkRecoveryDriverError::host)?;
    Ok(true)
}

async fn submit_confirmation_observation<H>(
    host: &H,
    runtime: &Arc<FederatedBarkRuntime>,
    projection: &FederationBarkProjectionSnapshot,
    plan_entry: &FederationJournalEntry,
    registered_entry: &FederationJournalEntry,
    published_entry: &FederationJournalEntry,
) -> Result<bool, FederatedBarkRecoveryDriverError<H::Error>>
where
    H: FederatedBarkRecoveryHost,
{
    let plan = maintenance_plan(plan_entry)?;
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
    let epoch = next_observation_epoch(projection, plan.action_id, runtime.member_id())?;
    let Some(observation) = runtime
        .observe_emergency_exit_confirmation(
            projection,
            plan_entry,
            registered_entry,
            published_entry,
            epoch,
        )
        .await
        .map_err(FederatedBarkRecoveryDriverError::backend)?
    else {
        return Ok(false);
    };
    host.submit_operation(FederationBarkOperation::MemberObservation(observation))
        .await
        .map_err(FederatedBarkRecoveryDriverError::host)?;
    Ok(true)
}

async fn accepted_custody_authorization<H>(
    host: &H,
    projection: &FederationBarkProjectionSnapshot,
    action: &FederationBarkActionRecord,
    plan_entry: &FederationJournalEntry,
) -> Result<FederationJournalEntry, FederatedBarkRecoveryDriverError<H::Error>>
where
    H: FederatedBarkRecoveryHost,
{
    let plan = maintenance_plan(plan_entry)?;
    if action.action_id != plan.action_id
        || action.plan_operation_id != Some(plan_entry.operation_id)
        || action.attempt < plan.attempt
    {
        return Err(FederatedBarkRecoveryDriverError::InvalidAcceptedState);
    }
    if action.attempt == plan.attempt {
        return Ok(plan_entry.clone());
    }
    let operation_id = projection
        .transitions
        .iter()
        .filter_map(|record| match &record.operation {
            FederationBarkOperation::ActionTransition(transition)
                if record.action_id == action.action_id
                    && transition.phase == FederationBarkSemanticPhase::SigningAttemptAdvanced
                    && transition.plan_operation_id == plan_entry.operation_id
                    && transition.attempt == action.attempt =>
            {
                Some((record.operation_index, record.operation_id))
            }
            _ => None,
        })
        .max_by_key(|(operation_index, _)| *operation_index)
        .map(|(_, operation_id)| operation_id)
        .ok_or(FederatedBarkRecoveryDriverError::InvalidAcceptedState)?;
    host.journal_entry(operation_id)
        .await
        .map_err(FederatedBarkRecoveryDriverError::host)?
        .ok_or(FederatedBarkRecoveryDriverError::InvalidAcceptedState)
}

async fn exit_entries<H>(
    host: &H,
    projection: &FederationBarkProjectionSnapshot,
    action_id: FederationBarkActionId,
) -> Result<
    (FederationJournalEntry, FederationJournalEntry),
    FederatedBarkRecoveryDriverError<H::Error>,
>
where
    H: FederatedBarkRecoveryHost,
{
    Ok((
        maintenance_entry(
            host,
            projection,
            action_id,
            FederationBarkSemanticPhase::ExitPlanned,
        )
        .await?,
        maintenance_entry(
            host,
            projection,
            action_id,
            FederationBarkSemanticPhase::ExitRegistered,
        )
        .await?,
    ))
}

async fn maintenance_entry<H>(
    host: &H,
    projection: &FederationBarkProjectionSnapshot,
    action_id: FederationBarkActionId,
    phase: FederationBarkSemanticPhase,
) -> Result<FederationJournalEntry, FederatedBarkRecoveryDriverError<H::Error>>
where
    H: FederatedBarkRecoveryHost,
{
    let operation_id = projection
        .transitions
        .iter()
        .find(|record| record.action_id == action_id && record.phase == phase)
        .map(|record| record.operation_id)
        .ok_or(FederatedBarkRecoveryDriverError::InvalidAcceptedState)?;
    host.journal_entry(operation_id)
        .await
        .map_err(FederatedBarkRecoveryDriverError::host)?
        .filter(|entry| entry.status() == FederationOperationStatus::Applied)
        .ok_or(FederatedBarkRecoveryDriverError::InvalidAcceptedState)
}

fn maintenance_plan<E>(
    entry: &FederationJournalEntry,
) -> Result<FederationBarkActionPlan, FederatedBarkRecoveryDriverError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    match validate_federation_bark_operation(&entry.envelope.operation) {
        Ok(FederationBarkOperation::ActionPlan(plan)) => Ok(plan),
        _ => Err(FederatedBarkRecoveryDriverError::InvalidAcceptedState),
    }
}

fn maintenance_transition<E>(
    entry: &FederationJournalEntry,
    phase: FederationBarkSemanticPhase,
) -> Result<cdk_federation_bark::FederationBarkActionTransition, FederatedBarkRecoveryDriverError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    match validate_federation_bark_operation(&entry.envelope.operation) {
        Ok(FederationBarkOperation::ActionTransition(transition)) if transition.phase == phase => {
            Ok(transition)
        }
        _ => Err(FederatedBarkRecoveryDriverError::InvalidAcceptedState),
    }
}

fn maintenance_trigger_observers(
    projection: &FederationBarkProjectionSnapshot,
    plan: &FederationBarkActionPlan,
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
    action_id: FederationBarkActionId,
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

fn next_observation_epoch<E>(
    projection: &FederationBarkProjectionSnapshot,
    action_id: FederationBarkActionId,
    member_id: MemberId,
) -> Result<u64, FederatedBarkRecoveryDriverError<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
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
        .ok_or(FederatedBarkRecoveryDriverError::InvalidAcceptedState)
}
