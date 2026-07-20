//! Member-local Bark wallet runtime and bounded readiness reporting.

use core::fmt;
use std::sync::Arc;

use bark::actions::lightning::receive::LightningReceiveAntiDosMode;
use bark::ark::lightning::{PaymentHash, Preimage};
use bark::ark::mailbox::MailboxIdentifier;
use bark::ark::ProtocolEncoding;
use bark::custody::{KeySourceDescriptor, KeySourceKind, VtxoCustody, VtxoKeyLocator};
use bark::federation::{
    FederationActionId, FederationLightningReceiveClaimPlan, FederationLightningReceiveIntent,
    FederationLightningReceivePlan, FederationLightningReceiveStatus, FederationRequestCommitment,
    FederationWallet,
};
use bark::{OpenWalletArgs, Wallet, WalletProperties};
use bitcoin::secp256k1::PublicKey;
use cdk_common::federation::{
    federation_bark_receive_request_hash, FederationBarkActionKind, FederationBarkActionPlan,
    FederationBarkActionTransition, FederationBarkArtifact, FederationBarkArtifactKind,
    FederationBarkDigest, FederationBarkExternalId, FederationBarkMemberObservation,
    FederationBarkObservationStatus, FederationBarkReceiveIntent, FederationBarkReceivePrepared,
    FederationBarkReceiveSecretBinding, FederationBarkSemanticPhase, FederationBarkVtxoId,
    FederationBarkVtxoInput, FederationBarkVtxoLocator, FederationBarkVtxoOutput,
    FederationBarkVtxoState, FederationBarkVtxoTransition, FederationConfig,
    FederationOperationId, MemberId, FEDERATION_BARK_OPERATION_VERSION,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::{
    CdkFrostVtxoCustody, Error, FederatedBarkLimits, FederatedBarkLocalConfig,
    FederatedBarkReceiveSecretStore, FederatedBarkScope, ValidatedFederatedBarkConfig,
};

/// Whether member-local Bark state can serve an accepted frontier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederatedBarkReadiness {
    /// Local projection is valid and at or beyond the requested frontier.
    Ready,
    /// Local projection is valid but has not replayed the requested frontier.
    BehindConsensus,
    /// Local state or a required dependency is unavailable.
    Degraded,
}

/// Bounded reason for degraded operation; never an external error message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederatedBarkDegradedReason {
    /// Bark's member-local execution wallet is unavailable.
    WalletUnavailable,
    /// Deterministic accepted-state projection is invalid.
    ProjectionInvalid,
    /// The selected participant set cannot satisfy the operation.
    ParticipantsUnavailable,
}

/// Public, redacted runtime status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederatedBarkRuntimeStatus {
    /// Current readiness for the requested frontier.
    pub readiness: FederatedBarkReadiness,
    /// Highest locally replayed accepted frontier.
    pub local_frontier: u64,
    /// Bounded degraded reason, if any.
    pub degraded_reason: Option<FederatedBarkDegradedReason>,
}

#[derive(Debug, Clone, Copy)]
struct RuntimeHealth {
    local_frontier: u64,
    degraded_reason: Option<FederatedBarkDegradedReason>,
}

/// Restricted member-local Bark execution runtime.
pub struct FederatedBarkRuntime {
    scope: FederatedBarkScope,
    member_id: MemberId,
    federation: FederationConfig,
    limits: FederatedBarkLimits,
    wallet: FederationWallet,
    receive_secrets: Arc<FederatedBarkReceiveSecretStore>,
    health: RwLock<RuntimeHealth>,
}

impl fmt::Debug for FederatedBarkRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FederatedBarkRuntime")
            .field("scope", &self.scope)
            .field("limits", &self.limits)
            .field("wallet", &"<member-local Bark projection>")
            .field("receive_secrets", &"<encrypted member-local store>")
            .field("health", &"<bounded runtime status>")
            .finish()
    }
}

impl FederatedBarkRuntime {
    /// Validate all bindings and open a fresh or existing FROST-custodied Bark wallet.
    pub async fn open(
        federation_enabled: bool,
        federation: &FederationConfig,
        member_id: MemberId,
        local: FederatedBarkLocalConfig,
    ) -> Result<Arc<Self>, Error> {
        let validated =
            ValidatedFederatedBarkConfig::validate(federation_enabled, federation, &local)?;
        if !federation.members.contains_key(&member_id) {
            return Err(Error::InvalidPublicConfig);
        }
        let receive_secrets = Arc::new(FederatedBarkReceiveSecretStore::open(
            &local.receive_secret_store.database_path,
            Arc::clone(&local.receive_secret_store.encryption_key),
        )?);
        let custody = Arc::new(CdkFrostVtxoCustody::new(validated.clone()));
        validated.validate_descriptor(&custody.descriptor())?;
        let expected_operational_fingerprint = local.operational_seed.fingerprint();
        let expected_descriptor = custody.descriptor();
        let expected_server = validated.ark_server_public_key();
        let limits = local.limits;
        let wallet = Wallet::open_with_custody(
            validated.network(),
            local.operational_seed,
            custody,
            local.bark,
            OpenWalletArgs {
                run_daemon: false,
                datadir: Some(local.data_directory),
                create_if_not_exists: local.create_if_missing,
                create_without_server: false,
                ..OpenWalletArgs::default()
            },
        )
        .await
        .map_err(|_| Error::WalletOpenFailed)?;
        let properties = wallet
            .properties()
            .await
            .map_err(|_| Error::WalletPropertiesUnavailable)?;
        validate_wallet_properties(
            &properties,
            validated.network(),
            expected_operational_fingerprint,
            &expected_descriptor,
            expected_server,
        )?;
        let ark_info = wallet
            .require_ark_info()
            .await
            .map_err(|_| Error::ArkCapabilityMismatch)?;
        if ark_info.network != validated.network()
            || ark_info.server_pubkey != expected_server
            || !ark_info.federation_receive_idempotency
            || ark_info.ln_receive_anti_dos_required
            || ark_info.fees.lightning_receive.base_fee != bitcoin::Amount::ZERO
            || ark_info.fees.lightning_receive.ppm.0 != 0
            || u32::from(ark_info.max_user_invoice_cltv_delta)
                < validated.policy().minimum_incoming_cltv_delta
            || ark_info
                .max_vtxo_amount
                .is_some_and(|maximum| maximum.to_sat() < limits.maximum_sat())
        {
            return Err(Error::ArkCapabilityMismatch);
        }
        let wallet = FederationWallet::new(&wallet)
            .await
            .map_err(|_| Error::FederationFacadeUnavailable)?;
        Ok(Arc::new(Self {
            scope: validated.scope(),
            member_id,
            federation: federation.clone(),
            limits,
            wallet,
            receive_secrets,
            health: RwLock::new(RuntimeHealth {
                local_frontier: 0,
                degraded_reason: None,
            }),
        }))
    }

    /// Immutable accepted runtime scope.
    #[must_use]
    pub const fn scope(&self) -> FederatedBarkScope {
        self.scope
    }

    /// Startup-validated satoshi payment limits.
    #[must_use]
    pub const fn limits(&self) -> FederatedBarkLimits {
        self.limits
    }

    /// Local federation member represented by this runtime.
    #[must_use]
    pub const fn member_id(&self) -> MemberId {
        self.member_id
    }

    /// Encrypted member-local receive-secret store.
    #[must_use]
    pub fn receive_secret_store(&self) -> Arc<FederatedBarkReceiveSecretStore> {
        Arc::clone(&self.receive_secrets)
    }

    /// Purely derive one accepted locator without database or network I/O.
    pub fn derive_public_key(&self, index: u32) -> Result<PublicKey, Error> {
        self.wallet
            .derive_locator(VtxoKeyLocator { index })
            .map(|derived| derived.public_key)
            .map_err(|_| Error::PublicKeyDerivationFailed)
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
    pub async fn observe_receive_htlcs(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared_operation_id: FederationOperationId,
        prepared: &FederationBarkReceivePrepared,
        observation_epoch: u64,
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
        let observation = self
            .wallet
            .poll_lightning_receive(&receive)
            .await
            .map_err(|_| Error::ReceiveObservationFailed)?;
        match observation.status {
            FederationLightningReceiveStatus::AwaitingPayment
            | FederationLightningReceiveStatus::Canceled => return Ok(None),
            FederationLightningReceiveStatus::Settled => {
                return Err(Error::ReceiveClaimRecoveryRequired)
            }
            FederationLightningReceiveStatus::Accepted
            | FederationLightningReceiveStatus::HtlcsReady => {}
        }
        let evidence = self
            .wallet
            .prepare_lightning_receive_htlcs(&receive)
            .await
            .map_err(|_| Error::ReceiveObservationFailed)?;
        let result_hash = receive_htlc_evidence_hash(&evidence)?;
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
            result_hash,
        };
        let transition = receive_htlc_transition(
            self.member_id,
            prepared_operation_id,
            intent,
            &evidence,
            result_hash,
        )?;
        Ok(Some((member_observation, transition)))
    }

    /// Construct one exact incoming-claim action plan from the local copy of
    /// already accepted HTLC evidence. This performs no custody operation and
    /// does not disclose the preimage.
    pub async fn propose_receive_claim(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
        ready_operation_id: FederationOperationId,
        actor: MemberId,
        signer_set: Vec<MemberId>,
        attempt: u32,
        output_locator: FederationBarkVtxoLocator,
    ) -> Result<FederationBarkActionPlan, Error> {
        let receive = self.receive_plan(intent, prepared)?;
        let evidence = self
            .wallet
            .prepare_lightning_receive_htlcs(&receive)
            .await
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        let claim = self
            .wallet
            .plan_lightning_receive_claim(
                evidence,
                VtxoKeyLocator {
                    index: output_locator.index,
                },
                attempt,
            )
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        self.claim_action_plan(
            intent,
            ready_operation_id,
            actor,
            signer_set,
            output_locator,
            &claim,
        )
    }

    /// Reconstruct and compare every field of an incoming claim plan before it
    /// can authorize FROST or virtual MuSig2.
    pub async fn validate_receive_claim_plan(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
        plan: &FederationBarkActionPlan,
    ) -> Result<FederationLightningReceiveClaimPlan, Error> {
        if plan.kind != FederationBarkActionKind::IncomingClaim
            || plan.action_id != intent.action_id
            || plan.quote.as_deref() != Some(intent.quote.as_str())
            || plan.payment_hash != Some(intent.payment_hash)
            || plan.outputs.len() != 1
        {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let receive = self.receive_plan(intent, prepared)?;
        let evidence = self
            .wallet
            .prepare_lightning_receive_htlcs(&receive)
            .await
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        let output = plan
            .outputs
            .first()
            .ok_or(Error::ReceiveClaimPlanInvalid)?;
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
        self.wallet
            .validate_lightning_receive_claim_plan(&receive, &claim)
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        let expected = self.claim_action_plan(
            intent,
            plan.predecessor_operation_id,
            plan.actor,
            plan.signer_set.clone(),
            output.locator,
            &claim,
        )?;
        if &expected != plan {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        Ok(claim)
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
        if signer_set.len() < usize::from(
            self.federation
                .frost_wallet
                .as_ref()
                .and_then(|wallet| wallet.bark_application.as_ref())
                .ok_or(Error::MissingApplication)?
                .threshold,
        ) {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let mut inputs = claim
            .input_vtxos
            .iter()
            .map(|vtxo| {
                Ok(FederationBarkVtxoInput {
                    vtxo_id: FederationBarkVtxoId::from_bytes(vtxo.id().to_bytes())
                        .map_err(|_| Error::ReceiveClaimPlanInvalid)?,
                    amount: cdk_common::Amount::from(vtxo.amount().to_sat()),
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        inputs.sort();
        let request_bytes = receive_claim_request_bytes(claim);
        let request_artifact = bark_artifact(
            FederationBarkArtifactKind::ArkRequest,
            request_bytes,
        )?;
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
            external_amount: cdk_common::Amount::ZERO,
            fee_amount: cdk_common::Amount::ZERO,
            fee_bound: cdk_common::Amount::ZERO,
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

    /// Return readiness relative to one accepted frontier.
    pub async fn status(&self, required_frontier: u64) -> FederatedBarkRuntimeStatus {
        let health = *self.health.read().await;
        let readiness = match health.degraded_reason {
            Some(_) => FederatedBarkReadiness::Degraded,
            None if health.local_frontier < required_frontier => {
                FederatedBarkReadiness::BehindConsensus
            }
            None => FederatedBarkReadiness::Ready,
        };
        FederatedBarkRuntimeStatus {
            readiness,
            local_frontier: health.local_frontier,
            degraded_reason: health.degraded_reason,
        }
    }

    /// Fail closed unless this member can serve the required accepted frontier.
    pub async fn ensure_ready(&self, required_frontier: u64) -> Result<(), Error> {
        match self.status(required_frontier).await.readiness {
            FederatedBarkReadiness::Ready => Ok(()),
            FederatedBarkReadiness::BehindConsensus | FederatedBarkReadiness::Degraded => {
                Err(Error::NotReady)
            }
        }
    }

    /// Return the current bounded degradation reason.
    pub async fn degraded_reason(&self) -> Option<FederatedBarkDegradedReason> {
        self.health.read().await.degraded_reason
    }

    /// Advance the member-local replay frontier monotonically.
    pub async fn record_projection_frontier(&self, frontier: u64) {
        let mut health = self.health.write().await;
        health.local_frontier = health.local_frontier.max(frontier);
    }

    /// Mark the runtime degraded with a bounded, non-sensitive reason.
    pub async fn mark_degraded(&self, reason: FederatedBarkDegradedReason) {
        self.health.write().await.degraded_reason = Some(reason);
    }
}

fn receive_htlc_evidence_hash(
    evidence: &bark::federation::FederationLightningReceiveHtlcEvidence,
) -> Result<FederationBarkDigest, Error> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"cdk/bark/receive-htlc-evidence/v1");
    bytes.extend_from_slice(&evidence.action_id.to_bytes());
    bytes.extend_from_slice(&evidence.payment_hash.to_vec());
    bytes.extend_from_slice(&evidence.destination.index.to_be_bytes());
    bytes.extend_from_slice(&evidence.gross_amount.to_sat().to_be_bytes());
    bytes.extend_from_slice(&evidence.net_amount.to_sat().to_be_bytes());
    bytes.extend_from_slice(&evidence.fee.to_sat().to_be_bytes());
    bytes.extend_from_slice(&evidence.minimum_expiry.to_be_bytes());
    for vtxo in &evidence.vtxos {
        append_bounded_bytes(&mut bytes, &vtxo.serialize())?;
    }
    bark_digest(&bytes)
}

fn receive_htlc_transition(
    actor: MemberId,
    prepared_operation_id: FederationOperationId,
    intent: &FederationBarkReceiveIntent,
    evidence: &bark::federation::FederationLightningReceiveHtlcEvidence,
    result_hash: FederationBarkDigest,
) -> Result<FederationBarkActionTransition, Error> {
    let mut artifacts = evidence
        .vtxos
        .iter()
        .map(|vtxo| bark_artifact(FederationBarkArtifactKind::Vtxo, vtxo.serialize()))
        .collect::<Result<Vec<_>, _>>()?;
    artifacts.sort();
    let mut vtxo_transitions = evidence
        .vtxos
        .iter()
        .map(|vtxo| {
            let bytes = vtxo.serialize();
            Ok(FederationBarkVtxoTransition {
                vtxo_id: FederationBarkVtxoId::from_bytes(vtxo.id().to_bytes())
                    .map_err(|_| Error::ReceiveObservationFailed)?,
                locator: intent.destination,
                amount: cdk_common::Amount::from(vtxo.amount().to_sat()),
                previous_state: None,
                new_state: FederationBarkVtxoState::Observed,
                artifact_hash: bark_digest(&bytes)?,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    vtxo_transitions.sort();
    let mut request = Vec::new();
    request.extend_from_slice(b"cdk/bark/receive-htlc-request/v1");
    request.extend_from_slice(&prepared_operation_id.to_bytes());
    request.extend_from_slice(&intent.action_id.to_bytes());
    request.extend_from_slice(&intent.destination.index.to_be_bytes());
    request.extend_from_slice(&evidence.minimum_expiry.to_be_bytes());
    Ok(FederationBarkActionTransition {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: intent.action_id,
        quote: Some(intent.quote.clone()),
        phase: FederationBarkSemanticPhase::ReceiveHtlcsReady,
        actor,
        plan_operation_id: prepared_operation_id,
        predecessor_operation_id: prepared_operation_id,
        payment_hash: Some(intent.payment_hash),
        attempt: 1,
        signer_set: Vec::new(),
        request_hash: bark_digest(&request)?,
        result_hash,
        external_id: receive_external_id(
            b"cdk/bark/receive-htlc-external/v1",
            intent.action_id,
            prepared_operation_id,
            1,
        )?,
        artifacts,
        vtxo_transitions,
    })
}

fn receive_claim_request_bytes(claim: &FederationLightningReceiveClaimPlan) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"cdk/bark/receive-claim-request/v1");
    bytes.extend_from_slice(&claim.action_id.to_bytes());
    bytes.extend_from_slice(&claim.payment_hash.to_vec());
    bytes.extend_from_slice(&claim.destination.locator.index.to_be_bytes());
    bytes.extend_from_slice(&claim.destination.public_key.serialize());
    bytes.extend_from_slice(&claim.amount.to_sat().to_be_bytes());
    bytes.extend_from_slice(&claim.minimum_expiry.to_be_bytes());
    bytes.extend_from_slice(&claim.custody_attempt.to_be_bytes());
    for vtxo in &claim.input_vtxos {
        let encoded = vtxo.serialize();
        bytes.extend_from_slice(&(encoded.len() as u64).to_be_bytes());
        bytes.extend_from_slice(&encoded);
    }
    bytes
}

fn bark_artifact(
    kind: FederationBarkArtifactKind,
    bytes: Vec<u8>,
) -> Result<FederationBarkArtifact, Error> {
    Ok(FederationBarkArtifact {
        kind,
        hash: bark_digest(&bytes)?,
        bytes,
    })
}

fn bark_digest(bytes: &[u8]) -> Result<FederationBarkDigest, Error> {
    FederationBarkDigest::from_bytes(Sha256::digest(bytes).into())
        .map_err(|_| Error::ReceiveClaimPlanInvalid)
}

fn receive_external_id(
    domain: &[u8],
    action_id: cdk_common::federation::FederationBarkActionId,
    predecessor: FederationOperationId,
    attempt: u32,
) -> Result<FederationBarkExternalId, Error> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(&action_id.to_bytes());
    bytes.extend_from_slice(&predecessor.to_bytes());
    bytes.extend_from_slice(&attempt.to_be_bytes());
    FederationBarkExternalId::from_bytes(Sha256::digest(bytes).into())
        .map_err(|_| Error::ReceiveClaimPlanInvalid)
}

fn append_bounded_bytes(target: &mut Vec<u8>, bytes: &[u8]) -> Result<(), Error> {
    let length = u64::try_from(bytes.len()).map_err(|_| Error::ReceiveObservationFailed)?;
    target.extend_from_slice(&length.to_be_bytes());
    target.extend_from_slice(bytes);
    Ok(())
}

pub(crate) fn validate_wallet_properties(
    properties: &WalletProperties,
    expected_network: bitcoin::Network,
    expected_operational_fingerprint: bitcoin::bip32::Fingerprint,
    expected_descriptor: &KeySourceDescriptor,
    expected_server: PublicKey,
) -> Result<(), Error> {
    if properties.network != expected_network {
        return Err(Error::ConfigMismatch {
            field: "wallet_network",
        });
    }
    if properties.fingerprint != expected_operational_fingerprint {
        return Err(Error::ConfigMismatch {
            field: "operational_seed",
        });
    }
    let descriptor = properties
        .key_source
        .as_ref()
        .ok_or(Error::StandaloneCustodyForbidden)?;
    if descriptor.kind() != KeySourceKind::Frost {
        return Err(Error::StandaloneCustodyForbidden);
    }
    if descriptor != expected_descriptor {
        return Err(Error::CustodyDescriptorMismatch);
    }
    if properties.server_pubkey != Some(expected_server) {
        return Err(Error::ConfigMismatch {
            field: "ark_server_public_key",
        });
    }
    if properties.server_mailbox_pubkey.is_none() {
        return Err(Error::ConfigMismatch {
            field: "ark_server_mailbox_public_key",
        });
    }
    Ok(())
}
