//! Participant-local application registry, nonce lifecycle, and signing API.

use std::collections::BTreeMap;
use std::fmt;

use frost_secp256k1_tr as frost;
use frost_secp256k1_tr::rand_core::{CryptoRng, RngCore};
use sha2::{Digest, Sha256};

use crate::canonical::{
    derive_signing_public, identifier, library_signing_public_package, CanonicalRootKeyPackage,
};
use crate::nonce::{
    CustodyProtocol, DurableNonceStore, MemoryNonceStore, NonceBurnReason, NonceLifecycle,
    NonceRecord, NonceReservation, SigningSessionContext,
};
use crate::protocol::{
    DurableProtocolStore, ThresholdDhLifecycle, ThresholdDhRecord, ThresholdDhReservation,
    VirtualMusigLifecycle, VirtualMusigRecord, VirtualMusigReservation,
};
use crate::threshold_dh::{self, ThresholdDhRequest, ThresholdDhShare};
use crate::types::{
    derive_nonce_id, AggregateSignature, ApplicationId, ApplicationRegistration,
    AuthorizationRequest, AuthorizedSigningRequest, DerivedPublicPackage,
    DerivedSigningPublicPackage, NonceCommitment, NonceCommitmentSet, NonceId, SignatureMode,
    SignatureShare, SigningDerivation, SigningSessionId, MAX_COMMITMENT_BYTES,
};
use crate::virtual_musig::{
    self, VirtualMusigNonceCommitment, VirtualMusigNonceCommitmentSet, VirtualMusigNonceId,
    VirtualMusigPartialShare, VirtualMusigTranscript,
};
use crate::{sealed, Error, SecretEncryptionKey};

trait ParticipantStore: DurableNonceStore + DurableProtocolStore {
    fn as_nonce_store(&self) -> &dyn DurableNonceStore;
    fn as_protocol_store(&self) -> &dyn DurableProtocolStore;
}

impl<T> ParticipantStore for T
where
    T: DurableNonceStore + DurableProtocolStore,
{
    fn as_nonce_store(&self) -> &dyn DurableNonceStore {
        self
    }

    fn as_protocol_store(&self) -> &dyn DurableProtocolStore {
        self
    }
}

/// Reviewed application adapter that authorizes typed signing requests.
///
/// Implementations are part of the custody boundary: registration metadata is
/// committed into key derivation, and `authorize` must reload and validate the
/// application's authoritative state before returning the exact 32-byte
/// message that may be signed. The key manager exposes no direct digest-signing
/// method.
pub trait ApplicationAuthorization: fmt::Debug + Send + Sync {
    /// Stable registration metadata for this adapter.
    fn registration(&self) -> ApplicationRegistration;

    /// Validate a typed application request and return its authorized BIP340
    /// message digest and exact reviewed signing derivation.
    fn authorize(&self, request: &AuthorizationRequest) -> Result<AuthorizedSigningRequest, Error>;
}

struct RegisteredApplication {
    adapter: Box<dyn ApplicationAuthorization>,
    public: DerivedPublicPackage,
}

/// Participant-local owner of one root share, registered applications, and
/// single-use nonce state.
pub struct ParticipantKeyManager {
    root: CanonicalRootKeyPackage,
    applications: BTreeMap<ApplicationId, RegisteredApplication>,
    nonce_store: Box<dyn ParticipantStore>,
    nonce_encryption_key: SecretEncryptionKey,
}

impl fmt::Debug for ParticipantKeyManager {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ParticipantKeyManager")
            .field("participant", &self.root.participant())
            .field("root_epoch", &self.root.public().epoch())
            .field("root_share", &"<redacted>")
            .field("registered_applications", &self.applications.keys())
            .field("nonce_store", &"<private durable state>")
            .finish()
    }
}

impl ParticipantKeyManager {
    /// Construct a key manager with an in-memory store for tests and prototypes.
    ///
    /// Production integrations must use [`ParticipantKeyManager::with_nonce_store`].
    pub fn new_in_memory(
        root: CanonicalRootKeyPackage,
        nonce_encryption_key: SecretEncryptionKey,
    ) -> Result<Self, Error> {
        Self::with_nonce_store(root, nonce_encryption_key, MemoryNonceStore::default())
    }

    /// Take ownership of an activated root package and one participant-wide
    /// durable nonce namespace.
    pub fn with_nonce_store<S>(
        root: CanonicalRootKeyPackage,
        nonce_encryption_key: SecretEncryptionKey,
        nonce_store: S,
    ) -> Result<Self, Error>
    where
        S: DurableNonceStore + DurableProtocolStore,
    {
        let manager = Self {
            root,
            applications: BTreeMap::new(),
            nonce_store: Box::new(nonce_store),
            nonce_encryption_key,
        };
        manager.recover_after_restart()?;
        Ok(manager)
    }

    /// Participant-wide nonce store used by every registered application.
    pub fn nonce_store(&self) -> &dyn DurableNonceStore {
        self.nonce_store.as_nonce_store()
    }

    /// Protocol-separated MuSig2 and threshold-DH store.
    pub fn protocol_store(&self) -> &dyn DurableProtocolStore {
        self.nonce_store.as_protocol_store()
    }

    /// Return the immutable application-neutral public root package.
    pub fn root_public_package(&self) -> &crate::RootPublicPackage {
        self.root.public()
    }

    /// Local participant identifier that owns this root share and nonce store.
    pub fn participant(&self) -> crate::MemberId {
        self.root.participant()
    }

    /// Register a reviewed application adapter exactly once.
    pub fn register_application(
        &mut self,
        adapter: impl ApplicationAuthorization + 'static,
    ) -> Result<DerivedPublicPackage, Error> {
        let registration = adapter.registration();
        if self.applications.contains_key(&registration.id()) {
            return Err(Error::Application(
                "application identifier is already registered".to_string(),
            ));
        }
        let derived = self.root.derive_application(registration)?;
        let public = derived.public_boundary;
        self.applications.insert(
            registration.id(),
            RegisteredApplication {
                adapter: Box::new(adapter),
                public: public.clone(),
            },
        );
        Ok(public)
    }

    /// Return the derived public package for a registered application.
    pub fn public_package(
        &self,
        application_id: ApplicationId,
    ) -> Result<&DerivedPublicPackage, Error> {
        self.applications
            .get(&application_id)
            .map(|application| &application.public)
            .ok_or_else(|| Error::Application("application is not registered".to_string()))
    }

    /// Generate and retain a single-use nonce pair, returning only its public
    /// commitment.
    pub fn commit_nonce<R>(
        &mut self,
        context: &SigningSessionContext,
        rng: &mut R,
    ) -> Result<NonceCommitment, Error>
    where
        R: CryptoRng + RngCore,
    {
        self.commit_nonce_for_derivation(context, SigningDerivation::application_key(), rng)
    }

    /// Generate a single-use nonce for one exact child derivation and final
    /// signature mode.
    pub fn commit_nonce_for_derivation<R>(
        &mut self,
        context: &SigningSessionContext,
        derivation: SigningDerivation,
        rng: &mut R,
    ) -> Result<NonceCommitment, Error>
    where
        R: CryptoRng + RngCore,
    {
        let registration = context.registration();
        let application = self
            .applications
            .get(&registration.id())
            .ok_or_else(|| Error::Application("application is not registered".to_string()))?;
        if application.adapter.registration() != registration
            || context.root_epoch() != self.root.public().epoch()
            || context.derivation_binding() != derivation.binding()
            || context.signer_set().len() < usize::from(self.root.public().threshold())
            || !context.signer_set().contains(&self.root.participant())
            || context
                .signer_set()
                .iter()
                .any(|member| !self.root.public().verifying_shares().contains_key(member))
        {
            return Err(Error::InvalidSigningSession(
                "session does not match the root, application, derivation, threshold, or participant"
                    .to_string(),
            ));
        }
        self.nonce_store.activate_attempt(context)?;
        let derived = self.root.derive_signing_target(registration, &derivation)?;
        let (nonces, commitments) = frost::round1::commit(derived.key.signing_share(), rng);
        let encoded = commitments
            .serialize()
            .map_err(|err| Error::Frost(err.to_string()))?;
        if encoded.len() > MAX_COMMITMENT_BYTES {
            return Err(Error::OversizedInput {
                kind: "nonce commitment",
                max: MAX_COMMITMENT_BYTES,
            });
        }
        let nonce_id = derive_nonce_id(
            self.root.public().epoch(),
            registration,
            context.session_id(),
            derivation.binding(),
            self.root.participant(),
            &encoded,
        );
        let public = NonceCommitment {
            root_epoch: self.root.public().epoch(),
            registration,
            session_id: context.session_id(),
            derivation_binding: derivation.binding(),
            nonce_id,
            participant: self.root.participant(),
            commitments: encoded,
        };
        let serialized_nonces = nonces
            .serialize()
            .map_err(|error| Error::Frost(error.to_string()))?;
        let encrypted_nonces = sealed::seal(
            &self.nonce_encryption_key,
            &nonce_secret_aad(context, nonce_id, self.root.participant()),
            &serialized_nonces,
            rng,
        )?;
        let generated = NonceRecord::generated(
            context.clone(),
            nonce_id,
            derivation,
            public.clone(),
            encrypted_nonces,
        );
        match self.nonce_store.reserve(generated.clone())? {
            NonceReservation::Existing(existing) => {
                if existing.context() != context
                    || existing.derivation() != generated.derivation()
                    || !matches!(
                        existing.lifecycle(),
                        NonceLifecycle::CommitmentSent | NonceLifecycle::Consumed { .. }
                    )
                {
                    return Err(Error::InvalidCommitment(
                        "signing session already exists in a non-retryable state".to_string(),
                    ));
                }
                Ok(existing.commitment().clone())
            }
            NonceReservation::Inserted => {
                let reserved = generated.transition(NonceLifecycle::Reserved, true, None)?;
                self.nonce_store
                    .compare_and_swap(&generated, reserved.clone())?;
                let sent = reserved.transition(NonceLifecycle::CommitmentSent, true, None)?;
                self.nonce_store.compare_and_swap(&reserved, sent)?;
                Ok(public)
            }
        }
    }

    /// Authorize a request through its registered adapter, consume the local
    /// nonce, and produce one signature share.
    pub fn sign_share(
        &mut self,
        nonce_id: NonceId,
        request: &AuthorizationRequest,
        commitment_set: &NonceCommitmentSet,
    ) -> Result<SignatureShare, Error> {
        let record = self
            .nonce_store
            .load_nonce(nonce_id)?
            .ok_or_else(|| Error::InvalidCommitment("nonce is unknown".to_string()))?;
        let validated = (|| {
            let application = self
                .applications
                .get(&request.application_id())
                .ok_or_else(|| Error::Application("application is not registered".to_string()))?;
            let registration = application.adapter.registration();
            if request.application_key_epoch() != registration.key_epoch()
                || request.derivation_version() != registration.derivation_version()
            {
                return Err(Error::Application(
                    "request uses another application key epoch or derivation version".to_string(),
                ));
            }
            if record.context().registration() != registration
                || record.context().session_id() != request.session_id()
            {
                return Err(Error::WrongSession);
            }
            let authorized = application.adapter.authorize(request)?;
            if authorized.derivation() != record.derivation()
                || authorized.message() != record.context().message_commitment()
            {
                return Err(Error::WrongSession);
            }
            validate_commitment_set(
                commitment_set,
                self.root.public().epoch(),
                registration,
                request.session_id(),
                record.derivation().binding(),
                self.root.public().threshold(),
            )?;
            let commitment_signers = commitment_set
                .commitments()
                .keys()
                .copied()
                .collect::<Vec<_>>();
            if commitment_signers != record.context().signer_set() {
                return Err(Error::InvalidCommitment(
                    "commitment signer set differs from the accepted session".to_string(),
                ));
            }
            let own_commitment = commitment_set
                .commitments()
                .get(&self.root.participant())
                .ok_or_else(|| {
                    Error::InvalidCommitment(
                        "local participant is absent from signer set".to_string(),
                    )
                })?;
            if own_commitment != record.commitment() {
                return Err(Error::InvalidCommitment(
                    "coordinator supplied another local commitment".to_string(),
                ));
            }
            let package_hash =
                signing_package_hash(request.session_id(), &authorized.message(), commitment_set)?;
            Ok((registration, authorized, package_hash))
        })();
        let (registration, authorized, package_hash) = match validated {
            Ok(validated) => validated,
            Err(error) => {
                self.burn_record(&record, NonceBurnReason::InvalidPackage)?;
                return Err(error);
            }
        };

        match record.lifecycle() {
            NonceLifecycle::Consumed {
                package_hash: consumed_hash,
            } if consumed_hash == &package_hash => {
                return record.cached_share().cloned().ok_or_else(|| {
                    Error::InvalidCommitment(
                        "consumed nonce retry metadata was unavailable".to_string(),
                    )
                });
            }
            NonceLifecycle::CommitmentSent => {}
            _ => {
                return Err(Error::InvalidCommitment(
                    "nonce is not available for round two".to_string(),
                ));
            }
        }

        let nonces = (|| {
            let encrypted = record.encrypted_nonces().ok_or_else(|| {
                Error::InvalidCommitment("nonce secret is unavailable before round two".to_string())
            })?;
            let serialized_nonces = sealed::open(
                &self.nonce_encryption_key,
                &nonce_secret_aad(record.context(), nonce_id, self.root.participant()),
                encrypted,
            )?;
            frost::round1::SigningNonces::deserialize(&serialized_nonces)
                .map_err(|error| Error::Frost(error.to_string()))
        })();
        let nonces = match nonces {
            Ok(nonces) => nonces,
            Err(error) => {
                self.burn_record(&record, NonceBurnReason::InternalFailure)?;
                return Err(error);
            }
        };
        let signing = record.transition(NonceLifecycle::Signing { package_hash }, false, None)?;
        if let Err(error) = self.nonce_store.compare_and_swap(&record, signing.clone()) {
            if let Some(latest) = self.nonce_store.load_nonce(nonce_id)? {
                if let NonceLifecycle::Consumed {
                    package_hash: latest_hash,
                } = latest.lifecycle()
                {
                    if latest_hash == &package_hash {
                        return latest.cached_share().cloned().ok_or_else(|| {
                            Error::InvalidCommitment(
                                "consumed nonce retry metadata was unavailable".to_string(),
                            )
                        });
                    }
                }
            }
            return Err(error);
        }

        let signing_package = match signing_package(commitment_set, &authorized.message()) {
            Ok(package) => package,
            Err(error) => {
                self.burn_record(&signing, NonceBurnReason::InternalFailure)?;
                return Err(error);
            }
        };
        let derived = match self
            .root
            .derive_signing_target(registration, signing.derivation())
        {
            Ok(derived) => derived,
            Err(error) => {
                self.burn_record(&signing, NonceBurnReason::InternalFailure)?;
                return Err(error);
            }
        };
        let share_result = match signing.derivation().signature_mode() {
            SignatureMode::Plain => frost::round2::sign(&signing_package, &nonces, &derived.key),
            SignatureMode::TaprootKeyPath { merkle_root } => frost::round2::sign_with_tweak(
                &signing_package,
                &nonces,
                &derived.key,
                merkle_root.as_ref().map(|root| root.as_slice()),
            ),
        };
        let share = match share_result {
            Ok(share) => share,
            Err(error) => {
                self.burn_record(&signing, NonceBurnReason::InternalFailure)?;
                return Err(Error::InvalidShare(error.to_string()));
            }
        };
        let share = match share.serialize().try_into() {
            Ok(share) => share,
            Err(_) => {
                self.burn_record(&signing, NonceBurnReason::InternalFailure)?;
                return Err(Error::Internal(
                    "FROST signature share is not 32 bytes".to_string(),
                ));
            }
        };
        let share = SignatureShare {
            root_epoch: self.root.public().epoch(),
            registration,
            session_id: request.session_id(),
            derivation_binding: signing.derivation().binding(),
            participant: self.root.participant(),
            share,
        };
        let produced = signing.transition(
            NonceLifecycle::ShareProduced { package_hash },
            false,
            Some(share.clone()),
        )?;
        self.nonce_store
            .compare_and_swap(&signing, produced.clone())?;
        let consumed = produced.transition(
            NonceLifecycle::Consumed { package_hash },
            false,
            Some(share.clone()),
        )?;
        self.nonce_store.compare_and_swap(&produced, consumed)?;
        Ok(share)
    }

    /// Generate, seal, and durably reserve a virtual MuSig2 nonce before
    /// returning its public two-column commitment.
    pub fn commit_virtual_musig_nonce<R>(
        &mut self,
        context: &SigningSessionContext,
        derivation: SigningDerivation,
        rng: &mut R,
    ) -> Result<VirtualMusigNonceCommitment, Error>
    where
        R: CryptoRng + RngCore,
    {
        if context.protocol() != CustodyProtocol::VirtualMusig2 {
            return Err(Error::InvalidSigningSession(
                "virtual MuSig2 requires its protocol namespace".to_string(),
            ));
        }
        let registration = context.registration();
        let application = self
            .applications
            .get(&registration.id())
            .ok_or_else(|| Error::Application("application is not registered".to_string()))?;
        if application.adapter.registration() != registration
            || context.root_epoch() != self.root.public().epoch()
            || context.derivation_binding() != derivation.binding()
            || derivation.signature_mode() != SignatureMode::Plain
            || context.signer_set().len() < usize::from(self.root.public().threshold())
            || !context.signer_set().contains(&self.root.participant())
            || context
                .signer_set()
                .iter()
                .any(|member| !self.root.public().verifying_shares().contains_key(member))
        {
            return Err(Error::InvalidSigningSession(
                "virtual MuSig2 session does not match the root, application, or signer set"
                    .to_string(),
            ));
        }
        self.nonce_store.activate_virtual_musig_attempt(context)?;
        let (secret_nonce, public_nonce) = virtual_musig::create_nonce(rng)?;
        let commitment =
            VirtualMusigNonceCommitment::new(context, self.root.participant(), public_nonce)?;
        let encrypted_nonce = sealed::seal(
            &self.nonce_encryption_key,
            &virtual_musig_secret_aad(context, commitment.nonce_id(), self.root.participant()),
            secret_nonce.as_ref(),
            rng,
        )?;
        let generated = VirtualMusigRecord::generated(
            context.clone(),
            derivation,
            commitment.clone(),
            encrypted_nonce,
        );
        match self.nonce_store.reserve_virtual_musig(generated.clone())? {
            VirtualMusigReservation::Existing(existing) => {
                if existing.context() != context
                    || existing.derivation() != generated.derivation()
                    || !matches!(
                        existing.lifecycle(),
                        VirtualMusigLifecycle::CommitmentSent
                            | VirtualMusigLifecycle::PartialProduced { .. }
                            | VirtualMusigLifecycle::Consumed { .. }
                    )
                {
                    return Err(Error::InvalidCommitment(
                        "virtual MuSig2 session exists in a non-retryable state".to_string(),
                    ));
                }
                Ok(existing.commitment().clone())
            }
            VirtualMusigReservation::Inserted => {
                let reserved = generated.transition(VirtualMusigLifecycle::Reserved, true, None)?;
                self.nonce_store
                    .compare_and_swap_virtual_musig(&generated, reserved.clone())?;
                let sent =
                    reserved.transition(VirtualMusigLifecycle::CommitmentSent, true, None)?;
                self.nonce_store
                    .compare_and_swap_virtual_musig(&reserved, sent)?;
                Ok(commitment)
            }
        }
    }

    /// Authorize and consume one virtual MuSig2 nonce, returning a verified
    /// participant partial for the exact finalized outer transcript.
    pub fn virtual_musig_partial(
        &mut self,
        nonce_id: VirtualMusigNonceId,
        authorization: &AuthorizationRequest,
        commitment_set: &VirtualMusigNonceCommitmentSet,
        transcript: &VirtualMusigTranscript,
    ) -> Result<VirtualMusigPartialShare, Error> {
        let record = self
            .nonce_store
            .load_virtual_musig_nonce(nonce_id)?
            .ok_or_else(|| {
                Error::InvalidCommitment("virtual MuSig2 nonce is unknown".to_string())
            })?;
        let validated = (|| {
            let application = self
                .applications
                .get(&authorization.application_id())
                .ok_or_else(|| Error::Application("application is not registered".to_string()))?;
            let registration = application.adapter.registration();
            if authorization.application_key_epoch() != registration.key_epoch()
                || authorization.derivation_version() != registration.derivation_version()
                || authorization.session_id() != record.context().session_id()
                || record.context().registration() != registration
                || record.context().protocol() != CustodyProtocol::VirtualMusig2
                || transcript.session_id() != record.context().session_id()
            {
                return Err(Error::WrongSession);
            }
            let authorized = application.adapter.authorize(authorization)?;
            if authorized.derivation() != record.derivation()
                || authorized.message() != record.context().message_commitment()
                || authorized.message() != transcript.message()
            {
                return Err(Error::WrongSession);
            }
            let target = derive_signing_public(&application.public, record.derivation())?;
            if target.signature_mode() != SignatureMode::Plain
                || target.output_group_key() != transcript.user_pubkey()
            {
                return Err(Error::WrongSession);
            }
            let commitment_signers = commitment_set
                .commitments()
                .keys()
                .copied()
                .collect::<Vec<_>>();
            if commitment_signers != record.context().signer_set()
                || commitment_set.aggregate_public_nonce()? != transcript.user_public_nonce()
            {
                return Err(Error::InvalidCommitment(
                    "virtual MuSig2 commitment set differs from the accepted session".to_string(),
                ));
            }
            let own = commitment_set
                .commitments()
                .get(&self.root.participant())
                .ok_or_else(|| {
                    Error::InvalidCommitment(
                        "local participant is absent from virtual MuSig2 set".to_string(),
                    )
                })?;
            if own != record.commitment() {
                return Err(Error::InvalidCommitment(
                    "coordinator supplied another local virtual MuSig2 nonce".to_string(),
                ));
            }
            Ok((registration, target, transcript.binding()))
        })();
        let (registration, target, transcript_binding) = match validated {
            Ok(validated) => validated,
            Err(error) => {
                self.burn_virtual_musig_record(&record, NonceBurnReason::InvalidPackage)?;
                return Err(error);
            }
        };

        match record.lifecycle() {
            VirtualMusigLifecycle::Consumed {
                transcript_binding: completed_binding,
            } if completed_binding == &transcript_binding => {
                let partial = record.cached_partial().cloned().ok_or_else(|| {
                    Error::InvalidCommitment(
                        "completed virtual MuSig2 retry metadata is unavailable".to_string(),
                    )
                })?;
                virtual_musig::verify_partial_share(
                    &target,
                    record.context().signer_set(),
                    record.commitment(),
                    transcript,
                    &partial,
                )?;
                return Ok(partial);
            }
            VirtualMusigLifecycle::PartialProduced {
                transcript_binding: produced_binding,
            } if produced_binding == &transcript_binding => {
                let partial = record.cached_partial().cloned().ok_or_else(|| {
                    Error::InvalidCommitment(
                        "produced virtual MuSig2 retry metadata is unavailable".to_string(),
                    )
                })?;
                virtual_musig::verify_partial_share(
                    &target,
                    record.context().signer_set(),
                    record.commitment(),
                    transcript,
                    &partial,
                )?;
                let consumed = record.transition(
                    VirtualMusigLifecycle::Consumed { transcript_binding },
                    false,
                    Some(partial.clone()),
                )?;
                self.nonce_store
                    .compare_and_swap_virtual_musig(&record, consumed)?;
                return Ok(partial);
            }
            VirtualMusigLifecycle::CommitmentSent => {}
            _ => {
                return Err(Error::InvalidCommitment(
                    "virtual MuSig2 nonce is not available for partial signing".to_string(),
                ));
            }
        }

        let secret_nonce = (|| {
            let encrypted = record.encrypted_nonce().ok_or_else(|| {
                Error::InvalidCommitment("virtual MuSig2 nonce secret is unavailable".to_string())
            })?;
            let plaintext = sealed::open(
                &self.nonce_encryption_key,
                &virtual_musig_secret_aad(record.context(), nonce_id, self.root.participant()),
                encrypted,
            )?;
            plaintext.as_slice().try_into().map_err(|_| {
                Error::InvalidCommitment("virtual MuSig2 nonce is not 64 bytes".to_string())
            })
        })();
        let secret_nonce: [u8; 64] = match secret_nonce {
            Ok(secret) => secret,
            Err(error) => {
                self.burn_virtual_musig_record(&record, NonceBurnReason::InternalFailure)?;
                return Err(error);
            }
        };
        let signing = record.transition(
            VirtualMusigLifecycle::PartialSigning { transcript_binding },
            false,
            None,
        )?;
        self.nonce_store
            .compare_and_swap_virtual_musig(&record, signing.clone())?;
        let partial = match (|| {
            let derived = self
                .root
                .derive_signing_target(registration, signing.derivation())?;
            virtual_musig::create_partial_share(
                self.root.participant(),
                derived.signing_share_bytes()?,
                &target,
                signing.context().signer_set(),
                &secret_nonce,
                signing.commitment(),
                transcript,
            )
        })() {
            Ok(partial) => partial,
            Err(error) => {
                self.burn_virtual_musig_record(&signing, NonceBurnReason::InternalFailure)?;
                return Err(error);
            }
        };
        let produced = signing.transition(
            VirtualMusigLifecycle::PartialProduced { transcript_binding },
            false,
            Some(partial.clone()),
        )?;
        self.nonce_store
            .compare_and_swap_virtual_musig(&signing, produced.clone())?;
        let consumed = produced.transition(
            VirtualMusigLifecycle::Consumed { transcript_binding },
            false,
            Some(partial.clone()),
        )?;
        self.nonce_store
            .compare_and_swap_virtual_musig(&produced, consumed)?;
        Ok(partial)
    }

    /// Authorize an exact threshold-DH request, cache its verified result, and
    /// return the identical result for safe retries.
    pub fn threshold_dh_share<R>(
        &mut self,
        context: &SigningSessionContext,
        derivation: SigningDerivation,
        authorization: &AuthorizationRequest,
        request: &ThresholdDhRequest,
        rng: &mut R,
    ) -> Result<ThresholdDhShare, Error>
    where
        R: CryptoRng + RngCore,
    {
        if context.protocol() != CustodyProtocol::ThresholdDh
            || request.session_id() != context.session_id()
            || authorization.session_id() != context.session_id()
            || context.derivation_binding() != derivation.binding()
            || context.message_commitment() != request.context()
            || derivation.signature_mode() != SignatureMode::Plain
        {
            return Err(Error::WrongSession);
        }
        let application = self
            .applications
            .get(&authorization.application_id())
            .ok_or_else(|| Error::Application("application is not registered".to_string()))?;
        let registration = application.adapter.registration();
        if context.registration() != registration
            || authorization.application_key_epoch() != registration.key_epoch()
            || authorization.derivation_version() != registration.derivation_version()
            || context.root_epoch() != self.root.public().epoch()
            || context.signer_set().len() < usize::from(self.root.public().threshold())
            || !context.signer_set().contains(&self.root.participant())
        {
            return Err(Error::WrongSession);
        }
        let authorized = application.adapter.authorize(authorization)?;
        if authorized.derivation() != &derivation || authorized.message() != request.context() {
            return Err(Error::WrongSession);
        }
        let target = derive_signing_public(&application.public, &derivation)?;
        self.nonce_store.activate_threshold_dh_attempt(context)?;
        let pending =
            ThresholdDhRecord::pending(context.clone(), derivation.clone(), request.binding());
        match self.nonce_store.reserve_threshold_dh(pending.clone())? {
            ThresholdDhReservation::Existing(existing) => {
                if existing.context() != context
                    || existing.derivation() != &derivation
                    || existing.request_binding() != request.binding()
                {
                    return Err(Error::InvalidSigningSession(
                        "threshold-DH retry conflicts with the claimed session".to_string(),
                    ));
                }
                return match existing.lifecycle() {
                    ThresholdDhLifecycle::Completed => {
                        let result = existing.cached_result().cloned().ok_or_else(|| {
                            Error::NoncePersistence(
                                "threshold-DH completed result is unavailable".to_string(),
                            )
                        })?;
                        threshold_dh::verify_share(
                            &target,
                            context.signer_set(),
                            request,
                            &result,
                        )?;
                        Ok(result)
                    }
                    _ => Err(Error::InvalidSigningSession(
                        "threshold-DH session is pending or terminally burned".to_string(),
                    )),
                };
            }
            ThresholdDhReservation::Inserted => {}
        }
        let result = match (|| {
            let derived = self.root.derive_signing_target(registration, &derivation)?;
            threshold_dh::create_share(
                self.root.participant(),
                derived.signing_share_bytes()?,
                &target,
                context.signer_set(),
                request,
                rng,
            )
        })() {
            Ok(result) => result,
            Err(error) => {
                let burned = pending.transition(
                    ThresholdDhLifecycle::Burned {
                        reason: NonceBurnReason::InternalFailure,
                    },
                    None,
                )?;
                self.nonce_store
                    .compare_and_swap_threshold_dh(&pending, burned)?;
                return Err(error);
            }
        };
        let completed =
            pending.transition(ThresholdDhLifecycle::Completed, Some(result.clone()))?;
        self.nonce_store
            .compare_and_swap_threshold_dh(&pending, completed)?;
        Ok(result)
    }

    /// Compact protocol-specific terminal details while retaining permanent
    /// replay evidence.
    pub fn cleanup_terminal_protocols(&self, retain_detailed: usize) -> Result<usize, Error> {
        let virtual_count = self.nonce_store.compact_virtual_musig(retain_detailed)?;
        let dh_count = self.nonce_store.compact_threshold_dh(retain_detailed)?;
        virtual_count
            .checked_add(dh_count)
            .ok_or_else(|| Error::NoncePersistence("protocol cleanup count overflow".to_string()))
    }

    /// Permanently burn a virtual MuSig2 attempt by exact protocol session.
    pub fn burn_virtual_musig_session(
        &self,
        session_id: SigningSessionId,
        reason: NonceBurnReason,
    ) -> Result<(), Error> {
        let Some(record) = self.nonce_store.load_virtual_musig_session(session_id)? else {
            return Ok(());
        };
        self.burn_virtual_musig_record(&record, reason)
    }

    /// Permanently tombstone a pending threshold-DH attempt.
    pub fn burn_threshold_dh_session(
        &self,
        session_id: SigningSessionId,
        reason: NonceBurnReason,
    ) -> Result<(), Error> {
        let Some(record) = self.nonce_store.load_threshold_dh_session(session_id)? else {
            return Ok(());
        };
        if record.lifecycle().is_terminal() {
            return Ok(());
        }
        let burned = record.transition(ThresholdDhLifecycle::Burned { reason }, None)?;
        self.nonce_store
            .compare_and_swap_threshold_dh(&record, burned)
    }

    /// Permanently burn one nonce after abort, timeout, ambiguity, or failover.
    pub fn burn_nonce(&self, nonce_id: NonceId, reason: NonceBurnReason) -> Result<(), Error> {
        let record = self
            .nonce_store
            .load_nonce(nonce_id)?
            .ok_or_else(|| Error::InvalidCommitment("nonce is unknown".to_string()))?;
        self.burn_record(&record, reason)
    }

    /// Permanently burn the nonce for one exact session, if it was generated.
    pub fn burn_session(
        &self,
        session_id: SigningSessionId,
        reason: NonceBurnReason,
    ) -> Result<(), Error> {
        let Some(record) = self.nonce_store.load_session(session_id)? else {
            return Ok(());
        };
        self.burn_record(&record, reason)
    }

    /// Compact old terminal detail while preserving replay tombstones.
    pub fn cleanup_terminal_nonces(&self, retain_detailed: usize) -> Result<usize, Error> {
        self.nonce_store.compact_terminal(retain_detailed)
    }

    /// Burn every nonterminal record during process restart recovery.
    pub fn recover_after_restart(&self) -> Result<(), Error> {
        for record in self.nonce_store.nonterminal_records()? {
            self.burn_record(&record, NonceBurnReason::RestartRecovery)?;
        }
        for record in self.nonce_store.nonterminal_virtual_musig_records()? {
            match record.lifecycle() {
                VirtualMusigLifecycle::PartialProduced { transcript_binding } => {
                    let consumed = record.transition(
                        VirtualMusigLifecycle::Consumed {
                            transcript_binding: *transcript_binding,
                        },
                        false,
                        record.cached_partial().cloned(),
                    )?;
                    self.nonce_store
                        .compare_and_swap_virtual_musig(&record, consumed)?;
                }
                _ => self.burn_virtual_musig_record(&record, NonceBurnReason::RestartRecovery)?,
            }
        }
        for record in self.nonce_store.nonterminal_threshold_dh_records()? {
            let burned = record.transition(
                ThresholdDhLifecycle::Burned {
                    reason: NonceBurnReason::RestartRecovery,
                },
                None,
            )?;
            self.nonce_store
                .compare_and_swap_threshold_dh(&record, burned)?;
        }
        Ok(())
    }

    fn burn_record(&self, record: &NonceRecord, reason: NonceBurnReason) -> Result<(), Error> {
        if record.lifecycle().is_terminal() {
            return Ok(());
        }
        let burned = record.transition(NonceLifecycle::Burned { reason }, false, None)?;
        self.nonce_store.compare_and_swap(record, burned)
    }

    fn burn_virtual_musig_record(
        &self,
        record: &VirtualMusigRecord,
        reason: NonceBurnReason,
    ) -> Result<(), Error> {
        if record.lifecycle().is_terminal() {
            return Ok(());
        }
        let burned = record.transition(VirtualMusigLifecycle::Burned { reason }, false, None)?;
        self.nonce_store
            .compare_and_swap_virtual_musig(record, burned)
    }

    /// Verify and aggregate shares for an already-authorized 32-byte message.
    ///
    /// This coordinator operation has no root share and cannot create missing
    /// participant shares.
    pub fn aggregate(
        public: &DerivedPublicPackage,
        session_id: SigningSessionId,
        message: &[u8; 32],
        commitment_set: &NonceCommitmentSet,
        shares: impl IntoIterator<Item = SignatureShare>,
    ) -> Result<AggregateSignature, Error> {
        Self::aggregate_for_derivation(
            public,
            &SigningDerivation::application_key(),
            session_id,
            message,
            commitment_set,
            shares,
        )
    }

    /// Verify and aggregate shares for one exact derived signing target.
    pub fn aggregate_for_derivation(
        public: &DerivedPublicPackage,
        derivation: &SigningDerivation,
        session_id: SigningSessionId,
        message: &[u8; 32],
        commitment_set: &NonceCommitmentSet,
        shares: impl IntoIterator<Item = SignatureShare>,
    ) -> Result<AggregateSignature, Error> {
        let target = derive_signing_public(public, derivation)?;
        validate_commitment_set(
            commitment_set,
            public.root_epoch,
            public.registration,
            session_id,
            derivation.binding(),
            public.threshold,
        )?;
        let signing_package = signing_package(commitment_set, message)?;
        let mut library_shares = BTreeMap::new();
        for share in shares {
            if share.root_epoch != public.root_epoch
                || share.registration != public.registration
                || share.session_id != session_id
                || share.derivation_binding != derivation.binding()
            {
                return Err(Error::WrongSession);
            }
            if !commitment_set
                .commitments()
                .contains_key(&share.participant)
            {
                return Err(Error::InvalidShare(
                    "share participant is absent from commitment set".to_string(),
                ));
            }
            let library_share = frost::round2::SignatureShare::deserialize(&share.share)
                .map_err(|err| Error::InvalidShare(err.to_string()))?;
            if library_shares
                .insert(identifier(share.participant)?, library_share)
                .is_some()
            {
                return Err(Error::InvalidShare(
                    "duplicate participant signature share".to_string(),
                ));
            }
        }
        if library_shares.len() < usize::from(public.threshold) {
            return Err(Error::InsufficientThreshold {
                required: public.threshold,
                provided: library_shares.len(),
            });
        }
        if library_shares.len() != commitment_set.commitments().len() {
            return Err(Error::InvalidShare(
                "signature-share set differs from commitment signer set".to_string(),
            ));
        }
        let library_public = library_signing_public_package(&target)?;
        let signature = match derivation.signature_mode() {
            SignatureMode::Plain => {
                frost::aggregate(&signing_package, &library_shares, &library_public)
            }
            SignatureMode::TaprootKeyPath { merkle_root } => frost::aggregate_with_tweak(
                &signing_package,
                &library_shares,
                &library_public,
                merkle_root.as_ref().map(|root| root.as_slice()),
            ),
        }
        .map_err(|err| Error::InvalidShare(err.to_string()))?;
        let output_public = match derivation.signature_mode() {
            SignatureMode::Plain => library_public,
            SignatureMode::TaprootKeyPath { merkle_root } => {
                use frost_secp256k1_tr::keys::Tweak;
                library_public.tweak(merkle_root.as_ref().map(|root| root.as_slice()))
            }
        };
        output_public
            .verifying_key()
            .verify(message, &signature)
            .map_err(|_| Error::AggregateVerification)?;
        let encoded: [u8; 64] = signature
            .serialize()
            .map_err(|err| Error::Frost(err.to_string()))?
            .try_into()
            .map_err(|_| Error::Internal("FROST signature is not 64 bytes".to_string()))?;
        Ok(AggregateSignature::from_bytes_unchecked(encoded))
    }

    /// Independently verify an aggregate signature for one exact derived target.
    pub fn verify_aggregate_for_derivation(
        public: &DerivedPublicPackage,
        derivation: &SigningDerivation,
        message: &[u8; 32],
        signature: AggregateSignature,
    ) -> Result<(), Error> {
        let target = derive_signing_public(public, derivation)?;
        let mut library_public = library_signing_public_package(&target)?;
        if let SignatureMode::TaprootKeyPath { merkle_root } = derivation.signature_mode() {
            use frost_secp256k1_tr::keys::Tweak;
            library_public = library_public.tweak(merkle_root.as_ref().map(|root| root.as_slice()));
        }
        let signature = frost::Signature::deserialize(&signature.to_bytes())
            .map_err(|error| Error::InvalidShare(error.to_string()))?;
        library_public
            .verifying_key()
            .verify(message, &signature)
            .map_err(|_| Error::AggregateVerification)
    }

    /// Publicly derive one registered application's exact signing target.
    pub fn signing_public_package(
        &self,
        application_id: ApplicationId,
        derivation: &SigningDerivation,
    ) -> Result<DerivedSigningPublicPackage, Error> {
        derive_signing_public(self.public_package(application_id)?, derivation)
    }
}

fn nonce_secret_aad(
    context: &SigningSessionContext,
    nonce_id: NonceId,
    participant: crate::MemberId,
) -> Vec<u8> {
    let mut aad = Vec::with_capacity(8 + 32 + 32 + 32 + 2);
    aad.extend_from_slice(b"CDKFNON1");
    aad.extend_from_slice(&context.slot_id().to_bytes());
    aad.extend_from_slice(&context.session_id().to_bytes());
    aad.extend_from_slice(&nonce_id.to_bytes());
    aad.extend_from_slice(&participant.get().to_be_bytes());
    aad
}

fn virtual_musig_secret_aad(
    context: &SigningSessionContext,
    nonce_id: VirtualMusigNonceId,
    participant: crate::MemberId,
) -> Vec<u8> {
    let mut aad = Vec::with_capacity(8 + 32 + 32 + 32 + 2);
    aad.extend_from_slice(b"CDKFMUS1");
    aad.extend_from_slice(&context.slot_id().to_bytes());
    aad.extend_from_slice(&context.session_id().to_bytes());
    aad.extend_from_slice(&nonce_id.to_bytes());
    aad.extend_from_slice(&participant.get().to_be_bytes());
    aad
}

/// Canonical hash of the exact session, message, signer set, and commitments.
pub fn signing_package_hash(
    session_id: SigningSessionId,
    message: &[u8; 32],
    set: &NonceCommitmentSet,
) -> Result<[u8; 32], Error> {
    let mut hasher = Sha256::new();
    hasher.update(b"cdk/frost/signing-package/v1");
    hasher.update(session_id.to_bytes());
    hasher.update(message);
    hasher.update((set.commitments().len() as u16).to_be_bytes());
    for commitment in set.commitments().values() {
        let encoded = commitment.encode();
        hasher.update((encoded.len() as u16).to_be_bytes());
        hasher.update(encoded);
    }
    Ok(hasher.finalize().into())
}

fn validate_commitment_set(
    set: &NonceCommitmentSet,
    root_epoch: crate::RootKeyEpoch,
    registration: ApplicationRegistration,
    session_id: SigningSessionId,
    derivation_binding: [u8; 32],
    threshold: u16,
) -> Result<(), Error> {
    if set.commitments().len() < usize::from(threshold) {
        return Err(Error::InsufficientThreshold {
            required: threshold,
            provided: set.commitments().len(),
        });
    }
    for commitment in set.commitments().values() {
        if commitment.root_epoch != root_epoch
            || commitment.registration != registration
            || commitment.session_id != session_id
            || commitment.derivation_binding != derivation_binding
        {
            return Err(Error::WrongSession);
        }
        if commitment.commitments.len() > MAX_COMMITMENT_BYTES {
            return Err(Error::OversizedInput {
                kind: "nonce commitment",
                max: MAX_COMMITMENT_BYTES,
            });
        }
        if !commitment.has_valid_nonce_id() {
            return Err(Error::InvalidCommitment(
                "nonce identifier does not match its commitment".to_string(),
            ));
        }
        frost::round1::SigningCommitments::deserialize(&commitment.commitments)
            .map_err(|err| Error::InvalidCommitment(err.to_string()))?;
    }
    Ok(())
}

fn signing_package(
    set: &NonceCommitmentSet,
    message: &[u8; 32],
) -> Result<frost::SigningPackage, Error> {
    let mut commitments = BTreeMap::new();
    for (member, commitment) in set.commitments() {
        let parsed = frost::round1::SigningCommitments::deserialize(&commitment.commitments)
            .map_err(|err| Error::InvalidCommitment(err.to_string()))?;
        commitments.insert(identifier(*member)?, parsed);
    }
    Ok(frost::SigningPackage::new(commitments, message))
}
