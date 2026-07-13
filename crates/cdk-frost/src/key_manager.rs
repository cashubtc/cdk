//! Participant-local application registry, nonce lifecycle, and signing API.

use std::collections::BTreeMap;
use std::fmt;

use frost_secp256k1_tr as frost;
use frost_secp256k1_tr::rand_core::{CryptoRng, RngCore};

use crate::canonical::{
    derive_signing_public, identifier, library_signing_public_package, CanonicalRootKeyPackage,
};
use crate::types::{
    derive_nonce_id, AggregateSignature, ApplicationId, ApplicationRegistration,
    AuthorizationRequest, AuthorizedSigningRequest, DerivedPublicPackage,
    DerivedSigningPublicPackage, NonceCommitment, NonceCommitmentSet, NonceId, SignatureMode,
    SignatureShare, SigningDerivation, SigningSessionId, MAX_COMMITMENT_BYTES,
};
use crate::Error;

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

struct PendingNonce {
    registration: ApplicationRegistration,
    session_id: SigningSessionId,
    derivation: SigningDerivation,
    nonces: frost::round1::SigningNonces,
    public: NonceCommitment,
}

/// Participant-local owner of one root share, registered applications, and
/// single-use nonce state.
pub struct ParticipantKeyManager {
    root: CanonicalRootKeyPackage,
    applications: BTreeMap<ApplicationId, RegisteredApplication>,
    pending_nonces: BTreeMap<NonceId, PendingNonce>,
}

impl fmt::Debug for ParticipantKeyManager {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ParticipantKeyManager")
            .field("participant", &self.root.participant())
            .field("root_epoch", &self.root.public().epoch())
            .field("root_share", &"<redacted>")
            .field("registered_applications", &self.applications.keys())
            .field("pending_nonce_count", &self.pending_nonces.len())
            .finish()
    }
}

impl ParticipantKeyManager {
    /// Take ownership of an activated canonical root package.
    pub fn new(root: CanonicalRootKeyPackage) -> Self {
        Self {
            root,
            applications: BTreeMap::new(),
            pending_nonces: BTreeMap::new(),
        }
    }

    /// Return the immutable application-neutral public root package.
    pub fn root_public_package(&self) -> &crate::RootPublicPackage {
        self.root.public()
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
        application_id: ApplicationId,
        session_id: SigningSessionId,
        rng: &mut R,
    ) -> Result<NonceCommitment, Error>
    where
        R: CryptoRng + RngCore,
    {
        self.commit_nonce_for_derivation(
            application_id,
            session_id,
            SigningDerivation::application_key(),
            rng,
        )
    }

    /// Generate a single-use nonce for one exact child derivation and final
    /// signature mode.
    pub fn commit_nonce_for_derivation<R>(
        &mut self,
        application_id: ApplicationId,
        session_id: SigningSessionId,
        derivation: SigningDerivation,
        rng: &mut R,
    ) -> Result<NonceCommitment, Error>
    where
        R: CryptoRng + RngCore,
    {
        let application = self
            .applications
            .get(&application_id)
            .ok_or_else(|| Error::Application("application is not registered".to_string()))?;
        if self.pending_nonces.values().any(|nonce| {
            nonce.registration.id() == application_id && nonce.session_id == session_id
        }) {
            return Err(Error::InvalidCommitment(
                "a nonce already exists for this application session".to_string(),
            ));
        }
        let derived = self
            .root
            .derive_signing_target(application.adapter.registration(), &derivation)?;
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
            application.adapter.registration(),
            session_id,
            derivation.binding(),
            self.root.participant(),
            &encoded,
        );
        if self.pending_nonces.contains_key(&nonce_id) {
            return Err(Error::InvalidCommitment(
                "nonce identifier collision".to_string(),
            ));
        }
        let public = NonceCommitment {
            root_epoch: self.root.public().epoch(),
            registration: application.adapter.registration(),
            session_id,
            derivation_binding: derivation.binding(),
            nonce_id,
            participant: self.root.participant(),
            commitments: encoded,
        };
        self.pending_nonces.insert(
            nonce_id,
            PendingNonce {
                registration: application.adapter.registration(),
                session_id,
                derivation,
                nonces,
                public: public.clone(),
            },
        );
        Ok(public)
    }

    /// Authorize a request through its registered adapter, consume the local
    /// nonce, and produce one signature share.
    pub fn sign_share(
        &mut self,
        nonce_id: NonceId,
        request: &AuthorizationRequest,
        commitment_set: &NonceCommitmentSet,
    ) -> Result<SignatureShare, Error> {
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
        let pending = self
            .pending_nonces
            .get(&nonce_id)
            .ok_or_else(|| Error::InvalidCommitment("nonce is unknown or consumed".to_string()))?;
        if pending.registration != registration || pending.session_id != request.session_id() {
            return Err(Error::WrongSession);
        }
        let authorized = application.adapter.authorize(request)?;
        if authorized.derivation() != &pending.derivation {
            return Err(Error::WrongSession);
        }
        validate_commitment_set(
            commitment_set,
            self.root.public().epoch(),
            registration,
            request.session_id(),
            pending.derivation.binding(),
            self.root.public().threshold(),
        )?;
        let own_commitment = commitment_set
            .commitments()
            .get(&self.root.participant())
            .ok_or_else(|| {
                Error::InvalidCommitment("local participant is absent from signer set".to_string())
            })?;
        if own_commitment != &pending.public {
            return Err(Error::InvalidCommitment(
                "coordinator supplied another local commitment".to_string(),
            ));
        }
        let signing_package = signing_package(commitment_set, &authorized.message())?;
        let derived = self
            .root
            .derive_signing_target(registration, &pending.derivation)?;

        // Removal happens before the secret nonce is used. Every subsequent
        // outcome consumes it and retry requires a fresh commitment.
        let pending = self
            .pending_nonces
            .remove(&nonce_id)
            .ok_or_else(|| Error::InvalidCommitment("nonce was consumed".to_string()))?;
        let share = match pending.derivation.signature_mode() {
            SignatureMode::Plain => {
                frost::round2::sign(&signing_package, &pending.nonces, &derived.key)
            }
            SignatureMode::TaprootKeyPath { merkle_root } => frost::round2::sign_with_tweak(
                &signing_package,
                &pending.nonces,
                &derived.key,
                merkle_root.as_ref().map(|root| root.as_slice()),
            ),
        }
        .map_err(|err| Error::InvalidShare(err.to_string()))?;
        let share = share
            .serialize()
            .try_into()
            .map_err(|_| Error::Internal("FROST signature share is not 32 bytes".to_string()))?;
        Ok(SignatureShare {
            root_epoch: self.root.public().epoch(),
            registration,
            session_id: request.session_id(),
            derivation_binding: pending.derivation.binding(),
            participant: self.root.participant(),
            share,
        })
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

    /// Publicly derive one registered application's exact signing target.
    pub fn signing_public_package(
        &self,
        application_id: ApplicationId,
        derivation: &SigningDerivation,
    ) -> Result<DerivedSigningPublicPackage, Error> {
        derive_signing_public(self.public_package(application_id)?, derivation)
    }
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
