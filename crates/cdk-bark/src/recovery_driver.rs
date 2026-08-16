//! Single backend-owned selector and retry loop for Bark recovery work.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cdk_common::federation::{
    FederationJournalEntry, FederationOperationId, FederationOperationStatus,
};
use cdk_common::mint::MeltQuote;
use cdk_federation_bark::{
    validate_federation_bark_operation, FederationBarkActionId, FederationBarkActionKind,
    FederationBarkOperation, FederationBarkProjectionSnapshot, FederationBarkReceiveIntent,
    FederationBarkReceivePrepared, FederationBarkSemanticPhase,
};

use crate::{
    maintenance::execute_recovery_step, Error, FederatedBarkDegradedReason,
    FederatedBarkMaintenanceControls, FederatedBarkReceiveSecretAcceptedRef,
    FederatedBarkRecoveryDriverError, FederatedBarkRecoveryHost, FederatedBarkRecoveryStep,
    FederatedBarkRuntime,
};

const DRIVER_INTERVAL: Duration = Duration::from_secs(5);
const DRIVER_RETRY_MAX: Duration = Duration::from_secs(300);

/// Read-only accepted-state boundary used by backend-owned startup replay.
#[async_trait]
pub trait FederatedBarkReplaySource: Send + Sync {
    /// Read the authoritative Bark projection at the accepted frontier.
    async fn replay_projection(&self) -> Result<FederationBarkProjectionSnapshot, Error>;

    /// Load one exact applied journal entry by its accepted identifier.
    async fn replay_entry(
        &self,
        operation_id: FederationOperationId,
    ) -> Result<FederationJournalEntry, Error>;

    /// Load the accepted receive intent at one action origin.
    async fn replay_receive_intent(
        &self,
        operation_id: FederationOperationId,
    ) -> Result<FederationBarkReceiveIntent, Error>;

    /// Load the accepted melt quote bound to an outgoing Bark action.
    async fn replay_melt_quote(&self, quote: &str) -> Result<MeltQuote, Error>;
}

/// Sole selector and retry-loop owner for Bark refresh and exit recovery.
#[derive(Debug, Clone)]
pub struct FederatedBarkRecoveryDriver {
    runtime: Arc<FederatedBarkRuntime>,
}

impl FederatedBarkRecoveryDriver {
    /// Construct one driver for one configured Bark backend.
    #[must_use]
    pub fn new(runtime: Arc<FederatedBarkRuntime>) -> Self {
        Self { runtime }
    }

    /// Select and execute at most one accepted recovery step.
    pub async fn advance<H>(
        &self,
        host: &H,
        target_action_id: Option<FederationBarkActionId>,
    ) -> Result<usize, FederatedBarkRecoveryDriverError<H::Error>>
    where
        H: FederatedBarkRecoveryHost,
    {
        let projection = host
            .recovery_projection(&self.runtime)
            .await
            .map_err(FederatedBarkRecoveryDriverError::Host)?;
        let Some(step) = select_recovery_step(
            &projection,
            self.runtime.maintenance_controls(),
            target_action_id,
        ) else {
            return Ok(0);
        };
        execute_recovery_step(
            host,
            &self.runtime,
            projection,
            step,
            target_action_id.is_some(),
        )
        .await
    }

    /// Rebuild all member-local Bark state from accepted operations only.
    ///
    /// This path performs no Ark, chain, signing, randomness, or clock work.
    /// It is the same backend owner used by startup and readiness catch-up.
    pub async fn reconcile<S>(
        &self,
        source: &S,
        entries: &[FederationJournalEntry],
    ) -> Result<(), Error>
    where
        S: FederatedBarkReplaySource,
    {
        let secret_store = self.runtime.receive_secret_store();
        for entry in entries
            .iter()
            .filter(|entry| entry.status() == FederationOperationStatus::Applied)
        {
            let Ok(operation) = validate_federation_bark_operation(&entry.envelope.operation)
            else {
                continue;
            };
            match &operation {
                FederationBarkOperation::ReceiveIntent(intent) => {
                    if secret_store.status(intent.action_id)?.is_some() {
                        secret_store.mark_intent_accepted(
                            intent.action_id,
                            FederatedBarkReceiveSecretAcceptedRef {
                                operation_id: entry.operation_id,
                                operation_index: entry.index,
                            },
                        )?;
                    }
                }
                FederationBarkOperation::ReceivePrepared(prepared) => {
                    let projection = source.replay_projection().await?;
                    let Some((intent, _, _)) = self
                        .receive_replay_context(source, &projection, prepared.action_id)
                        .await?
                    else {
                        continue;
                    };
                    if self
                        .runtime
                        .apply_receive_prepared(&intent, prepared)
                        .await
                        .is_err()
                    {
                        return self
                            .replay_failed(
                                FederatedBarkDegradedReason::ProjectionInvalid,
                                Error::ReceiveProjectionFailed,
                            )
                            .await;
                    }
                }
                FederationBarkOperation::ActionPlan(plan)
                    if plan.kind == FederationBarkActionKind::IncomingClaim =>
                {
                    let projection = source.replay_projection().await?;
                    let Some((intent, _, prepared)) = self
                        .receive_replay_context(source, &projection, plan.action_id)
                        .await?
                    else {
                        continue;
                    };
                    if self
                        .runtime
                        .apply_receive_claim_plan(&intent, &prepared, entry)
                        .await
                        .is_err()
                    {
                        return self
                            .replay_failed(
                                FederatedBarkDegradedReason::RecoveryRequired,
                                Error::ReceiveClaimRecoveryRequired,
                            )
                            .await;
                    }
                }
                FederationBarkOperation::ActionPlan(plan)
                    if plan.kind == FederationBarkActionKind::LightningSend =>
                {
                    let quote = source
                        .replay_melt_quote(plan.quote.as_deref().ok_or(Error::SendPlanInvalid)?)
                        .await?;
                    let projection = source.replay_projection().await?;
                    if self
                        .runtime
                        .apply_lightning_send_plan(&quote, &projection, plan)
                        .await
                        .is_err()
                    {
                        return self
                            .replay_failed(
                                FederatedBarkDegradedReason::ProjectionInvalid,
                                Error::SendProjectionFailed,
                            )
                            .await;
                    }
                }
                FederationBarkOperation::ActionTransition(transition)
                    if matches!(
                        transition.phase,
                        FederationBarkSemanticPhase::ReceiveHtlcsReady
                            | FederationBarkSemanticPhase::ReceivePreimageDisclosed
                            | FederationBarkSemanticPhase::ReceiveClaimed
                    ) =>
                {
                    let projection = source.replay_projection().await?;
                    let Some((intent, prepared_operation_id, prepared)) = self
                        .receive_replay_context(source, &projection, transition.action_id)
                        .await?
                    else {
                        continue;
                    };
                    let replay = match transition.phase {
                        FederationBarkSemanticPhase::ReceiveHtlcsReady => {
                            self.runtime
                                .apply_receive_htlcs_ready(
                                    &intent,
                                    prepared_operation_id,
                                    &prepared,
                                    entry,
                                )
                                .await
                        }
                        FederationBarkSemanticPhase::ReceivePreimageDisclosed
                        | FederationBarkSemanticPhase::ReceiveClaimed => {
                            let plan_entry =
                                source.replay_entry(transition.plan_operation_id).await?;
                            self.runtime
                                .apply_receive_claim_plan(&intent, &prepared, &plan_entry)
                                .await?;
                            self.runtime
                                .apply_receive_claim_transition(
                                    &intent,
                                    &prepared,
                                    &plan_entry,
                                    entry,
                                )
                                .await
                        }
                        _ => unreachable!("receive replay phase was matched above"),
                    };
                    if replay.is_err() {
                        return self
                            .replay_failed(
                                FederatedBarkDegradedReason::RecoveryRequired,
                                Error::ReceiveClaimRecoveryRequired,
                            )
                            .await;
                    }
                }
                FederationBarkOperation::ActionTransition(transition)
                    if matches!(
                        transition.phase,
                        FederationBarkSemanticPhase::SendHtlcsCosigned
                            | FederationBarkSemanticPhase::PaymentSucceeded
                            | FederationBarkSemanticPhase::PaymentFailed
                            | FederationBarkSemanticPhase::RevocationCompleted
                    ) =>
                {
                    self.replay_send_transition(source, entry, transition)
                        .await?;
                }
                FederationBarkOperation::ActionPlan(_)
                | FederationBarkOperation::ActionTransition(_)
                | FederationBarkOperation::MemberObservation(_)
                | FederationBarkOperation::EvidenceCertificate(_) => {}
            }
        }
        self.reconcile_maintenance(source).await?;
        if let Some(frontier) = source.replay_projection().await?.frontier() {
            self.runtime.record_projection_frontier(frontier).await;
        }
        self.runtime
            .clear_degraded(FederatedBarkDegradedReason::ProjectionInvalid)
            .await;
        Ok(())
    }

    async fn replay_send_transition<S>(
        &self,
        source: &S,
        entry: &FederationJournalEntry,
        transition: &cdk_federation_bark::FederationBarkActionTransition,
    ) -> Result<(), Error>
    where
        S: FederatedBarkReplaySource,
    {
        let projection = source.replay_projection().await?;
        let send_plan_operation_id = projection
            .transitions
            .iter()
            .find(|record| {
                record.action_id == transition.action_id
                    && record.phase == FederationBarkSemanticPhase::SendPlanned
            })
            .map(|record| record.operation_id)
            .ok_or(Error::SendReconciliationFailed)?;
        let plan_entry = source.replay_entry(send_plan_operation_id).await?;
        let FederationBarkOperation::ActionPlan(plan) =
            validate_federation_bark_operation(&plan_entry.envelope.operation)
                .map_err(|_| Error::SendReconciliationFailed)?
        else {
            return Err(Error::SendReconciliationFailed);
        };
        let quote = source
            .replay_melt_quote(
                plan.quote
                    .as_deref()
                    .ok_or(Error::SendReconciliationFailed)?,
            )
            .await?;
        let replay = match transition.phase {
            FederationBarkSemanticPhase::SendHtlcsCosigned => {
                self.runtime
                    .apply_lightning_send_htlcs(&quote, &projection, &plan_entry, entry)
                    .await
            }
            FederationBarkSemanticPhase::PaymentSucceeded => self
                .runtime
                .apply_lightning_send_succeeded(&quote, &projection, &plan_entry, entry, 1)
                .await
                .map(|_| ()),
            FederationBarkSemanticPhase::PaymentFailed => {
                self.runtime
                    .apply_lightning_send_failed(&quote, &projection, &plan_entry, entry)
                    .await
            }
            FederationBarkSemanticPhase::RevocationCompleted => {
                let revocation_plan_entry =
                    source.replay_entry(transition.plan_operation_id).await?;
                let result = self
                    .runtime
                    .apply_failed_send_revocation(
                        &quote,
                        &projection,
                        &plan_entry,
                        &revocation_plan_entry,
                        entry,
                        1,
                    )
                    .await
                    .map(|_| ());
                if result.is_ok() {
                    self.runtime
                        .clear_degraded(FederatedBarkDegradedReason::RecoveryRequired)
                        .await;
                }
                result
            }
            _ => unreachable!("send replay phase was matched above"),
        };
        if replay.is_err() {
            let reason = match transition.phase {
                FederationBarkSemanticPhase::PaymentFailed
                | FederationBarkSemanticPhase::RevocationCompleted => {
                    FederatedBarkDegradedReason::RecoveryRequired
                }
                _ => FederatedBarkDegradedReason::ProjectionInvalid,
            };
            return self
                .replay_failed(reason, Error::SendReconciliationFailed)
                .await;
        }
        Ok(())
    }

    async fn receive_replay_context<S>(
        &self,
        source: &S,
        projection: &FederationBarkProjectionSnapshot,
        action_id: FederationBarkActionId,
    ) -> Result<
        Option<(
            FederationBarkReceiveIntent,
            FederationOperationId,
            FederationBarkReceivePrepared,
        )>,
        Error,
    >
    where
        S: FederatedBarkReplaySource,
    {
        let action = projection
            .actions
            .iter()
            .find(|action| action.action_id == action_id)
            .ok_or(Error::ReceiveProjectionFailed)?;
        let intent = source
            .replay_receive_intent(action.origin_operation_id)
            .await?;
        if self
            .runtime
            .receive_secret_store()
            .status(action_id)?
            .is_none()
        {
            let acknowledged = intent
                .secret_acknowledgements
                .iter()
                .any(|acknowledgement| acknowledgement.member == self.runtime.member_id());
            if acknowledged && action.phase != FederationBarkSemanticPhase::ReceiveClaimed {
                return self
                    .replay_failed(
                        FederatedBarkDegradedReason::RecoveryRequired,
                        Error::ReceiveSecretUnavailable,
                    )
                    .await;
            }
            return Ok(None);
        }
        let prepared = projection
            .transitions
            .iter()
            .find_map(|record| {
                if record.action_id != action_id
                    || record.phase != FederationBarkSemanticPhase::ReceivePrepared
                {
                    return None;
                }
                match &record.operation {
                    FederationBarkOperation::ReceivePrepared(prepared) => {
                        Some((record.operation_id, prepared.clone()))
                    }
                    _ => None,
                }
            })
            .ok_or(Error::ReceiveProjectionFailed)?;
        Ok(Some((intent, prepared.0, prepared.1)))
    }

    async fn reconcile_maintenance<S>(&self, source: &S) -> Result<(), Error>
    where
        S: FederatedBarkReplaySource,
    {
        let projection = source.replay_projection().await?;
        for action in &projection.actions {
            match action.phase {
                FederationBarkSemanticPhase::RefreshPlanned => {
                    let plan_entry = self
                        .maintenance_plan_entry(source, action, FederationBarkActionKind::Refresh)
                        .await?;
                    let FederationBarkOperation::ActionPlan(plan) =
                        validate_federation_bark_operation(&plan_entry.envelope.operation)
                            .map_err(|_| Error::MaintenanceReconciliationFailed)?
                    else {
                        return Err(Error::MaintenanceReconciliationFailed);
                    };
                    self.runtime.apply_refresh_plan(&projection, &plan).await?;
                }
                FederationBarkSemanticPhase::RefreshCompleted => {
                    let plan_entry = self
                        .maintenance_plan_entry(source, action, FederationBarkActionKind::Refresh)
                        .await?;
                    let completed_entry = self
                        .maintenance_transition_entry(
                            source,
                            &projection,
                            action.action_id,
                            FederationBarkSemanticPhase::RefreshCompleted,
                        )
                        .await?;
                    let FederationBarkOperation::ActionPlan(plan) =
                        validate_federation_bark_operation(&plan_entry.envelope.operation)
                            .map_err(|_| Error::MaintenanceReconciliationFailed)?
                    else {
                        return Err(Error::MaintenanceReconciliationFailed);
                    };
                    self.runtime.apply_refresh_plan(&projection, &plan).await?;
                    self.runtime
                        .apply_refresh_completed(&projection, &plan_entry, &completed_entry)
                        .await?;
                }
                FederationBarkSemanticPhase::ExitPlanned => {
                    let plan_entry = self
                        .maintenance_plan_entry(
                            source,
                            action,
                            FederationBarkActionKind::EmergencyExit,
                        )
                        .await?;
                    let FederationBarkOperation::ActionPlan(plan) =
                        validate_federation_bark_operation(&plan_entry.envelope.operation)
                            .map_err(|_| Error::MaintenanceReconciliationFailed)?
                    else {
                        return Err(Error::MaintenanceReconciliationFailed);
                    };
                    self.runtime
                        .apply_emergency_exit_plan(&projection, &plan)
                        .await?;
                }
                FederationBarkSemanticPhase::ExitRegistered
                | FederationBarkSemanticPhase::ExitPublished
                | FederationBarkSemanticPhase::ExitConfirmed => {
                    let plan_entry = self
                        .maintenance_plan_entry(
                            source,
                            action,
                            FederationBarkActionKind::EmergencyExit,
                        )
                        .await?;
                    let registered_entry = self
                        .maintenance_transition_entry(
                            source,
                            &projection,
                            action.action_id,
                            FederationBarkSemanticPhase::ExitRegistered,
                        )
                        .await?;
                    self.runtime
                        .apply_emergency_exit_registered(
                            &projection,
                            &plan_entry,
                            &registered_entry,
                        )
                        .await?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn maintenance_plan_entry<S>(
        &self,
        source: &S,
        action: &cdk_federation_bark::FederationBarkActionRecord,
        kind: FederationBarkActionKind,
    ) -> Result<FederationJournalEntry, Error>
    where
        S: FederatedBarkReplaySource,
    {
        let operation_id = action
            .plan_operation_id
            .ok_or(Error::MaintenanceReconciliationFailed)?;
        let entry = source.replay_entry(operation_id).await?;
        let operation = validate_federation_bark_operation(&entry.envelope.operation)
            .map_err(|_| Error::MaintenanceReconciliationFailed)?;
        if entry.status() != FederationOperationStatus::Applied
            || !matches!(
                operation,
                FederationBarkOperation::ActionPlan(plan)
                    if plan.action_id == action.action_id && plan.kind == kind
            )
        {
            return Err(Error::MaintenanceReconciliationFailed);
        }
        Ok(entry)
    }

    async fn maintenance_transition_entry<S>(
        &self,
        source: &S,
        projection: &FederationBarkProjectionSnapshot,
        action_id: FederationBarkActionId,
        phase: FederationBarkSemanticPhase,
    ) -> Result<FederationJournalEntry, Error>
    where
        S: FederatedBarkReplaySource,
    {
        let operation_id = projection
            .transitions
            .iter()
            .find(|record| record.action_id == action_id && record.phase == phase)
            .map(|record| record.operation_id)
            .ok_or(Error::MaintenanceReconciliationFailed)?;
        let entry = source.replay_entry(operation_id).await?;
        let operation = validate_federation_bark_operation(&entry.envelope.operation)
            .map_err(|_| Error::MaintenanceReconciliationFailed)?;
        if entry.status() != FederationOperationStatus::Applied
            || !matches!(
                operation,
                FederationBarkOperation::ActionTransition(transition)
                    if transition.action_id == action_id && transition.phase == phase
            )
        {
            return Err(Error::MaintenanceReconciliationFailed);
        }
        Ok(entry)
    }

    async fn replay_failed<T>(
        &self,
        reason: FederatedBarkDegradedReason,
        error: Error,
    ) -> Result<T, Error> {
        self.runtime.mark_degraded(reason).await;
        Err(error)
    }

    /// Spawn the one bounded automatic recovery loop for this backend.
    pub fn spawn<H>(self, host: Arc<H>) -> tokio::task::JoinHandle<()>
    where
        H: FederatedBarkRecoveryHost,
    {
        self.spawn_until(host, std::future::pending())
    }

    /// Spawn the bounded recovery loop until its owning service shuts down.
    pub fn spawn_until<H, S>(self, host: Arc<H>, shutdown: S) -> tokio::task::JoinHandle<()>
    where
        H: FederatedBarkRecoveryHost,
        S: Future<Output = ()> + Send + 'static,
    {
        tokio::spawn(async move {
            let mut consecutive_no_progress = 0_u32;
            let mut next_delay = DRIVER_INTERVAL;
            tokio::pin!(shutdown);
            loop {
                tokio::select! {
                    () = &mut shutdown => break,
                    () = tokio::time::sleep(next_delay) => {}
                }
                match self.advance(host.as_ref(), None).await {
                    Ok(advanced) if advanced > 0 => {
                        consecutive_no_progress = 0;
                        next_delay = DRIVER_INTERVAL;
                        tracing::debug!(advanced, "advanced federated Bark recovery driver");
                    }
                    Ok(_) => {
                        consecutive_no_progress = consecutive_no_progress.saturating_add(1);
                        next_delay = recovery_retry_delay(consecutive_no_progress);
                    }
                    Err(_) => {
                        consecutive_no_progress = consecutive_no_progress.saturating_add(1);
                        next_delay = recovery_retry_delay(consecutive_no_progress);
                        tracing::warn!(
                            consecutive_no_progress,
                            retry_delay_secs = next_delay.as_secs(),
                            "federated Bark recovery driver step failed"
                        );
                    }
                }
            }
        })
    }
}

/// Pure next-step selection shared by startup, automatic work, and retries.
#[must_use]
pub fn select_recovery_step(
    projection: &FederationBarkProjectionSnapshot,
    controls: FederatedBarkMaintenanceControls,
    target_action_id: Option<FederationBarkActionId>,
) -> Option<FederatedBarkRecoveryStep> {
    let active = projection.actions.iter().find(|action| {
        target_action_id.is_none_or(|target| action.action_id == target)
            && matches!(
                action.phase,
                FederationBarkSemanticPhase::RefreshPlanned
                    | FederationBarkSemanticPhase::ExitPlanned
                    | FederationBarkSemanticPhase::ExitRegistered
                    | FederationBarkSemanticPhase::ExitPublished
                    | FederationBarkSemanticPhase::SigningAttemptAdvanced
            )
    });
    let Some(action) = active else {
        return (target_action_id.is_none() && controls.automatic_refresh)
            .then_some(FederatedBarkRecoveryStep::PlanRefresh);
    };
    match action.signing_retry_phase.unwrap_or(action.phase) {
        FederationBarkSemanticPhase::RefreshPlanned => {
            Some(FederatedBarkRecoveryStep::RefreshPlanned {
                action_id: action.action_id,
            })
        }
        FederationBarkSemanticPhase::ExitPlanned => Some(FederatedBarkRecoveryStep::ExitPlanned {
            action_id: action.action_id,
        }),
        FederationBarkSemanticPhase::ExitRegistered => {
            Some(FederatedBarkRecoveryStep::ExitRegistered {
                action_id: action.action_id,
            })
        }
        FederationBarkSemanticPhase::ExitPublished => {
            Some(FederatedBarkRecoveryStep::ExitPublished {
                action_id: action.action_id,
            })
        }
        _ => None,
    }
}

fn recovery_retry_delay(consecutive_failures: u32) -> Duration {
    let exponent = consecutive_failures.saturating_sub(1).min(6);
    Duration::from_secs(
        DRIVER_INTERVAL
            .as_secs()
            .saturating_mul(1_u64 << exponent)
            .min(DRIVER_RETRY_MAX.as_secs()),
    )
}

#[cfg(test)]
mod tests {
    use super::recovery_retry_delay;

    #[test]
    fn retry_delay_is_bounded() {
        assert_eq!(recovery_retry_delay(1).as_secs(), 5);
        assert_eq!(recovery_retry_delay(64).as_secs(), 300);
    }
}
