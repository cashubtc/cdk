//! Outgoing operations.

use bitcoin::hashes::Hash;

use super::{
    accepted_custody_attempt, accepted_revocation_plan, accepted_send_plan,
    accepted_send_transition, bark_artifact, bark_digest, classify_send_execution_error,
    failed_send_revocation_request_bytes, payment_failed_result, payment_succeeded_transition,
    revocation_completed_transition, revocation_evidence_from_transition,
    send_htlc_evidence_from_transition, send_htlc_transition, send_success_preimage,
    validate_payment_failed_result, validate_send_transition_binding, FederatedBarkRuntime,
    FederatedBarkSendProposal, FederatedBarkSendTerminalStatus, BARK_PAYMENT_UPDATE_WAIT,
};
use crate::Error;
use bark::ark::lightning::Invoice;
use bark::ark::ProtocolEncoding;
use bark::custody::VtxoKeyLocator;
use bark::federation::{
    FederationActionId, FederationLightningSendCandidate, FederationLightningSendPlan,
    FederationLightningSendRevocationPlan, FederationLightningSendStatus,
    FederationRequestCommitment, FederationSendInput, FederationVtxoImport,
    FederationVtxoState as BarkFederationVtxoState,
};
use cdk_common::federation::{
    FederationJournalEntry, FederationOperationId, FEDERATION_MAX_DERIVATION_INDEX,
};
use cdk_common::mint::MeltQuote;
use cdk_common::MeltQuoteState;
use cdk_federation_bark::{
    ark_server_commitment, validate_federation_bark_lightning_send_expiry, FederationBarkActionId,
    FederationBarkActionKind, FederationBarkActionPlan, FederationBarkActionTransition,
    FederationBarkArtifactKind, FederationBarkDigest, FederationBarkExternalId,
    FederationBarkMemberObservation, FederationBarkObservationStatus, FederationBarkOperation,
    FederationBarkProjectionSnapshot, FederationBarkSemanticPhase, FederationBarkVtxoInput,
    FederationBarkVtxoLocator, FederationBarkVtxoOutput, FederationBarkVtxoState,
    FEDERATION_BARK_OPERATION_VERSION,
};

impl FederatedBarkRuntime {
    /// Select exact spendable VTXOs and construct a complete outgoing plan
    /// using only accepted public state. This performs no database mutation,
    /// chain lookup, custody operation, clock read, or Ark request.
    pub fn propose_lightning_send(
        &self,
        quote: &MeltQuote,
        predecessor_operation_id: FederationOperationId,
        projection: &FederationBarkProjectionSnapshot,
        mut proposal: FederatedBarkSendProposal,
    ) -> Result<FederationBarkActionPlan, Error> {
        if quote.state != MeltQuoteState::Pending {
            return Err(Error::SendPlanInvalid);
        }
        let (invoice, amount, fee) = self.validate_send_quote(quote)?;
        if proposal.attempt == 0 || !self.federation.members.contains_key(&proposal.actor) {
            return Err(Error::SendPlanInvalid);
        }
        proposal.signer_set.sort();
        proposal.signer_set.dedup();
        let application = self.bark_application();
        if proposal.signer_set.len() < usize::from(application.threshold)
            || proposal
                .signer_set
                .iter()
                .any(|member| !self.federation.members.contains_key(member))
        {
            return Err(Error::SendPlanInvalid);
        }

        let target = amount.checked_add(fee).ok_or(Error::SendPlanInvalid)?;
        let mut selected = Vec::new();
        let mut selected_amount = cdk_common::Amount::ZERO;
        let mut minimum_expiry = u32::MAX;
        for vtxo in projection
            .vtxos
            .iter()
            .filter(|vtxo| vtxo.state == FederationBarkVtxoState::Spendable)
        {
            selected.push(FederationBarkVtxoInput {
                vtxo_id: vtxo.vtxo_id,
                locator: vtxo.locator,
                amount: vtxo.amount,
            });
            selected_amount = selected_amount
                .checked_add(vtxo.amount)
                .ok_or(Error::SendPlanInvalid)?;
            minimum_expiry = minimum_expiry.min(vtxo.expiry_height);
            if selected_amount >= target {
                break;
            }
        }
        if selected_amount < target || selected.is_empty() {
            return Err(Error::InsufficientAcceptedBalance);
        }
        selected.sort();
        validate_federation_bark_lightning_send_expiry(minimum_expiry, proposal.expires_at)
            .map_err(|_| Error::SendPlanInvalid)?;
        let output_index = projection
            .derivation_allocations
            .iter()
            .map(|allocation| allocation.locator.index)
            .max()
            .map_or(Ok(0), |index| {
                index.checked_add(1).ok_or(Error::SendPlanInvalid)
            })?;
        let revocation_index = output_index
            .checked_add(1)
            .filter(|index| *index <= FEDERATION_MAX_DERIVATION_INDEX)
            .ok_or(Error::SendPlanInvalid)?;
        let output_locator = FederationBarkVtxoLocator::vtxo_custody(output_index);
        let revocation_locator = FederationBarkVtxoLocator::vtxo_custody(revocation_index);
        let action_id = FederationBarkActionId::derive(
            self.scope.federation_id(),
            predecessor_operation_id,
            FederationBarkSemanticPhase::SendPlanned,
            0,
        )
        .map_err(|_| Error::SendPlanInvalid)?;
        let external_id = FederationBarkExternalId::derive_lightning_send(
            action_id,
            predecessor_operation_id,
            proposal.attempt,
        )
        .map_err(|_| Error::SendPlanInvalid)?;
        let payment_hash =
            FederationBarkDigest::from_bytes(*invoice.payment_hash().as_byte_array())
                .map_err(|_| Error::SendPlanInvalid)?;
        let change = selected_amount
            .checked_sub(target)
            .ok_or(Error::SendPlanInvalid)?;
        let mut plan = FederationBarkActionPlan {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id,
            quote: Some(quote.id.to_string()),
            kind: FederationBarkActionKind::LightningSend,
            actor: proposal.actor,
            predecessor_operation_id,
            payment_hash: Some(payment_hash),
            inputs: selected,
            outputs: vec![FederationBarkVtxoOutput {
                locator: output_locator,
                amount: change,
                artifact_hash: FederationBarkDigest::from_bytes([1; 32])
                    .map_err(|_| Error::SendPlanInvalid)?,
            }],
            revocation_locator: Some(revocation_locator),
            external_amount: amount,
            fee_amount: fee,
            fee_bound: fee,
            certified_receive_htlcs: None,
            artifacts: Vec::new(),
            external_id,
            signer_set: proposal.signer_set,
            attempt: proposal.attempt,
            expires_at: u64::from(proposal.expires_at),
        };
        plan.bind_lightning_send_artifacts(invoice)
            .map_err(|_| Error::SendPlanInvalid)?;
        FederationBarkOperation::ActionPlan(plan.clone())
            .validate()
            .map_err(|_| Error::SendPlanInvalid)?;
        Ok(plan)
    }

    /// Reconstruct Bark's exact member-local checkpoint from one accepted
    /// outgoing plan. Inputs may still be spendable during proposal validation
    /// or already reserved by this action during replay.
    pub fn validate_lightning_send_plan(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        plan: &FederationBarkActionPlan,
    ) -> Result<FederationLightningSendPlan, Error> {
        let (invoice, amount, fee) = self.validate_send_quote(quote)?;
        let operation = FederationBarkOperation::ActionPlan(plan.clone());
        operation.validate().map_err(|_| Error::SendPlanInvalid)?;
        if plan.kind != FederationBarkActionKind::LightningSend
            || plan.quote.as_deref() != Some(quote.id.to_string().as_str())
            || plan.external_amount != amount
            || plan.fee_amount != fee
            || plan.fee_bound != fee
            || plan.payment_hash
                != Some(
                    FederationBarkDigest::from_bytes(*invoice.payment_hash().as_byte_array())
                        .map_err(|_| Error::SendPlanInvalid)?,
                )
        {
            return Err(Error::SendPlanInvalid);
        }
        let output = plan.outputs.first().ok_or(Error::SendPlanInvalid)?;
        let revocation_locator = plan.revocation_locator.ok_or(Error::SendPlanInvalid)?;
        let mut inputs = Vec::with_capacity(plan.inputs.len());
        for input in &plan.inputs {
            let accepted = projection
                .vtxos
                .iter()
                .find(|vtxo| vtxo.vtxo_id == input.vtxo_id)
                .ok_or(Error::SendPlanInvalid)?;
            if accepted.amount != input.amount
                || accepted.locator != input.locator
                || !matches!(
                    accepted.state,
                    FederationBarkVtxoState::Spendable
                        | FederationBarkVtxoState::Reserved
                        | FederationBarkVtxoState::PendingSpend
                        | FederationBarkVtxoState::Spent
                )
                || (matches!(
                    accepted.state,
                    FederationBarkVtxoState::Reserved | FederationBarkVtxoState::PendingSpend
                ) && accepted.reservation_action_id != Some(plan.action_id))
            {
                return Err(Error::SendPlanInvalid);
            }
            inputs.push(FederationSendInput {
                vtxo_id: bark::ark::VtxoId::from_slice(&input.vtxo_id.to_bytes())
                    .map_err(|_| Error::SendPlanInvalid)?,
                amount: bitcoin::Amount::from_sat(u64::from(input.amount)),
                locator: VtxoKeyLocator {
                    index: input.locator.index,
                },
            });
        }
        let action_id = FederationActionId::from_bytes(plan.action_id.to_bytes())
            .map_err(|_| Error::SendPlanInvalid)?;
        let request_commitment = FederationRequestCommitment::from_bytes(
            operation
                .commitment()
                .map_err(|_| Error::SendPlanInvalid)?
                .to_bytes(),
        )
        .map_err(|_| Error::SendPlanInvalid)?;
        let bark_invoice = Invoice::Bolt11(invoice.clone());
        let candidate = FederationLightningSendCandidate {
            action_id,
            request_commitment,
            invoice: bark_invoice.clone(),
            original_payment_method: bark_invoice.into(),
            inputs,
            payment_amount: bitcoin::Amount::from_sat(u64::from(amount)),
            fee: bitcoin::Amount::from_sat(u64::from(fee)),
            htlc_locator: VtxoKeyLocator {
                index: output.locator.index,
            },
            revocation_locator: VtxoKeyLocator {
                index: revocation_locator.index,
            },
            htlc_expiry: u32::try_from(plan.expires_at).map_err(|_| Error::SendPlanInvalid)?,
            custody_attempt: plan.attempt,
            accepted_at_unix_seconds: i64::try_from(quote.created_time)
                .map_err(|_| Error::SendPlanInvalid)?,
        };
        let bark_plan = self
            .wallet
            .plan_lightning_send(candidate)
            .map_err(|_| Error::SendPlanInvalid)?;
        self.wallet
            .validate_lightning_send_plan(&bark_plan)
            .map_err(|_| Error::SendPlanInvalid)?;
        Ok(bark_plan)
    }

    /// Apply or replay an accepted outgoing plan into Bark without starting
    /// its executor or making an Ark request.
    pub async fn apply_lightning_send_plan(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        plan: &FederationBarkActionPlan,
    ) -> Result<(), Error> {
        let bark_plan = self.validate_lightning_send_plan(quote, projection, plan)?;
        for input in &plan.inputs {
            let accepted = projection
                .vtxos
                .iter()
                .find(|vtxo| vtxo.vtxo_id == input.vtxo_id)
                .ok_or(Error::SendProjectionFailed)?;
            let vtxo = bark::ark::vtxo::Vtxo::<bark::ark::vtxo::Full>::deserialize(
                &accepted.artifact.bytes,
            )
            .map_err(|_| Error::SendProjectionFailed)?;
            if vtxo.serialize() != accepted.artifact.bytes
                || vtxo.id().to_bytes() != accepted.vtxo_id.to_bytes()
                || vtxo.amount().to_sat() != u64::from(accepted.amount)
                || vtxo.expiry_height() != accepted.expiry_height
                || vtxo.server_pubkey().serialize() != self.bark_application().ark_server_public_key
            {
                return Err(Error::SendProjectionFailed);
            }
            self.wallet
                .import_vtxo(&FederationVtxoImport {
                    locator: VtxoKeyLocator {
                        index: accepted.locator.index,
                    },
                    vtxo,
                    // Import the accepted origin before applying its accepted
                    // reservation. `apply_lightning_send` performs the exact
                    // spendable -> action-lock transition atomically in Bark.
                    state: BarkFederationVtxoState::Spendable,
                })
                .await
                .map_err(|_| Error::SendProjectionFailed)?;
        }
        self.wallet
            .apply_lightning_send(&bark_plan)
            .await
            .map_err(|_| Error::SendProjectionFailed)?;
        Ok(())
    }

    /// Execute only the HTLC-cosign phase of one applied outgoing plan and
    /// return the exact semantic transition candidate.
    pub async fn execute_lightning_send_htlcs(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        accepted_authorization: &FederationJournalEntry,
    ) -> Result<FederationBarkActionTransition, Error> {
        let plan = accepted_send_plan(accepted_plan)?;
        if plan.actor != self.member_id {
            return Err(Error::SendExecutionFailed);
        }
        let bark_plan = self.validate_lightning_send_plan(quote, projection, &plan)?;
        let plan_operation = FederationBarkOperation::ActionPlan(plan.clone());
        let plan_commitment = plan_operation
            .commitment()
            .map_err(|_| Error::SendExecutionFailed)?;
        let custody_attempt = accepted_custody_attempt(accepted_plan, accepted_authorization)?;
        self.custody
            .activate_send(
                accepted_plan.clone(),
                custody_attempt.entry.clone(),
                plan_commitment.to_bytes().to_vec(),
            )
            .map_err(|_| Error::SendExecutionFailed)?;
        let result = self.wallet.execute_lightning_send_htlcs(&bark_plan).await;
        let signer_unavailable = self.custody.take_signer_unavailable();
        self.custody.deactivate_send();
        let evidence =
            result.map_err(|error| classify_send_execution_error(error, signer_unavailable))?;
        send_htlc_transition(
            self.member_id,
            accepted_plan,
            &plan,
            projection,
            &evidence,
            plan_commitment,
            &custody_attempt,
        )
    }

    /// Replay one accepted HTLC result into member-local Bark state without a
    /// second Ark cosign request.
    pub async fn apply_lightning_send_htlcs(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        accepted_htlcs: &FederationJournalEntry,
    ) -> Result<(), Error> {
        let plan = accepted_send_plan(accepted_plan)?;
        let transition = accepted_send_transition(
            accepted_htlcs,
            FederationBarkSemanticPhase::SendHtlcsCosigned,
        )?;
        validate_send_transition_binding(&self.federation, accepted_plan, &plan, &transition)?;
        let bark_plan = self.validate_lightning_send_plan(quote, projection, &plan)?;
        let evidence = send_htlc_evidence_from_transition(&plan, &transition)?;
        self.wallet
            .apply_lightning_send_htlcs(&bark_plan, &evidence)
            .await
            .map_err(|_| Error::SendReconciliationFailed)?;
        Ok(())
    }

    /// Initiate the exact already-cosigned outgoing payment. The coordinator
    /// selects the accepted actor first and may later authorize a fallback;
    /// Ark's persisted identity makes concurrent or delayed retries exact.
    pub async fn initiate_lightning_send(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        accepted_htlcs: &FederationJournalEntry,
    ) -> Result<(), Error> {
        let plan = accepted_send_plan(accepted_plan)?;
        let htlcs = accepted_send_transition(
            accepted_htlcs,
            FederationBarkSemanticPhase::SendHtlcsCosigned,
        )?;
        validate_send_transition_binding(&self.federation, accepted_plan, &plan, &htlcs)?;
        let bark_plan = self.validate_lightning_send_plan(quote, projection, &plan)?;
        self.wallet
            .initiate_lightning_send(&bark_plan)
            .await
            .map_err(|_| Error::SendExecutionFailed)?;
        Ok(())
    }

    /// Poll one selected member's accepted payment exactly once and preserve
    /// Bark's typed terminal result. A success carries an objective preimage
    /// bound to the accepted invoice, so a deterministic fallback may publish
    /// it after actor loss without receiving initiation authority. A failed
    /// result creates only one member observation; threshold matching evidence
    /// remains required before the federation can authorize revocation.
    pub async fn observe_lightning_send_terminal(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        accepted_htlcs: &FederationJournalEntry,
        observation_epoch: u64,
        wait_for_update: bool,
    ) -> Result<FederatedBarkSendTerminalStatus, Error> {
        if observation_epoch == 0 {
            return Err(Error::SendReconciliationFailed);
        }
        let plan = accepted_send_plan(accepted_plan)?;
        let htlcs = accepted_send_transition(
            accepted_htlcs,
            FederationBarkSemanticPhase::SendHtlcsCosigned,
        )?;
        validate_send_transition_binding(&self.federation, accepted_plan, &plan, &htlcs)?;
        let bark_plan = self.validate_lightning_send_plan(quote, projection, &plan)?;
        let status = if wait_for_update {
            match tokio::time::timeout(
                BARK_PAYMENT_UPDATE_WAIT,
                self.wallet.wait_lightning_send(&bark_plan),
            )
            .await
            {
                Ok(status) => status,
                Err(_) => self.wallet.poll_lightning_send(&bark_plan).await,
            }
        } else {
            self.wallet.poll_lightning_send(&bark_plan).await
        }
        .map_err(|_| Error::SendReconciliationFailed)?;
        match status {
            FederationLightningSendStatus::Pending => Ok(FederatedBarkSendTerminalStatus::Pending),
            FederationLightningSendStatus::Missing => Ok(FederatedBarkSendTerminalStatus::Missing),
            FederationLightningSendStatus::Succeeded(preimage) => Ok(
                FederatedBarkSendTerminalStatus::Succeeded(Box::new(payment_succeeded_transition(
                    plan.actor,
                    accepted_plan,
                    accepted_htlcs,
                    &plan,
                    &htlcs,
                    projection,
                    preimage,
                )?)),
            ),
            FederationLightningSendStatus::Failed => Ok(FederatedBarkSendTerminalStatus::Failed(
                self.lightning_send_failure_observation(&plan, &htlcs, observation_epoch)?,
            )),
        }
    }

    /// Apply an accepted success proof locally and construct this member's
    /// independently bound success observation for the existing melt saga.
    pub async fn apply_lightning_send_succeeded(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        accepted_success: &FederationJournalEntry,
        observation_epoch: u64,
    ) -> Result<FederationBarkMemberObservation, Error> {
        if observation_epoch == 0 {
            return Err(Error::SendReconciliationFailed);
        }
        let plan = accepted_send_plan(accepted_plan)?;
        let success = accepted_send_transition(
            accepted_success,
            FederationBarkSemanticPhase::PaymentSucceeded,
        )?;
        validate_send_transition_binding(&self.federation, accepted_plan, &plan, &success)?;
        let preimage = send_success_preimage(&plan, &success)?;
        let bark_plan = self.validate_lightning_send_plan(quote, projection, &plan)?;
        self.wallet
            .apply_lightning_send_succeeded(&bark_plan, preimage)
            .await
            .map_err(|_| Error::SendReconciliationFailed)?;
        Ok(FederationBarkMemberObservation {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: plan.action_id,
            quote: plan.quote.clone(),
            observer: self.member_id,
            observation_epoch,
            source_scope: ark_server_commitment(&self.bark_application().ark_server_public_key)
                .map_err(|_| Error::SendReconciliationFailed)?,
            payment_hash: plan.payment_hash,
            status: FederationBarkObservationStatus::Succeeded,
            amount: plan.external_amount,
            fee: plan.fee_amount,
            result_hash: success.result_hash,
        })
    }

    /// Validate one not-yet-accepted success result independently, apply its
    /// exact preimage locally, and produce this member's signed-evidence input.
    /// Consensus accepts the result only after a threshold certificate is
    /// attached to the same transition.
    pub async fn attest_lightning_send_succeeded_candidate(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        accepted_htlcs: &FederationJournalEntry,
        success: &FederationBarkActionTransition,
        observation_epoch: u64,
    ) -> Result<FederationBarkMemberObservation, Error> {
        if observation_epoch == 0
            || success.phase != FederationBarkSemanticPhase::PaymentSucceeded
            || success.evidence_certificate.is_some()
        {
            return Err(Error::SendReconciliationFailed);
        }
        let plan = accepted_send_plan(accepted_plan)?;
        let htlcs = accepted_send_transition(
            accepted_htlcs,
            FederationBarkSemanticPhase::SendHtlcsCosigned,
        )?;
        validate_send_transition_binding(&self.federation, accepted_plan, &plan, &htlcs)?;
        validate_send_transition_binding(&self.federation, accepted_plan, &plan, success)?;
        if success.predecessor_operation_id != accepted_htlcs.operation_id
            || success.request_hash != htlcs.result_hash
        {
            return Err(Error::SendReconciliationFailed);
        }
        FederationBarkOperation::ActionTransition(success.clone())
            .validate()
            .map_err(|_| Error::SendReconciliationFailed)?;
        let preimage = send_success_preimage(&plan, success)?;
        let bark_plan = self.validate_lightning_send_plan(quote, projection, &plan)?;
        self.wallet
            .apply_lightning_send_succeeded(&bark_plan, preimage)
            .await
            .map_err(|_| Error::SendReconciliationFailed)?;
        Ok(FederationBarkMemberObservation {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: plan.action_id,
            quote: plan.quote.clone(),
            observer: self.member_id,
            observation_epoch,
            source_scope: ark_server_commitment(&self.bark_application().ark_server_public_key)
                .map_err(|_| Error::SendReconciliationFailed)?,
            payment_hash: plan.payment_hash,
            status: FederationBarkObservationStatus::Succeeded,
            amount: plan.external_amount,
            fee: plan.fee_amount,
            result_hash: success.result_hash,
        })
    }

    fn lightning_send_failure_observation(
        &self,
        plan: &FederationBarkActionPlan,
        htlcs: &FederationBarkActionTransition,
        observation_epoch: u64,
    ) -> Result<FederationBarkMemberObservation, Error> {
        let (_, result_hash) = payment_failed_result(plan, htlcs.result_hash)?;
        Ok(FederationBarkMemberObservation {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: plan.action_id,
            quote: plan.quote.clone(),
            observer: self.member_id,
            observation_epoch,
            source_scope: ark_server_commitment(&self.bark_application().ark_server_public_key)
                .map_err(|_| Error::SendReconciliationFailed)?,
            payment_hash: plan.payment_hash,
            status: FederationBarkObservationStatus::Failed,
            amount: plan.external_amount,
            fee: plan.fee_amount,
            result_hash,
        })
    }

    /// Construct the terminal-payment-failure transition after the accepted
    /// distinct-member evidence threshold is present.
    pub fn finalize_lightning_send_failure(
        &self,
        accepted_plan: &FederationJournalEntry,
        accepted_htlcs: &FederationJournalEntry,
    ) -> Result<FederationBarkActionTransition, Error> {
        let plan = accepted_send_plan(accepted_plan)?;
        if plan.actor != self.member_id {
            return Err(Error::SendExecutionFailed);
        }
        let htlcs = accepted_send_transition(
            accepted_htlcs,
            FederationBarkSemanticPhase::SendHtlcsCosigned,
        )?;
        validate_send_transition_binding(&self.federation, accepted_plan, &plan, &htlcs)?;
        let (artifact, result_hash) = payment_failed_result(&plan, htlcs.result_hash)?;
        Ok(FederationBarkActionTransition {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: plan.action_id,
            quote: plan.quote.clone(),
            phase: FederationBarkSemanticPhase::PaymentFailed,
            actor: self.member_id,
            plan_operation_id: accepted_plan.operation_id,
            predecessor_operation_id: accepted_htlcs.operation_id,
            payment_hash: plan.payment_hash,
            attempt: htlcs.attempt,
            signer_set: htlcs.signer_set.clone(),
            request_hash: htlcs.result_hash,
            result_hash,
            evidence_certificate: None,
            external_id: plan.external_id,
            artifacts: vec![artifact],
            vtxo_transitions: Vec::new(),
        })
    }

    /// Replay accepted terminal failure into Bark while retaining all HTLC
    /// value in the revocation-required checkpoint.
    pub async fn apply_lightning_send_failed(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        accepted_failed: &FederationJournalEntry,
    ) -> Result<(), Error> {
        let plan = accepted_send_plan(accepted_plan)?;
        let failed =
            accepted_send_transition(accepted_failed, FederationBarkSemanticPhase::PaymentFailed)?;
        validate_send_transition_binding(&self.federation, accepted_plan, &plan, &failed)?;
        validate_payment_failed_result(&plan, &failed)?;
        let bark_plan = self.validate_lightning_send_plan(quote, projection, &plan)?;
        self.wallet
            .apply_lightning_send_failed(&bark_plan)
            .await
            .map_err(|_| Error::SendReconciliationFailed)?;
        Ok(())
    }

    /// Build the one exact recovery plan for the accepted failed payment.
    pub fn propose_failed_send_revocation(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        send_plan_entry: &FederationJournalEntry,
        accepted_failed: &FederationJournalEntry,
    ) -> Result<FederationBarkActionPlan, Error> {
        let send = accepted_send_plan(send_plan_entry)?;
        if send.actor != self.member_id {
            return Err(Error::SendExecutionFailed);
        }
        let failed =
            accepted_send_transition(accepted_failed, FederationBarkSemanticPhase::PaymentFailed)?;
        validate_send_transition_binding(&self.federation, send_plan_entry, &send, &failed)?;
        validate_payment_failed_result(&send, &failed)?;
        let mut records = projection
            .vtxos
            .iter()
            .filter(|record| {
                record.state == FederationBarkVtxoState::PendingSpend
                    && record.reservation_action_id == Some(send.action_id)
            })
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.vtxo_id);
        if records.is_empty() {
            return Err(Error::SendExecutionFailed);
        }
        let inputs = records
            .iter()
            .map(|record| FederationBarkVtxoInput {
                vtxo_id: record.vtxo_id,
                locator: record.locator,
                amount: record.amount,
            })
            .collect::<Vec<_>>();
        let amount = inputs
            .iter()
            .try_fold(cdk_common::Amount::ZERO, |sum, input| {
                sum.checked_add(input.amount)
                    .ok_or(Error::SendExecutionFailed)
            })?;
        if amount
            != send
                .external_amount
                .checked_add(send.fee_amount)
                .ok_or(Error::SendExecutionFailed)?
        {
            return Err(Error::SendExecutionFailed);
        }
        let output_locator = send.revocation_locator.ok_or(Error::SendExecutionFailed)?;
        let request_bytes = failed_send_revocation_request_bytes(
            send.action_id,
            accepted_failed.operation_id,
            send.payment_hash.ok_or(Error::SendExecutionFailed)?,
            output_locator,
            &inputs,
        )?;
        let request_artifact =
            bark_artifact(FederationBarkArtifactKind::ArkRequest, request_bytes)?;
        let mut artifacts = records
            .iter()
            .map(|record| record.artifact.clone())
            .collect::<Vec<_>>();
        artifacts.push(request_artifact.clone());
        artifacts.sort();
        artifacts.dedup();
        let plan = FederationBarkActionPlan {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: send.action_id,
            quote: send.quote.clone(),
            kind: FederationBarkActionKind::FailedSendRevocation,
            actor: send.actor,
            predecessor_operation_id: accepted_failed.operation_id,
            payment_hash: send.payment_hash,
            inputs,
            outputs: vec![FederationBarkVtxoOutput {
                locator: output_locator,
                amount,
                artifact_hash: request_artifact.hash,
            }],
            revocation_locator: None,
            external_amount: cdk_common::Amount::ZERO,
            fee_amount: cdk_common::Amount::ZERO,
            fee_bound: cdk_common::Amount::ZERO,
            certified_receive_htlcs: None,
            artifacts,
            external_id: FederationBarkExternalId::derive_failed_send_revocation(
                send.action_id,
                accepted_failed.operation_id,
                failed.attempt,
            )
            .map_err(|_| Error::SendExecutionFailed)?,
            signer_set: failed.signer_set.clone(),
            attempt: failed.attempt,
            expires_at: send.expires_at,
        };
        FederationBarkOperation::ActionPlan(plan.clone())
            .validate()
            .map_err(|_| Error::SendExecutionFailed)?;
        Ok(plan)
    }

    /// Execute the accepted revocation plan once and return its exact semantic
    /// result candidate.
    pub async fn execute_failed_send_revocation(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        send_plan_entry: &FederationJournalEntry,
        revocation_plan_entry: &FederationJournalEntry,
        accepted_authorization: &FederationJournalEntry,
    ) -> Result<FederationBarkActionTransition, Error> {
        let send = accepted_send_plan(send_plan_entry)?;
        let revocation = accepted_revocation_plan(revocation_plan_entry)?;
        if revocation.actor != self.member_id {
            return Err(Error::SendExecutionFailed);
        }
        let bark_send = self.validate_lightning_send_plan(quote, projection, &send)?;
        let bark_revocation = self.bark_revocation_plan(&bark_send, &revocation)?;
        let operation = FederationBarkOperation::ActionPlan(revocation.clone());
        let commitment = operation
            .commitment()
            .map_err(|_| Error::SendExecutionFailed)?;
        let custody_attempt =
            accepted_custody_attempt(revocation_plan_entry, accepted_authorization)?;
        self.custody
            .activate_revocation(
                revocation_plan_entry.clone(),
                custody_attempt.entry.clone(),
                commitment.to_bytes().to_vec(),
            )
            .map_err(|_| Error::SendExecutionFailed)?;
        let result = self
            .wallet
            .execute_lightning_send_revocation(&bark_send, &bark_revocation)
            .await;
        let signer_unavailable = self.custody.take_signer_unavailable();
        self.custody.deactivate_revocation();
        let evidence =
            result.map_err(|error| classify_send_execution_error(error, signer_unavailable))?;
        revocation_completed_transition(
            self.member_id,
            revocation_plan_entry,
            &revocation,
            projection,
            &evidence,
            commitment,
            &custody_attempt,
        )
    }

    /// Apply accepted recovered value locally and return the independently
    /// validated failure observation that may safely release Cashu proofs.
    pub async fn apply_failed_send_revocation(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        send_plan_entry: &FederationJournalEntry,
        revocation_plan_entry: &FederationJournalEntry,
        accepted_completed: &FederationJournalEntry,
        observation_epoch: u64,
    ) -> Result<FederationBarkMemberObservation, Error> {
        if observation_epoch == 0 {
            return Err(Error::SendReconciliationFailed);
        }
        let send = accepted_send_plan(send_plan_entry)?;
        let revocation = accepted_revocation_plan(revocation_plan_entry)?;
        let completed = accepted_send_transition(
            accepted_completed,
            FederationBarkSemanticPhase::RevocationCompleted,
        )?;
        if completed.plan_operation_id != revocation_plan_entry.operation_id
            || completed.action_id != revocation.action_id
            || completed.quote != revocation.quote
            || completed.payment_hash != revocation.payment_hash
            || completed.predecessor_operation_id != revocation_plan_entry.operation_id
        {
            return Err(Error::SendReconciliationFailed);
        }
        let bark_send = self.validate_lightning_send_plan(quote, projection, &send)?;
        let bark_revocation = self.bark_revocation_plan(&bark_send, &revocation)?;
        let evidence = revocation_evidence_from_transition(&revocation, &completed)?;
        self.wallet
            .apply_lightning_send_revocation(&bark_send, &bark_revocation, &evidence)
            .await
            .map_err(|_| Error::SendReconciliationFailed)?;
        Ok(FederationBarkMemberObservation {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: send.action_id,
            quote: send.quote.clone(),
            observer: self.member_id,
            observation_epoch,
            source_scope: ark_server_commitment(&self.bark_application().ark_server_public_key)
                .map_err(|_| Error::SendReconciliationFailed)?,
            payment_hash: send.payment_hash,
            status: FederationBarkObservationStatus::Failed,
            amount: send.external_amount,
            fee: send.fee_amount,
            result_hash: completed.result_hash,
        })
    }

    fn bark_revocation_plan(
        &self,
        send_plan: &FederationLightningSendPlan,
        plan: &FederationBarkActionPlan,
    ) -> Result<FederationLightningSendRevocationPlan, Error> {
        if plan.kind != FederationBarkActionKind::FailedSendRevocation
            || plan.action_id.to_bytes() != send_plan.action_id.to_bytes()
            || plan
                .payment_hash
                .map(|payment_hash| payment_hash.to_bytes().to_vec())
                != Some(send_plan.invoice.payment_hash().to_vec())
            || plan.outputs.len() != 1
        {
            return Err(Error::SendPlanInvalid);
        }
        let output = plan.outputs.first().ok_or(Error::SendPlanInvalid)?;
        if output.locator.index != send_plan.revocation_key.locator.index {
            return Err(Error::SendPlanInvalid);
        }
        let mut input_amount = cdk_common::Amount::ZERO;
        let mut input_vtxo_ids = Vec::with_capacity(plan.inputs.len());
        for input in &plan.inputs {
            input_amount = input_amount
                .checked_add(input.amount)
                .ok_or(Error::SendPlanInvalid)?;
            input_vtxo_ids.push(
                bark::ark::VtxoId::from_slice(&input.vtxo_id.to_bytes())
                    .map_err(|_| Error::SendPlanInvalid)?,
            );
        }
        let operation = FederationBarkOperation::ActionPlan(plan.clone());
        let commitment = operation.commitment().map_err(|_| Error::SendPlanInvalid)?;
        let mut requests = plan
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkRequest);
        let request = requests.next().ok_or(Error::SendPlanInvalid)?;
        let expected_request = failed_send_revocation_request_bytes(
            plan.action_id,
            plan.predecessor_operation_id,
            plan.payment_hash.ok_or(Error::SendPlanInvalid)?,
            output.locator,
            &plan.inputs,
        )?;
        if requests.next().is_some()
            || request.bytes != expected_request
            || request.hash != bark_digest(&expected_request)?
            || output.artifact_hash != request.hash
            || output.amount != input_amount
        {
            return Err(Error::SendPlanInvalid);
        }
        Ok(FederationLightningSendRevocationPlan {
            action_id: send_plan.action_id,
            request_commitment: FederationRequestCommitment::from_bytes(commitment.to_bytes())
                .map_err(|_| Error::SendPlanInvalid)?,
            payment_hash: send_plan.invoice.payment_hash(),
            input_vtxo_ids,
            input_amount: bitcoin::Amount::from_sat(u64::from(input_amount)),
            output_key: send_plan.revocation_key,
            custody_attempt: plan.attempt,
            expires_at: u32::try_from(plan.expires_at).map_err(|_| Error::SendPlanInvalid)?,
        })
    }
}
