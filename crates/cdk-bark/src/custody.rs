//! Bark custody adapter rooted in the accepted CDK FROST application.

use core::fmt;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bark::ark::mailbox::BlindedMailboxIdentifier;
use bark::ark::musig::PartialSignature;
use bark::custody::{
    AbortReason, CustodySessionId, DerivedVtxoPublicKey, KeySourceDescriptor,
    MailboxBlindingRequest, Musig2Commitment, Musig2CommitmentRequest, Musig2SigningRequest,
    SchnorrSigningRequest, VtxoCustody, VtxoKeyLocator,
};
use bitcoin::secp256k1::schnorr;
use cdk_common::federation::{FederationConfig, FederationJournalEntry, FederationOperationStatus};
use cdk_federation_bark::{
    federation_bark_application, federation_bark_custody_transcript_hash,
    validate_federation_bark_operation, FederationBarkActionId, FederationBarkActionKind,
    FederationBarkAuthorizationReference, FederationBarkCustodyProtocol,
    FederationBarkFrostAuthorization, FederationBarkOperation, FederationBarkSemanticPhase,
    FederationBarkVtxoId, FederationBarkVtxoLocator, FEDERATION_BARK_AUTHORIZATION_VERSION,
};
use cdk_frost::{
    AggregateSignature, AuthorizationRequest, CustodyProtocol, NonceBurnReason,
    SigningAuthorizationScope, SigningDerivation, SigningSessionContext,
    VirtualMusigNonceCommitmentSet, VirtualMusigTranscript,
};
use cdk_frost_bark::BarkKeyLocator;
use futures::stream::{self, StreamExt};

use crate::{Error, ValidatedFederatedBarkConfig};

/// Maximum number of virtual MuSig2 inputs signed concurrently in one batch.
const MAX_CONCURRENT_VIRTUAL_MUSIG_INPUTS: usize = 8;

/// Input descriptor for one virtual MuSig2 commitment in a multi-input batch.
#[derive(Debug, Clone)]
pub struct VirtualMusigCommitmentInput {
    /// Accepted signing session context for this exact input.
    pub context: SigningSessionContext,
    /// Public signing derivation for this input's VTXO key.
    pub derivation: SigningDerivation,
    /// Accepted authorization proof bound to this input.
    pub authorization: AuthorizationRequest,
}

/// Input descriptor for one virtual MuSig2 partial signature in a multi-input batch.
#[derive(Debug, Clone)]
pub struct VirtualMusigPartialInput {
    /// Accepted signing session context for this exact input.
    pub context: SigningSessionContext,
    /// Public signing derivation for this input's VTXO key.
    pub derivation: SigningDerivation,
    /// Accepted authorization proof bound to this input.
    pub authorization: AuthorizationRequest,
    /// Commitment set collected during round 1 for this exact input.
    pub commitments: VirtualMusigNonceCommitmentSet,
    /// Outer MuSig2 transcript binding this exact input and server nonce.
    pub transcript: VirtualMusigTranscript,
}

/// Network coordinator used by the Bark custody adapter after an exact action
/// plan has been accepted.
#[async_trait]
pub trait FederatedBarkCustodyClient: fmt::Debug + Send + Sync {
    /// Produce one ordinary threshold Schnorr signature.
    async fn sign_frost(
        &self,
        context: &SigningSessionContext,
        derivation: &SigningDerivation,
        authorization: &AuthorizationRequest,
    ) -> anyhow::Result<AggregateSignature>;

    /// Collect the accepted signer set's virtual MuSig2 commitments.
    async fn collect_virtual_musig_commitments(
        &self,
        context: &SigningSessionContext,
        derivation: &SigningDerivation,
        authorization: &AuthorizationRequest,
    ) -> anyhow::Result<VirtualMusigNonceCommitmentSet>;

    /// Produce the virtual threshold signer's finalized outer partial.
    async fn collect_virtual_musig_partial(
        &self,
        context: &SigningSessionContext,
        derivation: &SigningDerivation,
        authorization: &AuthorizationRequest,
        commitments: &VirtualMusigNonceCommitmentSet,
        transcript: &VirtualMusigTranscript,
    ) -> anyhow::Result<[u8; 32]>;

    /// Best-effort burn of protocol nonce state for an abandoned session.
    async fn abort(
        &self,
        context: &SigningSessionContext,
        authorization: &AuthorizationRequest,
        reason: NonceBurnReason,
    );

    /// Collect virtual MuSig2 commitments across multiple distinct inputs in
    /// parallel while preserving each input's independent single-use durable
    /// nonce session. On any failure, already-committed inputs are aborted.
    async fn collect_virtual_musig_commitments_batch(
        &self,
        inputs: &[VirtualMusigCommitmentInput],
    ) -> anyhow::Result<Vec<VirtualMusigNonceCommitmentSet>> {
        let mut futures = Vec::with_capacity(inputs.len());
        for (index, input) in inputs.iter().enumerate() {
            futures.push(async move {
                let result = self
                    .collect_virtual_musig_commitments(
                        &input.context,
                        &input.derivation,
                        &input.authorization,
                    )
                    .await;
                (index, result)
            });
        }
        let mut results = stream::iter(futures)
            .buffer_unordered(MAX_CONCURRENT_VIRTUAL_MUSIG_INPUTS)
            .collect::<Vec<_>>()
            .await;
        results.sort_by_key(|(index, _)| *index);

        let mut first_error = None;
        let mut commitments = Vec::with_capacity(inputs.len());
        let mut succeeded_indices = Vec::new();
        for (index, result) in results {
            match result {
                Ok(set) => {
                    succeeded_indices.push(index);
                    commitments.push(set);
                }
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
        if let Some(error) = first_error {
            for index in succeeded_indices {
                let input = &inputs[index];
                self.abort(
                    &input.context,
                    &input.authorization,
                    NonceBurnReason::Aborted,
                )
                .await;
            }
            return Err(error);
        }
        Ok(commitments)
    }

    /// Collect virtual MuSig2 partial signatures across multiple distinct
    /// inputs in parallel while preserving each input's single-use nonce
    /// commitment binding. On any failure, unfinished inputs are aborted.
    async fn collect_virtual_musig_partial_batch(
        &self,
        inputs: &[VirtualMusigPartialInput],
    ) -> anyhow::Result<Vec<[u8; 32]>> {
        let mut futures = Vec::with_capacity(inputs.len());
        for (index, input) in inputs.iter().enumerate() {
            futures.push(async move {
                let result = self
                    .collect_virtual_musig_partial(
                        &input.context,
                        &input.derivation,
                        &input.authorization,
                        &input.commitments,
                        &input.transcript,
                    )
                    .await;
                (index, result)
            });
        }
        let mut results = stream::iter(futures)
            .buffer_unordered(MAX_CONCURRENT_VIRTUAL_MUSIG_INPUTS)
            .collect::<Vec<_>>()
            .await;
        results.sort_by_key(|(index, _)| *index);

        let mut first_error = None;
        let mut partials = Vec::with_capacity(inputs.len());
        let mut failed_indices = Vec::new();
        for (index, result) in results {
            match result {
                Ok(partial) => partials.push(partial),
                Err(error) => {
                    failed_indices.push(index);
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
        if let Some(error) = first_error {
            for index in failed_indices {
                let input = &inputs[index];
                self.abort(
                    &input.context,
                    &input.authorization,
                    NonceBurnReason::AmbiguousResponse,
                )
                .await;
            }
            return Err(error);
        }
        Ok(partials)
    }
}

#[derive(Debug)]
struct DisabledBarkCustodyClient;

#[async_trait]
impl FederatedBarkCustodyClient for DisabledBarkCustodyClient {
    async fn sign_frost(
        &self,
        _context: &SigningSessionContext,
        _derivation: &SigningDerivation,
        _authorization: &AuthorizationRequest,
    ) -> anyhow::Result<AggregateSignature> {
        Err(Error::ConsensusOnly.into())
    }

    async fn collect_virtual_musig_commitments(
        &self,
        _context: &SigningSessionContext,
        _derivation: &SigningDerivation,
        _authorization: &AuthorizationRequest,
    ) -> anyhow::Result<VirtualMusigNonceCommitmentSet> {
        Err(Error::ConsensusOnly.into())
    }

    async fn collect_virtual_musig_partial(
        &self,
        _context: &SigningSessionContext,
        _derivation: &SigningDerivation,
        _authorization: &AuthorizationRequest,
        _commitments: &VirtualMusigNonceCommitmentSet,
        _transcript: &VirtualMusigTranscript,
    ) -> anyhow::Result<[u8; 32]> {
        Err(Error::ConsensusOnly.into())
    }

    async fn abort(
        &self,
        _context: &SigningSessionContext,
        _authorization: &AuthorizationRequest,
        _reason: NonceBurnReason,
    ) {
    }
}

#[derive(Debug)]
struct AcceptedCustodyAction {
    action_id: FederationBarkActionId,
    plan_entry: FederationJournalEntry,
    authorization_entry: FederationJournalEntry,
    authorized_locators: Vec<FederationBarkVtxoLocator>,
    reserved_vtxos: Vec<FederationBarkVtxoId>,
    operation: bark::custody::VtxoSigningOperation,
    authorization_context: Vec<u8>,
    object_ordinals: BarkCustodyObjectOrdinals,
    signer_unavailable: Arc<AtomicBool>,
}

#[derive(Debug, Default)]
struct CustodySessionRegistry {
    actions: BTreeMap<FederationBarkActionId, AcceptedCustodyAction>,
    reserved_vtxos: BTreeMap<FederationBarkVtxoId, FederationBarkActionId>,
}

#[derive(Debug, Clone)]
struct MusigSession {
    action_id: FederationBarkActionId,
    signer_unavailable: Arc<AtomicBool>,
    request: Musig2CommitmentRequest,
    context: SigningSessionContext,
    derivation: SigningDerivation,
    authorization: AuthorizationRequest,
    commitments: VirtualMusigNonceCommitmentSet,
}

struct AuthorizedRequest {
    action_id: FederationBarkActionId,
    signer_unavailable: Arc<AtomicBool>,
    context: SigningSessionContext,
    derivation: SigningDerivation,
    authorization: AuthorizationRequest,
}

type BarkCustodyObjectOrdinals = BTreeMap<(u8, [u8; 32]), u32>;

/// RAII guard for one active accepted Bark custody action.
///
/// Deactivates only its own `action_id`, releasing its reserved VTXOs and
/// cleaning up only its own in-flight MuSig2 sessions on explicit deactivation
/// or drop (e.g. task cancellation or early return).
#[derive(Debug)]
pub(crate) struct CustodyActionGuard {
    custody: CdkFrostVtxoCustody,
    action_id: FederationBarkActionId,
    signer_unavailable: Arc<AtomicBool>,
    active: bool,
}

impl CustodyActionGuard {
    pub(crate) fn take_signer_unavailable(&self) -> bool {
        self.signer_unavailable.swap(false, Ordering::AcqRel)
    }

    pub(crate) fn deactivate(mut self) {
        if self.active {
            self.active = false;
            self.custody.deactivate_action(self.action_id);
        }
    }
}

impl Drop for CustodyActionGuard {
    fn drop(&mut self) {
        if self.active {
            self.active = false;
            self.custody.deactivate_action(self.action_id);
        }
    }
}

/// Bark [`VtxoCustody`] implementation backed by the registered CDK FROST root.
///
/// Supports concurrent multi-session custody for non-overlapping accepted Bark
/// actions (e.g. concurrent incoming claims, disjoint outgoing melts, and
/// maintenance refreshes). Active actions are indexed by `action_id` with
/// per-VTXO conflict reservation so no two concurrent actions can sign the same
/// VTXO, and per-action `object_ordinals`, `signer_unavailable`, and MuSig2
/// session cleanup so concurrent sessions remain strictly isolated.
#[derive(Clone)]
pub struct CdkFrostVtxoCustody {
    config: ValidatedFederatedBarkConfig,
    federation: Option<FederationConfig>,
    client: Arc<dyn FederatedBarkCustodyClient>,
    registry: Arc<Mutex<CustodySessionRegistry>>,
    musig_sessions: Arc<Mutex<BTreeMap<[u8; 32], MusigSession>>>,
}

impl CdkFrostVtxoCustody {
    /// Construct from fully validated accepted and local configuration.
    #[must_use]
    pub fn new(config: ValidatedFederatedBarkConfig) -> Self {
        Self {
            config,
            federation: None,
            client: Arc::new(DisabledBarkCustodyClient),
            registry: Arc::new(Mutex::new(CustodySessionRegistry::default())),
            musig_sessions: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Construct the production adapter with authenticated peer coordination.
    #[must_use]
    pub fn with_client(
        config: ValidatedFederatedBarkConfig,
        federation: FederationConfig,
        client: Arc<dyn FederatedBarkCustodyClient>,
    ) -> Self {
        Self {
            config,
            federation: Some(federation),
            client,
            registry: Arc::new(Mutex::new(CustodySessionRegistry::default())),
            musig_sessions: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub(crate) fn activate_claim(
        &self,
        plan_entry: FederationJournalEntry,
        authorization_entry: FederationJournalEntry,
        input_locator: FederationBarkVtxoLocator,
        authorization_context: Vec<u8>,
    ) -> anyhow::Result<CustodyActionGuard> {
        anyhow::ensure!(
            plan_entry.status() == FederationOperationStatus::Applied,
            "Bark custody requires an applied operation"
        );
        let FederationBarkOperation::ActionPlan(plan) = bark_operation(&plan_entry)? else {
            anyhow::bail!("Bark custody requires an accepted action plan");
        };
        anyhow::ensure!(
            plan.kind == FederationBarkActionKind::IncomingClaim
                && plan.outputs.len() == 1
                && input_locator.role == 1
                && input_locator.version == 1
                && !authorization_context.is_empty(),
            "accepted Bark claim scope is invalid"
        );
        self.activate_action(
            plan_entry,
            authorization_entry,
            vec![input_locator],
            bark::custody::VtxoSigningOperation::LightningReceive,
            authorization_context,
        )
    }

    pub(crate) fn activate_send(
        &self,
        plan_entry: FederationJournalEntry,
        authorization_entry: FederationJournalEntry,
        authorization_context: Vec<u8>,
    ) -> anyhow::Result<CustodyActionGuard> {
        anyhow::ensure!(
            plan_entry.status() == FederationOperationStatus::Applied,
            "Bark custody requires an applied operation"
        );
        let FederationBarkOperation::ActionPlan(plan) = bark_operation(&plan_entry)? else {
            anyhow::bail!("Bark custody requires an accepted action plan");
        };
        anyhow::ensure!(
            plan.kind == FederationBarkActionKind::LightningSend
                && !plan.inputs.is_empty()
                && !authorization_context.is_empty(),
            "accepted Bark send scope is invalid"
        );
        let input_locators = plan.inputs.iter().map(|input| input.locator).collect();
        self.activate_action(
            plan_entry,
            authorization_entry,
            input_locators,
            bark::custody::VtxoSigningOperation::LightningSend,
            authorization_context,
        )
    }

    pub(crate) fn activate_revocation(
        &self,
        plan_entry: FederationJournalEntry,
        authorization_entry: FederationJournalEntry,
        authorization_context: Vec<u8>,
    ) -> anyhow::Result<CustodyActionGuard> {
        anyhow::ensure!(
            plan_entry.status() == FederationOperationStatus::Applied,
            "Bark custody requires an applied operation"
        );
        let FederationBarkOperation::ActionPlan(plan) = bark_operation(&plan_entry)? else {
            anyhow::bail!("Bark custody requires an accepted action plan");
        };
        anyhow::ensure!(
            plan.kind == FederationBarkActionKind::FailedSendRevocation
                && !plan.inputs.is_empty()
                && !authorization_context.is_empty(),
            "accepted Bark revocation scope is invalid"
        );
        let input_locators = plan.inputs.iter().map(|input| input.locator).collect();
        self.activate_action(
            plan_entry,
            authorization_entry,
            input_locators,
            bark::custody::VtxoSigningOperation::LightningSend,
            authorization_context,
        )
    }

    pub(crate) fn activate_refresh(
        &self,
        plan_entry: FederationJournalEntry,
        authorization_entry: FederationJournalEntry,
        authorization_context: Vec<u8>,
    ) -> anyhow::Result<CustodyActionGuard> {
        anyhow::ensure!(
            plan_entry.status() == FederationOperationStatus::Applied,
            "Bark custody requires an applied operation"
        );
        let FederationBarkOperation::ActionPlan(plan) = bark_operation(&plan_entry)? else {
            anyhow::bail!("Bark custody requires an accepted action plan");
        };
        anyhow::ensure!(
            plan.kind == FederationBarkActionKind::Refresh
                && !plan.inputs.is_empty()
                && !authorization_context.is_empty(),
            "accepted Bark refresh scope is invalid"
        );
        let input_locators = plan
            .inputs
            .iter()
            .map(|input| input.locator)
            .chain(plan.outputs.iter().map(|output| output.locator))
            .collect();
        self.activate_action(
            plan_entry,
            authorization_entry,
            input_locators,
            bark::custody::VtxoSigningOperation::Round,
            authorization_context,
        )
    }

    pub(crate) fn activate_exit(
        &self,
        plan_entry: FederationJournalEntry,
        authorization_entry: FederationJournalEntry,
        authorization_context: Vec<u8>,
    ) -> anyhow::Result<CustodyActionGuard> {
        anyhow::ensure!(
            plan_entry.status() == FederationOperationStatus::Applied,
            "Bark custody requires an applied operation"
        );
        let FederationBarkOperation::ActionPlan(plan) = bark_operation(&plan_entry)? else {
            anyhow::bail!("Bark custody requires an accepted action plan");
        };
        anyhow::ensure!(
            plan.kind == FederationBarkActionKind::EmergencyExit
                && !plan.inputs.is_empty()
                && !authorization_context.is_empty(),
            "accepted Bark exit scope is invalid"
        );
        let input_locators = plan.inputs.iter().map(|input| input.locator).collect();
        self.activate_action(
            plan_entry,
            authorization_entry,
            input_locators,
            bark::custody::VtxoSigningOperation::UnilateralExit,
            authorization_context,
        )
    }

    fn activate_action(
        &self,
        plan_entry: FederationJournalEntry,
        authorization_entry: FederationJournalEntry,
        mut authorized_locators: Vec<FederationBarkVtxoLocator>,
        operation: bark::custody::VtxoSigningOperation,
        authorization_context: Vec<u8>,
    ) -> anyhow::Result<CustodyActionGuard> {
        authorized_locators.sort();
        authorized_locators.dedup();
        anyhow::ensure!(
            !authorized_locators.is_empty()
                && authorized_locators
                    .iter()
                    .all(|locator| locator.role == 1 && locator.version == 1)
                && !authorization_context.is_empty(),
            "accepted Bark custody scope is invalid"
        );
        validate_authorization_entry(&plan_entry, &authorization_entry)?;
        let FederationBarkOperation::ActionPlan(plan) = bark_operation(&plan_entry)? else {
            anyhow::bail!("Bark custody requires an accepted action plan");
        };
        let action_id = plan.action_id;
        let mut reserved_vtxos = plan
            .inputs
            .iter()
            .map(|input| input.vtxo_id)
            .collect::<Vec<_>>();
        reserved_vtxos.sort();
        reserved_vtxos.dedup();

        let signer_unavailable = Arc::new(AtomicBool::new(false));
        let mut registry = self
            .registry
            .lock()
            .map_err(|_| anyhow::anyhow!("Bark custody scope lock is poisoned"))?;
        anyhow::ensure!(
            !registry.actions.contains_key(&action_id),
            "Bark custody action is already active"
        );
        for vtxo_id in &reserved_vtxos {
            anyhow::ensure!(
                !registry.reserved_vtxos.contains_key(vtxo_id),
                "Bark custody VTXO is already reserved by an active action"
            );
        }
        anyhow::ensure!(
            !registry.actions.values().any(|existing| {
                authorization_context.starts_with(&existing.authorization_context)
                    || existing
                        .authorization_context
                        .starts_with(&authorization_context)
            }),
            "Bark custody authorization context conflicts with an active action"
        );

        for vtxo_id in &reserved_vtxos {
            registry.reserved_vtxos.insert(*vtxo_id, action_id);
        }
        registry.actions.insert(
            action_id,
            AcceptedCustodyAction {
                action_id,
                plan_entry,
                authorization_entry,
                authorized_locators,
                reserved_vtxos,
                operation,
                authorization_context,
                object_ordinals: BTreeMap::new(),
                signer_unavailable: Arc::clone(&signer_unavailable),
            },
        );
        drop(registry);

        Ok(CustodyActionGuard {
            custody: self.clone(),
            action_id,
            signer_unavailable,
            active: true,
        })
    }

    fn deactivate_action(&self, action_id: FederationBarkActionId) {
        if let Ok(mut registry) = self.registry.lock() {
            if let Some(removed) = registry.actions.remove(&action_id) {
                for vtxo_id in removed.reserved_vtxos {
                    if registry.reserved_vtxos.get(&vtxo_id) == Some(&action_id) {
                        registry.reserved_vtxos.remove(&vtxo_id);
                    }
                }
            }
        }
        if let Ok(mut sessions) = self.musig_sessions.lock() {
            sessions.retain(|_, session| session.action_id != action_id);
        }
    }

    fn authorize_request(
        &self,
        protocol: FederationBarkCustodyProtocol,
        session_id: CustodySessionId,
        locator: VtxoKeyLocator,
        message: [u8; 32],
        operation: bark::custody::VtxoSigningOperation,
        request_transcript: Vec<u8>,
    ) -> anyhow::Result<AuthorizedRequest> {
        let protocol_tag = match protocol {
            FederationBarkCustodyProtocol::Frost => 1,
            FederationBarkCustodyProtocol::Musig2 => 2,
            FederationBarkCustodyProtocol::ThresholdDh => 3,
        };
        let object_key = (protocol_tag, session_id.to_bytes());
        let (action_id, plan_entry, authorization_entry, object_ordinal, signer_unavailable) = {
            let mut registry = self
                .registry
                .lock()
                .map_err(|_| anyhow::anyhow!("Bark custody scope lock is poisoned"))?;
            let mut matches = registry
                .actions
                .values_mut()
                .filter(|action| request_transcript.starts_with(&action.authorization_context));
            let accepted = matches
                .next()
                .ok_or_else(|| anyhow::anyhow!("Bark custody has no active accepted action"))?;
            anyhow::ensure!(
                matches.next().is_none(),
                "Bark custody request matches multiple active actions"
            );
            let operation_matches = signing_operation_is_authorized(accepted.operation, operation);
            let locator_matches = accepted
                .authorized_locators
                .iter()
                .any(|accepted_locator| locator.index == accepted_locator.index);
            anyhow::ensure!(
                operation_matches && locator_matches,
                "Bark custody operation or locator differs from the accepted action"
            );
            let object_ordinal = match accepted.object_ordinals.get(&object_key) {
                Some(ordinal) => *ordinal,
                None => {
                    let ordinal = u32::try_from(accepted.object_ordinals.len())?;
                    accepted.object_ordinals.insert(object_key, ordinal);
                    ordinal
                }
            };
            (
                accepted.action_id,
                accepted.plan_entry.clone(),
                accepted.authorization_entry.clone(),
                object_ordinal,
                Arc::clone(&accepted.signer_unavailable),
            )
        };
        let FederationBarkOperation::ActionPlan(plan) = bark_operation(&plan_entry)? else {
            anyhow::bail!("active Bark custody operation is not an action plan");
        };
        let federation = self
            .federation
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Bark custody coordination is disabled"))?;
        let application = federation_bark_application(federation)?
            .ok_or_else(|| anyhow::anyhow!("accepted Bark application is missing"))?;
        let transcript_hash = federation_bark_custody_transcript_hash(&request_transcript)?;
        let operation =
            validate_federation_bark_operation(&authorization_entry.envelope.operation)?;
        let (signer_set, attempt) = authorization_signers(&operation)?;
        let reference = FederationBarkAuthorizationReference {
            version: FEDERATION_BARK_AUTHORIZATION_VERSION,
            federation_id: federation.federation_id,
            application_id: application.application_id,
            root_key_epoch: application.root_key_epoch,
            application_key_epoch: application.application_key_epoch,
            application_derivation_version: application.derivation_version,
            accepted_operation_id: authorization_entry.operation_id,
            accepted_operation_index: authorization_entry.index,
            action_id: plan.action_id,
            phase: operation.phase(),
            object_ordinal,
            operation_kind: operation.kind(),
            plan_or_result_hash: operation.commitment()?,
            locator: FederationBarkVtxoLocator::vtxo_custody(locator.index),
            protocol,
            transcript_hash,
            signer_set: signer_set.to_vec(),
            attempt,
            network: application.network,
            ark_server_public_key: application.ark_server_public_key,
        };
        let derived = self
            .config
            .public_adapter()
            .derive(BarkKeyLocator::vtxo(locator.index))?;
        let authorization = FederationBarkFrostAuthorization::new(
            reference,
            message,
            derived.signing_derivation(),
            request_transcript,
        )?;
        let reference_commitment = authorization.reference.commitment()?.to_bytes();
        let frost_protocol = match protocol {
            FederationBarkCustodyProtocol::Frost => CustodyProtocol::OrdinaryFrost,
            FederationBarkCustodyProtocol::Musig2 => CustodyProtocol::VirtualMusig2,
            FederationBarkCustodyProtocol::ThresholdDh => CustodyProtocol::ThresholdDh,
        };
        let signer_set = signer_set
            .iter()
            .map(|member| cdk_frost::MemberId::new(member.get()))
            .collect::<Result<Vec<_>, _>>()?;
        let registration = self.config.public_adapter().application().registration();
        let scope = SigningAuthorizationScope::new(
            federation.federation_id.to_bytes(),
            cdk_frost::RootKeyEpoch::new(application.root_key_epoch)?,
            registration,
            derived.signing_derivation().binding(),
            reference_commitment,
            message,
        )?;
        let context =
            SigningSessionContext::new_for_protocol(scope, attempt, signer_set, frost_protocol)?;
        let authorization = AuthorizationRequest::new(
            registration.id(),
            registration.key_epoch(),
            registration.derivation_version(),
            context.session_id(),
            authorization.canonical_bytes()?,
        )?;
        Ok(AuthorizedRequest {
            action_id,
            signer_unavailable,
            context,
            derivation: derived.signing_derivation().clone(),
            authorization,
        })
    }
}

fn bark_operation(entry: &FederationJournalEntry) -> anyhow::Result<FederationBarkOperation> {
    validate_federation_bark_operation(&entry.envelope.operation).map_err(Into::into)
}

fn validate_authorization_entry(
    plan_entry: &FederationJournalEntry,
    authorization_entry: &FederationJournalEntry,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        authorization_entry.status() == FederationOperationStatus::Applied,
        "Bark custody authorization requires an applied operation"
    );
    let FederationBarkOperation::ActionPlan(plan) = bark_operation(plan_entry)? else {
        anyhow::bail!("Bark custody plan entry is not an action plan");
    };
    let operation = bark_operation(authorization_entry)?;
    match &operation {
        FederationBarkOperation::ActionPlan(authorized_plan) => anyhow::ensure!(
            authorization_entry.operation_id == plan_entry.operation_id
                && authorization_entry.index == plan_entry.index
                && authorized_plan == &plan,
            "Bark custody action-plan authorization differs from its plan"
        ),
        FederationBarkOperation::ActionTransition(transition) => {
            let plan_commitment = FederationBarkOperation::ActionPlan(plan.clone()).commitment()?;
            anyhow::ensure!(
                transition.phase == FederationBarkSemanticPhase::SigningAttemptAdvanced
                    && transition.action_id == plan.action_id
                    && transition.quote == plan.quote
                    && transition.actor == plan.actor
                    && transition.plan_operation_id == plan_entry.operation_id
                    && transition.payment_hash == plan.payment_hash
                    && transition.attempt > plan.attempt
                    && !transition.signer_set.is_empty()
                    && transition.request_hash == plan_commitment
                    && transition.external_id == plan.external_id
                    && transition.artifacts.is_empty()
                    && transition.vtxo_transitions.is_empty(),
                "Bark custody retry authorization differs from its action plan"
            );
        }
        FederationBarkOperation::ReceiveIntent(_)
        | FederationBarkOperation::ReceivePrepared(_)
        | FederationBarkOperation::MemberObservation(_)
        | FederationBarkOperation::EvidenceCertificate(_) => {
            anyhow::bail!("Bark custody authorization has no signer attempt")
        }
    }
    Ok(())
}

fn authorization_signers(
    operation: &FederationBarkOperation,
) -> anyhow::Result<(&[cdk_common::federation::MemberId], u32)> {
    match operation {
        FederationBarkOperation::ActionPlan(plan) => Ok((&plan.signer_set, plan.attempt)),
        FederationBarkOperation::ActionTransition(transition)
            if transition.phase == FederationBarkSemanticPhase::SigningAttemptAdvanced =>
        {
            Ok((&transition.signer_set, transition.attempt))
        }
        FederationBarkOperation::ReceiveIntent(_)
        | FederationBarkOperation::ReceivePrepared(_)
        | FederationBarkOperation::ActionTransition(_)
        | FederationBarkOperation::MemberObservation(_)
        | FederationBarkOperation::EvidenceCertificate(_) => {
            anyhow::bail!("Bark custody operation has no signer attempt")
        }
    }
}

fn signing_operation_is_authorized(
    accepted: bark::custody::VtxoSigningOperation,
    requested: bark::custody::VtxoSigningOperation,
) -> bool {
    // One Ark round signs its attempt and output leaves under `Round`, then
    // signs the accepted inputs' forfeits before those outputs are unlocked.
    accepted == requested
        || matches!(
            (accepted, requested),
            (
                bark::custody::VtxoSigningOperation::Round,
                bark::custody::VtxoSigningOperation::Forfeit
            )
        )
}

impl fmt::Debug for CdkFrostVtxoCustody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CdkFrostVtxoCustody")
            .field("scope", &self.config.scope())
            .field("descriptor", &self.config.descriptor())
            .finish()
    }
}

#[async_trait]
impl VtxoCustody for CdkFrostVtxoCustody {
    fn descriptor(&self) -> KeySourceDescriptor {
        self.config.descriptor()
    }

    fn derive_public_key(&self, locator: VtxoKeyLocator) -> anyhow::Result<DerivedVtxoPublicKey> {
        let derived = self
            .config
            .public_adapter()
            .derive(BarkKeyLocator::vtxo(locator.index))
            .map_err(|_| Error::PublicKeyDerivationFailed)?;
        Ok(DerivedVtxoPublicKey {
            locator,
            public_key: derived.public_key(),
        })
    }

    async fn sign_schnorr(
        &self,
        request: SchnorrSigningRequest,
    ) -> anyhow::Result<schnorr::Signature> {
        let authorized = self.authorize_request(
            FederationBarkCustodyProtocol::Frost,
            request.session_id,
            request.locator,
            request.message,
            request.operation,
            schnorr_request_transcript(&request)?,
        )?;
        let signature = self
            .client
            .sign_frost(
                &authorized.context,
                &authorized.derivation,
                &authorized.authorization,
            )
            .await
            .map_err(|error| {
                tracing::warn!(
                    reason = %cdk_common::federation::redact_federation_operator_message(
                        &error.to_string()
                    ),
                    "Federated Bark FROST signing attempt is unavailable"
                );
                authorized.signer_unavailable.store(true, Ordering::Release);
                Error::CustodySignerUnavailable
            })?;
        Ok(schnorr::Signature::from_slice(&signature.to_bytes())?)
    }

    async fn begin_musig2(
        &self,
        request: Musig2CommitmentRequest,
    ) -> anyhow::Result<Musig2Commitment> {
        let authorized = self.authorize_request(
            FederationBarkCustodyProtocol::Musig2,
            request.session_id,
            request.locator,
            request.message,
            request.operation,
            musig_request_transcript(&request)?,
        )?;
        let commitments = self
            .client
            .collect_virtual_musig_commitments(
                &authorized.context,
                &authorized.derivation,
                &authorized.authorization,
            )
            .await
            .map_err(|error| {
                tracing::warn!(
                    reason = %cdk_common::federation::redact_federation_operator_message(
                        &error.to_string()
                    ),
                    "Federated Bark MuSig2 commitment attempt is unavailable"
                );
                authorized.signer_unavailable.store(true, Ordering::Release);
                Error::CustodySignerUnavailable
            })?;
        let public_nonce =
            bark::ark::musig::PublicNonce::from_byte_array(&commitments.aggregate_public_nonce()?)?;
        let session = MusigSession {
            action_id: authorized.action_id,
            signer_unavailable: authorized.signer_unavailable,
            request: request.clone(),
            context: authorized.context,
            derivation: authorized.derivation,
            authorization: authorized.authorization,
            commitments,
        };
        let mut sessions = self
            .musig_sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("Bark MuSig2 session lock is poisoned"))?;
        match sessions.get(&request.session_id.to_bytes()) {
            Some(existing) if existing.request != request => {
                anyhow::bail!("Bark MuSig2 session conflicts with an existing request")
            }
            Some(_) => {}
            None => {
                sessions.insert(request.session_id.to_bytes(), session);
            }
        }
        Ok(Musig2Commitment {
            session_id: request.session_id,
            public_nonce,
        })
    }

    async fn finish_musig2(
        &self,
        request: Musig2SigningRequest,
    ) -> anyhow::Result<PartialSignature> {
        let session = self
            .musig_sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("Bark MuSig2 session lock is poisoned"))?
            .get(&request.commitment.session_id.to_bytes())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Bark MuSig2 session is unknown"))?;
        anyhow::ensure!(
            session.request == request.commitment && request.signer_nonces.len() == 2,
            "Bark MuSig2 final request differs from its accepted commitment"
        );
        let derived = self
            .config
            .public_adapter()
            .derive(BarkKeyLocator::vtxo(request.commitment.locator.index))?;
        let user_public_nonce = session.commitments.aggregate_public_nonce()?;
        let mut outer_keys = [[0; 33]; 2];
        let mut server_public_nonce = None;
        let mut saw_user = false;
        for (index, signer) in request.signer_nonces.iter().enumerate() {
            outer_keys[index] = signer.public_key.serialize();
            if signer.public_key == derived.public_key() {
                anyhow::ensure!(
                    signer.public_nonce.serialize() == user_public_nonce,
                    "Bark MuSig2 user nonce differs from accepted federation commitments"
                );
                saw_user = true;
            } else {
                server_public_nonce = Some(signer.public_nonce.serialize());
            }
        }
        anyhow::ensure!(saw_user, "Bark MuSig2 transcript omits the federation key");
        let transcript = VirtualMusigTranscript::new(
            session.context.session_id(),
            outer_keys,
            derived.public_key().serialize(),
            user_public_nonce,
            server_public_nonce
                .ok_or_else(|| anyhow::anyhow!("Bark MuSig2 server nonce is missing"))?,
            request.commitment.message,
            request.commitment.xonly_tweak,
        )?;
        let partial = self
            .client
            .collect_virtual_musig_partial(
                &session.context,
                &session.derivation,
                &session.authorization,
                &session.commitments,
                &transcript,
            )
            .await
            .inspect_err(|error| {
                tracing::warn!(
                    reason = %cdk_common::federation::redact_federation_operator_message(
                        &error.to_string()
                    ),
                    "Federated Bark MuSig2 signature attempt is unavailable"
                );
                session.signer_unavailable.store(true, Ordering::Release);
            })?;
        self.musig_sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("Bark MuSig2 session lock is poisoned"))?
            .remove(&request.commitment.session_id.to_bytes());
        Ok(PartialSignature::from_byte_array(&partial)?)
    }

    async fn blind_mailbox(
        &self,
        _request: MailboxBlindingRequest,
    ) -> anyhow::Result<BlindedMailboxIdentifier> {
        Err(Error::ConsensusOnly.into())
    }

    async fn abort_session(
        &self,
        session: CustodySessionId,
        reason: AbortReason,
    ) -> anyhow::Result<()> {
        let stored = self
            .musig_sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("Bark MuSig2 session lock is poisoned"))?
            .remove(&session.to_bytes());
        if let Some(stored) = stored {
            let reason = match reason {
                AbortReason::Timeout => NonceBurnReason::Timeout,
                AbortReason::CoordinatorFailover => NonceBurnReason::CoordinatorLost,
                AbortReason::InvalidCounterparty => NonceBurnReason::InvalidPackage,
                AbortReason::TransportFailure => NonceBurnReason::AmbiguousResponse,
                AbortReason::UserCancelled | AbortReason::Other(_) => NonceBurnReason::Aborted,
            };
            self.client
                .abort(&stored.context, &stored.authorization, reason)
                .await;
        }
        Ok(())
    }
}

fn schnorr_request_transcript(request: &SchnorrSigningRequest) -> anyhow::Result<Vec<u8>> {
    let mut transcript = request.authorization.clone();
    append_request_field(&mut transcript, b"cdk/bark/schnorr-request/v1")?;
    append_request_field(&mut transcript, &request.session_id.to_bytes())?;
    append_request_field(&mut transcript, &request.locator.index.to_be_bytes())?;
    append_request_field(&mut transcript, &request.message)?;
    transcript.push(operation_tag(request.operation));
    Ok(transcript)
}

fn musig_request_transcript(request: &Musig2CommitmentRequest) -> anyhow::Result<Vec<u8>> {
    let mut transcript = request.authorization.clone();
    append_request_field(&mut transcript, b"cdk/bark/musig2-request/v1")?;
    append_request_field(&mut transcript, &request.session_id.to_bytes())?;
    append_request_field(&mut transcript, &request.locator.index.to_be_bytes())?;
    append_request_field(&mut transcript, &request.message)?;
    transcript.push(operation_tag(request.operation));
    for public_key in &request.public_keys {
        append_request_field(&mut transcript, &public_key.serialize())?;
    }
    match request.xonly_tweak {
        Some(tweak) => {
            transcript.push(1);
            append_request_field(&mut transcript, &tweak)?;
        }
        None => transcript.push(0),
    }
    Ok(transcript)
}

fn append_request_field(target: &mut Vec<u8>, value: &[u8]) -> anyhow::Result<()> {
    let length = u32::try_from(value.len())?;
    target.extend_from_slice(&length.to_be_bytes());
    target.extend_from_slice(value);
    Ok(())
}

const fn operation_tag(operation: bark::custody::VtxoSigningOperation) -> u8 {
    use bark::custody::VtxoSigningOperation as Operation;
    match operation {
        Operation::Arkoor => 1,
        Operation::Forfeit => 2,
        Operation::Offboard => 3,
        Operation::Board => 4,
        Operation::Round => 5,
        Operation::LightningSend => 6,
        Operation::LightningReceive => 7,
        Operation::Attestation => 8,
        Operation::UnilateralExit => 9,
        Operation::MailboxBlinding => 10,
    }
}

#[cfg(test)]
mod tests {
    use bark::custody::VtxoSigningOperation;

    use super::signing_operation_is_authorized;

    #[test]
    fn refresh_round_authorizes_its_forfeit_phase() {
        assert!(signing_operation_is_authorized(
            VtxoSigningOperation::Round,
            VtxoSigningOperation::Round,
        ));
        assert!(signing_operation_is_authorized(
            VtxoSigningOperation::Round,
            VtxoSigningOperation::Forfeit,
        ));
        assert!(!signing_operation_is_authorized(
            VtxoSigningOperation::Round,
            VtxoSigningOperation::LightningSend,
        ));
        assert!(!signing_operation_is_authorized(
            VtxoSigningOperation::LightningSend,
            VtxoSigningOperation::Forfeit,
        ));
    }
}
