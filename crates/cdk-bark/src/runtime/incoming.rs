//! Incoming operations.

use super::{
    accepted_claim_plan, accepted_custody_attempt, bark_artifact, bark_digest, bark_operation,
    certified_receive_htlc_transition, classify_receive_claim_error, receive_claim_outputs,
    receive_claim_request_bytes, receive_claim_result, receive_external_id,
    receive_htlc_evidence_from_transition, receive_htlc_transition,
    receive_preimage_disclosed_transition, FederatedBarkReceiveClaimOutcome,
    FederatedBarkReceiveClaimProposal, FederatedBarkReceiveClaimResult, FederatedBarkRuntime,
    BARK_PAYMENT_UPDATE_WAIT,
};
use crate::Error;
use bark::actions::lightning::receive::{
    minimum_lightning_receive_cltv_delta, LightningReceiveAntiDosMode,
};
use bark::ark::lightning::{PaymentHash, Preimage};
use bark::ark::mailbox::MailboxIdentifier;
use bark::ark::ProtocolEncoding;
use bark::custody::VtxoCustody;
use bark::custody::VtxoKeyLocator;
use bark::federation::{
    FederationActionId, FederationLightningReceiveClaimExecution,
    FederationLightningReceiveClaimPlan, FederationLightningReceiveIntent,
    FederationLightningReceivePhase, FederationLightningReceivePlan,
    FederationLightningReceiveStatus, FederationRequestCommitment,
};
use bitcoin::secp256k1::PublicKey;
use cdk_common::federation::{
    FederationJournalEntry, FederationOperationId, FederationOperationStatus, MemberId,
};
use cdk_federation_bark::{
    federation_bark_receive_request_hash, federation_bark_signer_set_for_attempt,
    FederationBarkActionKind, FederationBarkActionPlan, FederationBarkActionTransition,
    FederationBarkArtifactKind, FederationBarkCertifiedReceiveHtlcs,
    FederationBarkMemberObservation, FederationBarkObservationStatus, FederationBarkOperation,
    FederationBarkReceiveIntent, FederationBarkReceivePrepared, FederationBarkReceiveSecretBinding,
    FederationBarkSemanticPhase, FederationBarkVtxoId, FederationBarkVtxoInput,
    FederationBarkVtxoLocator, FederationBarkVtxoOutput, FederationBarkVtxoState,
    FEDERATION_BARK_OPERATION_VERSION,
};
use std::collections::BTreeSet;

impl FederatedBarkRuntime {
    /// Return the minimum safe CLTV delta for a new federated receive intent.
    ///
    /// This combines the accepted federation floor with Bark's unilateral-exit
    /// safety calculation using the Ark server's advertised timelock deltas.
    pub async fn minimum_receive_cltv_delta(&self) -> Result<u32, Error> {
        let ark_info = self
            .bark_wallet
            .require_ark_info()
            .await
            .map_err(|_| Error::ArkCapabilityMismatch)?;
        let safe_delta = minimum_lightning_receive_cltv_delta(
            ark_info.vtxo_exit_delta,
            ark_info.htlc_expiry_delta,
            self.bark_wallet.config(),
        )
        .map_err(|_| Error::ArkCapabilityMismatch)?;
        let minimum = self
            .application
            .policy
            .minimum_incoming_cltv_delta
            .max(u32::from(safe_delta));
        if minimum > u32::from(ark_info.max_user_invoice_cltv_delta) {
            return Err(Error::ArkCapabilityMismatch);
        }
        Ok(minimum)
    }

    /// Perform the sole Ark invoice RPC for an already accepted receive intent.
    ///
    /// The protected preimage is loaded only after the caller supplies the
    /// accepted intent operation ID. Bark receives no locally generated
    /// identity, secret, locator, mailbox, amount, or policy value.
    pub async fn prepare_receive_invoice(
        &self,
        execution_actor: MemberId,
        intent_operation_id: FederationOperationId,
        intent: &FederationBarkReceiveIntent,
    ) -> Result<FederationBarkReceivePrepared, Error> {
        let binding =
            FederationBarkReceiveSecretBinding::from_intent(self.scope.federation_id(), intent);
        let secret = self.receive_secrets.load_secret(&binding)?;
        let bark_intent =
            self.bark_receive_intent(intent_operation_id, intent, *secret.as_bytes())?;
        let plan = self
            .wallet
            .start_lightning_receive(&bark_intent)
            .await
            .map_err(|_| Error::InvoicePreparationFailed)?;
        FederationBarkReceivePrepared::from_invoice(
            execution_actor,
            intent_operation_id,
            intent,
            plan.invoice,
        )
        .map_err(|_| Error::InvoicePreparationFailed)
    }

    /// Materialize an accepted receive plan without another Ark RPC.
    pub async fn apply_receive_prepared(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
    ) -> Result<(), Error> {
        prepared
            .validate_against_intent(intent)
            .map_err(|_| Error::ReceiveProjectionFailed)?;
        let plan = self.receive_plan(intent, prepared)?;
        self.wallet
            .validate_lightning_receive_plan(&plan)
            .map_err(|_| Error::ReceiveProjectionFailed)?;
        self.wallet
            .apply_lightning_receive(&plan)
            .await
            .map_err(|_| Error::ReceiveProjectionFailed)?;
        Ok(())
    }

    fn receive_plan(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
    ) -> Result<FederationLightningReceivePlan, Error> {
        prepared
            .validate_against_intent(intent)
            .map_err(|_| Error::ReceiveProjectionFailed)?;
        let binding =
            FederationBarkReceiveSecretBinding::from_intent(self.scope.federation_id(), intent);
        let secret = self.receive_secrets.load_secret(&binding)?;
        let bark_intent =
            self.bark_receive_intent(prepared.intent_operation_id, intent, *secret.as_bytes())?;
        Ok(FederationLightningReceivePlan {
            action_id: bark_intent.action_id,
            request_commitment: bark_intent.request_commitment,
            payment_hash: bark_intent.payment_hash,
            cashu_amount: bark_intent.cashu_amount,
            gross_amount: bark_intent.gross_amount,
            receive_fee_bound: bark_intent.receive_fee_bound,
            minimum_net_amount: bark_intent.minimum_net_amount,
            description: bark_intent.description,
            invoice: prepared
                .invoice
                .parse()
                .map_err(|_| Error::ReceiveProjectionFailed)?,
            payment_preimage: bark_intent.payment_preimage,
            locator: bark_intent.locator,
            mailbox_id: bark_intent.mailbox_id,
            htlc_recv_cltv_delta: bark_intent.htlc_recv_cltv_delta,
            anti_dos: bark_intent.anti_dos,
            policy_version: bark_intent.policy_version,
            custody_attempt: bark_intent.custody_attempt,
        })
    }

    /// Poll and validate exact incoming HTLC evidence. The returned member
    /// observation is safe to order immediately; the transition candidate must
    /// wait until the federation has accepted the required matching evidence.
    /// Only the current receive-worker lease holder may prepare an accepted
    /// payment; every member can validate an already-ready package read-only.
    pub async fn observe_receive_htlcs(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared_operation_id: FederationOperationId,
        prepared: &FederationBarkReceivePrepared,
        observation_epoch: u64,
        prepare_if_accepted: bool,
        wait_for_update: bool,
    ) -> Result<
        Option<(
            FederationBarkMemberObservation,
            FederationBarkActionTransition,
        )>,
        Error,
    > {
        if observation_epoch == 0 {
            return Err(Error::ReceiveObservationFailed);
        }
        let receive = self.receive_plan(intent, prepared)?;
        let observation = if wait_for_update {
            match tokio::time::timeout(
                BARK_PAYMENT_UPDATE_WAIT,
                self.wallet.wait_lightning_receive(&receive),
            )
            .await
            {
                Ok(observation) => observation,
                Err(_) => self.wallet.poll_lightning_receive(&receive).await,
            }
        } else {
            self.wallet.poll_lightning_receive(&receive).await
        }
        .map_err(|error| {
            tracing::warn!(
                action_id = %intent.action_id,
                error = %error,
                "Federated Bark receive status observation failed"
            );
            Error::ReceiveObservationFailed
        })?;
        let evidence = match observation.status {
            FederationLightningReceiveStatus::AwaitingPayment
            | FederationLightningReceiveStatus::Canceled => return Ok(None),
            FederationLightningReceiveStatus::Settled => {
                return Err(Error::ReceiveClaimRecoveryRequired)
            }
            FederationLightningReceiveStatus::Accepted if prepare_if_accepted => self
                .wallet
                .prepare_lightning_receive_htlcs(&receive)
                .await
                .map_err(|error| {
                    tracing::warn!(
                        action_id = %intent.action_id,
                        error = %error,
                        "Federated Bark accepted receive preparation failed"
                    );
                    Error::ReceiveObservationFailed
                })?,
            FederationLightningReceiveStatus::Accepted => return Ok(None),
            // The original Ark status response contains VTXO IDs, not full
            // VTXOs. Once ready, this endpoint is idempotent and returns the
            // same complete evidence to every federation member.
            FederationLightningReceiveStatus::HtlcsReady => self
                .wallet
                .hydrate_lightning_receive_htlcs(&receive)
                .await
                .map_err(|error| {
                    tracing::warn!(
                        action_id = %intent.action_id,
                        error = %error,
                        "Federated Bark ready receive hydration failed"
                    );
                    Error::ReceiveObservationFailed
                })?,
        };
        let transition =
            receive_htlc_transition(self.member_id, prepared_operation_id, intent, &evidence)?;
        let member_observation = FederationBarkMemberObservation {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: intent.action_id,
            quote: Some(intent.quote.clone()),
            observer: self.member_id,
            observation_epoch,
            source_scope: intent.ark_server,
            payment_hash: Some(intent.payment_hash),
            status: FederationBarkObservationStatus::Accepted,
            amount: cdk_common::Amount::from(evidence.net_amount.to_sat()),
            fee: cdk_common::Amount::from(evidence.fee.to_sat()),
            result_hash: transition.result_hash,
        };
        Ok(Some((member_observation, transition)))
    }

    /// Rebuild an HTLC-ready transition from this member's durable evidence.
    ///
    /// A member observation is submitted only after Bark persists the exact
    /// HTLC package, so the receive actor can continue from an accepted local
    /// observation without polling Ark again while it waits for the remaining
    /// federation observations.
    pub async fn receive_htlc_transition_from_accepted_observation(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared_operation_id: FederationOperationId,
        prepared: &FederationBarkReceivePrepared,
        accepted: &FederationBarkMemberObservation,
    ) -> Result<FederationBarkActionTransition, Error> {
        if accepted.action_id != intent.action_id
            || accepted.quote.as_deref() != Some(intent.quote.as_str())
            || accepted.observer != self.member_id
            || accepted.source_scope != intent.ark_server
            || accepted.payment_hash != Some(intent.payment_hash)
            || accepted.status != FederationBarkObservationStatus::Accepted
        {
            return Err(Error::ReceiveObservationFailed);
        }
        let receive = self.receive_plan(intent, prepared)?;
        let evidence = self
            .wallet
            .prepare_lightning_receive_htlcs(&receive)
            .await
            .map_err(|error| {
                tracing::warn!(
                    action_id = %intent.action_id,
                    error = %error,
                    "Federated Bark accepted receive evidence reload failed"
                );
                Error::ReceiveObservationFailed
            })?;
        let transition =
            receive_htlc_transition(self.member_id, prepared_operation_id, intent, &evidence)?;
        if accepted.amount != cdk_common::Amount::from(evidence.net_amount.to_sat())
            || accepted.fee != cdk_common::Amount::from(evidence.fee.to_sat())
            || accepted.result_hash != transition.result_hash
        {
            return Err(Error::ReceiveObservationFailed);
        }
        Ok(transition)
    }

    /// Construct one exact incoming-claim action plan from the local copy of
    /// already accepted HTLC evidence. This performs no custody operation and
    /// does not disclose the preimage.
    async fn receive_claim_plan_from_local_evidence(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
        ready_operation_id: FederationOperationId,
        proposal: FederatedBarkReceiveClaimProposal,
    ) -> Result<FederationBarkActionPlan, Error> {
        let receive = self.receive_plan(intent, prepared)?;
        let evidence = self
            .wallet
            .prepare_lightning_receive_htlcs(&receive)
            .await
            .map_err(|error| {
                tracing::warn!(
                    action_id = %intent.action_id,
                    error = %error,
                    "Federated Bark receive claim evidence load failed"
                );
                Error::ReceiveClaimPlanInvalid
            })?;
        let claim = self
            .wallet
            .plan_lightning_receive_claim(
                evidence,
                VtxoKeyLocator {
                    index: proposal.output_locator.index,
                },
                proposal.attempt,
            )
            .map_err(|error| {
                tracing::warn!(
                    action_id = %intent.action_id,
                    error = %error,
                    "Federated Bark receive claim planning failed"
                );
                Error::ReceiveClaimPlanInvalid
            })?;
        self.claim_action_plan(
            intent,
            ready_operation_id,
            proposal.actor,
            proposal.signer_set,
            proposal.output_locator,
            &claim,
        )
    }

    /// Combine threshold-certified HTLC evidence with the exact incoming
    /// claim reservation. The certificate still authorizes only readiness;
    /// the outer plan separately commits the custody inputs, output, and
    /// signer set before any preimage disclosure.
    pub async fn propose_certified_receive_claim(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
        certified_ready: FederationBarkActionTransition,
        proposal: FederatedBarkReceiveClaimProposal,
    ) -> Result<FederationBarkActionPlan, Error> {
        let certificate = certified_ready
            .evidence_certificate
            .as_deref()
            .ok_or(Error::ReceiveClaimPlanInvalid)?
            .clone();
        if certified_ready.phase != FederationBarkSemanticPhase::ReceiveHtlcsReady
            || certified_ready.action_id != intent.action_id
            || certified_ready.quote.as_deref() != Some(intent.quote.as_str())
            || certified_ready.plan_operation_id != certified_ready.predecessor_operation_id
            || certified_ready.payment_hash != Some(intent.payment_hash)
            || certificate.binding.evidence_digest != certified_ready.result_hash
        {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let receive = self.receive_plan(intent, prepared)?;
        let certified_evidence = receive_htlc_evidence_from_transition(
            &self.wallet,
            &receive,
            intent,
            &certified_ready,
        )?;
        let predecessor_operation_id = certified_ready.predecessor_operation_id;
        let mut plan = self
            .receive_claim_plan_from_local_evidence(
                intent,
                prepared,
                predecessor_operation_id,
                proposal,
            )
            .await?;
        let ready_artifacts = certified_ready
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Vtxo)
            .cloned()
            .collect::<Vec<_>>();
        let plan_artifacts = plan
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Vtxo)
            .cloned()
            .collect::<Vec<_>>();
        if ready_artifacts != plan_artifacts {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        plan.certified_receive_htlcs = Some(Box::new(FederationBarkCertifiedReceiveHtlcs {
            version: FEDERATION_BARK_OPERATION_VERSION,
            minimum_expiry: certified_evidence.minimum_expiry,
            request_hash: certified_ready.request_hash,
            vtxo_transitions: certified_ready.vtxo_transitions,
            certificate,
        }));
        FederationBarkOperation::ActionPlan(plan.clone())
            .validate()
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        Ok(plan)
    }

    /// Reconstruct and compare every field of an incoming claim plan before it
    /// can authorize FROST or virtual MuSig2.
    pub async fn validate_receive_claim_plan(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
        plan: &FederationBarkActionPlan,
    ) -> Result<FederationLightningReceiveClaimPlan, Error> {
        let (receive, accepted_claim) = self.accepted_receive_claim_plan(intent, prepared, plan)?;
        let evidence = self
            .wallet
            .prepare_lightning_receive_htlcs(&receive)
            .await
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        let output = plan.outputs.first().ok_or(Error::ReceiveClaimPlanInvalid)?;
        let claim = self
            .wallet
            .plan_lightning_receive_claim(
                evidence,
                VtxoKeyLocator {
                    index: output.locator.index,
                },
                plan.attempt,
            )
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        if claim != accepted_claim {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        Ok(accepted_claim)
    }

    fn accepted_receive_claim_plan(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
        plan: &FederationBarkActionPlan,
    ) -> Result<
        (
            bark::federation::FederationLightningReceivePlan,
            FederationLightningReceiveClaimPlan,
        ),
        Error,
    > {
        if plan.kind != FederationBarkActionKind::IncomingClaim
            || plan.action_id != intent.action_id
            || plan.quote.as_deref() != Some(intent.quote.as_str())
            || plan.payment_hash != Some(intent.payment_hash)
            || plan.outputs.len() != 1
        {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let receive = self.receive_plan(intent, prepared)?;
        let mut input_vtxos = plan
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Vtxo)
            .map(|artifact| {
                let vtxo =
                    bark::ark::vtxo::Vtxo::<bark::ark::vtxo::Full>::deserialize(&artifact.bytes)
                        .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
                if vtxo.serialize() != artifact.bytes {
                    return Err(Error::ReceiveClaimPlanInvalid);
                }
                Ok(vtxo)
            })
            .collect::<Result<Vec<_>, Error>>()?;
        input_vtxos.sort_by_key(bark::ark::vtxo::Vtxo::id);
        if input_vtxos.len() != plan.inputs.len()
            || plan.inputs.iter().zip(&input_vtxos).any(|(input, vtxo)| {
                input.vtxo_id.to_bytes() != vtxo.id().to_bytes()
                    || u64::from(input.amount) != vtxo.amount().to_sat()
                    || input.locator != intent.destination
            })
        {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let output = plan.outputs.first().ok_or(Error::ReceiveClaimPlanInvalid)?;
        let destination_locator = VtxoKeyLocator {
            index: output.locator.index,
        };
        let destination = self
            .custody
            .derive_public_key(destination_locator)
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        let claim = FederationLightningReceiveClaimPlan {
            action_id: FederationActionId::from_bytes(plan.action_id.to_bytes())
                .map_err(|_| Error::ReceiveClaimPlanInvalid)?,
            payment_hash: receive.payment_hash,
            destination,
            amount: bitcoin::Amount::from_sat(u64::from(output.amount)),
            minimum_expiry: u32::try_from(plan.expires_at)
                .map_err(|_| Error::ReceiveClaimPlanInvalid)?,
            custody_attempt: plan.attempt,
            input_vtxos,
        };
        self.wallet
            .validate_lightning_receive_claim_plan(&receive, &claim)
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        {
            let certified_ready = certified_receive_htlc_transition(plan)?;
            receive_htlc_evidence_from_transition(
                &self.wallet,
                &receive,
                intent,
                &certified_ready,
            )?;
        }
        let mut expected = self.claim_action_plan(
            intent,
            plan.predecessor_operation_id,
            plan.actor,
            plan.signer_set.clone(),
            output.locator,
            &claim,
        )?;
        expected.certified_receive_htlcs = plan.certified_receive_htlcs.clone();
        if &expected != plan {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        Ok((receive, claim))
    }

    /// Replay an accepted incoming-claim plan without observing Ark or
    /// releasing custody material. This reconstructs the exact HTLC inputs
    /// from accepted artifacts and stops before preimage disclosure.
    pub async fn apply_receive_claim_plan(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
        accepted_plan: &FederationJournalEntry,
    ) -> Result<(), Error> {
        let plan = accepted_claim_plan(accepted_plan)?;
        let (receive, claim) = self.accepted_receive_claim_plan(intent, prepared, &plan)?;
        {
            let certified_ready = certified_receive_htlc_transition(&plan)?;
            let evidence = receive_htlc_evidence_from_transition(
                &self.wallet,
                &receive,
                intent,
                &certified_ready,
            )?;
            self.wallet
                .apply_lightning_receive_htlc_replay(&receive, &evidence.vtxos)
                .await
                .map_err(|_| Error::ReceiveClaimRecoveryRequired)?;
        }
        self.wallet
            .apply_lightning_receive_claim_replay(
                &receive,
                &claim,
                FederationLightningReceivePhase::HtlcsReady,
                &[],
            )
            .await
            .map_err(|_| Error::ReceiveClaimRecoveryRequired)
    }

    /// Replay an accepted preimage-disclosure or terminal claim transition
    /// without signing or external I/O.
    pub async fn apply_receive_claim_transition(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
        accepted_plan: &FederationJournalEntry,
        accepted_transition: &FederationJournalEntry,
    ) -> Result<(), Error> {
        let plan = accepted_claim_plan(accepted_plan)?;
        if accepted_transition.status() != FederationOperationStatus::Applied {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let Some(FederationBarkOperation::ActionTransition(transition)) =
            bark_operation(accepted_transition)
        else {
            return Err(Error::ReceiveClaimPlanInvalid);
        };
        let expected_signers =
            federation_bark_signer_set_for_attempt(&self.federation, transition.attempt)
                .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        if transition.action_id != plan.action_id
            || transition.quote.as_deref() != plan.quote.as_deref()
            || transition.plan_operation_id != accepted_plan.operation_id
            || transition.payment_hash != plan.payment_hash
            || transition.attempt < plan.attempt
            || transition.signer_set != expected_signers
        {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let phase = match transition.phase {
            FederationBarkSemanticPhase::ReceivePreimageDisclosed => {
                FederationLightningReceivePhase::PreimageRevealed
            }
            FederationBarkSemanticPhase::ReceiveClaimed => FederationLightningReceivePhase::Claimed,
            _ => return Err(Error::ReceiveClaimPlanInvalid),
        };
        let (receive, claim) = self.accepted_receive_claim_plan(intent, prepared, &plan)?;
        let outputs = if phase == FederationLightningReceivePhase::Claimed {
            receive_claim_outputs(&transition, &claim)?
        } else {
            Vec::new()
        };
        self.wallet
            .apply_lightning_receive_claim_replay(&receive, &claim, phase, &outputs)
            .await
            .map_err(|_| Error::ReceiveClaimRecoveryRequired)?;
        if phase == FederationLightningReceivePhase::Claimed {
            self.wallet
                .acknowledge_lightning_receive_claim(&receive)
                .await
                .map_err(|_| Error::ReceiveClaimRecoveryRequired)?;
        }
        Ok(())
    }

    /// Execute an already applied claim plan through threshold custody and
    /// return only semantic transition candidates. An error after Bark's
    /// durable preimage checkpoint is converted to `RecoveryRequired` so the
    /// caller cannot reinterpret the quote as unpaid.
    pub async fn execute_receive_claim(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
        accepted_plan: FederationJournalEntry,
        accepted_authorization: FederationJournalEntry,
    ) -> Result<FederatedBarkReceiveClaimOutcome, Error> {
        if accepted_plan.status() != FederationOperationStatus::Applied {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let Some(FederationBarkOperation::ActionPlan(plan)) = bark_operation(&accepted_plan) else {
            return Err(Error::ReceiveClaimPlanInvalid);
        };
        let claim = self
            .validate_receive_claim_plan(intent, prepared, &plan)
            .await?;
        let receive = self.receive_plan(intent, prepared)?;
        let operation = FederationBarkOperation::ActionPlan(plan.clone());
        let plan_commitment = operation
            .commitment()
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        let action_id = FederationActionId::from_bytes(plan.action_id.to_bytes())
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        let request_commitment =
            FederationRequestCommitment::from_bytes(plan_commitment.to_bytes())
                .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        let authorization_context = plan_commitment.to_bytes().to_vec();
        let custody_attempt = accepted_custody_attempt(&accepted_plan, &accepted_authorization)?;
        self.custody
            .activate_claim(
                accepted_plan.clone(),
                custody_attempt.entry.clone(),
                intent.destination,
                authorization_context.clone(),
            )
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        let execution = FederationLightningReceiveClaimExecution {
            external_action_id: action_id,
            request_commitment,
            authorization_context,
        };
        let result = self
            .wallet
            .execute_lightning_receive_claim(&receive, &claim, &execution)
            .await;
        let signer_unavailable = self.custody.take_signer_unavailable();
        let disclosed = match &result {
            Ok(_) => true,
            Err(_) => matches!(
                self.wallet.lightning_receive_phase(&receive).await,
                Ok(FederationLightningReceivePhase::PreimageRevealed)
            ),
        };
        self.custody.deactivate_claim();
        if !disclosed {
            return match result {
                Err(error) => Err(classify_receive_claim_error(&error, signer_unavailable)),
                Ok(_) => Err(Error::ReceiveClaimPlanInvalid),
            };
        }
        let preimage_disclosed = receive_preimage_disclosed_transition(
            self.member_id,
            &accepted_plan,
            intent,
            &plan,
            &claim,
            plan_commitment,
            &custody_attempt,
        )?;
        match result {
            Ok(outputs) => Ok(FederatedBarkReceiveClaimOutcome::Claimed {
                preimage_disclosed,
                result: Box::new(receive_claim_result(
                    &accepted_plan,
                    intent,
                    &plan,
                    &claim,
                    outputs,
                    plan_commitment,
                    &custody_attempt,
                )?),
            }),
            Err(_) => Ok(FederatedBarkReceiveClaimOutcome::RecoveryRequired {
                preimage_disclosed,
                signer_unavailable,
            }),
        }
    }

    /// Bind a verified local claim result to the now-applied disclosure
    /// transition. This enforces the ordered `Reserved -> PendingSpend ->
    /// Spent` projection rather than skipping the point-of-no-return record.
    pub fn finalize_receive_claim(
        &self,
        accepted_preimage: &FederationJournalEntry,
        result: FederatedBarkReceiveClaimResult,
    ) -> Result<FederationBarkActionTransition, Error> {
        if accepted_preimage.status() != FederationOperationStatus::Applied {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let Some(FederationBarkOperation::ActionTransition(disclosed)) =
            bark_operation(accepted_preimage)
        else {
            return Err(Error::ReceiveClaimPlanInvalid);
        };
        if disclosed.phase != FederationBarkSemanticPhase::ReceivePreimageDisclosed
            || disclosed.action_id != result.action_id
            || disclosed.plan_operation_id != result.plan_operation_id
        {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        self.prepare_receive_claim_successor(&disclosed, accepted_preimage.operation_id, result)
    }

    /// Build the claim successor with its original disclosure evidence.
    /// At the disclosure's attempt both transitions can be ordered atomically;
    /// a later signing attempt instead follows its accepted custody operation.
    pub fn prepare_receive_claim_successor(
        &self,
        disclosed: &FederationBarkActionTransition,
        disclosed_operation_id: FederationOperationId,
        result: FederatedBarkReceiveClaimResult,
    ) -> Result<FederationBarkActionTransition, Error> {
        if disclosed.phase != FederationBarkSemanticPhase::ReceivePreimageDisclosed
            || disclosed.action_id != result.action_id
            || disclosed.plan_operation_id != result.plan_operation_id
            || result.attempt < disclosed.attempt
        {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        // A later signing attempt extends the accepted disclosure's lineage.
        // Keep its evidence binding, but follow the custody operation that
        // authorized this result rather than forking the original disclosure.
        let predecessor_operation_id = if result.attempt > disclosed.attempt {
            result.custody_operation_id
        } else {
            disclosed_operation_id
        };
        let mut transitions = result.input_transitions;
        transitions.extend(result.output_transitions);
        transitions.sort();
        Ok(FederationBarkActionTransition {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: result.action_id,
            quote: Some(result.quote),
            phase: FederationBarkSemanticPhase::ReceiveClaimed,
            actor: self.member_id,
            plan_operation_id: result.plan_operation_id,
            predecessor_operation_id,
            payment_hash: Some(result.payment_hash),
            attempt: result.attempt,
            signer_set: result.signer_set,
            request_hash: disclosed.result_hash,
            result_hash: result.result_hash,
            evidence_certificate: None,
            external_id: receive_external_id(
                b"cdk/bark/receive-claimed-external/v1",
                result.action_id,
                disclosed_operation_id,
                result.attempt,
            )?,
            artifacts: result.artifacts,
            vtxo_transitions: transitions,
        })
    }

    /// Acknowledge that the terminal claim transition is durably accepted and
    /// allow Bark to remove its successful-result retry checkpoint.
    pub async fn acknowledge_receive_claim(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
    ) -> Result<(), Error> {
        let receive = self.receive_plan(intent, prepared)?;
        self.wallet
            .acknowledge_lightning_receive_claim(&receive)
            .await
            .map_err(|_| Error::ReceiveClaimRecoveryRequired)
    }

    /// Independently validate every spendable VTXO in an accepted claim
    /// candidate before this member attests to the result. The transition's
    /// opaque bytes are not trusted: they must canonically decode to the exact
    /// ID, amount, and locator in the semantic mutation, use the configured
    /// Ark key, and pass Bark's complete signed-chain validation.
    pub async fn validate_receive_claim_result(
        &self,
        transition: &FederationBarkActionTransition,
    ) -> Result<cdk_common::Amount, Error> {
        if transition.phase != FederationBarkSemanticPhase::ReceiveClaimed {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let outputs = transition
            .vtxo_transitions
            .iter()
            .filter(|mutation| {
                mutation.previous_state.is_none()
                    && mutation.new_state == FederationBarkVtxoState::Spendable
            })
            .collect::<Vec<_>>();
        if outputs.is_empty() {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let mut ids = BTreeSet::new();
        let mut amount = cdk_common::Amount::ZERO;
        for output in outputs {
            if !ids.insert(output.vtxo_id) {
                return Err(Error::ReceiveClaimPlanInvalid);
            }
            let mut artifacts = transition.artifacts.iter().filter(|artifact| {
                artifact.kind == FederationBarkArtifactKind::Vtxo
                    && artifact.hash == output.artifact_hash
            });
            let artifact = artifacts.next().ok_or(Error::ReceiveClaimPlanInvalid)?;
            if artifacts.next().is_some() || artifact.hash != bark_digest(&artifact.bytes)? {
                return Err(Error::ReceiveClaimPlanInvalid);
            }
            let vtxo = bark::ark::vtxo::Vtxo::<bark::ark::vtxo::Full>::deserialize(&artifact.bytes)
                .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
            if vtxo.serialize() != artifact.bytes
                || vtxo.id().to_bytes() != output.vtxo_id.to_bytes()
                || vtxo.amount().to_sat() != u64::from(output.amount)
            {
                return Err(Error::ReceiveClaimPlanInvalid);
            }
            self.wallet
                .validate_claimed_vtxo(
                    VtxoKeyLocator {
                        index: output.locator.index,
                    },
                    &vtxo,
                )
                .await
                .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
            amount = amount
                .checked_add(output.amount)
                .ok_or(Error::ReceiveClaimPlanInvalid)?;
        }
        Ok(amount)
    }

    fn claim_action_plan(
        &self,
        intent: &FederationBarkReceiveIntent,
        ready_operation_id: FederationOperationId,
        actor: MemberId,
        mut signer_set: Vec<MemberId>,
        output_locator: FederationBarkVtxoLocator,
        claim: &FederationLightningReceiveClaimPlan,
    ) -> Result<FederationBarkActionPlan, Error> {
        if output_locator.role != 1 || output_locator.version != 1 {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        signer_set.sort();
        signer_set.dedup();
        if signer_set.len() < usize::from(self.application.threshold) {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let mut inputs = claim
            .input_vtxos
            .iter()
            .map(|vtxo| {
                Ok(FederationBarkVtxoInput {
                    vtxo_id: FederationBarkVtxoId::from_bytes(vtxo.id().to_bytes())
                        .map_err(|_| Error::ReceiveClaimPlanInvalid)?,
                    locator: intent.destination,
                    amount: cdk_common::Amount::from(vtxo.amount().to_sat()),
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        inputs.sort();
        let request_bytes = receive_claim_request_bytes(claim);
        let request_artifact =
            bark_artifact(FederationBarkArtifactKind::ArkRequest, request_bytes)?;
        let mut artifacts = claim
            .input_vtxos
            .iter()
            .map(|vtxo| bark_artifact(FederationBarkArtifactKind::Vtxo, vtxo.serialize()))
            .collect::<Result<Vec<_>, _>>()?;
        artifacts.push(request_artifact.clone());
        artifacts.sort();
        Ok(FederationBarkActionPlan {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: intent.action_id,
            quote: Some(intent.quote.clone()),
            kind: FederationBarkActionKind::IncomingClaim,
            actor,
            predecessor_operation_id: ready_operation_id,
            payment_hash: Some(intent.payment_hash),
            inputs,
            outputs: vec![FederationBarkVtxoOutput {
                locator: output_locator,
                amount: cdk_common::Amount::from(claim.amount.to_sat()),
                artifact_hash: request_artifact.hash,
            }],
            revocation_locator: None,
            external_amount: cdk_common::Amount::ZERO,
            fee_amount: cdk_common::Amount::ZERO,
            fee_bound: cdk_common::Amount::ZERO,
            certified_receive_htlcs: None,
            artifacts,
            external_id: receive_external_id(
                b"cdk/bark/receive-claim-external/v1",
                intent.action_id,
                ready_operation_id,
                claim.custody_attempt,
            )?,
            signer_set,
            attempt: claim.custody_attempt,
            expires_at: u64::from(claim.minimum_expiry),
        })
    }

    fn bark_receive_intent(
        &self,
        intent_operation_id: FederationOperationId,
        intent: &FederationBarkReceiveIntent,
        preimage: [u8; 32],
    ) -> Result<FederationLightningReceiveIntent, Error> {
        let action_id = FederationActionId::from_bytes(intent.action_id.to_bytes())
            .map_err(|_| Error::InvoicePreparationFailed)?;
        let request_commitment = FederationRequestCommitment::from_bytes(
            federation_bark_receive_request_hash(intent_operation_id, intent)
                .map_err(|_| Error::InvoicePreparationFailed)?
                .to_bytes(),
        )
        .map_err(|_| Error::InvoicePreparationFailed)?;
        let mailbox_public_key = PublicKey::from_slice(&intent.mailbox_id)
            .map_err(|_| Error::InvoicePreparationFailed)?;
        let htlc_recv_cltv_delta = u16::try_from(intent.minimum_cltv_delta)
            .map_err(|_| Error::InvoicePreparationFailed)?;
        Ok(FederationLightningReceiveIntent {
            action_id,
            request_commitment,
            payment_hash: PaymentHash::from(intent.payment_hash.to_bytes()),
            cashu_amount: bitcoin::Amount::from_sat(u64::from(intent.cashu_amount)),
            gross_amount: bitcoin::Amount::from_sat(u64::from(intent.gross_amount)),
            receive_fee_bound: bitcoin::Amount::from_sat(u64::from(intent.receive_fee)),
            minimum_net_amount: bitcoin::Amount::from_sat(u64::from(intent.minimum_net_amount)),
            description: intent.description.clone(),
            payment_preimage: Preimage::from(preimage),
            locator: VtxoKeyLocator {
                index: intent.destination.index,
            },
            mailbox_id: MailboxIdentifier::from_pubkey(mailbox_public_key),
            htlc_recv_cltv_delta,
            anti_dos: LightningReceiveAntiDosMode::NoProof,
            policy_version: intent.fee_policy_version,
            custody_attempt: 1,
        })
    }
}
