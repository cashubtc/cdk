//! Maintenance operations.

use super::{
    accepted_custody_attempt, accepted_maintenance_plan, accepted_send_transition, bark_digest,
    bark_operation, classify_maintenance_execution_error, exit_evidence_from_transition,
    exit_result_bytes, exit_transition, next_locator_index, refresh_completed_transition,
    refresh_evidence_from_transition, validate_maintenance_transition_binding,
    ExitTransitionAuthorization, FederatedBarkExitOutcome, FederatedBarkRefreshOutcome,
    FederatedBarkRuntime,
};
use crate::Error;
use bark::ark::ProtocolEncoding;
use bark::custody::VtxoCustody;
use bark::custody::VtxoKeyLocator;
use bark::federation::{
    FederationActionId, FederationExitPhase, FederationExitPlan, FederationMaintenanceInput,
    FederationRefreshExecution, FederationRefreshPlan, FederationRequestCommitment,
    FederationVtxoImport, FederationVtxoState as BarkFederationVtxoState,
};
use bitcoin::consensus::serialize as serialize_transaction;
use bitcoin::{FeeRate, ScriptBuf};
use cdk_common::federation::{FederationAllocatedAddressRef, FederationJournalEntry};
use cdk_federation_bark::{
    ark_server_commitment, federation_bark_exit_allocation_quote,
    federation_bark_refresh_candidates, federation_bark_signer_set_for_attempt,
    FederationBarkActionId, FederationBarkActionKind, FederationBarkActionPlan,
    FederationBarkActionRecord, FederationBarkActionTransition, FederationBarkCpfpPackageSet,
    FederationBarkCpfpPublication, FederationBarkDigest, FederationBarkExternalId,
    FederationBarkMemberObservation, FederationBarkObservationStatus, FederationBarkOperation,
    FederationBarkProjectionSnapshot, FederationBarkSemanticPhase, FederationBarkVtxoInput,
    FederationBarkVtxoLocator, FederationBarkVtxoOutput, FederationBarkVtxoRecord,
    FederationBarkVtxoState, FEDERATION_BARK_EXIT_FEE_RATE_SAT_PER_KWU,
    FEDERATION_BARK_OPERATION_VERSION, FEDERATION_MAX_BARK_REFRESH_INPUT_ARTIFACT_BYTES,
};

impl FederatedBarkRuntime {
    /// Build the deterministic earliest-expiry refresh plan once the local
    /// chain tip reaches its accepted refresh margin.
    pub async fn propose_refresh(
        &self,
        projection: &FederationBarkProjectionSnapshot,
    ) -> Result<Option<FederationBarkActionPlan>, Error> {
        let minimum_expiry = match projection
            .vtxos
            .iter()
            .filter(|record| record.state == FederationBarkVtxoState::Spendable)
            .map(|record| record.expiry_height)
            .min()
        {
            Some(expiry) => expiry,
            None => return Ok(None),
        };
        let records = federation_bark_refresh_candidates(projection);
        if records.is_empty() {
            let actual = projection
                .vtxos
                .iter()
                .filter(|record| {
                    record.state == FederationBarkVtxoState::Spendable
                        && record.expiry_height == minimum_expiry
                })
                .min_by_key(|record| record.vtxo_id)
                .map_or(0, |record| record.artifact.bytes.len());
            return Err(Error::MaintenanceAuthorizationTooLarge {
                actual,
                maximum: FEDERATION_MAX_BARK_REFRESH_INPUT_ARTIFACT_BYTES,
            });
        }
        let predecessor_operation_id = records
            .first()
            .ok_or(Error::MaintenancePlanInvalid)?
            .latest_operation_id;
        let application = self.bark_application();
        let trigger_height = minimum_expiry
            .checked_sub(application.policy.refresh_before_expiry_blocks)
            .filter(|height| *height != 0)
            .ok_or(Error::MaintenancePlanInvalid)?;
        let emergency_height = minimum_expiry
            .checked_sub(application.policy.emergency_exit_before_expiry_blocks)
            .filter(|height| *height > trigger_height)
            .ok_or(Error::MaintenancePlanInvalid)?;
        let action_id = FederationBarkActionId::derive(
            self.scope.federation_id(),
            predecessor_operation_id,
            FederationBarkSemanticPhase::RefreshPlanned,
            records
                .first()
                .ok_or(Error::MaintenancePlanInvalid)?
                .locator
                .index,
        )
        .map_err(|_| Error::MaintenancePlanInvalid)?;
        let bark_action_id = FederationActionId::from_bytes(action_id.to_bytes())
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        let output_index = next_locator_index(projection)?;
        let output_locator = FederationBarkVtxoLocator::vtxo_custody(output_index);
        let output_key = self
            .custody
            .derive_public_key(VtxoKeyLocator {
                index: output_locator.index,
            })
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        let bark_inputs = self.maintenance_inputs_from_records(&records)?;
        let preview = self
            .wallet
            .plan_refresh(
                bark_action_id,
                FederationRequestCommitment::from_bytes([1; 32])
                    .map_err(|_| Error::MaintenancePlanInvalid)?,
                bark_inputs,
                output_key.locator,
                trigger_height,
                emergency_height,
                1,
            )
            .await
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        let observation = self
            .wallet
            .observe_refresh_expiry(&preview)
            .await
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        if observation.tip_height < trigger_height {
            return Ok(None);
        }
        let inputs = records
            .iter()
            .map(|record| FederationBarkVtxoInput {
                vtxo_id: record.vtxo_id,
                locator: record.locator,
                amount: record.amount,
            })
            .collect::<Vec<_>>();
        let input_amount = cdk_common::Amount::from(preview.input_amount.to_sat());
        let fee = cdk_common::Amount::from(preview.fee.to_sat());
        let fee_bound = application
            .policy
            .maximum_outgoing_fee(input_amount)
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        if fee > fee_bound {
            return Err(Error::MaintenancePlanInvalid);
        }
        let actor = self
            .federation
            .members
            .keys()
            .next()
            .copied()
            .ok_or(Error::MaintenancePlanInvalid)?;
        let signer_set = self
            .federation
            .members
            .keys()
            .copied()
            .take(usize::from(application.threshold))
            .collect::<Vec<_>>();
        let mut plan = FederationBarkActionPlan {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id,
            quote: None,
            kind: FederationBarkActionKind::Refresh,
            actor,
            predecessor_operation_id,
            payment_hash: None,
            inputs,
            outputs: vec![FederationBarkVtxoOutput {
                locator: output_locator,
                amount: cdk_common::Amount::from(preview.output_amount.to_sat()),
                artifact_hash: FederationBarkDigest::from_bytes([1; 32])
                    .map_err(|_| Error::MaintenancePlanInvalid)?,
            }],
            revocation_locator: None,
            external_amount: cdk_common::Amount::ZERO,
            fee_amount: fee,
            fee_bound,
            certified_receive_htlcs: None,
            artifacts: Vec::new(),
            external_id: FederationBarkExternalId::derive_refresh(
                action_id,
                predecessor_operation_id,
                1,
            )
            .map_err(|_| Error::MaintenancePlanInvalid)?,
            signer_set,
            attempt: 1,
            expires_at: u64::from(emergency_height),
        };
        plan.bind_refresh_artifacts(
            trigger_height,
            minimum_expiry,
            records.iter().map(|record| record.artifact.clone()),
        )
        .map_err(|_| Error::MaintenancePlanInvalid)?;
        FederationBarkOperation::ActionPlan(plan.clone())
            .validate()
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        Ok(Some(plan))
    }

    /// Report that this member's chain view has crossed an accepted refresh or
    /// exit trigger. The exact plan commitment is the quorum result hash.
    pub async fn observe_maintenance_trigger(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan_entry: &FederationJournalEntry,
        observation_epoch: u64,
    ) -> Result<Option<FederationBarkMemberObservation>, Error> {
        if observation_epoch == 0 {
            return Err(Error::MaintenanceReconciliationFailed);
        }
        let Some(FederationBarkOperation::ActionPlan(plan)) = bark_operation(plan_entry) else {
            return Err(Error::MaintenancePlanInvalid);
        };
        let (tip_height, trigger_height, amount) = match plan.kind {
            FederationBarkActionKind::Refresh => {
                let bark_plan = self.bark_refresh_plan(projection, &plan)?;
                let observation = self
                    .wallet
                    .observe_refresh_expiry(&bark_plan)
                    .await
                    .map_err(|_| Error::MaintenanceReconciliationFailed)?;
                let amount =
                    plan.inputs
                        .iter()
                        .try_fold(cdk_common::Amount::ZERO, |sum, input| {
                            sum.checked_add(input.amount)
                                .ok_or(Error::MaintenanceReconciliationFailed)
                        })?;
                (observation.tip_height, observation.trigger_height, amount)
            }
            FederationBarkActionKind::EmergencyExit => {
                let bark_plan = self.bark_exit_plan(projection, &plan)?;
                let observation = self
                    .wallet
                    .observe_exit_expiry(&bark_plan)
                    .await
                    .map_err(|_| Error::MaintenanceReconciliationFailed)?;
                (
                    observation.tip_height,
                    observation.trigger_height,
                    plan.external_amount,
                )
            }
            _ => return Err(Error::MaintenancePlanInvalid),
        };
        if tip_height < trigger_height {
            return Ok(None);
        }
        let commitment = FederationBarkOperation::ActionPlan(plan.clone())
            .commitment()
            .map_err(|_| Error::MaintenanceReconciliationFailed)?;
        Ok(Some(FederationBarkMemberObservation {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: plan.action_id,
            quote: None,
            observer: self.member_id,
            observation_epoch,
            source_scope: ark_server_commitment(&self.bark_application().ark_server_public_key)
                .map_err(|_| Error::MaintenanceReconciliationFailed)?,
            payment_hash: None,
            status: FederationBarkObservationStatus::Accepted,
            amount,
            fee: plan.fee_amount,
            result_hash: commitment,
        }))
    }

    /// Return whether an accepted refresh has crossed its immutable emergency
    /// escalation height in this member's chain view.
    pub async fn emergency_exit_due(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        refresh_plan_entry: &FederationJournalEntry,
    ) -> Result<bool, Error> {
        let plan =
            accepted_maintenance_plan(refresh_plan_entry, FederationBarkActionKind::Refresh)?;
        let bark_plan = self.bark_refresh_plan(projection, &plan)?;
        let observation = self
            .wallet
            .observe_refresh_expiry(&bark_plan)
            .await
            .map_err(|_| Error::MaintenanceReconciliationFailed)?;
        Ok(observation.tip_height >= observation.emergency_height)
    }

    /// Apply or replay an accepted refresh plan without entering a round.
    pub async fn apply_refresh_plan(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan: &FederationBarkActionPlan,
    ) -> Result<(), Error> {
        self.apply_refresh_plan_for_attempt(projection, plan, plan.attempt)
            .await
            .map(|_| ())
    }

    async fn apply_refresh_plan_for_attempt(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan: &FederationBarkActionPlan,
        custody_attempt: u32,
    ) -> Result<FederationRefreshPlan, Error> {
        let mut bark_plan = self.bark_refresh_plan(projection, plan)?;
        bark_plan.custody_attempt = custody_attempt;
        for input in &bark_plan.inputs {
            self.wallet
                .import_vtxo(&FederationVtxoImport {
                    locator: input.locator,
                    vtxo: input.vtxo.clone(),
                    state: BarkFederationVtxoState::Spendable,
                })
                .await
                .map_err(|_| Error::MaintenanceProjectionFailed)?;
        }
        self.wallet
            .apply_refresh_plan(&bark_plan)
            .await
            .map_err(|_| Error::MaintenanceProjectionFailed)?;
        Ok(bark_plan)
    }

    /// Execute only the exact accepted refresh round.
    pub async fn execute_refresh(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        accepted_authorization: &FederationJournalEntry,
    ) -> Result<FederatedBarkRefreshOutcome, Error> {
        let plan = accepted_maintenance_plan(accepted_plan, FederationBarkActionKind::Refresh)?;
        let custody_attempt = accepted_custody_attempt(accepted_plan, accepted_authorization)?;
        if custody_attempt.actor != self.member_id {
            return Err(Error::MaintenanceExecutionFailed);
        }
        let bark_plan = self
            .apply_refresh_plan_for_attempt(projection, &plan, custody_attempt.attempt)
            .await?;
        let operation = FederationBarkOperation::ActionPlan(plan.clone());
        let commitment = operation
            .commitment()
            .map_err(|_| Error::MaintenanceExecutionFailed)?;
        self.custody
            .activate_refresh(
                accepted_plan.clone(),
                custody_attempt.entry.clone(),
                commitment.to_bytes().to_vec(),
            )
            .map_err(|_| Error::MaintenanceExecutionFailed)?;
        let execution = self.wallet.execute_refresh(&bark_plan).await;
        let signer_unavailable = self.custody.take_signer_unavailable();
        self.custody.deactivate_refresh();
        match execution
            .map_err(|error| classify_maintenance_execution_error(error, signer_unavailable))?
        {
            FederationRefreshExecution::Pending { .. } => Ok(FederatedBarkRefreshOutcome::Pending),
            FederationRefreshExecution::Completed(evidence) => {
                let transition = refresh_completed_transition(
                    self.member_id,
                    accepted_plan,
                    &plan,
                    projection,
                    &evidence,
                    commitment,
                    &custody_attempt,
                )?;
                Ok(FederatedBarkRefreshOutcome::Completed(Box::new(transition)))
            }
        }
    }

    /// Apply a consensus-accepted confirmed refresh result locally.
    pub async fn apply_refresh_completed(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan_entry: &FederationJournalEntry,
        completed_entry: &FederationJournalEntry,
    ) -> Result<(), Error> {
        let plan = accepted_maintenance_plan(plan_entry, FederationBarkActionKind::Refresh)?;
        let transition = accepted_send_transition(
            completed_entry,
            FederationBarkSemanticPhase::RefreshCompleted,
        )?;
        validate_maintenance_transition_binding(&self.federation, plan_entry, &plan, &transition)?;
        let bark_plan = self.bark_refresh_plan(projection, &plan)?;
        let evidence = refresh_evidence_from_transition(&plan, &transition)?;
        self.wallet
            .apply_refresh_result(&bark_plan, &evidence)
            .await
            .map_err(|_| Error::MaintenanceReconciliationFailed)
    }

    fn bark_refresh_plan(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan: &FederationBarkActionPlan,
    ) -> Result<FederationRefreshPlan, Error> {
        FederationBarkOperation::ActionPlan(plan.clone())
            .validate()
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        if plan.kind != FederationBarkActionKind::Refresh || plan.outputs.len() != 1 {
            return Err(Error::MaintenancePlanInvalid);
        }
        let request = plan
            .refresh_request()
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        let mut records = Vec::with_capacity(plan.inputs.len());
        for input in &plan.inputs {
            let record = projection
                .vtxos
                .iter()
                .find(|record| record.vtxo_id == input.vtxo_id)
                .ok_or(Error::MaintenancePlanInvalid)?;
            if record.locator != input.locator
                || record.amount != input.amount
                || !matches!(
                    record.state,
                    FederationBarkVtxoState::Spendable
                        | FederationBarkVtxoState::Reserved
                        | FederationBarkVtxoState::Spent
                )
                || (record.state == FederationBarkVtxoState::Reserved
                    && record.reservation_action_id != Some(plan.action_id))
            {
                return Err(Error::MaintenancePlanInvalid);
            }
            records.push(record);
        }
        records.sort_by_key(|record| record.vtxo_id);
        let inputs = self.maintenance_inputs_from_records(&records)?;
        let output = plan.outputs.first().ok_or(Error::MaintenancePlanInvalid)?;
        let output_key = self
            .custody
            .derive_public_key(VtxoKeyLocator {
                index: output.locator.index,
            })
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        let commitment = FederationBarkOperation::ActionPlan(plan.clone())
            .commitment()
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        Ok(FederationRefreshPlan {
            action_id: FederationActionId::from_bytes(plan.action_id.to_bytes())
                .map_err(|_| Error::MaintenancePlanInvalid)?,
            request_commitment: FederationRequestCommitment::from_bytes(commitment.to_bytes())
                .map_err(|_| Error::MaintenancePlanInvalid)?,
            inputs,
            input_amount: bitcoin::Amount::from_sat(
                plan.inputs
                    .iter()
                    .map(|input| u64::from(input.amount))
                    .try_fold(0_u64, u64::checked_add)
                    .ok_or(Error::MaintenancePlanInvalid)?,
            ),
            output_key,
            output_amount: bitcoin::Amount::from_sat(u64::from(output.amount)),
            fee: bitcoin::Amount::from_sat(u64::from(plan.fee_amount)),
            trigger_height: request.trigger_height,
            emergency_height: request.emergency_height,
            minimum_expiry: request.minimum_expiry,
            custody_attempt: plan.attempt,
        })
    }

    /// Escalate one accepted refresh reservation into an exact federated BDK
    /// emergency-exit plan. No exit registration or signing occurs here.
    pub fn propose_emergency_exit(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        action: &FederationBarkActionRecord,
        refresh_plan_entry: &FederationJournalEntry,
        destination: FederationAllocatedAddressRef,
        destination_script: Vec<u8>,
        minimum_confirmations: u32,
    ) -> Result<FederationBarkActionPlan, Error> {
        let refresh =
            accepted_maintenance_plan(refresh_plan_entry, FederationBarkActionKind::Refresh)?;
        let effective_phase = action.signing_retry_phase.unwrap_or(action.phase);
        let signer_set = federation_bark_signer_set_for_attempt(&self.federation, action.attempt)
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        if action.action_id != refresh.action_id
            || action.action_kind != Some(FederationBarkActionKind::Refresh)
            || effective_phase != FederationBarkSemanticPhase::RefreshPlanned
            || action.plan_operation_id != Some(refresh_plan_entry.operation_id)
            || action.attempt < refresh.attempt
            || destination.quote != federation_bark_exit_allocation_quote(refresh.action_id)
            || minimum_confirmations == 0
        {
            return Err(Error::MaintenancePlanInvalid);
        }
        let mut records = Vec::with_capacity(refresh.inputs.len());
        for input in &refresh.inputs {
            let record = projection
                .vtxos
                .iter()
                .find(|record| record.vtxo_id == input.vtxo_id)
                .ok_or(Error::MaintenancePlanInvalid)?;
            if record.state != FederationBarkVtxoState::Reserved
                || record.reservation_action_id != Some(refresh.action_id)
                || record.locator != input.locator
                || record.amount != input.amount
            {
                return Err(Error::MaintenancePlanInvalid);
            }
            records.push(record);
        }
        records.sort_by_key(|record| record.vtxo_id);
        let bark_inputs = self.maintenance_inputs_from_records(&records)?;
        let input_amount =
            refresh
                .inputs
                .iter()
                .try_fold(cdk_common::Amount::ZERO, |sum, input| {
                    sum.checked_add(input.amount)
                        .ok_or(Error::MaintenancePlanInvalid)
                })?;
        let fee_bound = self
            .bark_application()
            .policy
            .maximum_outgoing_fee(input_amount)
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        let minimum_recovered = input_amount
            .checked_sub(fee_bound)
            .ok_or(Error::MaintenancePlanInvalid)?;
        let minimum_expiry = records
            .iter()
            .map(|record| record.expiry_height)
            .min()
            .ok_or(Error::MaintenancePlanInvalid)?;
        let trigger_height = minimum_expiry
            .checked_sub(
                self.bark_application()
                    .policy
                    .emergency_exit_before_expiry_blocks,
            )
            .filter(|height| *height != 0)
            .ok_or(Error::MaintenancePlanInvalid)?;
        let fee_rate = FeeRate::from_sat_per_kwu(FEDERATION_BARK_EXIT_FEE_RATE_SAT_PER_KWU);
        let bark_plan = self
            .wallet
            .plan_exit(
                FederationActionId::from_bytes(refresh.action_id.to_bytes())
                    .map_err(|_| Error::MaintenancePlanInvalid)?,
                FederationRequestCommitment::from_bytes([1; 32])
                    .map_err(|_| Error::MaintenancePlanInvalid)?,
                bark_inputs.clone(),
                ScriptBuf::from_bytes(destination_script.clone()),
                fee_rate,
                bitcoin::Amount::from_sat(u64::from(fee_bound)),
                trigger_height,
                minimum_confirmations,
                action.attempt,
            )
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        if bark_plan.minimum_recovered.to_sat() != u64::from(minimum_recovered)
            || bark_plan.minimum_expiry != minimum_expiry
        {
            return Err(Error::MaintenancePlanInvalid);
        }
        let mut exit_transactions = Vec::new();
        for input in &bark_inputs {
            exit_transactions.extend(
                input
                    .vtxo
                    .transactions()
                    .map(|transaction| serialize_transaction(&transaction.tx)),
            );
        }
        let mut plan = FederationBarkActionPlan {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: refresh.action_id,
            quote: None,
            kind: FederationBarkActionKind::EmergencyExit,
            actor: action.actor,
            predecessor_operation_id: action.latest_operation_id,
            payment_hash: None,
            inputs: refresh.inputs.clone(),
            outputs: Vec::new(),
            revocation_locator: None,
            external_amount: minimum_recovered,
            fee_amount: fee_bound,
            fee_bound,
            certified_receive_htlcs: None,
            artifacts: Vec::new(),
            external_id: FederationBarkExternalId::derive_emergency_exit(
                refresh.action_id,
                action.latest_operation_id,
                action.attempt,
            )
            .map_err(|_| Error::MaintenancePlanInvalid)?,
            signer_set,
            attempt: action.attempt,
            expires_at: u64::from(minimum_expiry),
        };
        plan.bind_emergency_exit_artifacts(
            destination,
            destination_script,
            self.network(),
            FEDERATION_BARK_EXIT_FEE_RATE_SAT_PER_KWU,
            trigger_height,
            minimum_confirmations,
            records.iter().map(|record| record.artifact.clone()),
            exit_transactions,
        )
        .map_err(|_| Error::MaintenancePlanInvalid)?;
        FederationBarkOperation::ActionPlan(plan.clone())
            .validate()
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        Ok(plan)
    }

    /// Apply or replay an accepted emergency-exit plan without registration,
    /// signing, or publication.
    pub async fn apply_emergency_exit_plan(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan: &FederationBarkActionPlan,
    ) -> Result<(), Error> {
        let bark_plan = self.bark_exit_plan(projection, plan)?;
        for input in &bark_plan.inputs {
            self.wallet
                .import_vtxo(&FederationVtxoImport {
                    locator: input.locator,
                    vtxo: input.vtxo.clone(),
                    state: BarkFederationVtxoState::Spendable,
                })
                .await
                .map_err(|_| Error::MaintenanceProjectionFailed)?;
        }
        self.wallet
            .apply_exit_plan(&bark_plan)
            .await
            .map_err(|_| Error::MaintenanceProjectionFailed)?;
        Ok(())
    }

    /// Register the exact VTXOs from an accepted emergency-exit plan.
    pub async fn register_emergency_exit(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        action: &FederationBarkActionRecord,
        accepted_plan: &FederationJournalEntry,
        package_set: &FederationBarkCpfpPackageSet,
    ) -> Result<FederationBarkActionTransition, Error> {
        let plan =
            accepted_maintenance_plan(accepted_plan, FederationBarkActionKind::EmergencyExit)?;
        if action.action_id != plan.action_id
            || action.actor != self.member_id
            || action.plan_operation_id != Some(accepted_plan.operation_id)
        {
            return Err(Error::MaintenanceExecutionFailed);
        }
        self.apply_emergency_exit_plan(projection, &plan).await?;
        let bark_plan = self.bark_exit_plan(projection, &plan)?;
        let mut evidence = self
            .wallet
            .register_exit(&bark_plan)
            .await
            .map_err(|_| Error::MaintenanceExecutionFailed)?;
        if !matches!(
            evidence.phase,
            FederationExitPhase::Registered
                | FederationExitPhase::Published
                | FederationExitPhase::Claimable
                | FederationExitPhase::DrainPublished
                | FederationExitPhase::Confirmed
        ) {
            return Err(Error::MaintenanceExecutionFailed);
        }
        evidence.phase = FederationExitPhase::Registered;
        evidence.drain_transaction = None;
        evidence.confirmed_in = None;
        if package_set.action_id != plan.action_id
            || package_set.plan_operation_id != accepted_plan.operation_id
            || package_set.actor != action.actor
            || package_set.attempt != action.attempt
            || package_set.signer_set
                != federation_bark_signer_set_for_attempt(&self.federation, action.attempt)
                    .map_err(|_| Error::MaintenanceExecutionFailed)?
        {
            return Err(Error::MaintenanceExecutionFailed);
        }
        let mut transition = exit_transition(
            self.member_id,
            accepted_plan,
            accepted_plan.operation_id,
            &plan,
            projection,
            &evidence,
            ExitTransitionAuthorization {
                phase: FederationBarkSemanticPhase::ExitRegistered,
                attempt: package_set.attempt,
                signer_set: &package_set.signer_set,
            },
        )?;
        transition.artifacts.push(
            package_set
                .artifact()
                .map_err(|_| Error::MaintenanceExecutionFailed)?,
        );
        transition.artifacts.sort();
        transition.artifacts.dedup();
        FederationBarkOperation::ActionTransition(transition.clone())
            .validate()
            .map_err(|_| Error::MaintenanceExecutionFailed)?;
        Ok(transition)
    }

    /// Replay an accepted exit registration into member-local tracking without
    /// progressing or publishing any transaction.
    pub async fn apply_emergency_exit_registered(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan_entry: &FederationJournalEntry,
        registered_entry: &FederationJournalEntry,
    ) -> Result<(), Error> {
        let plan = accepted_maintenance_plan(plan_entry, FederationBarkActionKind::EmergencyExit)?;
        let registered = accepted_send_transition(
            registered_entry,
            FederationBarkSemanticPhase::ExitRegistered,
        )?;
        validate_maintenance_transition_binding(&self.federation, plan_entry, &plan, &registered)?;
        self.apply_emergency_exit_plan(projection, &plan).await?;
        let bark_plan = self.bark_exit_plan(projection, &plan)?;
        self.wallet
            .register_exit(&bark_plan)
            .await
            .map_err(|_| Error::MaintenanceProjectionFailed)?;
        Ok(())
    }

    /// Progress an accepted registered exit and publish its deterministic
    /// final drain once every exact input is claimable.
    pub async fn publish_emergency_exit(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        registered_entry: &FederationJournalEntry,
        accepted_authorization: &FederationJournalEntry,
        package_set: &FederationBarkCpfpPackageSet,
        publication: &FederationBarkCpfpPublication,
    ) -> Result<FederatedBarkExitOutcome, Error> {
        let plan =
            accepted_maintenance_plan(accepted_plan, FederationBarkActionKind::EmergencyExit)?;
        let custody_attempt = accepted_custody_attempt(accepted_plan, accepted_authorization)?;
        if custody_attempt.actor != self.member_id {
            return Err(Error::MaintenanceExecutionFailed);
        }
        let registered = accepted_send_transition(
            registered_entry,
            FederationBarkSemanticPhase::ExitRegistered,
        )?;
        validate_maintenance_transition_binding(
            &self.federation,
            accepted_plan,
            &plan,
            &registered,
        )?;
        self.apply_emergency_exit_registered(projection, accepted_plan, registered_entry)
            .await?;
        let bark_plan = self.bark_exit_plan(projection, &plan)?;
        self.custody
            .activate_exit(
                accepted_plan.clone(),
                custody_attempt.entry.clone(),
                registered.request_hash.to_bytes().to_vec(),
            )
            .map_err(|_| Error::MaintenanceExecutionFailed)?;
        let result = async {
            self.wallet.progress_exit(&bark_plan).await?;
            self.provide_accepted_cpfp_packages(package_set, publication)
                .await?;
            let evidence = self.wallet.progress_exit(&bark_plan).await?;
            match evidence.phase {
                FederationExitPhase::Claimable
                | FederationExitPhase::DrainPublished
                | FederationExitPhase::Confirmed => {
                    let drain = self.wallet.build_exit_drain(&bark_plan).await?;
                    self.wallet.publish_exit_drain(&bark_plan, &drain).await
                }
                FederationExitPhase::Planned
                | FederationExitPhase::Registered
                | FederationExitPhase::Published => Ok(evidence),
            }
        }
        .await;
        let signer_unavailable = self.custody.take_signer_unavailable();
        self.custody.deactivate_exit();
        let evidence = result
            .map_err(|error| classify_maintenance_execution_error(error, signer_unavailable))?;
        if !matches!(
            evidence.phase,
            FederationExitPhase::DrainPublished | FederationExitPhase::Confirmed
        ) {
            return Ok(FederatedBarkExitOutcome::Pending);
        }
        let mut transition = exit_transition(
            self.member_id,
            accepted_plan,
            if custody_attempt.entry.operation_id == accepted_plan.operation_id {
                registered_entry.operation_id
            } else {
                custody_attempt.entry.operation_id
            },
            &plan,
            projection,
            &evidence,
            ExitTransitionAuthorization {
                phase: FederationBarkSemanticPhase::ExitPublished,
                attempt: custody_attempt.attempt,
                signer_set: &custody_attempt.signer_set,
            },
        )?;
        transition.artifacts.push(
            publication
                .artifact()
                .map_err(|_| Error::MaintenanceExecutionFailed)?,
        );
        transition.artifacts.sort();
        transition.artifacts.dedup();
        FederationBarkOperation::ActionTransition(transition.clone())
            .validate()
            .map_err(|_| Error::MaintenanceExecutionFailed)?;
        Ok(FederatedBarkExitOutcome::Published(Box::new(transition)))
    }

    /// Observe confirmation of the exact accepted drain transaction.
    pub async fn observe_emergency_exit_confirmation(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan_entry: &FederationJournalEntry,
        registered_entry: &FederationJournalEntry,
        published_entry: &FederationJournalEntry,
        observation_epoch: u64,
    ) -> Result<Option<FederationBarkMemberObservation>, Error> {
        if observation_epoch == 0 {
            return Err(Error::MaintenanceReconciliationFailed);
        }
        let plan = accepted_maintenance_plan(plan_entry, FederationBarkActionKind::EmergencyExit)?;
        let published =
            accepted_send_transition(published_entry, FederationBarkSemanticPhase::ExitPublished)?;
        validate_maintenance_transition_binding(&self.federation, plan_entry, &plan, &published)?;
        let accepted_evidence = exit_evidence_from_transition(&plan, &published)?;
        let drain = accepted_evidence
            .drain_transaction
            .as_ref()
            .ok_or(Error::MaintenanceReconciliationFailed)?;
        self.apply_emergency_exit_registered(projection, plan_entry, registered_entry)
            .await?;
        let bark_plan = self.bark_exit_plan(projection, &plan)?;
        let evidence = self
            .wallet
            .observe_exit(&bark_plan, Some(drain))
            .await
            .map_err(|_| Error::MaintenanceReconciliationFailed)?;
        if evidence.phase != FederationExitPhase::Confirmed {
            return Ok(None);
        }
        let result_hash = bark_digest(&exit_result_bytes(&evidence)?)?;
        Ok(Some(FederationBarkMemberObservation {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: plan.action_id,
            quote: None,
            observer: self.member_id,
            observation_epoch,
            source_scope: ark_server_commitment(&self.bark_application().ark_server_public_key)
                .map_err(|_| Error::MaintenanceReconciliationFailed)?,
            payment_hash: None,
            status: FederationBarkObservationStatus::Succeeded,
            amount: plan.external_amount,
            fee: plan.fee_amount,
            result_hash,
        }))
    }

    /// Build the terminal exit transition after confirmation observations have
    /// reached consensus threshold.
    pub async fn confirm_emergency_exit(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        action: &FederationBarkActionRecord,
        plan_entry: &FederationJournalEntry,
        registered_entry: &FederationJournalEntry,
        published_entry: &FederationJournalEntry,
    ) -> Result<Option<FederationBarkActionTransition>, Error> {
        let plan = accepted_maintenance_plan(plan_entry, FederationBarkActionKind::EmergencyExit)?;
        if action.action_id != plan.action_id
            || action.actor != self.member_id
            || action.signing_retry_phase.unwrap_or(action.phase)
                != FederationBarkSemanticPhase::ExitPublished
            || action.plan_operation_id != Some(plan_entry.operation_id)
        {
            return Err(Error::MaintenanceReconciliationFailed);
        }
        let published =
            accepted_send_transition(published_entry, FederationBarkSemanticPhase::ExitPublished)?;
        let accepted_evidence = exit_evidence_from_transition(&plan, &published)?;
        let drain = accepted_evidence
            .drain_transaction
            .as_ref()
            .ok_or(Error::MaintenanceReconciliationFailed)?;
        self.apply_emergency_exit_registered(projection, plan_entry, registered_entry)
            .await?;
        let bark_plan = self.bark_exit_plan(projection, &plan)?;
        let evidence = self
            .wallet
            .observe_exit(&bark_plan, Some(drain))
            .await
            .map_err(|_| Error::MaintenanceReconciliationFailed)?;
        if evidence.phase != FederationExitPhase::Confirmed {
            return Ok(None);
        }
        exit_transition(
            self.member_id,
            plan_entry,
            action.latest_operation_id,
            &plan,
            projection,
            &evidence,
            ExitTransitionAuthorization {
                phase: FederationBarkSemanticPhase::ExitConfirmed,
                attempt: action.attempt,
                signer_set: &federation_bark_signer_set_for_attempt(
                    &self.federation,
                    action.attempt,
                )
                .map_err(|_| Error::MaintenanceReconciliationFailed)?,
            },
        )
        .map(Some)
    }

    fn bark_exit_plan(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan: &FederationBarkActionPlan,
    ) -> Result<FederationExitPlan, Error> {
        FederationBarkOperation::ActionPlan(plan.clone())
            .validate()
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        if plan.kind != FederationBarkActionKind::EmergencyExit {
            return Err(Error::MaintenancePlanInvalid);
        }
        let request = plan
            .emergency_exit_request()
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        let mut records = Vec::with_capacity(plan.inputs.len());
        for input in &plan.inputs {
            let record = projection
                .vtxos
                .iter()
                .find(|record| record.vtxo_id == input.vtxo_id)
                .ok_or(Error::MaintenancePlanInvalid)?;
            if record.locator != input.locator
                || record.amount != input.amount
                || !matches!(
                    record.state,
                    FederationBarkVtxoState::Reserved
                        | FederationBarkVtxoState::Exiting
                        | FederationBarkVtxoState::Exited
                )
                || record
                    .reservation_action_id
                    .is_some_and(|action| action != plan.action_id)
            {
                return Err(Error::MaintenancePlanInvalid);
            }
            records.push(record);
        }
        records.sort_by_key(|record| record.vtxo_id);
        let inputs = self.maintenance_inputs_from_records(&records)?;
        let commitment = FederationBarkOperation::ActionPlan(plan.clone())
            .commitment()
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        Ok(FederationExitPlan {
            action_id: FederationActionId::from_bytes(plan.action_id.to_bytes())
                .map_err(|_| Error::MaintenancePlanInvalid)?,
            request_commitment: FederationRequestCommitment::from_bytes(commitment.to_bytes())
                .map_err(|_| Error::MaintenancePlanInvalid)?,
            inputs,
            input_amount: bitcoin::Amount::from_sat(
                plan.inputs
                    .iter()
                    .map(|input| u64::from(input.amount))
                    .try_fold(0_u64, u64::checked_add)
                    .ok_or(Error::MaintenancePlanInvalid)?,
            ),
            destination_script: ScriptBuf::from_bytes(request.destination_script),
            network: request.network,
            fee_rate: FeeRate::from_sat_per_kwu(request.fee_rate_sat_per_kwu),
            fee_bound: bitcoin::Amount::from_sat(u64::from(request.fee_bound)),
            minimum_recovered: bitcoin::Amount::from_sat(u64::from(request.minimum_recovered)),
            trigger_height: request.trigger_height,
            minimum_expiry: request.minimum_expiry,
            minimum_confirmations: request.minimum_confirmations,
            custody_attempt: plan.attempt,
        })
    }

    fn maintenance_inputs_from_records(
        &self,
        records: &[&FederationBarkVtxoRecord],
    ) -> Result<Vec<FederationMaintenanceInput>, Error> {
        records
            .iter()
            .map(|record| {
                let vtxo = bark::ark::vtxo::Vtxo::<bark::ark::vtxo::Full>::deserialize(
                    &record.artifact.bytes,
                )
                .map_err(|_| Error::MaintenancePlanInvalid)?;
                if vtxo.serialize() != record.artifact.bytes
                    || vtxo.id().to_bytes() != record.vtxo_id.to_bytes()
                    || vtxo.amount().to_sat() != u64::from(record.amount)
                    || vtxo.expiry_height() != record.expiry_height
                    || vtxo.server_pubkey().serialize()
                        != self.bark_application().ark_server_public_key
                {
                    return Err(Error::MaintenancePlanInvalid);
                }
                Ok(FederationMaintenanceInput {
                    locator: VtxoKeyLocator {
                        index: record.locator.index,
                    },
                    vtxo,
                })
            })
            .collect()
    }
}
