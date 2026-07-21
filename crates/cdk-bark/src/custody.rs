//! Bark custody adapter rooted in the accepted CDK FROST application.

use core::fmt;
use std::collections::BTreeMap;
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
use cdk_common::federation::{
    federation_bark_custody_transcript_hash, FederationBarkActionKind,
    FederationBarkAuthorizationReference, FederationBarkCustodyProtocol,
    FederationBarkFrostAuthorization, FederationBarkOperation, FederationBarkOperationKind,
    FederationBarkSemanticPhase, FederationBarkVtxoLocator, FederationConfig,
    FederationJournalEntry, FederationOperation, FederationOperationStatus,
    FEDERATION_BARK_AUTHORIZATION_VERSION,
};
use cdk_frost::{
    AggregateSignature, AuthorizationRequest, CustodyProtocol, NonceBurnReason,
    SigningAuthorizationScope, SigningDerivation, SigningSessionContext,
    VirtualMusigNonceCommitmentSet, VirtualMusigTranscript,
};
use cdk_frost_bark::BarkKeyLocator;

use crate::{Error, ValidatedFederatedBarkConfig};

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

#[derive(Debug, Clone)]
struct AcceptedCustodyAction {
    entry: FederationJournalEntry,
    authorized_locators: Vec<FederationBarkVtxoLocator>,
    operation: bark::custody::VtxoSigningOperation,
    phase: FederationBarkSemanticPhase,
    authorization_context: Vec<u8>,
}

#[derive(Debug, Clone)]
struct MusigSession {
    request: Musig2CommitmentRequest,
    context: SigningSessionContext,
    derivation: SigningDerivation,
    authorization: AuthorizationRequest,
    commitments: VirtualMusigNonceCommitmentSet,
}

struct AuthorizedRequest {
    context: SigningSessionContext,
    derivation: SigningDerivation,
    authorization: AuthorizationRequest,
}

type BarkCustodyObjectOrdinals = BTreeMap<(u8, [u8; 32]), u32>;

/// Bark [`VtxoCustody`] implementation backed by the registered CDK FROST root.
///
/// Milestone 6 exposes public derivation only. Every custody operation that can
/// release a signature or DH-derived value remains fail-closed until accepted
/// Bark operations install the typed authorization adapter in Milestone 7.
#[derive(Clone)]
pub struct CdkFrostVtxoCustody {
    config: ValidatedFederatedBarkConfig,
    federation: Option<FederationConfig>,
    client: Arc<dyn FederatedBarkCustodyClient>,
    accepted: Arc<Mutex<Option<AcceptedCustodyAction>>>,
    object_ordinals: Arc<Mutex<BarkCustodyObjectOrdinals>>,
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
            accepted: Arc::new(Mutex::new(None)),
            object_ordinals: Arc::new(Mutex::new(BTreeMap::new())),
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
            accepted: Arc::new(Mutex::new(None)),
            object_ordinals: Arc::new(Mutex::new(BTreeMap::new())),
            musig_sessions: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub(crate) fn activate_claim(
        &self,
        entry: FederationJournalEntry,
        input_locator: FederationBarkVtxoLocator,
        authorization_context: Vec<u8>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            entry.status() == FederationOperationStatus::Applied,
            "Bark custody requires an applied operation"
        );
        let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
            &entry.envelope.operation
        else {
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
            entry,
            vec![input_locator],
            bark::custody::VtxoSigningOperation::LightningReceive,
            FederationBarkSemanticPhase::ClaimPlanned,
            authorization_context,
        )
    }

    pub(crate) fn activate_send(
        &self,
        entry: FederationJournalEntry,
        authorization_context: Vec<u8>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            entry.status() == FederationOperationStatus::Applied,
            "Bark custody requires an applied operation"
        );
        let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
            &entry.envelope.operation
        else {
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
            entry,
            input_locators,
            bark::custody::VtxoSigningOperation::LightningSend,
            FederationBarkSemanticPhase::SendPlanned,
            authorization_context,
        )
    }

    pub(crate) fn activate_revocation(
        &self,
        entry: FederationJournalEntry,
        authorization_context: Vec<u8>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            entry.status() == FederationOperationStatus::Applied,
            "Bark custody requires an applied operation"
        );
        let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
            &entry.envelope.operation
        else {
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
            entry,
            input_locators,
            bark::custody::VtxoSigningOperation::LightningSend,
            FederationBarkSemanticPhase::RevocationPlanned,
            authorization_context,
        )
    }

    pub(crate) fn activate_refresh(
        &self,
        entry: FederationJournalEntry,
        authorization_context: Vec<u8>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            entry.status() == FederationOperationStatus::Applied,
            "Bark custody requires an applied operation"
        );
        let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
            &entry.envelope.operation
        else {
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
            entry,
            input_locators,
            bark::custody::VtxoSigningOperation::Round,
            FederationBarkSemanticPhase::RefreshPlanned,
            authorization_context,
        )
    }

    pub(crate) fn activate_exit(
        &self,
        entry: FederationJournalEntry,
        authorization_context: Vec<u8>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            entry.status() == FederationOperationStatus::Applied,
            "Bark custody requires an applied operation"
        );
        let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
            &entry.envelope.operation
        else {
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
            entry,
            input_locators,
            bark::custody::VtxoSigningOperation::UnilateralExit,
            FederationBarkSemanticPhase::ExitPlanned,
            authorization_context,
        )
    }

    fn activate_action(
        &self,
        entry: FederationJournalEntry,
        mut authorized_locators: Vec<FederationBarkVtxoLocator>,
        operation: bark::custody::VtxoSigningOperation,
        phase: FederationBarkSemanticPhase,
        authorization_context: Vec<u8>,
    ) -> anyhow::Result<()> {
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
        let mut accepted = self
            .accepted
            .lock()
            .map_err(|_| anyhow::anyhow!("Bark custody scope lock is poisoned"))?;
        anyhow::ensure!(accepted.is_none(), "another Bark custody action is active");
        *accepted = Some(AcceptedCustodyAction {
            entry,
            authorized_locators,
            operation,
            phase,
            authorization_context,
        });
        self.object_ordinals
            .lock()
            .map_err(|_| anyhow::anyhow!("Bark custody object lock is poisoned"))?
            .clear();
        self.musig_sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("Bark MuSig2 session lock is poisoned"))?
            .clear();
        Ok(())
    }

    pub(crate) fn deactivate_claim(&self) {
        self.deactivate_action();
    }

    pub(crate) fn deactivate_send(&self) {
        self.deactivate_action();
    }

    pub(crate) fn deactivate_revocation(&self) {
        self.deactivate_action();
    }

    pub(crate) fn deactivate_refresh(&self) {
        self.deactivate_action();
    }

    pub(crate) fn deactivate_exit(&self) {
        self.deactivate_action();
    }

    fn deactivate_action(&self) {
        if let Ok(mut accepted) = self.accepted.lock() {
            *accepted = None;
        }
        if let Ok(mut sessions) = self.musig_sessions.lock() {
            sessions.clear();
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
        let accepted = self
            .accepted
            .lock()
            .map_err(|_| anyhow::anyhow!("Bark custody scope lock is poisoned"))?
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Bark custody has no active accepted action"))?;
        let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
            &accepted.entry.envelope.operation
        else {
            anyhow::bail!("active Bark custody operation is not an action plan");
        };
        let operation_matches = signing_operation_is_authorized(accepted.operation, operation);
        let locator_matches = accepted
            .authorized_locators
            .iter()
            .any(|accepted| locator.index == accepted.index);
        anyhow::ensure!(
            operation_matches && locator_matches,
            "Bark custody operation or locator differs from the accepted action"
        );
        anyhow::ensure!(
            request_transcript.starts_with(&accepted.authorization_context),
            "Bark custody request is not prefixed by the accepted action context"
        );
        let protocol_tag = match protocol {
            FederationBarkCustodyProtocol::Frost => 1,
            FederationBarkCustodyProtocol::Musig2 => 2,
            FederationBarkCustodyProtocol::ThresholdDh => 3,
        };
        let object_key = (protocol_tag, session_id.to_bytes());
        let object_ordinal = {
            let mut ordinals = self
                .object_ordinals
                .lock()
                .map_err(|_| anyhow::anyhow!("Bark custody object lock is poisoned"))?;
            match ordinals.get(&object_key) {
                Some(ordinal) => *ordinal,
                None => {
                    let ordinal = u32::try_from(ordinals.len())?;
                    ordinals.insert(object_key, ordinal);
                    ordinal
                }
            }
        };
        let federation = self
            .federation
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Bark custody coordination is disabled"))?;
        let application = federation
            .frost_wallet
            .as_ref()
            .and_then(|wallet| wallet.bark_application.as_ref())
            .ok_or_else(|| anyhow::anyhow!("accepted Bark application is missing"))?;
        let transcript_hash = federation_bark_custody_transcript_hash(&request_transcript)?;
        let operation = FederationBarkOperation::ActionPlan(plan.clone());
        let reference = FederationBarkAuthorizationReference {
            version: FEDERATION_BARK_AUTHORIZATION_VERSION,
            federation_id: federation.federation_id,
            application_id: application.application_id,
            root_key_epoch: application.root_key_epoch,
            application_key_epoch: application.application_key_epoch,
            application_derivation_version: application.derivation_version,
            accepted_operation_id: accepted.entry.operation_id,
            accepted_operation_index: accepted.entry.index,
            action_id: plan.action_id,
            phase: accepted.phase,
            object_ordinal,
            operation_kind: FederationBarkOperationKind::ActionPlan,
            plan_or_result_hash: operation.commitment()?,
            locator: FederationBarkVtxoLocator::vtxo_custody(locator.index),
            protocol,
            transcript_hash,
            signer_set: plan.signer_set.clone(),
            attempt: plan.attempt,
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
        let signer_set = plan
            .signer_set
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
        let context = SigningSessionContext::new_for_protocol(
            scope,
            plan.attempt,
            signer_set,
            frost_protocol,
        )?;
        let authorization = AuthorizationRequest::new(
            registration.id(),
            registration.key_epoch(),
            registration.derivation_version(),
            context.session_id(),
            authorization.canonical_bytes()?,
        )?;
        Ok(AuthorizedRequest {
            context,
            derivation: derived.signing_derivation().clone(),
            authorization,
        })
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
            .await?;
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
            .await?;
        let public_nonce =
            bark::ark::musig::PublicNonce::from_byte_array(&commitments.aggregate_public_nonce()?)?;
        let session = MusigSession {
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
            .await?;
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
