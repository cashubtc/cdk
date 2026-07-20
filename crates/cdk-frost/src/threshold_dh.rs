//! Authorized threshold Diffie-Hellman with Chaum-Pedersen proofs.

use std::collections::BTreeMap;

use frost_secp256k1_tr::rand_core::{CryptoRng, RngCore};
use k256::elliptic_curve::bigint::U256;
use k256::elliptic_curve::ops::Reduce;
use k256::elliptic_curve::{Field, Group, PrimeField};
use k256::{FieldBytes, ProjectivePoint, Scalar};
use sha2::{Digest, Sha256};

use crate::canonical::{point_from_bytes, point_to_bytes, scalar_from_bytes};
use crate::{DerivedSigningPublicPackage, Error, MemberId, SignatureMode, SigningSessionId};

/// Exact operation-bound input for one threshold-DH contribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThresholdDhRequest {
    session_id: SigningSessionId,
    server_point: [u8; 33],
    context: [u8; 32],
}

impl ThresholdDhRequest {
    /// Construct and validate a threshold-DH request.
    pub fn new(
        session_id: SigningSessionId,
        server_point: [u8; 33],
        context: [u8; 32],
    ) -> Result<Self, Error> {
        point_from_bytes(&server_point, "threshold-DH server point")?;
        if context == [0; 32] {
            return Err(Error::InvalidSigningSession(
                "threshold-DH context cannot be all zeroes".to_string(),
            ));
        }
        Ok(Self {
            session_id,
            server_point,
            context,
        })
    }

    /// Protocol-separated session identifier.
    pub fn session_id(&self) -> SigningSessionId {
        self.session_id
    }

    /// Full compressed peer point.
    pub fn server_point(&self) -> [u8; 33] {
        self.server_point
    }

    /// Application-defined operation binding.
    pub fn context(&self) -> [u8; 32] {
        self.context
    }

    /// Commitment to the exact threshold-DH request.
    pub fn binding(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"cdk/frost/threshold-dh-request/v1");
        hash.update(self.session_id.to_bytes());
        hash.update(self.server_point);
        hash.update(self.context);
        hash.finalize().into()
    }
}

/// Chaum-Pedersen proof that two public points use the same secret scalar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThresholdDhProof {
    commitment_g: [u8; 33],
    commitment_s: [u8; 33],
    response: [u8; 32],
}

impl ThresholdDhProof {
    /// Commitment relative to the secp256k1 generator.
    pub fn commitment_g(&self) -> [u8; 33] {
        self.commitment_g
    }

    /// Commitment relative to the request's peer point.
    pub fn commitment_s(&self) -> [u8; 33] {
        self.commitment_s
    }

    /// Proof response scalar.
    pub fn response(&self) -> [u8; 32] {
        self.response
    }

    pub(crate) fn from_parts(
        commitment_g: [u8; 33],
        commitment_s: [u8; 33],
        response: [u8; 32],
    ) -> Result<Self, Error> {
        point_from_bytes(&commitment_g, "threshold-DH generator commitment")?;
        point_from_bytes(&commitment_s, "threshold-DH server commitment")?;
        Option::<Scalar>::from(Scalar::from_repr(response.into())).ok_or_else(|| {
            Error::InvalidShare("threshold-DH proof response is not canonical".to_string())
        })?;
        Ok(Self {
            commitment_g,
            commitment_s,
            response,
        })
    }
}

/// One participant's verifiable threshold-DH result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThresholdDhShare {
    participant: MemberId,
    session_id: SigningSessionId,
    request_binding: [u8; 32],
    shared_point: [u8; 33],
    proof: ThresholdDhProof,
}

impl ThresholdDhShare {
    /// Participant that created this result.
    pub fn participant(&self) -> MemberId {
        self.participant
    }

    /// Exact protocol session.
    pub fn session_id(&self) -> SigningSessionId {
        self.session_id
    }

    /// Commitment to the authorized request.
    pub fn request_binding(&self) -> [u8; 32] {
        self.request_binding
    }

    /// Participant-local shared point.
    pub fn shared_point(&self) -> [u8; 33] {
        self.shared_point
    }

    /// DLEQ proof binding this result to the participant verification share.
    pub fn proof(&self) -> &ThresholdDhProof {
        &self.proof
    }

    pub(crate) fn from_parts(
        participant: MemberId,
        session_id: SigningSessionId,
        request_binding: [u8; 32],
        shared_point: [u8; 33],
        proof: ThresholdDhProof,
    ) -> Result<Self, Error> {
        point_from_bytes(&shared_point, "threshold-DH shared point")?;
        Ok(Self {
            participant,
            session_id,
            request_binding,
            shared_point,
            proof,
        })
    }
}

pub(crate) fn create_share<R>(
    participant: MemberId,
    signing_share: [u8; 32],
    target: &DerivedSigningPublicPackage,
    signer_set: &[MemberId],
    request: &ThresholdDhRequest,
    rng: &mut R,
) -> Result<ThresholdDhShare, Error>
where
    R: CryptoRng + RngCore,
{
    validate_request(target, signer_set, request)?;
    if !signer_set.contains(&participant) {
        return Err(Error::InvalidParticipant {
            participant: participant.get(),
            reason: "participant is absent from threshold-DH signer set".to_string(),
        });
    }
    let secret = scalar_from_bytes(&signing_share, "threshold-DH signing share")?;
    let server = point_from_bytes(&request.server_point, "threshold-DH server point")?;
    let shared = server * secret;
    if bool::from(shared.is_identity()) {
        return Err(Error::InvalidShare(
            "threshold-DH result is the identity point".to_string(),
        ));
    }
    let witness = random_nonzero_scalar(rng);
    let commitment_g = ProjectivePoint::GENERATOR * witness;
    let commitment_s = server * witness;
    let verification = target
        .internal_verifying_shares()
        .get(&participant)
        .ok_or_else(|| Error::InvalidParticipant {
            participant: participant.get(),
            reason: "derived verification share is absent".to_string(),
        })?;
    let challenge = challenge(
        target,
        signer_set,
        request,
        participant,
        verification,
        &point_to_bytes(shared),
        &point_to_bytes(commitment_g),
        &point_to_bytes(commitment_s),
    );
    let proof = ThresholdDhProof::from_parts(
        point_to_bytes(commitment_g),
        point_to_bytes(commitment_s),
        (witness + challenge * secret).to_bytes().into(),
    )?;
    let share = ThresholdDhShare::from_parts(
        participant,
        request.session_id,
        request.binding(),
        point_to_bytes(shared),
        proof,
    )?;
    verify_share(target, signer_set, request, &share)?;
    Ok(share)
}

/// Verify a threshold-DH contribution against one derived verification share.
pub fn verify_share(
    target: &DerivedSigningPublicPackage,
    signer_set: &[MemberId],
    request: &ThresholdDhRequest,
    share: &ThresholdDhShare,
) -> Result<(), Error> {
    validate_request(target, signer_set, request)?;
    if share.session_id != request.session_id
        || share.request_binding != request.binding()
        || !signer_set.contains(&share.participant)
    {
        return Err(Error::WrongSession);
    }
    let verification_bytes = target
        .internal_verifying_shares()
        .get(&share.participant)
        .ok_or_else(|| Error::InvalidParticipant {
            participant: share.participant.get(),
            reason: "participant is absent from derived target".to_string(),
        })?;
    let verification = point_from_bytes(verification_bytes, "threshold-DH verification share")?;
    let server = point_from_bytes(&request.server_point, "threshold-DH server point")?;
    let shared = point_from_bytes(&share.shared_point, "threshold-DH shared point")?;
    let commitment_g = point_from_bytes(
        &share.proof.commitment_g,
        "threshold-DH generator commitment",
    )?;
    let commitment_s =
        point_from_bytes(&share.proof.commitment_s, "threshold-DH server commitment")?;
    let response = Option::<Scalar>::from(Scalar::from_repr(share.proof.response.into()))
        .ok_or_else(|| Error::InvalidShare("invalid threshold-DH response".to_string()))?;
    let challenge = challenge(
        target,
        signer_set,
        request,
        share.participant,
        verification_bytes,
        &share.shared_point,
        &share.proof.commitment_g,
        &share.proof.commitment_s,
    );
    if ProjectivePoint::GENERATOR * response != commitment_g + verification * challenge
        || server * response != commitment_s + shared * challenge
    {
        return Err(Error::InvalidShare(
            "threshold-DH DLEQ proof failed verification".to_string(),
        ));
    }
    Ok(())
}

/// Verify and Lagrange-combine a threshold of shared points into `x*S`.
pub fn combine_shares(
    target: &DerivedSigningPublicPackage,
    signer_set: &[MemberId],
    request: &ThresholdDhRequest,
    shares: impl IntoIterator<Item = ThresholdDhShare>,
) -> Result<[u8; 33], Error> {
    validate_request(target, signer_set, request)?;
    let mut ordered = BTreeMap::new();
    for share in shares {
        let participant = share.participant;
        if ordered.insert(participant, share).is_some() {
            return Err(Error::InvalidParticipant {
                participant: participant.get(),
                reason: "duplicate threshold-DH share".to_string(),
            });
        }
    }
    if ordered.keys().copied().collect::<Vec<_>>() != signer_set {
        return Err(Error::InvalidSigningSession(
            "threshold-DH share set differs from signer set".to_string(),
        ));
    }
    let mut combined = ProjectivePoint::IDENTITY;
    for (participant, share) in ordered {
        verify_share(target, signer_set, request, &share)?;
        let point = point_from_bytes(&share.shared_point, "threshold-DH shared point")?;
        combined += point * lagrange_coefficient(participant, signer_set)?;
    }
    if bool::from(combined.is_identity()) {
        return Err(Error::InvalidShare(
            "combined threshold-DH point is the identity".to_string(),
        ));
    }
    Ok(point_to_bytes(combined))
}

fn validate_request(
    target: &DerivedSigningPublicPackage,
    signer_set: &[MemberId],
    request: &ThresholdDhRequest,
) -> Result<(), Error> {
    if target.signature_mode() != SignatureMode::Plain
        || signer_set.len() < usize::from(target.application().threshold())
        || signer_set.windows(2).any(|pair| pair[0] >= pair[1])
        || signer_set
            .iter()
            .any(|member| !target.internal_verifying_shares().contains_key(member))
    {
        return Err(Error::InvalidSigningSession(
            "threshold-DH target or signer set is invalid".to_string(),
        ));
    }
    point_from_bytes(&request.server_point, "threshold-DH server point")?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn challenge(
    target: &DerivedSigningPublicPackage,
    signer_set: &[MemberId],
    request: &ThresholdDhRequest,
    participant: MemberId,
    verification_share: &[u8; 33],
    shared_point: &[u8; 33],
    commitment_g: &[u8; 33],
    commitment_s: &[u8; 33],
) -> Scalar {
    let mut hash = Sha256::new();
    hash.update(b"cdk/frost/threshold-dh-dleq/v1");
    hash.update(target.application().root_epoch().get().to_be_bytes());
    let registration = target.application().registration();
    hash.update(registration.id().to_bytes());
    hash.update(registration.key_epoch().get().to_be_bytes());
    hash.update(registration.derivation_version().get().to_be_bytes());
    hash.update(registration.authorization_policy());
    hash.update(target.derivation_binding());
    hash.update(target.internal_group_key());
    hash.update(request.session_id.to_bytes());
    hash.update((signer_set.len() as u16).to_be_bytes());
    for signer in signer_set {
        hash.update(signer.get().to_be_bytes());
    }
    hash.update(request.server_point);
    hash.update(request.context);
    hash.update(participant.get().to_be_bytes());
    hash.update(verification_share);
    hash.update(shared_point);
    hash.update(commitment_g);
    hash.update(commitment_s);
    let bytes: [u8; 32] = hash.finalize().into();
    <Scalar as Reduce<U256>>::reduce_bytes(&FieldBytes::from(bytes))
}

fn lagrange_coefficient(participant: MemberId, signer_set: &[MemberId]) -> Result<Scalar, Error> {
    if !signer_set.contains(&participant) {
        return Err(Error::InvalidParticipant {
            participant: participant.get(),
            reason: "participant is absent from signer set".to_string(),
        });
    }
    let own = Scalar::from(u64::from(participant.get()));
    let mut numerator = Scalar::ONE;
    let mut denominator = Scalar::ONE;
    for other in signer_set
        .iter()
        .copied()
        .filter(|member| *member != participant)
    {
        let other = Scalar::from(u64::from(other.get()));
        numerator *= other;
        denominator *= other - own;
    }
    let inverse = Option::<Scalar>::from(denominator.invert()).ok_or_else(|| {
        Error::InvalidSigningSession("signer set has no Lagrange inverse".to_string())
    })?;
    Ok(numerator * inverse)
}

fn random_nonzero_scalar<R>(rng: &mut R) -> Scalar
where
    R: CryptoRng + RngCore,
{
    loop {
        let scalar = Scalar::random(&mut *rng);
        if !bool::from(scalar.is_zero()) {
            return scalar;
        }
    }
}
