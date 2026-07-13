//! Pure, restartable planning for a dealer-free federation DKG ceremony.

use core::fmt;
use std::collections::BTreeMap;
use std::sync::Arc;

use super::{
    FederationConfig, FederationConfigDigest, FederationDkgActivationRequest,
    FederationDkgCeremonyId, FederationDkgCommitmentRequest, FederationDkgFinalizedMemberConfig,
    FederationDkgKeysetSpec, FederationDkgMemberKeysetShare, FederationDkgParticipantCommitment,
    FederationDkgParticipantCommitmentReveal, FederationDkgParticipantState,
    FederationDkgRevealRequest, FederationDkgSecretShareRequest, FederationDkgSetupState,
    FederationDkgTranscriptSignature, FederationDkgTranscriptSignatureRequest, FederationError,
    MemberConfigPrivate, MemberId, Result,
};

/// Owned participant state carrying a non-serializable validation proof.
///
/// Reloaded state earns the proof through full validation. Fresh state may earn
/// it directly from construction with one coefficient source for commitments
/// and shares. An in-process driver may reuse either proof across immutable
/// ticks, but no proof survives serialization.
#[derive(Debug, Clone)]
pub struct ValidatedFederationDkgParticipantState {
    participant_state: Arc<FederationDkgParticipantState>,
    config_digest: FederationConfigDigest,
}

impl ValidatedFederationDkgParticipantState {
    /// Fully validate participant state and bind the proof to a federation.
    pub fn new(
        public_config: &FederationConfig,
        participant_state: FederationDkgParticipantState,
    ) -> Result<Self> {
        participant_state.validate(public_config)?;
        Self::from_validated_state(public_config, participant_state)
    }

    /// Generate fresh participant state whose invariants hold by construction.
    pub fn generate(
        public_config: &FederationConfig,
        ceremony_id: FederationDkgCeremonyId,
        member_id: MemberId,
        keyset_spec: FederationDkgKeysetSpec,
    ) -> Result<Self> {
        let participant_state = FederationDkgParticipantState::generate_by_construction(
            public_config,
            ceremony_id,
            member_id,
            keyset_spec,
        )?;
        Self::from_validated_state(public_config, participant_state)
    }

    fn from_validated_state(
        public_config: &FederationConfig,
        participant_state: FederationDkgParticipantState,
    ) -> Result<Self> {
        Ok(Self {
            participant_state: Arc::new(participant_state),
            config_digest: public_config.config_digest_for_validated_config()?,
        })
    }

    /// Return the validated immutable participant state.
    #[must_use]
    pub fn participant_state(&self) -> &FederationDkgParticipantState {
        self.participant_state.as_ref()
    }

    fn validate_binding(&self, public_config: &FederationConfig) -> Result<()> {
        let config_digest = public_config.config_digest_for_validated_config()?;
        if self.config_digest != config_digest {
            return Err(FederationError::FederationConfigDigestMismatch {
                member_id: self.participant_state.member_id(),
                expected: self.config_digest,
                found: config_digest,
            });
        }
        Ok(())
    }
}

/// Durable records used to plan one local member's next DKG work.
///
/// This view contains private contributions and local keyset shares. Its custom
/// `Debug` implementation reports counts only.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct FederationDkgDriverRecords {
    /// Public pre-reveal commitments received by this member.
    pub commitments: Vec<FederationDkgParticipantCommitment>,
    /// Public commitment reveals received by this member.
    pub reveals: Vec<FederationDkgParticipantCommitmentReveal>,
    /// Private secret-share contributions addressed to this member.
    pub secret_share_contributions: Vec<FederationDkgSecretShareRequest>,
    /// Completed local keyset shares from prior planning/application steps.
    pub keyset_shares: Vec<FederationDkgMemberKeysetShare>,
    /// Identity signatures received for candidate final transcripts.
    pub transcript_signatures: Vec<FederationDkgTranscriptSignatureRequest>,
    /// Signed all-member confirmations for candidate finalized configs.
    pub activations: Vec<FederationDkgActivationRequest>,
}

impl fmt::Debug for FederationDkgDriverRecords {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FederationDkgDriverRecords")
            .field("commitment_count", &self.commitments.len())
            .field("reveal_count", &self.reveals.len())
            .field(
                "secret_share_contribution_count",
                &self.secret_share_contributions.len(),
            )
            .field("keyset_share_count", &self.keyset_shares.len())
            .field(
                "transcript_signature_count",
                &self.transcript_signatures.len(),
            )
            .field("activation_count", &self.activations.len())
            .finish()
    }
}

/// Current phase selected from one validated durable DKG view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FederationDkgDriverPhase {
    /// Rebroadcast the local pre-reveal commitment while commitments are missing.
    Commitment,
    /// Reveal the local public polynomial only after every commitment is present.
    Reveal,
    /// Deliver per-recipient private contributions after every reveal is present.
    SecretShares,
    /// Persist the locally derived aggregate keyset share.
    LocalKeysetShare,
    /// Broadcast the local identity signature over the complete public ceremony.
    TranscriptSignature,
    /// Confirm complete final config assembly and collect every member confirmation.
    Activation,
    /// FROST root DKG is still collecting round-one broadcasts.
    FrostRound1,
    /// FROST root DKG is still collecting confidential round-two packages.
    FrostRound2,
    /// FROST roots exist but all members have not confirmed one identical public package.
    FrostConfirmation,
    /// Every member confirmed the same finalized config.
    Complete,
}

/// Redacted progress counters for one planned DKG step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FederationDkgDriverProgress {
    /// Configured federation member count.
    pub member_count: usize,
    /// Valid commitments present locally.
    pub commitment_count: usize,
    /// Valid reveals present locally.
    pub reveal_count: usize,
    /// Valid incoming private contributions present locally.
    pub secret_share_contribution_count: usize,
    /// Whether the matching local keyset-share record is durable.
    pub local_keyset_share_ready: bool,
    /// Valid signatures for the expected transcript present locally.
    pub transcript_signature_count: usize,
    /// Valid all-member activation confirmations present locally.
    pub activation_count: usize,
}

/// Idempotent next work selected for one local DKG participant.
///
/// Later phases include all earlier outbound artifacts so a lagging peer can
/// recover without a separate acknowledgement protocol. The custom `Debug`
/// implementation never renders private contributions or local secret shares.
#[derive(Clone, PartialEq, Eq)]
pub struct FederationDkgDriverPlan {
    /// Highest phase justified by the durable local view.
    pub phase: FederationDkgDriverPhase,
    /// Redacted progress counters.
    pub progress: FederationDkgDriverProgress,
    /// Local commitment to deliver to every member, including self.
    pub commitment: Option<FederationDkgCommitmentRequest>,
    /// Local reveal to deliver once commitments are complete.
    pub reveal: Option<FederationDkgRevealRequest>,
    /// Private local contribution addressed to each federation member.
    pub secret_shares: BTreeMap<MemberId, FederationDkgSecretShareRequest>,
    /// Newly derived local keyset share that must be persisted atomically.
    pub derived_keyset_share: Option<FederationDkgMemberKeysetShare>,
    /// Local transcript signature to deliver to every member.
    pub transcript_signature: Option<FederationDkgTranscriptSignatureRequest>,
    /// Local finalized-config activation confirmation to deliver to every member.
    pub activation: Option<FederationDkgActivationRequest>,
    /// Fully validated final member config available from the activation phase onward.
    ///
    /// This process-local value is never serialized and lets a driver validate
    /// later activation-only record changes without rebuilding immutable DKG state.
    pub finalized_member_config: Option<Arc<FederationDkgFinalizedMemberConfig>>,
    /// Setup-config digest bound to [`Self::finalized_member_config`].
    pub finalized_setup_config_digest: Option<FederationConfigDigest>,
}

impl fmt::Debug for FederationDkgDriverPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let secret_share_receivers = self.secret_shares.keys().copied().collect::<Vec<_>>();
        let derived_keyset_id = self
            .derived_keyset_share
            .as_ref()
            .map(|share| share.keyset.id);
        let transcript_hash = self
            .transcript_signature
            .as_ref()
            .map(|signature| signature.transcript_hash);
        let activation_digest = self
            .activation
            .as_ref()
            .map(|activation| activation.final_config_digest);
        f.debug_struct("FederationDkgDriverPlan")
            .field("phase", &self.phase)
            .field("progress", &self.progress)
            .field("has_commitment", &self.commitment.is_some())
            .field("has_reveal", &self.reveal.is_some())
            .field("secret_share_receivers", &secret_share_receivers)
            .field("derived_keyset_id", &derived_keyset_id)
            .field("transcript_hash", &transcript_hash)
            .field("activation_digest", &activation_digest)
            .field(
                "has_finalized_member_config",
                &self.finalized_member_config.is_some(),
            )
            .finish()
    }
}

impl FederationDkgDriverRecords {
    /// Validate durable records and select the next idempotent local DKG work.
    pub fn plan(
        &self,
        public_config: &FederationConfig,
        private_config: &MemberConfigPrivate,
        participant_state: &FederationDkgParticipantState,
    ) -> Result<FederationDkgDriverPlan> {
        let participant_state =
            ValidatedFederationDkgParticipantState::new(public_config, participant_state.clone())?;
        self.plan_with_validated_participant(public_config, private_config, &participant_state)
    }

    /// Select the next work using participant state validated at the load boundary.
    pub fn plan_with_validated_participant(
        &self,
        public_config: &FederationConfig,
        private_config: &MemberConfigPrivate,
        participant_state: &ValidatedFederationDkgParticipantState,
    ) -> Result<FederationDkgDriverPlan> {
        public_config.validate()?;
        private_config.validate_against_public(public_config)?;
        participant_state.validate_binding(public_config)?;
        let participant_state = participant_state.participant_state();
        let local_member = private_config.member_id;
        if participant_state.member_id() != local_member {
            return Err(FederationError::MemberIdMismatch {
                expected: local_member,
                found: participant_state.member_id(),
            });
        }
        if !public_config.keysets.is_empty() || public_config.setup_transcript.is_some() {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: "live DKG setup requires a public config without finalized keysets or transcript"
                    .to_string(),
            });
        }

        let mut setup_state = FederationDkgSetupState::new(public_config);
        for commitment in &self.commitments {
            setup_state.insert_commitment(public_config, *commitment)?;
        }
        for reveal in &self.reveals {
            setup_state.insert_reveal(public_config, reveal.clone())?;
        }

        let mut incoming_contributions = BTreeMap::new();
        for contribution in &self.secret_share_contributions {
            contribution.validate_sender_receiver(
                public_config,
                contribution.sender,
                local_member,
            )?;
            if contribution.ceremony_id != participant_state.ceremony_id() {
                return Err(FederationError::DkgCommitmentMismatch {
                    member_id: contribution.sender,
                    reason: "secret contribution belongs to another ceremony".to_string(),
                });
            }
            match incoming_contributions.insert(contribution.sender, contribution.clone()) {
                Some(existing) if existing != *contribution => {
                    return Err(FederationError::DkgCommitmentMismatch {
                        member_id: contribution.sender,
                        reason: format!(
                            "conflicting secret-share contribution for receiver {local_member}"
                        ),
                    });
                }
                Some(_) | None => {}
            }
        }

        let member_count = public_config.members.len();
        let commitment_count = setup_state.commitments.len();
        let reveal_count = setup_state.reveals.len();
        let secret_share_contribution_count = incoming_contributions.len();
        let commitment = participant_state.commitment_request()?;
        let base_progress = FederationDkgDriverProgress {
            member_count,
            commitment_count,
            reveal_count,
            secret_share_contribution_count,
            local_keyset_share_ready: false,
            transcript_signature_count: 0,
            activation_count: 0,
        };

        if !setup_state.commitments_complete(public_config) {
            return Ok(Self::plan_for_phase(
                FederationDkgDriverPhase::Commitment,
                base_progress,
                Some(commitment),
                None,
                BTreeMap::new(),
                None,
                None,
                None,
                None,
                None,
            ));
        }

        let reveal = participant_state.reveal_request();
        if !setup_state.reveals_complete(public_config) {
            return Ok(Self::plan_for_phase(
                FederationDkgDriverPhase::Reveal,
                base_progress,
                Some(commitment),
                Some(reveal),
                BTreeMap::new(),
                None,
                None,
                None,
                None,
                None,
            ));
        }

        let secret_shares = public_config
            .members
            .keys()
            .copied()
            .map(|receiver| Ok((receiver, participant_state.secret_share_request(receiver)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        if incoming_contributions.len() != member_count {
            return Ok(Self::plan_for_phase(
                FederationDkgDriverPhase::SecretShares,
                base_progress,
                Some(commitment),
                Some(reveal),
                secret_shares,
                None,
                None,
                None,
                None,
                None,
            ));
        }

        let round = setup_state.complete_round(public_config)?.ok_or(
            FederationError::DkgCommitmentRoundMismatch {
                reason: "complete DKG records did not produce a commitment round".to_string(),
            },
        )?;
        let matching_keyset_shares = self
            .keyset_shares
            .iter()
            .filter(|share| share.public_ceremony.ceremony_id() == participant_state.ceremony_id())
            .collect::<Vec<_>>();
        if matching_keyset_shares.len() > 1 {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: format!(
                    "multiple local keyset shares for ceremony {}",
                    participant_state.ceremony_id()
                ),
            });
        }
        let Some(stored_keyset_share) = matching_keyset_shares.first().copied() else {
            let derived_keyset_share = round.derive_member_keyset_share(
                public_config,
                &setup_state.reveals,
                local_member,
                &incoming_contributions,
            )?;
            return Ok(Self::plan_for_phase(
                FederationDkgDriverPhase::LocalKeysetShare,
                base_progress,
                Some(commitment),
                Some(reveal),
                secret_shares,
                Some(derived_keyset_share),
                None,
                None,
                None,
                None,
            ));
        };
        if stored_keyset_share.federation_id != public_config.federation_id
            || stored_keyset_share.member_id != local_member
            || stored_keyset_share.public_ceremony.commitment_round != round
            || stored_keyset_share.public_ceremony.reveals != setup_state.reveals
        {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: format!(
                    "stored local keyset share does not match durable ceremony {}",
                    participant_state.ceremony_id()
                ),
            });
        }

        let mut final_public_config = public_config.clone();
        final_public_config.keysets = vec![stored_keyset_share.keyset.clone()];
        final_public_config.setup_transcript = None;
        let transcript_hash =
            FederationDkgMemberKeysetShare::transcript_hash_for_locally_derived_records(
                public_config,
                std::slice::from_ref(stored_keyset_share),
            )?;
        let mut matching_signatures = BTreeMap::new();
        for signature in self
            .transcript_signatures
            .iter()
            .filter(|signature| signature.transcript_hash == transcript_hash)
        {
            signature.validate_sender(&final_public_config, signature.signer)?;
            match matching_signatures.insert(signature.signer, *signature) {
                Some(existing) if existing != *signature => {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!(
                            "conflicting transcript signature for member {}",
                            signature.signer
                        ),
                    });
                }
                Some(_) | None => {}
            }
        }
        let progress = FederationDkgDriverProgress {
            local_keyset_share_ready: true,
            transcript_signature_count: matching_signatures.len(),
            ..base_progress
        };

        let transcript_signature = matching_signatures.get(&local_member).copied().unwrap_or(
            FederationDkgTranscriptSignatureRequest::new(
                final_public_config.federation_id,
                local_member,
                transcript_hash,
                FederationDkgTranscriptSignature::sign_no_aux_rand(
                    transcript_hash,
                    private_config,
                )?,
            ),
        );

        if matching_signatures.len() != member_count {
            return Ok(Self::plan_for_phase(
                FederationDkgDriverPhase::TranscriptSignature,
                progress,
                Some(commitment),
                Some(reveal),
                secret_shares,
                None,
                Some(transcript_signature),
                None,
                None,
                None,
            ));
        }

        let signature_records = matching_signatures.values().copied().collect::<Vec<_>>();
        let finalized = Arc::new(FederationDkgFinalizedMemberConfig::from_records(
            public_config,
            private_config,
            std::slice::from_ref(stored_keyset_share),
            &signature_records,
        )?);
        let activation = FederationDkgActivationRequest::sign_for_validated_config(
            participant_state.ceremony_id(),
            &finalized.public_config,
            private_config,
        )?;
        let mut matching_activations = BTreeMap::new();
        for candidate in self
            .activations
            .iter()
            .filter(|candidate| candidate.ceremony_id == participant_state.ceremony_id())
        {
            candidate.validate_sender_against_validated_configs(
                public_config,
                &finalized.public_config,
                candidate.signer,
            )?;
            match matching_activations.insert(candidate.signer, *candidate) {
                Some(existing) if existing != *candidate => {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!(
                            "conflicting DKG activation for member {} ceremony {}",
                            candidate.signer, candidate.ceremony_id
                        ),
                    });
                }
                Some(_) | None => {}
            }
        }
        let progress = FederationDkgDriverProgress {
            activation_count: matching_activations.len(),
            ..progress
        };
        let phase = if matching_activations.len() == member_count {
            FederationDkgDriverPhase::Complete
        } else {
            FederationDkgDriverPhase::Activation
        };
        Ok(Self::plan_for_phase(
            phase,
            progress,
            Some(commitment),
            Some(reveal),
            secret_shares,
            None,
            Some(transcript_signature),
            Some(activation),
            Some(finalized),
            Some(public_config.config_digest_for_validated_config()?),
        ))
    }

    /// Replan when only durable activation records changed.
    ///
    /// Returns `None` unless every commitment, reveal, private contribution,
    /// local keyset share, and transcript signature exactly matches the records
    /// that produced `previous_plan`. A matching update reuses that plan's
    /// validated final config but still verifies every activation signature,
    /// config digest, ceremony, and duplicate signer before advancing progress.
    pub fn replan_activation_only_change(
        &self,
        setup_public_config: &FederationConfig,
        previous_records: &Self,
        previous_plan: &FederationDkgDriverPlan,
    ) -> Result<Option<FederationDkgDriverPlan>> {
        if self.commitments != previous_records.commitments
            || self.reveals != previous_records.reveals
            || self.secret_share_contributions != previous_records.secret_share_contributions
            || self.keyset_shares != previous_records.keyset_shares
            || self.transcript_signatures != previous_records.transcript_signatures
        {
            return Ok(None);
        }
        let (Some(finalized), Some(expected_setup_digest), Some(local_activation)) = (
            previous_plan.finalized_member_config.as_ref(),
            previous_plan.finalized_setup_config_digest,
            previous_plan.activation,
        ) else {
            return Ok(None);
        };
        let found_setup_digest = setup_public_config.config_digest_for_validated_config()?;
        if found_setup_digest != expected_setup_digest {
            return Err(FederationError::FederationConfigDigestMismatch {
                member_id: finalized.private_config.member_id,
                expected: expected_setup_digest,
                found: found_setup_digest,
            });
        }

        let mut matching_activations = BTreeMap::new();
        for candidate in self
            .activations
            .iter()
            .filter(|candidate| candidate.ceremony_id == local_activation.ceremony_id)
        {
            candidate.validate_sender_against_validated_configs(
                setup_public_config,
                &finalized.public_config,
                candidate.signer,
            )?;
            match matching_activations.insert(candidate.signer, *candidate) {
                Some(existing) if existing != *candidate => {
                    return Err(FederationError::DkgTranscriptMismatch {
                        reason: format!(
                            "conflicting DKG activation for member {} ceremony {}",
                            candidate.signer, candidate.ceremony_id
                        ),
                    });
                }
                Some(_) | None => {}
            }
        }

        let mut plan = previous_plan.clone();
        plan.progress.activation_count = matching_activations.len();
        plan.phase = if matching_activations.len() == setup_public_config.members.len() {
            FederationDkgDriverPhase::Complete
        } else {
            FederationDkgDriverPhase::Activation
        };
        Ok(Some(plan))
    }

    fn plan_for_phase(
        phase: FederationDkgDriverPhase,
        progress: FederationDkgDriverProgress,
        commitment: Option<FederationDkgCommitmentRequest>,
        reveal: Option<FederationDkgRevealRequest>,
        secret_shares: BTreeMap<MemberId, FederationDkgSecretShareRequest>,
        derived_keyset_share: Option<FederationDkgMemberKeysetShare>,
        transcript_signature: Option<FederationDkgTranscriptSignatureRequest>,
        activation: Option<FederationDkgActivationRequest>,
        finalized_member_config: Option<Arc<FederationDkgFinalizedMemberConfig>>,
        finalized_setup_config_digest: Option<FederationConfigDigest>,
    ) -> FederationDkgDriverPlan {
        FederationDkgDriverPlan {
            phase,
            progress,
            commitment,
            reveal,
            secret_shares,
            derived_keyset_share,
            transcript_signature,
            activation,
            finalized_member_config,
            finalized_setup_config_digest,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::federation::{
        FederationDkgCeremonyId, FederationDkgKeysetSpec, FederationDkgSetupMethod,
        FederationDkgTranscript, FederationPaymentVerificationPolicy, MemberConfigPublic,
        ThresholdParams,
    };
    use crate::mint_url::MintUrl;
    use crate::{Amount, CurrencyUnit, SecretKey};

    fn member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("member id")
    }

    fn identity_secret(member: MemberId) -> SecretKey {
        let mut bytes = [0u8; 32];
        bytes[31] = member.get() as u8;
        SecretKey::from_slice(&bytes).expect("identity secret")
    }

    fn setup() -> (
        FederationConfig,
        BTreeMap<MemberId, MemberConfigPrivate>,
        BTreeMap<MemberId, FederationDkgParticipantState>,
    ) {
        let members = (1..=3)
            .map(member_id)
            .map(|member| {
                (
                    member,
                    MemberConfigPublic {
                        member_id: member,
                        public_mint_url: MintUrl::from_str(&format!(
                            "https://mint{}.example.com",
                            member.get()
                        ))
                        .expect("public mint URL"),
                        federation_api_url: MintUrl::from_str(&format!(
                            "https://fed{}.example.com",
                            member.get()
                        ))
                        .expect("federation API URL"),
                        identity_public_key: identity_secret(member).public_key(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let public_config = FederationConfig::new(
            ThresholdParams::with_bft_consensus(3, 2).expect("threshold params"),
            members,
            Vec::new(),
        )
        .expect("setup public config");
        let private_configs = public_config
            .members
            .keys()
            .copied()
            .map(|member| {
                (
                    member,
                    MemberConfigPrivate {
                        federation_id: Some(public_config.federation_id),
                        member_id: member,
                        identity_secret_key: identity_secret(member),
                        keyset_secret_shares: BTreeMap::new(),
                        setup_transcript_hash: None,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let keyset_spec = FederationDkgKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            [Amount::from(1), Amount::from(2)].into_iter().collect(),
        )
        .expect("keyset spec");
        let ceremony_id = FederationDkgCeremonyId::from_bytes([91; 32]);
        let participants = public_config
            .members
            .keys()
            .copied()
            .map(|member| {
                (
                    member,
                    FederationDkgParticipantState::generate(
                        &public_config,
                        ceremony_id,
                        member,
                        keyset_spec.clone(),
                    )
                    .expect("participant state"),
                )
            })
            .collect();
        (public_config, private_configs, participants)
    }

    fn all_commitments(
        participants: &BTreeMap<MemberId, FederationDkgParticipantState>,
    ) -> Vec<FederationDkgParticipantCommitment> {
        participants
            .values()
            .map(|state| state.commitment_request().expect("commitment").commitment)
            .collect()
    }

    fn all_reveals(
        participants: &BTreeMap<MemberId, FederationDkgParticipantState>,
    ) -> Vec<FederationDkgParticipantCommitmentReveal> {
        participants
            .values()
            .map(|state| state.reveal_request().reveal)
            .collect()
    }

    fn incoming_contributions(
        participants: &BTreeMap<MemberId, FederationDkgParticipantState>,
        receiver: MemberId,
    ) -> Vec<FederationDkgSecretShareRequest> {
        participants
            .values()
            .map(|state| state.secret_share_request(receiver).expect("secret share"))
            .collect()
    }

    #[test]
    fn validated_participant_plan_matches_full_validation_and_binds_federation() {
        let (public_config, private_configs, participants) = setup();
        let local_member = member_id(2);
        let private_config = private_configs.get(&local_member).expect("private config");
        let participant_state = participants.get(&local_member).expect("participant state");
        let records = FederationDkgDriverRecords::default();
        let expected = records
            .plan(&public_config, private_config, participant_state)
            .expect("fully validated plan");
        let validated =
            ValidatedFederationDkgParticipantState::new(&public_config, participant_state.clone())
                .expect("validated participant");
        let actual = records
            .plan_with_validated_participant(&public_config, private_config, &validated)
            .expect("plan using trust-boundary validation");
        assert_eq!(actual, expected);

        let mut different_config = public_config.clone();
        different_config.payment_observation_policy.mint_quotes =
            FederationPaymentVerificationPolicy::development_single_observation_for_testing();
        assert!(matches!(
            validated.validate_binding(&different_config),
            Err(FederationError::FederationConfigDigestMismatch {
                member_id,
                expected: _,
                found: _,
            }) if member_id == local_member
        ));
    }

    #[test]
    fn generated_validated_participant_passes_full_reload_validation() {
        let (public_config, _, participants) = setup();
        let local_member = member_id(2);
        let reference = participants.get(&local_member).expect("participant state");
        let generated = ValidatedFederationDkgParticipantState::generate(
            &public_config,
            reference.ceremony_id(),
            local_member,
            reference.keyset_spec().clone(),
        )
        .expect("validated-by-construction participant");
        generated
            .participant_state()
            .validate(&public_config)
            .expect("generated state passes independent validation");

        let encoded = serde_json::to_vec(generated.participant_state()).expect("participant json");
        let reloaded = serde_json::from_slice(&encoded).expect("reloaded participant state");
        let revalidated = ValidatedFederationDkgParticipantState::new(&public_config, reloaded)
            .expect("reloaded state receives a fresh validation proof");
        assert_eq!(
            revalidated.participant_state(),
            generated.participant_state()
        );
    }

    #[test]
    fn planner_advances_only_through_complete_durable_phases() {
        let (public_config, private_configs, participants) = setup();
        let local_member = member_id(2);
        let private_config = private_configs.get(&local_member).expect("private config");
        let participant_state = participants.get(&local_member).expect("participant state");
        let mut records = FederationDkgDriverRecords::default();

        let commitment_plan = records
            .plan(&public_config, private_config, participant_state)
            .expect("commitment plan");
        assert_eq!(commitment_plan.phase, FederationDkgDriverPhase::Commitment);
        assert!(commitment_plan.commitment.is_some());
        assert!(commitment_plan.reveal.is_none());

        records.commitments = all_commitments(&participants);
        let reveal_plan = records
            .plan(&public_config, private_config, participant_state)
            .expect("reveal plan");
        assert_eq!(reveal_plan.phase, FederationDkgDriverPhase::Reveal);
        assert!(reveal_plan.commitment.is_some());
        assert!(reveal_plan.reveal.is_some());
        assert!(reveal_plan.secret_shares.is_empty());

        records.reveals = all_reveals(&participants);
        let secret_share_plan = records
            .plan(&public_config, private_config, participant_state)
            .expect("secret-share plan");
        assert_eq!(
            secret_share_plan.phase,
            FederationDkgDriverPhase::SecretShares
        );
        assert_eq!(secret_share_plan.secret_shares.len(), 3);

        records.secret_share_contributions = incoming_contributions(&participants, local_member);
        let keyset_plan = records
            .plan(&public_config, private_config, participant_state)
            .expect("keyset plan");
        assert_eq!(
            keyset_plan.phase,
            FederationDkgDriverPhase::LocalKeysetShare
        );
        let keyset_share = keyset_plan
            .derived_keyset_share
            .expect("derived keyset share");
        let mut final_public_config = public_config.clone();
        final_public_config.keysets = vec![keyset_share.keyset.clone()];
        let fully_validated_hash = FederationDkgTranscript::hash_for_config(
            FederationDkgSetupMethod::PedersenDkg,
            &final_public_config,
            std::slice::from_ref(&keyset_share.public_ceremony),
        )
        .expect("fully validated transcript hash");
        let locally_derived_hash =
            FederationDkgMemberKeysetShare::transcript_hash_for_locally_derived_records(
                &public_config,
                std::slice::from_ref(&keyset_share),
            )
            .expect("locally derived transcript hash");
        assert_eq!(locally_derived_hash, fully_validated_hash);
        let mut mismatched_keyset_share = keyset_share.clone();
        mismatched_keyset_share.keyset.active = false;
        assert!(matches!(
            FederationDkgMemberKeysetShare::transcript_hash_for_locally_derived_records(
                &public_config,
                std::slice::from_ref(&mismatched_keyset_share),
            ),
            Err(FederationError::DkgTranscriptMismatch { reason })
                if reason.contains("keyset policy mismatch")
        ));
        records.keyset_shares.push(keyset_share.clone());

        let signature_plan = records
            .plan(&public_config, private_config, participant_state)
            .expect("signature plan");
        assert_eq!(
            signature_plan.phase,
            FederationDkgDriverPhase::TranscriptSignature
        );
        let local_signature = signature_plan
            .transcript_signature
            .expect("local transcript signature");
        let repeated_signature = records
            .plan(&public_config, private_config, participant_state)
            .expect("repeated signature plan")
            .transcript_signature
            .expect("repeated transcript signature");
        assert_eq!(local_signature, repeated_signature);

        let transcript_hash = local_signature.transcript_hash;
        records.transcript_signatures = private_configs
            .values()
            .map(|private_config| {
                FederationDkgTranscriptSignatureRequest::new(
                    public_config.federation_id,
                    private_config.member_id,
                    transcript_hash,
                    FederationDkgTranscriptSignature::sign_no_aux_rand(
                        transcript_hash,
                        private_config,
                    )
                    .expect("transcript signature"),
                )
            })
            .collect();
        let activation_plan = records
            .plan(&public_config, private_config, participant_state)
            .expect("activation plan");
        assert_eq!(activation_plan.phase, FederationDkgDriverPhase::Activation);
        assert!(activation_plan.finalized_member_config.is_some());
        assert!(activation_plan.finalized_setup_config_digest.is_some());
        let local_activation = activation_plan.activation.expect("local activation");
        let finalized = FederationDkgFinalizedMemberConfig::from_records(
            &public_config,
            private_config,
            std::slice::from_ref(&keyset_share),
            &records.transcript_signatures,
        )
        .expect("finalized member config");
        let checked_activation = FederationDkgActivationRequest::sign(
            participant_state.ceremony_id(),
            &finalized.public_config,
            private_config,
        )
        .expect("checked activation confirmation");
        let validated_activation = FederationDkgActivationRequest::sign_for_validated_config(
            participant_state.ceremony_id(),
            &finalized.public_config,
            private_config,
        )
        .expect("activation confirmation from validated config");
        assert_eq!(checked_activation, validated_activation);
        checked_activation
            .validate_sender(
                &public_config,
                &finalized.public_config,
                private_config.member_id,
            )
            .expect("checked activation validation");
        checked_activation
            .validate_sender_against_validated_configs(
                &public_config,
                &finalized.public_config,
                private_config.member_id,
            )
            .expect("activation validation from validated configs");
        let records_before_activations = records.clone();
        records.activations = private_configs
            .values()
            .map(|private_config| {
                FederationDkgActivationRequest::sign(
                    participant_state.ceremony_id(),
                    &finalized.public_config,
                    private_config,
                )
                .expect("activation confirmation")
            })
            .collect();
        let fast_complete = records
            .replan_activation_only_change(
                &public_config,
                &records_before_activations,
                &activation_plan,
            )
            .expect("activation-only replan")
            .expect("activation-only records use cached plan");
        let complete = records
            .plan(&public_config, private_config, participant_state)
            .expect("complete plan");
        assert_eq!(fast_complete, complete);
        assert_eq!(complete.phase, FederationDkgDriverPhase::Complete);
        assert_eq!(complete.progress.transcript_signature_count, 3);
        assert_eq!(complete.progress.activation_count, 3);
        assert!(complete.commitment.is_some());
        assert!(complete.reveal.is_some());
        assert_eq!(complete.secret_shares.len(), 3);
        assert_eq!(complete.transcript_signature, Some(local_signature));
        assert_eq!(complete.activation, Some(local_activation));

        let mut later_runtime = finalized.clone();
        later_runtime.public_config.keysets[0].active = false;
        later_runtime
            .public_config
            .validate()
            .expect("later keyset activation state validates");
        later_runtime
            .validate_activation_records(&public_config, &records.activations)
            .expect("setup-time activation evidence survives later activation changes");

        let mut tampered_records = records.clone();
        tampered_records.activations[0].final_config_digest =
            FederationConfigDigest::from_bytes([99; 32]);
        assert!(tampered_records
            .replan_activation_only_change(
                &public_config,
                &records_before_activations,
                &activation_plan,
            )
            .is_err());
        assert!(tampered_records
            .plan(&public_config, private_config, participant_state)
            .is_err());

        let mut changed_records = records.clone();
        changed_records.transcript_signatures.pop();
        assert!(changed_records
            .replan_activation_only_change(
                &public_config,
                &records_before_activations,
                &activation_plan,
            )
            .expect("changed immutable records are not an error")
            .is_none());
    }

    #[test]
    fn planner_rejects_out_of_order_or_cross_ceremony_records() {
        let (public_config, private_configs, participants) = setup();
        let local_member = member_id(2);
        let private_config = private_configs.get(&local_member).expect("private config");
        let participant_state = participants.get(&local_member).expect("participant state");
        let mut records = FederationDkgDriverRecords {
            reveals: all_reveals(&participants),
            ..FederationDkgDriverRecords::default()
        };
        assert!(matches!(
            records.plan(&public_config, private_config, participant_state),
            Err(FederationError::DkgCommitmentMismatch { reason, .. })
                if reason == "missing prior commitment"
        ));

        records.commitments = all_commitments(&participants);
        records.secret_share_contributions = incoming_contributions(&participants, local_member);
        records.secret_share_contributions[0].ceremony_id =
            FederationDkgCeremonyId::from_bytes([92; 32]);
        assert!(matches!(
            records.plan(&public_config, private_config, participant_state),
            Err(FederationError::DkgCommitmentMismatch { reason, .. })
                if reason == "secret contribution belongs to another ceremony"
        ));
    }

    #[test]
    fn planner_debug_redacts_private_dkg_material() {
        let (public_config, private_configs, participants) = setup();
        let local_member = member_id(2);
        let private_config = private_configs.get(&local_member).expect("private config");
        let participant_state = participants.get(&local_member).expect("participant state");
        let records = FederationDkgDriverRecords {
            commitments: all_commitments(&participants),
            reveals: all_reveals(&participants),
            secret_share_contributions: incoming_contributions(&participants, local_member),
            ..FederationDkgDriverRecords::default()
        };
        let plan = records
            .plan(&public_config, private_config, participant_state)
            .expect("keyset plan");
        let private_hex = crate::util::hex::encode(
            plan.secret_shares
                .values()
                .next()
                .expect("secret-share request")
                .secret_shares
                .values()
                .next()
                .expect("secret share")
                .to_secret_bytes(),
        );
        assert!(!format!("{records:?}").contains(&private_hex));
        assert!(!format!("{plan:?}").contains(&private_hex));
        assert!(!format!("{plan:?}").contains("BlsSecretKeyShare"));
    }
}
