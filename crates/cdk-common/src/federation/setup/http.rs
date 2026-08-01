//! Setup-only HTTP request correlation and authenticated response types.

use core::fmt;
use core::str::FromStr;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use super::{
    append_bytes, append_count, hash_transcript, invalid, transcript, validate_nonzero_identifier,
    validate_setup_version, FederationSetupApproval, FederationSetupCoordinatorPhase,
    FederationSetupCoordinatorPublicKey, FederationSetupEnrollment, FederationSetupEnrollmentHash,
    FederationSetupId, FederationSetupInvitationId, FederationSetupManifest,
    FederationSetupManifestHash, FederationSetupProposal, FederationSetupProposalHash,
    FederationSetupSignature, MemberId, Result, SecretKey, FEDERATION_MAX_SETUP_STATUS_BYTES,
    FEDERATION_SETUP_PROTOCOL_VERSION,
};

const ENROLLMENT_ACK_DOMAIN: &[u8] = b"cdk/federation/setup/enrollment-ack/v1";
const ENROLLMENT_RECORD_DOMAIN: &[u8] = b"cdk/federation/setup/enrollment-record/v1";
const RESPONSE_DOMAIN: &[u8] = b"cdk/federation/setup/response/v1";
const RESPONSE_BODY_DOMAIN: &[u8] = b"cdk/federation/setup/response-body/v1";

/// Header carrying the request nonce echoed by authenticated setup responses.
pub const FEDERATION_SETUP_CORRELATION_HEADER: &str = "x-cdk-setup-correlation";

/// Fresh correlation nonce for one logical setup HTTP request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FederationSetupCorrelationNonce([u8; 32]);

impl FederationSetupCorrelationNonce {
    /// Construct a non-zero correlation nonce.
    pub fn from_bytes(bytes: [u8; 32]) -> Result<Self> {
        validate_nonzero_identifier("setup correlation nonce", bytes)?;
        Ok(Self(bytes))
    }

    /// Return the raw nonce bytes.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Return lowercase hexadecimal.
    #[must_use]
    pub fn to_hex(self) -> String {
        crate::util::hex::encode(self.0)
    }
}

impl fmt::Display for FederationSetupCorrelationNonce {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl FromStr for FederationSetupCorrelationNonce {
    type Err = super::FederationError;

    fn from_str(value: &str) -> Result<Self> {
        let bytes = crate::util::hex::decode(value)?;
        let bytes = bytes
            .try_into()
            .map_err(|_| invalid("setup correlation nonce must contain exactly 32 bytes"))?;
        Self::from_bytes(bytes)
    }
}

impl Serialize for FederationSetupCorrelationNonce {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for FederationSetupCorrelationNonce {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(de::Error::custom)
    }
}

/// Stable setup response route discriminants from the frozen v1 transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationSetupResponseRouteKind {
    /// Enrollment acknowledgement.
    EnrollmentAck,
    /// Published proposal.
    Proposal,
    /// Approval acknowledgement.
    ApprovalAck,
    /// Certified manifest.
    Manifest,
    /// Setup progress status.
    Status,
}

impl FederationSetupResponseRouteKind {
    const fn discriminant(self) -> u8 {
        match self {
            Self::EnrollmentAck => 1,
            Self::Proposal => 2,
            Self::ApprovalAck => 3,
            Self::Manifest => 4,
            Self::Status => 5,
        }
    }
}

/// Coordinator-signed acknowledgement of one exact enrollment record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupEnrollmentAcknowledgement {
    /// Strict setup protocol version.
    pub version: u16,
    /// Setup attempt accepting the enrollment.
    pub setup_id: FederationSetupId,
    /// Invitation consumed by the enrollment.
    pub invitation_id: FederationSetupInvitationId,
    /// Enrolled member slot.
    pub member_id: MemberId,
    /// Hash of the exact complete signed enrollment.
    pub enrollment_hash: FederationSetupEnrollmentHash,
    /// Nonce of the request being acknowledged.
    pub correlation_nonce: FederationSetupCorrelationNonce,
    /// Coordinator setup-identity signature.
    pub coordinator_signature: FederationSetupSignature,
}

impl FederationSetupEnrollmentAcknowledgement {
    /// Sign an acknowledgement for one exact accepted enrollment.
    pub fn sign(
        enrollment: &FederationSetupEnrollment,
        correlation_nonce: FederationSetupCorrelationNonce,
        coordinator_secret_key: &SecretKey,
    ) -> Result<Self> {
        let enrollment_hash = enrollment_hash(enrollment)?;
        let signature_transcript = transcript(
            ENROLLMENT_ACK_DOMAIN,
            &[
                &enrollment.setup_id.to_bytes(),
                &enrollment.invitation_id.to_bytes(),
                &enrollment.member_id.get().to_be_bytes(),
                &enrollment_hash.to_bytes(),
                &correlation_nonce.to_bytes(),
                &FEDERATION_SETUP_PROTOCOL_VERSION.to_be_bytes(),
            ],
        );
        let coordinator_signature =
            FederationSetupSignature::sign(&signature_transcript, coordinator_secret_key)?;
        Ok(Self {
            version: FEDERATION_SETUP_PROTOCOL_VERSION,
            setup_id: enrollment.setup_id,
            invitation_id: enrollment.invitation_id,
            member_id: enrollment.member_id,
            enrollment_hash,
            correlation_nonce,
            coordinator_signature,
        })
    }

    /// Validate exact enrollment, request nonce, and pinned coordinator identity.
    pub fn validate(
        &self,
        enrollment: &FederationSetupEnrollment,
        correlation_nonce: FederationSetupCorrelationNonce,
        coordinator_identity: FederationSetupCoordinatorPublicKey,
    ) -> Result<()> {
        validate_setup_version("enrollment acknowledgement", self.version)?;
        if self.setup_id != enrollment.setup_id
            || self.invitation_id != enrollment.invitation_id
            || self.member_id != enrollment.member_id
            || self.enrollment_hash != enrollment_hash(enrollment)?
            || self.correlation_nonce != correlation_nonce
        {
            return Err(invalid(
                "setup enrollment acknowledgement does not match the request",
            ));
        }
        self.coordinator_signature.verify(
            &self.signature_transcript(),
            coordinator_identity.public_key(),
            "enrollment acknowledgement",
        )
    }

    fn signature_transcript(&self) -> Vec<u8> {
        transcript(
            ENROLLMENT_ACK_DOMAIN,
            &[
                &self.setup_id.to_bytes(),
                &self.invitation_id.to_bytes(),
                &self.member_id.get().to_be_bytes(),
                &self.enrollment_hash.to_bytes(),
                &self.correlation_nonce.to_bytes(),
                &self.version.to_be_bytes(),
            ],
        )
    }
}

/// Signature envelope authenticating one setup-only HTTP response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupResponseEnvelope {
    /// Strict setup protocol version.
    pub version: u16,
    /// Setup attempt producing the response.
    pub setup_id: FederationSetupId,
    /// Stable response route kind.
    pub route_kind: FederationSetupResponseRouteKind,
    /// Nonce supplied by the request.
    pub correlation_nonce: FederationSetupCorrelationNonce,
    /// Domain-separated hash of the canonical response body.
    pub body_hash: [u8; 32],
    /// Coordinator setup-identity signature.
    pub coordinator_signature: FederationSetupSignature,
}

impl FederationSetupResponseEnvelope {
    fn sign<T>(
        route_kind: FederationSetupResponseRouteKind,
        correlation_nonce: FederationSetupCorrelationNonce,
        body: &T,
        coordinator_secret_key: &SecretKey,
    ) -> Result<Self>
    where
        T: FederationSetupResponseBody,
    {
        let version = FEDERATION_SETUP_PROTOCOL_VERSION;
        let setup_id = body.setup_id();
        let body_hash = response_body_hash(&body.canonical_response_bytes()?)?;
        let signature_transcript = transcript(
            RESPONSE_DOMAIN,
            &[
                &setup_id.to_bytes(),
                &[route_kind.discriminant()],
                &correlation_nonce.to_bytes(),
                &body_hash,
                &version.to_be_bytes(),
            ],
        );
        let coordinator_signature =
            FederationSetupSignature::sign(&signature_transcript, coordinator_secret_key)?;
        Ok(Self {
            version,
            setup_id,
            route_kind,
            correlation_nonce,
            body_hash,
            coordinator_signature,
        })
    }

    fn validate<T>(
        &self,
        route_kind: FederationSetupResponseRouteKind,
        correlation_nonce: FederationSetupCorrelationNonce,
        body: &T,
        coordinator_identity: FederationSetupCoordinatorPublicKey,
    ) -> Result<()>
    where
        T: FederationSetupResponseBody,
    {
        validate_setup_version("setup response envelope", self.version)?;
        if self.setup_id != body.setup_id()
            || self.route_kind != route_kind
            || self.correlation_nonce != correlation_nonce
            || self.body_hash != response_body_hash(&body.canonical_response_bytes()?)?
        {
            return Err(invalid(
                "setup response envelope does not match its request or body",
            ));
        }
        self.coordinator_signature.verify(
            &self.signature_transcript(),
            coordinator_identity.public_key(),
            "setup response envelope",
        )
    }

    fn signature_transcript(&self) -> Vec<u8> {
        transcript(
            RESPONSE_DOMAIN,
            &[
                &self.setup_id.to_bytes(),
                &[self.route_kind.discriminant()],
                &self.correlation_nonce.to_bytes(),
                &self.body_hash,
                &self.version.to_be_bytes(),
            ],
        )
    }
}

/// Authenticated setup-only HTTP response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupSignedResponse<T> {
    /// Coordinator response signature and request binding.
    pub envelope: FederationSetupResponseEnvelope,
    /// Route-specific response body.
    pub body: T,
}

impl<T> FederationSetupSignedResponse<T>
where
    T: FederationSetupResponseBody,
{
    /// Sign one response body for an exact route and request nonce.
    pub fn sign(
        route_kind: FederationSetupResponseRouteKind,
        correlation_nonce: FederationSetupCorrelationNonce,
        body: T,
        coordinator_secret_key: &SecretKey,
    ) -> Result<Self> {
        let envelope = FederationSetupResponseEnvelope::sign(
            route_kind,
            correlation_nonce,
            &body,
            coordinator_secret_key,
        )?;
        Ok(Self { envelope, body })
    }

    /// Validate one response against the exact route, nonce, and pinned identity.
    pub fn validate(
        &self,
        route_kind: FederationSetupResponseRouteKind,
        correlation_nonce: FederationSetupCorrelationNonce,
        coordinator_identity: FederationSetupCoordinatorPublicKey,
    ) -> Result<()> {
        self.envelope.validate(
            route_kind,
            correlation_nonce,
            &self.body,
            coordinator_identity,
        )
    }
}

/// Bounded aggregate progress returned by the setup-only status route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupStatus {
    /// Strict setup protocol version.
    pub version: u16,
    /// Setup attempt being reported.
    pub setup_id: FederationSetupId,
    /// Coordinator lifecycle phase.
    pub coordinator_phase: FederationSetupCoordinatorPhase,
    /// Fixed federation member count.
    pub member_count: u16,
    /// Enrolled member slots in ascending order.
    pub enrolled_members: Vec<MemberId>,
    /// Members whose approvals are durably received, in ascending order.
    pub approved_members: Vec<MemberId>,
    /// Published proposal hash, when present.
    pub proposal_hash: Option<FederationSetupProposalHash>,
    /// Certified manifest hash, when present.
    pub manifest_hash: Option<FederationSetupManifestHash>,
}

impl FederationSetupStatus {
    /// Validate bounds, ordering, and lifecycle consistency.
    pub fn validate(&self) -> Result<()> {
        validate_setup_version("setup status", self.version)?;
        validate_nonzero_identifier("setup ID", self.setup_id.to_bytes())?;
        validate_member_list("enrolled", self.member_count, &self.enrolled_members)?;
        validate_member_list("approved", self.member_count, &self.approved_members)?;
        if self.approved_members.len() > self.enrolled_members.len() {
            return Err(invalid("setup status has more approvals than enrollments"));
        }
        match self.coordinator_phase {
            FederationSetupCoordinatorPhase::CollectingEnrollments => {
                if self.proposal_hash.is_some() || self.manifest_hash.is_some() {
                    return Err(invalid(
                        "collecting setup status contains a published artifact",
                    ));
                }
            }
            FederationSetupCoordinatorPhase::ProposalPublished => {
                if self.proposal_hash.is_none() || self.manifest_hash.is_some() {
                    return Err(invalid("published setup status has inconsistent artifacts"));
                }
            }
            FederationSetupCoordinatorPhase::Certified
            | FederationSetupCoordinatorPhase::Closed => {
                if self.proposal_hash.is_none() || self.manifest_hash.is_none() {
                    return Err(invalid("certified setup status is missing an artifact"));
                }
            }
            FederationSetupCoordinatorPhase::Aborted => {}
        }
        let canonical = self.canonical_response_bytes()?;
        if canonical.len() > FEDERATION_MAX_SETUP_STATUS_BYTES {
            return Err(invalid("setup status exceeds the response-size limit"));
        }
        Ok(())
    }
}

/// Canonical body contract for authenticated setup HTTP responses.
pub trait FederationSetupResponseBody {
    /// Return the setup attempt represented by this body.
    fn setup_id(&self) -> FederationSetupId;

    /// Return the canonical bytes covered by the response envelope.
    fn canonical_response_bytes(&self) -> Result<Vec<u8>>;
}

impl FederationSetupResponseBody for FederationSetupEnrollmentAcknowledgement {
    fn setup_id(&self) -> FederationSetupId {
        self.setup_id
    }

    fn canonical_response_bytes(&self) -> Result<Vec<u8>> {
        let mut output = self.signature_transcript();
        output.extend_from_slice(&self.coordinator_signature.to_bytes());
        Ok(output)
    }
}

impl FederationSetupResponseBody for FederationSetupProposal {
    fn setup_id(&self) -> FederationSetupId {
        self.body.setup_id
    }

    fn canonical_response_bytes(&self) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        append_bytes(&mut output, &self.body.canonical_bytes()?)?;
        output.extend_from_slice(&self.proposal_hash.to_bytes());
        output.extend_from_slice(&self.coordinator_signature.to_bytes());
        Ok(output)
    }
}

impl FederationSetupResponseBody for FederationSetupApproval {
    fn setup_id(&self) -> FederationSetupId {
        self.setup_id
    }

    fn canonical_response_bytes(&self) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        output.extend_from_slice(&self.version.to_be_bytes());
        output.extend_from_slice(&self.setup_id.to_bytes());
        output.extend_from_slice(&self.proposal_hash.to_bytes());
        output.extend_from_slice(&self.member_id.get().to_be_bytes());
        output.extend_from_slice(&self.signature.to_bytes());
        Ok(output)
    }
}

impl FederationSetupResponseBody for FederationSetupManifest {
    fn setup_id(&self) -> FederationSetupId {
        self.proposal.body.setup_id
    }

    fn canonical_response_bytes(&self) -> Result<Vec<u8>> {
        self.canonical_bytes()
    }
}

impl FederationSetupResponseBody for FederationSetupStatus {
    fn setup_id(&self) -> FederationSetupId {
        self.setup_id
    }

    fn canonical_response_bytes(&self) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        output.extend_from_slice(&self.version.to_be_bytes());
        output.extend_from_slice(&self.setup_id.to_bytes());
        output.push(coordinator_phase_discriminant(self.coordinator_phase));
        output.extend_from_slice(&self.member_count.to_be_bytes());
        append_member_list(&mut output, &self.enrolled_members)?;
        append_member_list(&mut output, &self.approved_members)?;
        append_optional_hash(
            &mut output,
            self.proposal_hash.map(|value| value.to_bytes()),
        );
        append_optional_hash(
            &mut output,
            self.manifest_hash.map(|value| value.to_bytes()),
        );
        Ok(output)
    }
}

fn enrollment_hash(
    enrollment: &FederationSetupEnrollment,
) -> Result<FederationSetupEnrollmentHash> {
    let enrollment = super::length_prefixed(&enrollment.canonical_bytes()?)?;
    Ok(FederationSetupEnrollmentHash::from_bytes(hash_transcript(
        ENROLLMENT_RECORD_DOMAIN,
        &[&enrollment],
    )))
}

fn response_body_hash(body: &[u8]) -> Result<[u8; 32]> {
    let body = super::length_prefixed(body)?;
    Ok(hash_transcript(RESPONSE_BODY_DOMAIN, &[&body]))
}

fn validate_member_list(
    label: &'static str,
    member_count: u16,
    members: &[MemberId],
) -> Result<()> {
    if members.len() > usize::from(member_count)
        || !members.windows(2).all(|pair| pair[0] < pair[1])
        || members.iter().any(|member| member.get() > member_count)
    {
        return Err(invalid(format!(
            "setup status {label} member list is invalid"
        )));
    }
    Ok(())
}

fn append_member_list(output: &mut Vec<u8>, members: &[MemberId]) -> Result<()> {
    append_count(output, members.len(), "status member")?;
    for member in members {
        output.extend_from_slice(&member.get().to_be_bytes());
    }
    Ok(())
}

fn append_optional_hash(output: &mut Vec<u8>, value: Option<[u8; 32]>) {
    match value {
        Some(value) => {
            output.push(1);
            output.extend_from_slice(&value);
        }
        None => output.push(0),
    }
}

const fn coordinator_phase_discriminant(phase: FederationSetupCoordinatorPhase) -> u8 {
    match phase {
        FederationSetupCoordinatorPhase::CollectingEnrollments => 1,
        FederationSetupCoordinatorPhase::ProposalPublished => 2,
        FederationSetupCoordinatorPhase::Certified => 3,
        FederationSetupCoordinatorPhase::Closed => 4,
        FederationSetupCoordinatorPhase::Aborted => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correlation_nonce_rejects_zero_and_round_trips_hex() {
        assert!(FederationSetupCorrelationNonce::from_bytes([0; 32]).is_err());
        let nonce = FederationSetupCorrelationNonce::from_bytes([7; 32]).expect("nonce");
        assert_eq!(
            nonce
                .to_string()
                .parse::<FederationSetupCorrelationNonce>()
                .expect("parsed nonce"),
            nonce
        );
    }

    #[test]
    fn status_response_envelope_binds_route_nonce_body_and_identity() {
        let secret = SecretKey::generate();
        let identity = FederationSetupCoordinatorPublicKey::new(secret.public_key())
            .expect("coordinator identity");
        let nonce = FederationSetupCorrelationNonce::from_bytes([9; 32]).expect("nonce");
        let status = FederationSetupStatus {
            version: FEDERATION_SETUP_PROTOCOL_VERSION,
            setup_id: FederationSetupId::from_bytes([1; 32]),
            coordinator_phase: FederationSetupCoordinatorPhase::CollectingEnrollments,
            member_count: 3,
            enrolled_members: vec![MemberId::new(1).expect("member")],
            approved_members: Vec::new(),
            proposal_hash: None,
            manifest_hash: None,
        };
        status.validate().expect("status");
        let response = FederationSetupSignedResponse::sign(
            FederationSetupResponseRouteKind::Status,
            nonce,
            status,
            &secret,
        )
        .expect("signed response");
        response
            .validate(FederationSetupResponseRouteKind::Status, nonce, identity)
            .expect("valid response");

        let mut tampered_body = response.clone();
        tampered_body
            .body
            .enrolled_members
            .push(MemberId::new(2).expect("member"));
        assert!(tampered_body
            .validate(FederationSetupResponseRouteKind::Status, nonce, identity)
            .is_err());

        assert!(response
            .validate(FederationSetupResponseRouteKind::Manifest, nonce, identity,)
            .is_err());
        let wrong_nonce = FederationSetupCorrelationNonce::from_bytes([8; 32]).expect("nonce");
        assert!(response
            .validate(
                FederationSetupResponseRouteKind::Status,
                wrong_nonce,
                identity,
            )
            .is_err());
        let wrong_identity =
            FederationSetupCoordinatorPublicKey::new(SecretKey::generate().public_key())
                .expect("coordinator identity");
        assert!(response
            .validate(
                FederationSetupResponseRouteKind::Status,
                nonce,
                wrong_identity,
            )
            .is_err());
    }
}
