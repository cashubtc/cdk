//! Durable member attestations and bounded quorum certificates for Bark evidence.

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::schnorr::Signature;
use serde::{Deserialize, Serialize};

use cdk_common::federation::{
    FederationConfig, FederationConfigDigest, FederationError, FederationId, FederationOperationId,
    FederationOperationSignature, MemberConfigPrivate, MemberId, Result,
};
use cdk_common::Amount;

use crate::{
    canonical_json_bytes, FederationBarkActionId, FederationBarkDigest,
    FederationBarkMemberObservation, FederationBarkObservationStatus,
};

/// Current signed Bark evidence-certificate format.
pub const FEDERATION_BARK_EVIDENCE_CERTIFICATE_VERSION: u16 = 1;
/// Maximum private request or response carrying Bark evidence attestations.
pub const FEDERATION_MAX_BARK_EVIDENCE_ATTESTATION_REQUEST_BYTES: usize = 16 * 1024;

const BARK_EVIDENCE_ATTESTATION_DOMAIN: &[u8] = b"cdk/federation/bark-evidence-attestation/v1";
const BARK_EVIDENCE_CERTIFICATE_ID_DOMAIN: &[u8] =
    b"cdk/federation/bark-evidence-certificate-id/v1";

fn invalid_certificate(reason: impl Into<String>) -> FederationError {
    FederationError::ApplicationOperationInvalid {
        reason: reason.into(),
    }
}

/// Bark evidence family whose complete canonical result one member verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationBarkEvidencePhase {
    /// Complete incoming HTLC/VTXO evidence before claim authorization.
    ReceiveHtlcsReady,
    /// Complete accepted incoming claim result before Cashu issuance.
    ReceiveClaimed,
    /// Complete successful outgoing payment result before consensus finalization.
    SendSucceeded,
}

/// Receiver-independent statement signed independently by Bark evidence verifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkEvidenceBinding {
    /// Evidence-certificate format.
    pub version: u16,
    /// Owning federation.
    pub federation_id: FederationId,
    /// Digest of the complete accepted public configuration.
    pub config_digest: FederationConfigDigest,
    /// Accepted Bark application-key epoch.
    pub application_key_epoch: u64,
    /// Stable Bark action lineage.
    pub action_id: FederationBarkActionId,
    /// Cashu quote bound to the action.
    pub quote: String,
    /// Exact evidence family.
    pub phase: FederationBarkEvidencePhase,
    /// Accepted predecessor or result operation that scopes this evidence.
    pub evidence_operation_id: FederationOperationId,
    /// Payment hash committed by the action.
    pub payment_hash: FederationBarkDigest,
    /// Commitment to the configured Ark source.
    pub source_scope: FederationBarkDigest,
    /// Conservative observed result.
    pub status: FederationBarkObservationStatus,
    /// Exact independently verified value.
    pub amount: Amount,
    /// Exact independently verified fee.
    pub fee: Amount,
    /// Digest of the complete canonical evidence, including its artifacts.
    pub evidence_digest: FederationBarkDigest,
    /// Exact evidence-producing attempt.
    pub attempt: u32,
}

impl FederationBarkEvidenceBinding {
    /// Construct the exact certificate statement represented by one local observation.
    ///
    /// `config` must be the public config validated when the federation runtime
    /// was created.
    pub fn from_observation(
        config: &FederationConfig,
        phase: FederationBarkEvidencePhase,
        evidence_operation_id: FederationOperationId,
        attempt: u32,
        observation: &FederationBarkMemberObservation,
    ) -> Result<Self> {
        let application = crate::federation_bark_application(config)?
            .ok_or_else(|| invalid_certificate("Bark evidence requires an accepted application"))?;
        let binding = Self {
            version: FEDERATION_BARK_EVIDENCE_CERTIFICATE_VERSION,
            federation_id: config.federation_id,
            // Federation configs are validated before they enter a mint runtime.
            // Revalidating here repeats threshold-key interpolation for every
            // attestation in the receive hot path.
            config_digest: config.config_digest_for_validated_config()?,
            application_key_epoch: application.application_key_epoch,
            action_id: observation.action_id,
            quote: observation
                .quote
                .clone()
                .ok_or_else(|| invalid_certificate("Bark evidence requires a quote"))?,
            phase,
            evidence_operation_id,
            payment_hash: observation
                .payment_hash
                .ok_or_else(|| invalid_certificate("Bark evidence requires a payment hash"))?,
            source_scope: observation.source_scope,
            status: observation.status,
            amount: observation.amount,
            fee: observation.fee,
            evidence_digest: observation.result_hash,
            attempt,
        };
        binding.validate(config)?;
        Ok(binding)
    }

    /// Validate the complete statement against the active accepted configuration.
    ///
    /// `config` must be the public config validated when the federation runtime
    /// was created.
    pub fn validate(&self, config: &FederationConfig) -> Result<()> {
        if self.version != FEDERATION_BARK_EVIDENCE_CERTIFICATE_VERSION
            || self.federation_id != config.federation_id
            || self.config_digest != config.config_digest_for_validated_config()?
            || self.quote.is_empty()
            || self.attempt == 0
            || self.amount == Amount::ZERO
        {
            return Err(invalid_certificate("invalid Bark evidence binding"));
        }
        let application = crate::federation_bark_application(config)?
            .ok_or_else(|| invalid_certificate("Bark evidence requires an accepted application"))?;
        if self.application_key_epoch != application.application_key_epoch {
            return Err(invalid_certificate(
                "Bark evidence references another application epoch",
            ));
        }
        let expected_status = match self.phase {
            FederationBarkEvidencePhase::ReceiveHtlcsReady => {
                FederationBarkObservationStatus::Accepted
            }
            FederationBarkEvidencePhase::ReceiveClaimed => {
                FederationBarkObservationStatus::Succeeded
            }
            FederationBarkEvidencePhase::SendSucceeded => {
                FederationBarkObservationStatus::Succeeded
            }
        };
        if self.status != expected_status {
            return Err(invalid_certificate(
                "Bark evidence status does not match its phase",
            ));
        }
        Ok(())
    }

    /// Derive the stable logical certificate identity independently of collector or signer subset.
    pub fn certificate_id(&self) -> Result<FederationBarkDigest> {
        let canonical = canonical_json_bytes(self)?;
        let mut transcript =
            Vec::with_capacity(BARK_EVIDENCE_CERTIFICATE_ID_DOMAIN.len() + canonical.len() + 8);
        append_bytes(&mut transcript, BARK_EVIDENCE_CERTIFICATE_ID_DOMAIN);
        append_bytes(&mut transcript, &canonical);
        FederationBarkDigest::from_bytes(Sha256Hash::hash(&transcript).to_byte_array())
    }

    /// Reconstruct the bounded observation statement for one verifier.
    #[must_use]
    pub fn observation_for(&self, observer: MemberId) -> FederationBarkMemberObservation {
        FederationBarkMemberObservation {
            version: crate::FEDERATION_BARK_OPERATION_VERSION,
            action_id: self.action_id,
            quote: Some(self.quote.clone()),
            observer,
            observation_epoch: u64::from(self.attempt),
            source_scope: self.source_scope,
            payment_hash: Some(self.payment_hash),
            status: self.status,
            amount: self.amount,
            fee: self.fee,
            result_hash: self.evidence_digest,
        }
    }
}

/// One independently produced member signature over an exact Bark evidence binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkEvidenceAttestation {
    /// Member that independently obtained and verified the evidence.
    pub signer: MemberId,
    /// Member identity signature over the binding and signer identity.
    pub signature: FederationOperationSignature,
}

impl FederationBarkEvidenceAttestation {
    /// Sign an independently verified binding with the member identity key.
    pub fn new_signed(
        binding: &FederationBarkEvidenceBinding,
        config: &FederationConfig,
        private_config: &MemberConfigPrivate,
    ) -> Result<Self> {
        binding.validate(config)?;
        private_config.validate_against_public(config)?;
        let signer = private_config.member_id;
        let signature = private_config
            .identity_secret_key
            .sign_no_aux_rand(&attestation_transcript(binding, signer)?)?;
        Ok(Self {
            signer,
            signature: FederationOperationSignature::from_slice(&signature.serialize())?,
        })
    }

    /// Verify this durable attestation against one exact binding and roster.
    pub fn verify(
        &self,
        binding: &FederationBarkEvidenceBinding,
        config: &FederationConfig,
    ) -> Result<()> {
        binding.validate(config)?;
        let member = config
            .member(self.signer)
            .ok_or(FederationError::UnknownMember {
                member_id: self.signer,
            })?;
        let signature = Signature::from_slice(&self.signature.to_bytes())
            .map_err(|_| invalid_certificate("invalid Bark evidence signature"))?;
        member
            .identity_public_key
            .verify(&attestation_transcript(binding, self.signer)?, &signature)
            .map_err(|_| invalid_certificate("Bark evidence signature verification failed"))
    }
}

/// One consensus-portable quorum certificate over independently verified Bark evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkEvidenceCertificate {
    /// Exact statement shared by every attester.
    pub binding: FederationBarkEvidenceBinding,
    /// Strictly sorted unique member attestations.
    pub attestations: Vec<FederationBarkEvidenceAttestation>,
}

/// Authenticated private delivery of one independently verified attestation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkEvidenceAttestationRequest {
    /// Private exchange format.
    pub version: u16,
    /// Exact independently verified statement.
    pub binding: FederationBarkEvidenceBinding,
    /// Member's durable signature over the statement.
    pub attestation: FederationBarkEvidenceAttestation,
}

impl FederationBarkEvidenceAttestationRequest {
    /// Construct a bounded delivery request.
    #[must_use]
    pub const fn new(
        binding: FederationBarkEvidenceBinding,
        attestation: FederationBarkEvidenceAttestation,
    ) -> Self {
        Self {
            version: FEDERATION_BARK_EVIDENCE_CERTIFICATE_VERSION,
            binding,
            attestation,
        }
    }

    /// Verify the authenticated sender and exact attestation.
    pub fn validate(&self, config: &FederationConfig, sender: MemberId) -> Result<()> {
        if self.version != FEDERATION_BARK_EVIDENCE_CERTIFICATE_VERSION
            || self.attestation.signer != sender
        {
            return Err(invalid_certificate(
                "Bark evidence attestation sender is invalid",
            ));
        }
        self.attestation.verify(&self.binding, config)
    }

    /// Verify an attestation relayed by another authenticated federation member.
    pub fn validate_relayed(&self, config: &FederationConfig, relay: MemberId) -> Result<()> {
        if self.version != FEDERATION_BARK_EVIDENCE_CERTIFICATE_VERSION
            || !config.members.contains_key(&relay)
        {
            return Err(invalid_certificate(
                "Bark evidence attestation relay is invalid",
            ));
        }
        self.attestation.verify(&self.binding, config)
    }
}

/// Bounded private acknowledgement that one exact attestation is durable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkEvidenceAttestationResponse {
    /// Private exchange format.
    pub version: u16,
    /// Stable identity of the exact statement stored by the collector.
    pub certificate_id: FederationBarkDigest,
    /// Signer whose attestation the collector durably stored.
    pub attestation_signer: MemberId,
}

impl FederationBarkEvidenceAttestationResponse {
    /// Construct a deterministic bounded acknowledgement for one delivery.
    pub fn new(
        binding: &FederationBarkEvidenceBinding,
        attestation_signer: MemberId,
    ) -> Result<Self> {
        Ok(Self {
            version: FEDERATION_BARK_EVIDENCE_CERTIFICATE_VERSION,
            certificate_id: binding.certificate_id()?,
            attestation_signer,
        })
    }

    /// Verify an acknowledgement against the exact requested statement.
    pub fn validate(
        &self,
        config: &FederationConfig,
        expected: &FederationBarkEvidenceBinding,
        expected_signer: MemberId,
    ) -> Result<()> {
        if self.version != FEDERATION_BARK_EVIDENCE_CERTIFICATE_VERSION
            || self.certificate_id != expected.certificate_id()?
            || self.attestation_signer != expected_signer
        {
            return Err(invalid_certificate("invalid Bark evidence response"));
        }
        expected.validate(config)
    }
}

impl FederationBarkEvidenceCertificate {
    /// Assemble the smallest deterministic certificate from available valid attestations.
    pub fn new(
        binding: FederationBarkEvidenceBinding,
        mut attestations: Vec<FederationBarkEvidenceAttestation>,
        config: &FederationConfig,
        required: u16,
    ) -> Result<Self> {
        for attestation in &attestations {
            attestation.verify(&binding, config)?;
        }
        attestations.sort_by_key(|attestation| attestation.signer);
        if attestations
            .windows(2)
            .any(|pair| pair[0].signer == pair[1].signer)
        {
            return Err(invalid_certificate(
                "Bark evidence certificate contains a duplicate signer",
            ));
        }
        if required == 0
            || usize::from(required) > config.members.len()
            || attestations.len() < usize::from(required)
        {
            return Err(invalid_certificate(
                "Bark evidence certificate is below threshold",
            ));
        }
        attestations.truncate(usize::from(required));
        let certificate = Self {
            binding,
            attestations,
        };
        certificate.verify(config, required)?;
        Ok(certificate)
    }

    /// Verify canonical ordering, roster eligibility, signatures, and exact threshold.
    pub fn verify(&self, config: &FederationConfig, required: u16) -> Result<()> {
        self.binding.validate(config)?;
        if required == 0
            || usize::from(required) > config.members.len()
            || self.attestations.len() != usize::from(required)
            || self
                .attestations
                .windows(2)
                .any(|pair| pair[0].signer >= pair[1].signer)
        {
            return Err(invalid_certificate(
                "Bark evidence certificate has an invalid signer set",
            ));
        }
        for attestation in &self.attestations {
            attestation.verify(&self.binding, config)?;
        }
        Ok(())
    }

    /// Return the stable logical certificate identity.
    pub fn certificate_id(&self) -> Result<FederationBarkDigest> {
        self.binding.certificate_id()
    }

    /// Return the strictly sorted independent verifier identities.
    pub fn signers(&self) -> impl Iterator<Item = MemberId> + '_ {
        self.attestations
            .iter()
            .map(|attestation| attestation.signer)
    }

    /// Reconstruct the bounded observation statement for an attesting member.
    pub fn observation_for(&self, observer: MemberId) -> FederationBarkMemberObservation {
        self.binding.observation_for(observer)
    }
}

fn attestation_transcript(
    binding: &FederationBarkEvidenceBinding,
    signer: MemberId,
) -> Result<Vec<u8>> {
    let canonical = canonical_json_bytes(binding)?;
    let mut transcript =
        Vec::with_capacity(BARK_EVIDENCE_ATTESTATION_DOMAIN.len() + canonical.len() + 16);
    append_bytes(&mut transcript, BARK_EVIDENCE_ATTESTATION_DOMAIN);
    append_bytes(&mut transcript, &signer.get().to_be_bytes());
    append_bytes(&mut transcript, &canonical);
    Ok(transcript)
}

fn append_bytes(transcript: &mut Vec<u8>, value: &[u8]) {
    transcript.extend_from_slice(&(value.len() as u64).to_be_bytes());
    transcript.extend_from_slice(value);
}
