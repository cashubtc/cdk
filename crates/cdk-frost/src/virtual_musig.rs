//! Application-neutral virtual MuSig2 transcript and share primitives.

use std::collections::BTreeMap;

use frost_secp256k1_tr::rand_core::{CryptoRng, RngCore};
use k256::elliptic_curve::Field;
use k256::Scalar as KScalar;
use musig2::secp::{MaybeScalar, Point, Scalar, G};
use musig2::{compute_challenge_hash_tweak, AggNonce, BinaryEncoding, KeyAggContext, PubNonce};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{
    DerivedSigningPublicPackage, Error, MemberId, RootKeyEpoch, SignatureMode,
    SigningSessionContext, SigningSessionId,
};

/// Opaque identifier for one protocol-separated virtual MuSig2 nonce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VirtualMusigNonceId([u8; 32]);

impl VirtualMusigNonceId {
    pub(crate) fn derive(
        context: &SigningSessionContext,
        participant: MemberId,
        public_nonce: &[u8; 66],
    ) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"cdk/frost/virtual-musig2-nonce-id/v1");
        hash.update(context.root_epoch().get().to_be_bytes());
        hash.update(context.registration().id().to_bytes());
        hash.update(context.session_id().to_bytes());
        hash.update(context.derivation_binding());
        hash.update(participant.get().to_be_bytes());
        hash.update(public_nonce);
        Self(hash.finalize().into())
    }

    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Result<Self, Error> {
        if bytes == [0; 32] {
            return Err(Error::malformed(
                "virtual MuSig2 nonce identifier",
                "identifier cannot be all zeroes",
            ));
        }
        Ok(Self(bytes))
    }

    /// Return canonical identifier bytes.
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }
}

/// Public two-column nonce contribution for one virtual MuSig2 participant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualMusigNonceCommitment {
    root_epoch: RootKeyEpoch,
    session_id: SigningSessionId,
    derivation_binding: [u8; 32],
    nonce_id: VirtualMusigNonceId,
    participant: MemberId,
    public_nonce: [u8; 66],
}

impl VirtualMusigNonceCommitment {
    /// Root epoch bound to this commitment.
    pub fn root_epoch(&self) -> RootKeyEpoch {
        self.root_epoch
    }

    /// Protocol-separated session bound to this commitment.
    pub fn session_id(&self) -> SigningSessionId {
        self.session_id
    }

    /// Exact child derivation bound to this commitment.
    pub fn derivation_binding(&self) -> [u8; 32] {
        self.derivation_binding
    }

    /// Single-use virtual MuSig2 nonce identifier.
    pub fn nonce_id(&self) -> VirtualMusigNonceId {
        self.nonce_id
    }

    /// Participant that owns this nonce.
    pub fn participant(&self) -> MemberId {
        self.participant
    }

    /// BIP327 two-column public nonce.
    pub fn public_nonce(&self) -> [u8; 66] {
        self.public_nonce
    }

    pub(crate) fn new(
        context: &SigningSessionContext,
        participant: MemberId,
        public_nonce: [u8; 66],
    ) -> Result<Self, Error> {
        PubNonce::from_bytes(&public_nonce)
            .map_err(|error| Error::InvalidCommitment(error.to_string()))?;
        let nonce_id = VirtualMusigNonceId::derive(context, participant, &public_nonce);
        Ok(Self {
            root_epoch: context.root_epoch(),
            session_id: context.session_id(),
            derivation_binding: context.derivation_binding(),
            nonce_id,
            participant,
            public_nonce,
        })
    }
}

/// Canonically ordered complete set of virtual MuSig2 nonce contributions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualMusigNonceCommitmentSet {
    commitments: BTreeMap<MemberId, VirtualMusigNonceCommitment>,
}

impl VirtualMusigNonceCommitmentSet {
    /// Validate, order, and construct a complete commitment set.
    pub fn new(
        commitments: impl IntoIterator<Item = VirtualMusigNonceCommitment>,
    ) -> Result<Self, Error> {
        let mut ordered = BTreeMap::new();
        for commitment in commitments {
            let participant = commitment.participant;
            if ordered.insert(participant, commitment).is_some() {
                return Err(Error::InvalidParticipant {
                    participant: participant.get(),
                    reason: "duplicate virtual MuSig2 commitment".to_string(),
                });
            }
        }
        let Some(first) = ordered.values().next() else {
            return Err(Error::InvalidCommitment(
                "virtual MuSig2 commitment set is empty".to_string(),
            ));
        };
        if ordered.values().any(|commitment| {
            commitment.root_epoch != first.root_epoch
                || commitment.session_id != first.session_id
                || commitment.derivation_binding != first.derivation_binding
        }) {
            return Err(Error::WrongSession);
        }
        Ok(Self {
            commitments: ordered,
        })
    }

    /// Commitments in canonical member order.
    pub fn commitments(&self) -> &BTreeMap<MemberId, VirtualMusigNonceCommitment> {
        &self.commitments
    }

    /// Sum both public-nonce columns for the virtual outer signer.
    pub fn aggregate_public_nonce(&self) -> Result<[u8; 66], Error> {
        let nonces = self
            .commitments
            .values()
            .map(|commitment| {
                PubNonce::from_bytes(&commitment.public_nonce)
                    .map_err(|error| Error::InvalidCommitment(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(AggNonce::sum(&nonces).to_bytes())
    }
}

/// Exact BIP327 outer transcript authorized for a virtual threshold partial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualMusigTranscript {
    session_id: SigningSessionId,
    outer_pubkeys: [[u8; 33]; 2],
    user_pubkey: [u8; 33],
    user_public_nonce: [u8; 66],
    server_public_nonce: [u8; 66],
    message: [u8; 32],
    xonly_tweak: Option<[u8; 32]>,
}

impl VirtualMusigTranscript {
    /// Construct and validate an exact two-party Bark-compatible transcript.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_id: SigningSessionId,
        mut outer_pubkeys: [[u8; 33]; 2],
        user_pubkey: [u8; 33],
        user_public_nonce: [u8; 66],
        server_public_nonce: [u8; 66],
        message: [u8; 32],
        xonly_tweak: Option<[u8; 32]>,
    ) -> Result<Self, Error> {
        outer_pubkeys.sort();
        if outer_pubkeys[0] == outer_pubkeys[1]
            || outer_pubkeys
                .iter()
                .filter(|key| **key == user_pubkey)
                .count()
                != 1
        {
            return Err(Error::InvalidCommitment(
                "virtual MuSig2 requires distinct user and server keys".to_string(),
            ));
        }
        for key in &outer_pubkeys {
            Point::try_from(key)
                .map_err(|_| Error::InvalidCommitment("invalid outer MuSig2 key".to_string()))?;
        }
        PubNonce::from_bytes(&user_public_nonce)
            .map_err(|error| Error::InvalidCommitment(error.to_string()))?;
        PubNonce::from_bytes(&server_public_nonce)
            .map_err(|error| Error::InvalidCommitment(error.to_string()))?;
        if let Some(tweak) = xonly_tweak {
            MaybeScalar::try_from(tweak)
                .map_err(|_| Error::InvalidCommitment("invalid MuSig2 x-only tweak".to_string()))?;
        }
        Ok(Self {
            session_id,
            outer_pubkeys,
            user_pubkey,
            user_public_nonce,
            server_public_nonce,
            message,
            xonly_tweak,
        })
    }

    /// Protocol-separated session identifier.
    pub fn session_id(&self) -> SigningSessionId {
        self.session_id
    }

    /// Canonically ordered outer signer keys.
    pub fn outer_pubkeys(&self) -> &[[u8; 33]; 2] {
        &self.outer_pubkeys
    }

    /// Virtual threshold signer's derived public key.
    pub fn user_pubkey(&self) -> [u8; 33] {
        self.user_pubkey
    }

    /// Aggregated threshold public nonce used as the outer user nonce.
    pub fn user_public_nonce(&self) -> [u8; 66] {
        self.user_public_nonce
    }

    /// Ark server's outer public nonce.
    pub fn server_public_nonce(&self) -> [u8; 66] {
        self.server_public_nonce
    }

    /// Exact BIP340 message digest.
    pub fn message(&self) -> [u8; 32] {
        self.message
    }

    /// Optional BIP327 x-only tweak.
    pub fn xonly_tweak(&self) -> Option<[u8; 32]> {
        self.xonly_tweak
    }

    /// Commitment to every byte of the finalized outer transcript.
    pub fn binding(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"cdk/frost/virtual-musig2-transcript/v1");
        hash.update(self.session_id.to_bytes());
        for key in self.outer_pubkeys {
            hash.update(key);
        }
        hash.update(self.user_pubkey);
        hash.update(self.user_public_nonce);
        hash.update(self.server_public_nonce);
        hash.update(self.message);
        match self.xonly_tweak {
            Some(tweak) => {
                hash.update([1]);
                hash.update(tweak);
            }
            None => hash.update([0]),
        }
        hash.finalize().into()
    }
}

/// One verified scalar contribution to the virtual outer partial signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualMusigPartialShare {
    participant: MemberId,
    session_id: SigningSessionId,
    transcript_binding: [u8; 32],
    share: [u8; 32],
}

impl VirtualMusigPartialShare {
    /// Participant that created this share.
    pub fn participant(&self) -> MemberId {
        self.participant
    }

    /// Exact protocol session.
    pub fn session_id(&self) -> SigningSessionId {
        self.session_id
    }

    /// Exact finalized transcript binding.
    pub fn transcript_binding(&self) -> [u8; 32] {
        self.transcript_binding
    }

    /// Canonical scalar bytes.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.share
    }

    pub(crate) fn from_parts(
        participant: MemberId,
        session_id: SigningSessionId,
        transcript_binding: [u8; 32],
        share: [u8; 32],
    ) -> Result<Self, Error> {
        MaybeScalar::try_from(share)
            .map_err(|_| Error::InvalidShare("invalid virtual MuSig2 scalar".to_string()))?;
        Ok(Self {
            participant,
            session_id,
            transcript_binding,
            share,
        })
    }
}

pub(crate) fn create_nonce<R>(rng: &mut R) -> Result<(Zeroizing<[u8; 64]>, [u8; 66]), Error>
where
    R: CryptoRng + RngCore,
{
    let k1 = random_nonzero_scalar(rng);
    let k2 = random_nonzero_scalar(rng);
    let r1 = Scalar::try_from(k1)
        .map_err(|_| Error::Internal("invalid first MuSig2 nonce scalar".to_string()))?
        .base_point_mul();
    let r2 = Scalar::try_from(k2)
        .map_err(|_| Error::Internal("invalid second MuSig2 nonce scalar".to_string()))?
        .base_point_mul();
    let mut secret = [0_u8; 64];
    secret[..32].copy_from_slice(&k1.to_bytes());
    secret[32..].copy_from_slice(&k2.to_bytes());
    Ok((Zeroizing::new(secret), PubNonce::new(r1, r2).to_bytes()))
}

pub(crate) fn create_partial_share(
    participant: MemberId,
    signing_share: [u8; 32],
    target: &DerivedSigningPublicPackage,
    signer_set: &[MemberId],
    secret_nonce: &[u8; 64],
    commitment: &VirtualMusigNonceCommitment,
    transcript: &VirtualMusigTranscript,
) -> Result<VirtualMusigPartialShare, Error> {
    validate_transcript(target, signer_set, commitment, transcript)?;
    let context = outer_context(transcript)?;
    let k1 = Scalar::try_from(&secret_nonce[..32])
        .map_err(|_| Error::InvalidCommitment("invalid first MuSig2 nonce".to_string()))?;
    let k2 = Scalar::try_from(&secret_nonce[32..])
        .map_err(|_| Error::InvalidCommitment("invalid second MuSig2 nonce".to_string()))?;
    let nonce_coefficient: MaybeScalar = context.aggregated_nonce.nonce_coefficient(
        context.key_agg.aggregated_pubkey::<Point>(),
        transcript.message,
    );
    let mut nonce_term = k1 + nonce_coefficient * k2;
    if context.final_nonce.has_odd_y() {
        nonce_term = -nonce_term;
    }

    let participant_secret = Scalar::try_from(signing_share)
        .map_err(|_| Error::Internal("invalid derived signing share".to_string()))?;
    let lagrange = Scalar::try_from(lagrange_coefficient(participant, signer_set)?.to_bytes())
        .map_err(|_| Error::Internal("invalid Lagrange coefficient".to_string()))?;
    let user = Point::try_from(&transcript.user_pubkey)
        .map_err(|_| Error::InvalidCommitment("invalid virtual user key".to_string()))?;
    let key_coefficient = context.key_agg.key_coefficient(user).ok_or_else(|| {
        Error::InvalidCommitment("virtual user key is outside key aggregation".to_string())
    })?;
    let challenge: MaybeScalar = compute_challenge_hash_tweak(
        &context.final_nonce.serialize_xonly(),
        &context.key_agg.aggregated_pubkey::<Point>(),
        transcript.message,
    );
    let mut key_term = challenge * key_coefficient * lagrange * participant_secret;
    if context.challenge_negated {
        key_term = -key_term;
    }
    let partial = VirtualMusigPartialShare::from_parts(
        participant,
        transcript.session_id,
        transcript.binding(),
        (nonce_term + key_term).serialize(),
    )?;
    verify_partial_share(target, signer_set, commitment, transcript, &partial)?;
    Ok(partial)
}

/// Verify one virtual partial against the participant's derived verification share.
pub fn verify_partial_share(
    target: &DerivedSigningPublicPackage,
    signer_set: &[MemberId],
    commitment: &VirtualMusigNonceCommitment,
    transcript: &VirtualMusigTranscript,
    share: &VirtualMusigPartialShare,
) -> Result<(), Error> {
    validate_transcript(target, signer_set, commitment, transcript)?;
    if share.participant != commitment.participant
        || share.session_id != transcript.session_id
        || share.transcript_binding != transcript.binding()
    {
        return Err(Error::WrongSession);
    }
    let context = outer_context(transcript)?;
    let public_nonce = PubNonce::from_bytes(&commitment.public_nonce)
        .map_err(|error| Error::InvalidCommitment(error.to_string()))?;
    let nonce_coefficient: MaybeScalar = context.aggregated_nonce.nonce_coefficient(
        context.key_agg.aggregated_pubkey::<Point>(),
        transcript.message,
    );
    let mut nonce_point = public_nonce.R1 + nonce_coefficient * public_nonce.R2;
    if context.final_nonce.has_odd_y() {
        nonce_point = -nonce_point;
    }
    let verification_bytes = target
        .internal_verifying_shares()
        .get(&share.participant)
        .ok_or_else(|| Error::InvalidParticipant {
            participant: share.participant.get(),
            reason: "participant is absent from derived target".to_string(),
        })?;
    let verification = Point::try_from(verification_bytes)
        .map_err(|_| Error::InvalidShare("invalid derived verification share".to_string()))?;
    let lagrange =
        Scalar::try_from(lagrange_coefficient(share.participant, signer_set)?.to_bytes())
            .map_err(|_| Error::Internal("invalid Lagrange coefficient".to_string()))?;
    let user = Point::try_from(&transcript.user_pubkey)
        .map_err(|_| Error::InvalidCommitment("invalid virtual user key".to_string()))?;
    let key_coefficient = context.key_agg.key_coefficient(user).ok_or_else(|| {
        Error::InvalidCommitment("virtual user key is outside key aggregation".to_string())
    })?;
    let challenge: MaybeScalar = compute_challenge_hash_tweak(
        &context.final_nonce.serialize_xonly(),
        &context.key_agg.aggregated_pubkey::<Point>(),
        transcript.message,
    );
    let mut challenge_point = challenge * key_coefficient * lagrange * verification;
    if context.challenge_negated {
        challenge_point = -challenge_point;
    }
    let partial = MaybeScalar::try_from(share.share)
        .map_err(|_| Error::InvalidShare("invalid virtual MuSig2 scalar".to_string()))?;
    if partial * G != nonce_point + challenge_point {
        return Err(Error::InvalidShare(
            "virtual MuSig2 partial failed verification".to_string(),
        ));
    }
    Ok(())
}

/// Verify and combine every threshold contribution into one virtual outer partial.
pub fn combine_partial_shares(
    target: &DerivedSigningPublicPackage,
    signer_set: &[MemberId],
    transcript: &VirtualMusigTranscript,
    contributions: impl IntoIterator<Item = (VirtualMusigNonceCommitment, VirtualMusigPartialShare)>,
) -> Result<[u8; 32], Error> {
    let mut ordered = BTreeMap::new();
    for (commitment, share) in contributions {
        if commitment.participant() != share.participant()
            || ordered
                .insert(share.participant(), (commitment, share))
                .is_some()
        {
            return Err(Error::InvalidSigningSession(
                "duplicate or mismatched virtual MuSig2 contribution".to_string(),
            ));
        }
    }
    if ordered.keys().copied().collect::<Vec<_>>() != signer_set {
        return Err(Error::InvalidSigningSession(
            "virtual MuSig2 contribution set differs from signer set".to_string(),
        ));
    }
    let mut combined = MaybeScalar::Zero;
    for (commitment, share) in ordered.values() {
        verify_partial_share(target, signer_set, commitment, transcript, share)?;
        combined += MaybeScalar::try_from(share.share)
            .map_err(|_| Error::InvalidShare("invalid virtual MuSig2 scalar".to_string()))?;
    }
    let context = outer_context(transcript)?;
    let user = Point::try_from(&transcript.user_pubkey)
        .map_err(|_| Error::InvalidCommitment("invalid virtual user key".to_string()))?;
    let user_nonce = PubNonce::from_bytes(&transcript.user_public_nonce)
        .map_err(|error| Error::InvalidCommitment(error.to_string()))?;
    musig2::verify_partial(
        &context.key_agg,
        combined,
        &context.aggregated_nonce,
        user,
        &user_nonce,
        transcript.message,
    )
    .map_err(|_| Error::InvalidShare("combined virtual MuSig2 partial is invalid".to_string()))?;
    Ok(combined.serialize())
}

fn validate_transcript(
    target: &DerivedSigningPublicPackage,
    signer_set: &[MemberId],
    commitment: &VirtualMusigNonceCommitment,
    transcript: &VirtualMusigTranscript,
) -> Result<(), Error> {
    if target.signature_mode() != SignatureMode::Plain
        || transcript.user_pubkey != target.output_group_key()
        || commitment.session_id != transcript.session_id
        || commitment.derivation_binding != target.derivation_binding()
        || signer_set.len() < usize::from(target.application().threshold())
        || signer_set.windows(2).any(|pair| pair[0] >= pair[1])
        || !signer_set.contains(&commitment.participant)
        || signer_set
            .iter()
            .any(|member| !target.internal_verifying_shares().contains_key(member))
    {
        return Err(Error::WrongSession);
    }
    Ok(())
}

struct OuterContext {
    key_agg: KeyAggContext,
    aggregated_nonce: AggNonce,
    final_nonce: Point,
    challenge_negated: bool,
}

fn outer_context(transcript: &VirtualMusigTranscript) -> Result<OuterContext, Error> {
    let keys = transcript
        .outer_pubkeys
        .iter()
        .map(|key| {
            Point::try_from(key)
                .map_err(|_| Error::InvalidCommitment("invalid outer MuSig2 key".to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let base =
        KeyAggContext::new(keys).map_err(|error| Error::InvalidCommitment(error.to_string()))?;
    let parity_acc =
        transcript.xonly_tweak.is_some() && base.aggregated_pubkey::<Point>().has_odd_y();
    let key_agg = match transcript.xonly_tweak {
        Some(tweak) => base
            .with_xonly_tweak(
                MaybeScalar::try_from(tweak)
                    .map_err(|_| Error::InvalidCommitment("invalid x-only tweak".to_string()))?,
            )
            .map_err(|error| Error::InvalidCommitment(error.to_string()))?,
        None => base,
    };
    let user_nonce = PubNonce::from_bytes(&transcript.user_public_nonce)
        .map_err(|error| Error::InvalidCommitment(error.to_string()))?;
    let server_nonce = PubNonce::from_bytes(&transcript.server_public_nonce)
        .map_err(|error| Error::InvalidCommitment(error.to_string()))?;
    let aggregated_nonce = AggNonce::sum([user_nonce, server_nonce]);
    let aggregate_key = key_agg.aggregated_pubkey::<Point>();
    let nonce_coefficient: MaybeScalar =
        aggregated_nonce.nonce_coefficient(aggregate_key, transcript.message);
    let final_nonce = aggregated_nonce.final_nonce(nonce_coefficient);
    let challenge_negated = aggregate_key.has_odd_y() ^ parity_acc;
    Ok(OuterContext {
        key_agg,
        aggregated_nonce,
        final_nonce,
        challenge_negated,
    })
}

fn lagrange_coefficient(participant: MemberId, signer_set: &[MemberId]) -> Result<KScalar, Error> {
    if !signer_set.contains(&participant) {
        return Err(Error::InvalidParticipant {
            participant: participant.get(),
            reason: "participant is absent from signer set".to_string(),
        });
    }
    let own = KScalar::from(u64::from(participant.get()));
    let mut numerator = KScalar::ONE;
    let mut denominator = KScalar::ONE;
    for other in signer_set
        .iter()
        .copied()
        .filter(|member| *member != participant)
    {
        let other = KScalar::from(u64::from(other.get()));
        numerator *= other;
        denominator *= other - own;
    }
    let inverse = Option::<KScalar>::from(denominator.invert()).ok_or_else(|| {
        Error::InvalidSigningSession("signer set has no Lagrange inverse".to_string())
    })?;
    Ok(numerator * inverse)
}

fn random_nonzero_scalar<R>(rng: &mut R) -> KScalar
where
    R: CryptoRng + RngCore,
{
    loop {
        let scalar = KScalar::random(&mut *rng);
        if !bool::from(scalar.is_zero()) {
            return scalar;
        }
    }
}
